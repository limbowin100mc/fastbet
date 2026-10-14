//! Penguin King driver (Outstanding Games' platform). The launcher lands on
//! `{api}/launch/<game>?mode=demo|real&brand=..&channel=..`, whose page carries
//! `window.__launchConfig = {api, params: JSON.parse("…")}` (the params hold the real-money
//! token; a demo makes one up). Protocol (from the client), plain HTTPS JSON:
//!   POST {api}/init {token, game, language, mode, fingerprint, context{channel}, brand}
//!     -> result.user{token, currency{code}, bets{available[], default, last}, balance{cash},
//!        timeNextAllowedBet}, result.game{buyChances[], buyFeatures[] ({id, key, multiplier,
//!        bets[{base, total}]})}
//!   POST {api}/bet  {…same, token, bet{slot}, buyChance?{id, bet, cost}, buyFeature?{id, bet, cost}}
//!     -> result.user{token (new every bet), balance{afterBet, afterSettle}, timeNextAllowedBet},
//!        result.game{cost{total}, win{total}}, result.round{id, delayed}
//! The whole round (free spins, pickers) comes back in one reply; a `delayed` round is
//! settled with POST {api}/settle {…, roundId}. The server sets the earliest time of the next
//! bet (`timeNextAllowedBet`, ~0.5–2.5 s); we wait for it. Amounts are decimal strings.

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::*;

/// A chance or buy offer: its id and what it costs on top, per base bet.
#[derive(Clone, Debug)]
struct Offer {
    id: String,
    chance: bool,
    /// base bet (minor units) -> total cost (minor units)
    totals: Vec<(i64, i64)>,
}

struct Turn {
    token: String,
    /// The earliest next bet, on the server's clock.
    next_at: Option<std::time::SystemTime>,
    /// How far the server's clock is ahead of ours, in ms (never more than it really is: the
    /// largest of the `Date` headers' lower bounds). A PC clock 1.3 s fast sent every second
    /// bet too early, and the server refused it ("A technical error has occurred").
    skew_ms: Option<i64>,
}

pub struct OutstandingSession {
    http: reqwest::Client,
    api: String,
    base: Value,
    offers: Vec<Offer>,
    turn: Mutex<Turn>,
    info: SessionInfo,
}

pub async fn open(launch_url: &str) -> Result<OutstandingSession, SlotError> {
    let (url, html) = follow_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
    let api = js_string(&html, "api: \"").unwrap_or(origin);
    let game_from_path = url.split('?').next().and_then(|p| p.rsplit('/').next()).unwrap_or("").to_string();
    // the launch params, as the page has them
    let params: Value = js_string(&html, "params: JSON.parse(\"").and_then(|s| serde_json::from_str(&s.replace("\\\"", "\"")).ok()).unwrap_or_else(|| {
        let m = hacksaw::query_map(&url);
        json!({ "mode": m.get("mode").cloned().unwrap_or_else(|| "demo".into()), "brand": m.get("brand"), "channel": m.get("channel"), "token": m.get("token") })
    });
    let game = params["game"].as_str().map(str::to_string).unwrap_or(game_from_path);
    let mode = params["mode"].as_str().unwrap_or("demo").to_string();
    let token = params["token"].as_str().map(str::to_string).unwrap_or_else(avatarux::uuid_v4);
    let mut base = json!({
        "game": game,
        "language": "en",
        "mode": mode,
        "fingerprint": avatarux::uuid_v4(),
        "context": { "channel": params["channel"].as_str().unwrap_or("GPL_DESKTOP") },
    });
    if let Some(b) = params["brand"].as_str() {
        base["brand"] = json!(b);
    }
    let http = reqwest::Client::builder().user_agent(UA).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let mut s = OutstandingSession { http, api: api.trim_end_matches('/').to_string(), base, offers: vec![], turn: Mutex::new(Turn { token, next_at: None, skew_ms: None }), info: SessionInfo::default() };

    let r = s.post("init", json!({})).await?;
    let user = &r["user"];
    let g = &r["game"];
    s.base["language"] = json!(user["language"]["fullCode"].as_str().unwrap_or("en-GB"));
    let bet_levels: Vec<BetLevel> = user["bets"]["available"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(|b| BetLevel { raw: b.to_string(), minor: minor(b) }).collect()).unwrap_or_default();
    let default_bet = user["bets"]["last"].as_str().or(user["bets"]["default"].as_str()).map(minor);
    let mut features = Vec::new();
    for (list, chance) in [(&g["buyChances"], true), (&g["buyFeatures"], false)] {
        for o in list.as_array().into_iter().flatten() {
            let Some(id) = o["id"].as_i64().map(|i| i.to_string()).or_else(|| o["id"].as_str().map(str::to_string)) else { continue };
            let totals: Vec<(i64, i64)> = o["bets"].as_array().map(|a| a.iter().filter_map(|b| Some((minor(b["base"].as_str()?), minor(b["total"].as_str()?)))).collect()).unwrap_or_default();
            let mult = o["multiplier"].as_str().and_then(|m| m.parse::<f64>().ok()).unwrap_or(1.0);
            let key = o["key"].as_str().unwrap_or("");
            let fid = format!("{}:{id}", if chance { "chance" } else { "buy" });
            features.push(Feature { id: fid.clone(), name: offer_name(key, chance, mult), cost_mult: mult, rtp: o["rtp"].as_str().and_then(|r| r.parse().ok()) });
            s.offers.push(Offer { id, chance, totals });
        }
    }
    s.info = SessionInfo {
        currency: user["currency"]["code"].as_str().unwrap_or("EUR").to_uppercase(),
        decimals: 2,
        bet_levels,
        default_bet,
        features,
        balance: user["balance"]["cash"].as_str().map(minor),
    };
    Ok(s)
}

/// "free-spins-140" (chance ×1.4) -> "Free spins chance (×1.4)", "picker-bonus-50" -> "Buy picker bonus (×50)".
fn offer_name(key: &str, chance: bool, mult: f64) -> String {
    let words: Vec<&str> = key.split('-').filter(|w| !w.chars().all(|c| c.is_ascii_digit())).collect();
    let what = if words.is_empty() { "bonus".to_string() } else { words.join(" ") };
    let m = if mult.fract().abs() < 1e-6 { format!("{}", mult.round() as i64) } else { format!("{mult}") };
    if chance {
        let mut s = format!("{what} chance");
        s.replace_range(0..1, &s[0..1].to_uppercase());
        format!("{s} (×{m})")
    } else {
        format!("Buy {what} (×{m})")
    }
}

/// "1.40" -> 140
fn minor(s: &str) -> i64 {
    s.trim().parse::<f64>().map(|v| (v * 100.0).round() as i64).unwrap_or(0)
}

fn units(m: i64) -> String {
    format!("{}.{:02}", m / 100, m % 100)
}

/// `name: "value"` out of the launch page's inline script.
fn js_string(html: &str, key: &str) -> Option<String> {
    let start = html.find(key)? + key.len();
    let rest = &html[start..];
    // the value ends at the first quote that isn't escaped
    let mut prev = '\0';
    for (i, c) in rest.char_indices() {
        if c == '"' && prev != '\\' {
            return Some(rest[..i].to_string());
        }
        prev = c;
    }
    None
}

impl OutstandingSession {
    async fn post(&self, path: &str, extra: Value) -> Result<Value, SlotError> {
        let mut turn = self.turn.lock().await;
        // the server says when the next bet may go, on its clock
        if let Some(at) = turn.next_at {
            let wait = epoch_ms(at) - turn.skew_ms.unwrap_or(0) - epoch_ms(std::time::SystemTime::now());
            if wait > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(wait.min(10_000) as u64 + 20)).await;
            }
        }
        let mut body = self.base.clone();
        body["token"] = json!(turn.token);
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        let res = self
            .http
            .post(format!("{}/{}", self.api, path))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/plain, */*")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| SlotError::Bridge(format!("Penguin King: {e}")))?;
        let status = res.status().as_u16();
        // the server's time is at least its `Date` (whole seconds) when the reply arrives
        if let Some(date) = res.headers().get("date").and_then(|v| v.to_str().ok()).and_then(parse_http_date) {
            let low = epoch_ms(date) - epoch_ms(std::time::SystemTime::now());
            turn.skew_ms = Some(turn.skew_ms.map_or(low, |s| s.max(low)));
        }
        let text = res.text().await.unwrap_or_default();
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != 200 || v.get("result").is_none() {
            let msg = v["error"]["message"].as_str().unwrap_or("").to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("insufficient") || lower.contains("balance") || lower.contains("funds") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || lower.contains("token") || status == 401 || status == 403 {
                return Err(SlotError::SessionExpired);
            }
            let detail = if msg.is_empty() { text.chars().take(160).collect() } else { msg };
            return Err(SlotError::Provider(format!("Penguin King HTTP {status}: {detail}")));
        }
        let user = &v["result"]["user"];
        if let Some(t) = user["token"].as_str() {
            turn.token = t.to_string();
        }
        turn.next_at = user["timeNextAllowedBet"].as_str().and_then(parse_time);
        Ok(v["result"].clone())
    }
}

/// "2026-10-12T18:00:02.316Z" -> SystemTime
fn parse_time(s: &str) -> Option<std::time::SystemTime> {
    let (date, time) = s.trim_end_matches('Z').split_once('T')?;
    let mut d = date.split('-').map(|x| x.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let mut t = time.split(':');
    let (h, min) = (t.next()?.parse::<i64>().ok()?, t.next()?.parse::<i64>().ok()?);
    let sec: f64 = t.next()?.parse().ok()?;
    // days since 1970-01-01 (civil from days, Howard Hinnant)
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let ms = ((days * 86_400 + h * 3600 + min * 60) as f64 + sec) * 1000.0;
    if ms < 0.0 {
        return None;
    }
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64))
}

/// "Tue, 14 Oct 2026 14:20:23 GMT" -> SystemTime
fn parse_http_date(s: &str) -> Option<std::time::SystemTime> {
    let mut f = s.split_whitespace().skip(1);
    let (day, mon, year, time) = (f.next()?, f.next()?, f.next()?, f.next()?);
    let m = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"].iter().position(|x| *x == mon)? + 1;
    parse_time(&format!("{year}-{m:02}-{day}T{time}Z"))
}

fn epoch_ms(t: std::time::SystemTime) -> i64 {
    t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[async_trait]
impl SlotSession for OutstandingSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let mut extra = json!({ "bet": { "slot": units(bet) } });
        if let Some(f) = feature {
            let (kind, id) = f.split_once(':').ok_or_else(|| SlotError::Provider(format!("Unknown feature {f}")))?;
            let offer = self.offers.iter().find(|o| o.id == id && o.chance == (kind == "chance")).ok_or_else(|| SlotError::Provider(format!("Unknown feature {f}")))?;
            let total = offer.totals.iter().find(|(b, _)| *b == bet).map(|(_, t)| *t).ok_or_else(|| SlotError::Provider("This feature isn't offered at this bet size; pick another bet".into()))?;
            if offer.chance {
                extra["buyChance"] = json!({ "id": id, "bet": units(total - bet), "cost": units(total) });
            } else {
                extra["buyFeature"] = json!({ "id": id, "bet": units(total), "cost": units(total) });
            }
        }
        let r = self.post("bet", extra).await?;
        let g = &r["game"];
        let cost = g["cost"]["total"].as_str().map(minor).unwrap_or(bet);
        let mut win = g["win"]["total"].as_str().map(minor).unwrap_or(0);
        let mut balance = r["user"]["balance"]["afterSettle"]["cash"].as_str().map(minor);
        let round_id = r["round"]["id"].as_i64().map(|i| i.to_string()).unwrap_or_default();
        let free_spins = g["win"].get("freeSpin").is_some() || g["spins"].as_array().is_some_and(|a| a.len() > 1) || r["round"]["type"].as_str() == Some("buy-feature");
        let delayed = r["round"]["delayed"].as_bool() == Some(true);
        // the bonus is decided already; left unsettled, the browser shows it
        let held = hold && free_spins && delayed;
        if held {
            // the browser shows the bonus and what it pays
            win = 0;
        }
        if delayed && !held {
            let s = self.post("settle", json!({ "roundId": r["round"]["id"] })).await?;
            if let Some(b) = s["user"]["balance"]["afterSettle"]["cash"].as_str().or(s["user"]["balance"]["cash"].as_str()) {
                balance = Some(minor(b));
            }
            if let Some(w) = s["game"]["win"]["total"].as_str() {
                win = minor(w);
            }
        }
        Ok(SpinResult {
            round_id,
            cost,
            win,
            multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
            balance,
            pending: held,
            free_spins,
            held,
            events: g["spins"].as_array().map(|a| a.len()).unwrap_or(1),
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn close(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_names() {
        let t = parse_time("1970-01-02T00:00:01.500Z").unwrap();
        assert_eq!(t.duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(), 86_401_500);
        assert_eq!(parse_http_date("Fri, 02 Jan 1970 00:00:01 GMT"), parse_time("1970-01-02T00:00:01Z"));
        assert_eq!(offer_name("free-spins-140", true, 1.4), "Free spins chance (×1.4)");
        assert_eq!(offer_name("picker-bonus-50", false, 50.0), "Buy picker bonus (×50)");
        assert_eq!(units(140), "1.40");
        assert_eq!(minor("0.10"), 10);
        let html = r#"api: "https://x.y", params: JSON.parse("{\"game\":\"g\",\"mode\":\"demo\"}"),"#;
        assert_eq!(js_string(html, "api: \"").as_deref(), Some("https://x.y"));
        assert_eq!(js_string(html, "params: JSON.parse(\"").as_deref(), Some(r#"{\"game\":\"g\",\"mode\":\"demo\"}"#));
    }
}
