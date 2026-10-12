//! Pragmatic Play driver. The launcher ends on `gs2c/html5Game.do` (demo) or
//! `gs2c/game/load` (real money, the same config HTML-entity encoded), whose page
//! config carries `gameService` (the per-environment endpoint), `mgckey` (the
//! session), `currencyOriginal` and `datapath` (…/games/vs/<symbol>/). Everything after that is form-encoded POSTs to
//! gameService with a text reply of `key=value&…` pairs:
//!   action=doInit                      -> sc (coin values), defc, l (lines), bls (bet per
//!                                         mode: bls[0] regular, bls[i] ante mode i),
//!                                         purInit=[{bet:N}…] (buys, cost N coins), balance
//!   action=doSpin c l bl [pur]         -> na (next action), tw (round win so far), balance
//!   action=doCollect                   -> settles tw into the balance
//! A round follows `na`: `c` collect, `s` with a pending tumble (`rs_p`) or free
//! spins (`fs` without `fs_total`) keeps spinning for free; plain `s` is done.
//! Every request carries an increasing `index` and the server's `counter` + 1.
//! Some games want a player choice with every spin (Zeus vs Hades: `ind` = which
//! god); a doSpin the server can't process says "Internal server error" and
//! freezes the session, so we start it again (doInit) and retry with `ind=0`,
//! which the session then keeps sending.

use async_trait::async_trait;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;

pub struct PragmaticSession {
    client: reqwest::Client,
    service: String,
    page: String,
    symbol: String,
    mgckey: String,
    /// Coin value per bet level, as the server spells it ("0.10").
    coins: Vec<String>,
    lines: u64,
    /// Coins charged per ante mode (index = `bl`); `[lines]` without antes.
    bls: Vec<u64>,
    /// Coins charged per buy (index = `pur`).
    buys: Vec<u64>,
    info: SessionInfo,
    seq: Mutex<Seq>,
    /// Send `ind=0` with every spin (a game that wants a player choice).
    choice: AtomicBool,
}

#[derive(Default)]
struct Seq {
    index: u64,
    counter: u64,
}

pub async fn open(launch_url: &str) -> Result<PragmaticSession, SlotError> {
    let (page, html) = follow_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let html = html.replace("&quot;", "\"").replace("&amp;", "&");
    let service = config_string(&html, "gameService").ok_or_else(|| SlotError::Provider("No gameService in the game page".into()))?;
    let q = super::hacksaw::query_map(&page);
    let mgckey = config_string(&html, "mgckey").or_else(|| q.get("mgckey").cloned()).ok_or_else(|| SlotError::Provider("No session key in the game page".into()))?;
    let symbol = q
        .get("symbol")
        .cloned()
        .or_else(|| config_string(&html, "datapath").and_then(|d| d.trim_end_matches('/').rsplit('/').next().map(str::to_string)))
        .filter(|s| !s.is_empty())
        .ok_or_else(|| SlotError::Provider("No game symbol".into()))?;
    // `currency` is a symbol ("$") on real-money pages; `currencyOriginal` is the code
    let currency = config_string(&html, "currencyOriginal").or_else(|| config_string(&html, "currency")).filter(|c| c.len() == 3 && c.chars().all(|ch| ch.is_ascii_alphabetic())).unwrap_or_else(|| "FUN".into()).to_uppercase();
    let client = reqwest::Client::builder().user_agent(UA).cookie_store(true).gzip(true).build().map_err(|e| SlotError::Bridge(e.to_string()))?;

    let mut s = PragmaticSession {
        client,
        service,
        page,
        symbol,
        mgckey,
        coins: vec![],
        lines: 0,
        bls: vec![],
        buys: vec![],
        info: SessionInfo { currency, decimals: 2, ..Default::default() },
        seq: Mutex::new(Seq { index: 0, counter: 0 }),
        choice: AtomicBool::new(false),
    };
    let init = s.call("doInit", &[]).await?;
    s.lines = init.get("l").and_then(|v| v.parse().ok()).unwrap_or(20);
    s.coins = init.get("sc").map(|v| v.split(',').map(str::to_string).collect()).unwrap_or_default();
    s.bls = init.get("bls").map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
    if s.bls.is_empty() {
        s.bls = vec![s.lines];
    }
    s.buys = init.get("purInit").map(|v| numbers_after(v, "bet:")).unwrap_or_default();
    let lines = s.lines as f64;
    s.info.bet_levels = s.coins.iter().filter_map(|c| c.parse::<f64>().ok().map(|v| BetLevel { raw: c.clone(), minor: (v * lines * 100.0).round() as i64 })).collect();
    s.info.default_bet = init.get("defc").and_then(|v| v.parse::<f64>().ok()).map(|c| (c * lines * 100.0).round() as i64);
    s.info.balance = init.get("balance").and_then(|b| money(b));
    let rtps = init.get("gameInfo").map(|g| rtp_map(g)).unwrap_or_default();
    let base = s.bls[0].max(1) as f64;
    for (i, coins) in s.bls.iter().enumerate().skip(1) {
        let cost = *coins as f64 / base;
        s.info.features.push(Feature { id: format!("ante{i}"), name: format!("Bet mode {i} (×{})", fmt_mult(cost)), cost_mult: cost, rtp: rtps.get(&format!("ante_a{i}")).copied() });
    }
    for (i, coins) in s.buys.clone().iter().enumerate() {
        let cost = *coins as f64 / lines;
        let name = if s.buys.len() == 1 { format!("Buy free spins (×{})", fmt_mult(cost)) } else { format!("Buy feature {} (×{})", i + 1, fmt_mult(cost)) };
        s.info.features.push(Feature { id: format!("buy{i}"), name, cost_mult: cost, rtp: rtps.get(&format!("purchase_{i}")).copied() });
    }
    // a round left open by an earlier session is played out first
    if init.get("na").map(|n| n != "s").unwrap_or(false) {
        let coin = init.get("c").cloned().unwrap_or_else(|| s.coins.first().cloned().unwrap_or_default());
        let _ = s.play_out(init, &coin, 0).await;
    }
    Ok(s)
}

impl PragmaticSession {
    async fn call(&self, action: &str, extra: &[(&str, String)]) -> Result<HashMap<String, String>, SlotError> {
        let (index, counter) = {
            let mut q = self.seq.lock();
            q.index += 1;
            q.counter += 1;
            (q.index, q.counter)
        };
        let mut form: Vec<(&str, String)> = vec![("action", action.into()), ("symbol", self.symbol.clone())];
        form.extend(extra.iter().cloned());
        form.extend([("index", index.to_string()), ("counter", counter.to_string()), ("repeat", "0".into()), ("mgckey", self.mgckey.clone())]);
        let origin = self.page.split('/').take(3).collect::<Vec<_>>().join("/");
        let res = self.client.post(&self.service).header("Origin", origin).header("Referer", &self.page).form(&form).send().await.map_err(|e| SlotError::Bridge(e.to_string()))?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|e| SlotError::Bridge(e.to_string()))?;
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        if status != 200 {
            return Err(SlotError::Provider(format!("{action} HTTP {status}")));
        }
        let map = parse(&text);
        match map.get("counter").and_then(|c| c.parse::<u64>().ok()) {
            Some(c) => self.seq.lock().counter = c,
            // a refused request doesn't move the server's counter
            None => self.seq.lock().counter = counter - 1,
        }
        if map.get("na").is_none() && action != "doCollect" {
            let msg = map.get("frozen").or(map.get("msg")).cloned().unwrap_or_else(|| text.chars().take(160).collect());
            let lower = msg.to_ascii_lowercase();
            if lower.contains("balance") || lower.contains("funds") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || lower.contains("expired") || lower.contains("logged") {
                return Err(SlotError::SessionExpired);
            }
            return Err(SlotError::Provider(format!("{action}: {}", msg.replace('+', " "))));
        }
        Ok(map)
    }

    /// Follows `na` until the round is settled; returns (win in minor units, last reply, steps,
    /// free spins seen).
    /// doSpin with the session's standing parameters.
    async fn do_spin(&self, coin: &str, bl: usize, pur: Option<usize>) -> Result<HashMap<String, String>, SlotError> {
        let mut params = vec![("c", coin.to_string()), ("l", self.lines.to_string()), ("bl", bl.to_string())];
        if self.choice.load(Ordering::Relaxed) {
            params.push(("ind", "0".into()));
        }
        if let Some(i) = pur {
            params.push(("pur", i.to_string()));
        }
        self.call("doSpin", &params).await
    }

    async fn play_out(&self, mut r: HashMap<String, String>, coin: &str, bl: usize) -> Result<(i64, HashMap<String, String>, usize, bool), SlotError> {
        let mut win = 0i64;
        let mut steps = 1usize;
        let mut free = false;
        loop {
            free |= r.contains_key("fs") || r.contains_key("fs_total");
            if let Some(tw) = r.get("tw").and_then(|t| money(t)) {
                win = tw;
            }
            let na = r.get("na").cloned().unwrap_or_else(|| "s".into());
            let pending = r.contains_key("rs_p") || (r.contains_key("fs") && !r.contains_key("fs_total"));
            let next = match na.as_str() {
                "s" if pending => self.do_spin(coin, bl, None).await?,
                "s" => return Ok((win, r, steps, free)),
                "c" => {
                    let c = self.call("doCollect", &[]).await?;
                    return Ok((win, c, steps + 1, free));
                }
                "cb" => self.call("doCollectBonus", &[]).await?,
                // a pick or an option the player would choose: take the first one
                "b" => self.call("doBonus", &[("ind", "0".into())]).await?,
                "fso" => self.call("doFSOption", &[("ind", "0".into())]).await?,
                "m" => self.call("doMysteryScatter", &[]).await?,
                other => return Err(SlotError::Provider(format!("This round waits for a step autoplay doesn't know ({other})"))),
            };
            r = next;
            steps += 1;
            if steps > 600 {
                return Err(SlotError::Provider("The round didn't end".into()));
            }
        }
    }
}

#[async_trait]
impl SlotSession for PragmaticSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let level = self.info.bet_levels.iter().position(|b| b.minor == bet).ok_or_else(|| SlotError::Provider("That bet size isn't offered by this game".into()))?;
        let coin = self.coins[level].clone();
        let coin_v: f64 = coin.parse().unwrap_or(0.0);
        let (mut bl, mut pur) = (0usize, None);
        match feature {
            Some(f) if f.starts_with("ante") => bl = f[4..].parse().unwrap_or(0),
            Some(f) if f.starts_with("buy") => pur = f[3..].parse::<usize>().ok(),
            _ => {}
        }
        let coins_charged = match pur {
            Some(i) => *self.buys.get(i).ok_or_else(|| SlotError::Provider("No such buy".into()))?,
            None => *self.bls.get(bl).unwrap_or(&self.lines),
        };
        let cost = (coin_v * coins_charged as f64 * 100.0).round() as i64;
        let first = match self.do_spin(&coin, bl, pur).await {
            Err(SlotError::Provider(m)) if m.contains("Internal server error") && !self.choice.load(Ordering::Relaxed) => {
                self.choice.store(true, Ordering::Relaxed);
                *self.seq.lock() = Seq::default();
                self.call("doInit", &[]).await?;
                self.do_spin(&coin, bl, pur).await?
            }
            other => other?,
        };
        let round_id = first.get("rid").cloned().unwrap_or_default();
        let (win, last, steps, free_spins) = self.play_out(first, &coin, bl).await?;
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(SpinResult {
            round_id,
            cost,
            win,
            multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
            balance: last.get("balance").and_then(|b| money(b)),
            pending: false,
            free_spins,
            events: steps,
            ms,
        })
    }

    fn close(&self) {}
}

/// `key=value&…` reply into a map (values stay as sent).
fn parse(text: &str) -> HashMap<String, String> {
    text.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// "100,000.00" -> 10000000 (minor units).
fn money(s: &str) -> Option<i64> {
    s.replace(',', "").parse::<f64>().ok().map(|v| (v * 100.0).round() as i64)
}

/// `"key":"value"` from the game page's inline config.
fn config_string(html: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\":\"");
    let start = html.find(&pat)? + pat.len();
    let end = html[start..].find('"')?;
    Some(html[start..start + end].replace("\\/", "/"))
}

/// Every number following `tag` in a JS literal: `[{bet:2000},{bet:10000}]` -> [2000, 10000].
fn numbers_after(s: &str, tag: &str) -> Vec<u64> {
    s.match_indices(tag).filter_map(|(i, _)| s[i + tag.len()..].chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()).collect()
}

/// `gameInfo={rtps:{regular:"96.52",ante_a1:"96.49",…}}` -> {"regular": 0.9652, …}.
fn rtp_map(game_info: &str) -> HashMap<String, f64> {
    let Some(start) = game_info.find("rtps:{") else { return HashMap::new() };
    let body = &game_info[start + 6..];
    let body = &body[..body.find('}').unwrap_or(body.len())];
    body.split(',')
        .filter_map(|kv| kv.split_once(':'))
        .filter_map(|(k, v)| v.trim_matches('"').parse::<f64>().ok().map(|r| (k.trim().to_string(), (r * 100.0).round() / 10_000.0)))
        .collect()
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
    fn replies() {
        let r = parse("tw=1.50&balance=99,998.00&na=s&rs_p=0");
        assert_eq!(money(&r["balance"]), Some(9_999_800));
        assert_eq!(money(&r["tw"]), Some(150));
        assert_eq!(numbers_after(r#"[{bet:2000,type:"default"},{bet:10000,type:"default"}]"#, "bet:"), vec![2000, 10000]);
        let rtps = rtp_map(r#"{rtps:{ante_a3:"96.50",regular:"96.52",purchase_0:"96.50"},props:{max:"1"}}"#);
        assert_eq!(rtps.get("regular"), Some(&0.9652));
        assert_eq!(rtps.get("purchase_0"), Some(&0.965));
    }
}
