//! Playson driver. The launcher page (`…/launch?key=…&gameName=…`) carries
//! `window.playsonConfig` with `partner.{key, server_url, wl, game, project_id}`
//! plus the client build (`version`, `revision`). The game server speaks XML
//! over form POSTs to `{server_url}/{command}?gid=…&wlCode=…&projectId=…`:
//!   connect   (playerguid=key, engine_version, <debug build_date build_version/>) -> session
//!   reconnect -> <stakeIncrement> (line bets), <defaultBet>, <game total_bet_mult
//!                buy_feature_cost="70,150">, <user_new cash> (minor units)
//!   bet       <bet cash="line bet"/> [<buy freegame="true" type="i"/>]
//! A round follows `<state current_state>`: `idle` = settled; `respin` -> `next`
//! with the same bet; `bonus` -> `bonus` <action round="n"/>; `choice` (pick a
//! free-spins kind) -> `game` with <player_choice type="t"/> (we take the first
//! of `<player_choice types>`); free spins (`fs`) are further `bet` commands the
//! server doesn't charge for.

use async_trait::async_trait;
use parking_lot::Mutex;

use super::*;

/// The unicore engine version connect claims: the server wants the field but
/// accepts any value (this is the one the 2026 game builds send).
const ENGINE: &str = "14.118";

pub struct PlaysonSession {
    client: reqwest::Client,
    server: String,
    query: String,
    page: String,
    session: String,
    /// Line bets the game offers; a total bet is line bet × `mult`.
    stakes: Vec<i64>,
    mult: i64,
    /// What a `choice` state offers (from reconnect), when the reply doesn't say.
    choices: Option<String>,
    info: SessionInfo,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    rnd: u64,
    balance: Option<i64>,
}

pub async fn open(launch_url: &str) -> Result<PlaysonSession, SlotError> {
    let (page, html) = follow_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let partner = html.find("partner:").and_then(|i| json_object_at(&html[i + 8..])).ok_or_else(|| SlotError::Provider("No game config in the Playson launcher".into()))?;
    let cfg: serde_json::Value = serde_json::from_str(partner).map_err(|e| SlotError::Provider(format!("Playson config: {e}")))?;
    let field = |k: &str| cfg[k].as_str().map(str::to_string).or_else(|| cfg[k].as_i64().map(|n| n.to_string())).unwrap_or_default();
    let (server, key, wl, game, project) = (field("server_url"), field("key"), field("wl"), field("game"), field("project_id"));
    if server.is_empty() || game.is_empty() {
        return Err(SlotError::Provider("Incomplete Playson launch config".into()));
    }
    let version = js_string_after(&html, "version:").unwrap_or_else(|| "0.0.0".into());
    let revision = html.find("revision:").map(|i| html[i + 9..].trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>()).unwrap_or_default();
    let client = reqwest::Client::builder().user_agent(UA).cookie_store(true).gzip(true).build().map_err(|e| SlotError::Bridge(e.to_string()))?;

    let mut s = PlaysonSession {
        client,
        server: server.trim_end_matches('/').to_string(),
        query: format!("gid={game}&wlCode={wl}&projectId={project}"),
        page,
        session: String::new(),
        stakes: vec![],
        mult: 1,
        choices: None,
        info: SessionInfo { decimals: 2, ..Default::default() },
        state: Mutex::new(State { rnd: now_ms(), balance: None }),
    };
    let connect = s
        .send(
            "connect",
            &format!(r#" playerguid="{}" gameid="{game}" platform="desk" engine_type="unicore" engine_version="{ENGINE}" wl="{wl}""#, xml_escape(&key)),
            &format!(r#"<debug build_date="{revision}" build_version="{version}"></debug>"#),
        )
        .await?;
    s.session = attr(&connect, "server", "session").filter(|x| x != "undefined").ok_or_else(|| SlotError::Provider(format!("Playson connect refused: {}", error_text(&connect))))?;
    s.info.currency = attr(&connect, "user", "currency-id").unwrap_or_else(|| "FUN".into()).to_uppercase();

    let rc = s.send("reconnect", "", "").await?;
    s.mult = attr(&rc, "game", "total_bet_mult").and_then(|m| m.parse().ok()).unwrap_or(1).max(1);
    s.stakes = tag_text(&rc, "stakeIncrement").map(|t| t.split(',').filter_map(|x| x.trim().parse().ok()).collect()).unwrap_or_default();
    if s.stakes.is_empty() {
        return Err(SlotError::Provider("Playson sent no bet sizes".into()));
    }
    s.info.bet_levels = s.stakes.iter().map(|l| BetLevel { raw: l.to_string(), minor: l * s.mult }).collect();
    // the default can be a line bet the list doesn't offer (7 among 6, 8): take the nearest one below
    s.info.default_bet = tag_text(&rc, "defaultBet").and_then(|d| d.trim().parse::<i64>().ok()).map(|d| s.stakes.iter().copied().filter(|x| *x <= d).max().unwrap_or(s.stakes[0]) * s.mult);
    s.info.balance = attr(&rc, "user_new", "cash").and_then(|c| c.parse().ok());
    s.choices = attr(&rc, "player_choice", "types");
    s.state.lock().balance = s.info.balance;
    let rtp = |name: &str| tag_text(&rc, name).and_then(|r| r.trim().parse::<f64>().ok()).map(|r| r / 100.0);
    if let Some(costs) = attr(&rc, "game", "buy_feature_cost") {
        let costs: Vec<f64> = costs.split(',').filter_map(|c| c.trim().parse().ok()).collect();
        for (i, c) in costs.iter().enumerate() {
            let name = if costs.len() == 1 { format!("Buy bonus (×{})", fmt_mult(*c)) } else { format!("Buy bonus {} (×{})", i + 1, fmt_mult(*c)) };
            s.info.features.push(Feature { id: format!("buy{i}"), name, cost_mult: *c, rtp: rtp(&format!("buyFeature{}", fmt_mult(*c))) });
        }
    }
    // a round left open by an earlier session is played out first
    if attr(&rc, "state", "current_state").map(|st| st != "idle").unwrap_or(false) {
        let stake = attr(&rc, "game", "line-bet").unwrap_or_else(|| s.stakes[0].to_string());
        let _ = s.play_out(rc, &stake, false).await;
    }
    Ok(s)
}

impl PlaysonSession {
    async fn send(&self, command: &str, attrs: &str, inner: &str) -> Result<String, SlotError> {
        let rnd = {
            let mut st = self.state.lock();
            st.rnd += 1;
            st.rnd
        };
        let session = if self.session.is_empty() { String::new() } else { format!(r#" session="{}""#, self.session) };
        let body = format!(r#"<client{session} rnd="{rnd}" command="{command}"{attrs}>{inner}</client>"#);
        let url = format!("{}/{command}?r={}&{}", self.server, now_ms(), self.query);
        let origin = self.page.split('/').take(3).collect::<Vec<_>>().join("/");
        let res = self
            .client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded; charset=UTF-8")
            .header("Accept", "text/plain, */*; q=0.01")
            .header("Origin", origin)
            .header("Referer", &self.page)
            .body(body)
            .send()
            .await
            .map_err(|e| SlotError::Bridge(e.to_string()))?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| SlotError::Bridge(e.to_string()))?;
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        if status != 200 {
            let lower = text.to_ascii_lowercase();
            if lower.contains("session") {
                return Err(SlotError::SessionExpired);
            }
            return Err(SlotError::Provider(format!("{command} HTTP {status}: {}", text.chars().take(120).collect::<String>())));
        }
        if text.contains("<error") {
            let code = attr(&text, "error", "code").unwrap_or_default().to_ascii_uppercase();
            let msg = error_text(&text);
            let lower = format!("{code} {msg}").to_ascii_lowercase();
            if lower.contains("money") || lower.contains("balance") || lower.contains("funds") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || lower.contains("expired") {
                return Err(SlotError::SessionExpired);
            }
            if command != "connect" {
                return Err(SlotError::Provider(format!("{command}: {msg} ({code})")));
            }
        }
        if let Some(c) = attr(&text, "user_new", "cash").and_then(|c| c.parse().ok()) {
            self.state.lock().balance = Some(c);
        }
        Ok(text)
    }

    /// Plays the round to `idle`; returns (last reply, steps, free spins seen, held). `hold`: stop
    /// where the bonus would start, leaving it for the browser.
    async fn play_out(&self, mut r: String, stake: &str, hold: bool) -> Result<(String, usize, bool, bool), SlotError> {
        let mut steps = 1usize;
        let mut bonus = false;
        loop {
            let state = attr(&r, "state", "current_state").unwrap_or_else(|| "idle".into());
            let status = attr(&r, "server", "status").unwrap_or_default();
            if state == "idle" {
                return Ok((r, steps, bonus, false));
            }
            if hold && state != "respin" {
                return Ok((r, steps, true, true));
            }
            bonus |= state != "respin";
            let bet = format!(r#"<bet cash="{stake}"/>"#);
            r = if state == "bonus" || status == "bonus" {
                self.send("bonus", "", &format!(r#"<action round="{steps}"/>"#)).await?
            } else if state == "choice" || status == "game" {
                let types = attr(&r, "player_choice", "types").or_else(|| self.choices.clone()).unwrap_or_else(|| "0".into());
                let pick = types.split(',').next().unwrap_or("0").trim().to_string();
                self.send("game", "", &format!(r#"{bet}<player_choice type="{pick}"></player_choice>"#)).await?
            } else if state == "respin" || status == "next" {
                self.send("next", "", &bet).await?
            } else {
                // free spins and other bonus states: further bets the server doesn't charge
                self.send("bet", "", &bet).await?
            };
            steps += 1;
            if steps > 600 {
                return Err(SlotError::Provider("The round didn't end".into()));
            }
        }
    }
}

#[async_trait]
impl SlotSession for PlaysonSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        if bet % self.mult != 0 || !self.stakes.contains(&(bet / self.mult)) {
            return Err(SlotError::Provider("That bet size isn't offered by this game".into()));
        }
        let stake = (bet / self.mult).to_string();
        let buy = feature.and_then(|f| f.strip_prefix("buy")).and_then(|i| i.parse::<usize>().ok());
        let cost = match buy {
            Some(i) => {
                let f = self.info.features.get(i).ok_or_else(|| SlotError::Provider("No such buy".into()))?;
                (bet as f64 * f.cost_mult).round() as i64
            }
            None => bet,
        };
        let before = self.state.lock().balance;
        let mut inner = format!(r#"<bet cash="{stake}"/>"#);
        if let Some(i) = buy {
            inner.push_str(&format!(r#"<buy freegame="true" type="{i}"/>"#));
        }
        let first = self.send("bet", "", &inner).await?;
        let round_id = attr(&first, "roundnum", "value").unwrap_or_default();
        let (_last, steps, bonus, held) = self.play_out(first, &stake, hold).await?;
        let after = self.state.lock().balance;
        // the server reports the balance after each step; the round's win is what came back
        let win = match (before, after) {
            (Some(b), Some(a)) => (a - b + cost).max(0),
            _ => 0,
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(SpinResult {
            round_id,
            cost,
            win,
            multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
            balance: after,
            pending: held,
            free_spins: bonus || buy.is_some(),
            held,
            events: steps,
            ms,
        })
    }

    fn close(&self) {}
}

/// `<tag … name="value"` (either quote style).
fn attr(xml: &str, tag: &str, name: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(i) = xml[from..].find(&open) {
        let start = from + i;
        let after = &xml[start + open.len()..];
        from = start + open.len();
        if !after.starts_with([' ', '>', '/', '\n', '\t', '\r']) {
            continue;
        }
        let end = after.find('>').unwrap_or(after.len());
        let head = &after[..end];
        for q in ['"', '\''] {
            let pat = format!(" {name}={q}");
            if let Some(p) = head.find(&pat) {
                let v = &head[p + pat.len()..];
                return v.find(q).map(|e| v[..e].to_string());
            }
        }
        return None;
    }
    None
}

/// Text inside `<tag>…</tag>`.
fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let start = xml.find(&format!("<{tag}>"))? + tag.len() + 2;
    let end = xml[start..].find(&format!("</{tag}>"))?;
    Some(xml[start..start + end].to_string())
}

fn error_text(xml: &str) -> String {
    tag_text(xml, "msg").unwrap_or_else(|| xml.chars().take(160).collect())
}

/// The balanced `{…}` object at the start of `s` (after whitespace).
fn json_object_at(s: &str) -> Option<&str> {
    let s = s.trim_start();
    if !s.starts_with('{') {
        return None;
    }
    let (mut depth, mut in_str, mut esc) = (0i32, false, false);
    for (i, c) in s.char_indices() {
        if in_str {
            match c {
                _ if esc => esc = false,
                '\\' => esc = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&s[..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// `key: "value"` in the launcher's inline script.
fn js_string_after(html: &str, key: &str) -> Option<String> {
    let rest = html[html.find(key)? + key.len()..].trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;")
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn fmt_mult(x: f64) -> String {
    if (x - x.round()).abs() < 1e-6 {
        format!("{}", x.round() as i64)
    } else {
        format!("{x:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml() {
        let x = r#"<server command="bet" status="ok" session="23"><state current_state="idle"></state><game line-bet="10" cash-bet="100" buy_feature_cost="70,150"><bonus win="0"/></game><user_new cash="1099990"></user_new><extra><stakeIncrement>2,3,4</stakeIncrement></extra></server>"#;
        assert_eq!(attr(x, "server", "session").as_deref(), Some("23"));
        assert_eq!(attr(x, "state", "current_state").as_deref(), Some("idle"));
        assert_eq!(attr(x, "game", "buy_feature_cost").as_deref(), Some("70,150"));
        assert_eq!(attr(x, "user_new", "cash").as_deref(), Some("1099990"));
        assert_eq!(tag_text(x, "stakeIncrement").as_deref(), Some("2,3,4"));
        assert_eq!(attr("<server command='connect' session='undefined'>", "server", "session").as_deref(), Some("undefined"));
        let page = "partner: {\"key\":\"TEST\",\"x\":{\"y\":\"}\"}},\n connectionData: null";
        assert_eq!(json_object_at(&page[8..]), Some("{\"key\":\"TEST\",\"x\":{\"y\":\"}\"}}"));
    }
}
