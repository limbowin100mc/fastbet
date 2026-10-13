//! Onlyplay driver. The launcher lands on `<game>.onlyplaygames.net/?sid=..&gid=..&api=<host>/internal`.
//! Protocol (from the game client), all POSTs to `https://{api}/<path>?sid=..&gid=..`:
//!   game_init    {ver:"1.0"}                         -> currency, decimals, partner_config{disableBuyBonus}
//!   game_request {ver, id:"config"}                  -> balance, availableBets[], buyBonusCoeffs[] | bonusCostMultiplier
//!   game_request {ver, id:"spin", betIndex, spinLevel}  -> balance (win included), spin{totalWin}, freespins{..}
//!   game_request {ver, id:"buy", betIndex, buyBonusLevel?}
//! Games with `buyBonusCoeffs` [s1, s2, b1, b2]: two spin modes (`spinLevel` 1|2, every spin
//! costs s × bet) and bonus buys (id "buy", `buyBonusLevel` 1|2). Games with one
//! `bonusCostMultiplier` buy with a plain id "buy". Free spins go on as "spin" calls while `freespinsTriggered` / `isFreespins`
//! is set, up to `isLastFreespin`. Amounts are currency units; a round's win is read from the
//! balance (the replies' win fields differ per game). Plain HTTPS, no browser needed.

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::*;

pub struct OnlyplaySession {
    http: reqwest::Client,
    api: String,
    query: String,
    /// bet amounts in currency units, by index
    bets: Vec<f64>,
    scale: f64,
    /// last balance seen, in minor units
    balance: Mutex<Option<i64>>,
    info: SessionInfo,
}

pub async fn open(launch_url: &str) -> Result<OnlyplaySession, SlotError> {
    let url = resolve_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let map = hacksaw::query_map(&url);
    let (Some(sid), Some(gid), Some(api)) = (map.get("sid"), map.get("gid"), map.get("api")) else {
        return Err(SlotError::Provider(format!("Could not read the launch parameters from {url}")));
    };
    let http = reqwest::Client::builder().user_agent(UA).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let mut s = OnlyplaySession {
        http,
        api: format!("https://{}", api.trim_end_matches('/')),
        query: format!("sid={sid}&gid={gid}"),
        bets: vec![],
        scale: 100.0,
        balance: Mutex::new(None),
        info: SessionInfo::default(),
    };
    let init = s.call("game_init", json!({ "ver": "1.0" })).await?;
    let decimals = init["decimals"].as_u64().unwrap_or(2).min(8) as u32;
    s.scale = 10f64.powi(decimals as i32);
    let cfg = s.call("game_request", json!({ "ver": "1.0", "id": "config" })).await?;
    s.bets = cfg["availableBets"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect()).unwrap_or_default();
    let bet_levels: Vec<BetLevel> = s.bets.iter().map(|b| BetLevel { raw: b.to_string(), minor: (b * s.scale).round() as i64 }).collect();
    let default_bet = cfg["betIndex"].as_u64().and_then(|i| bet_levels.get(i as usize)).map(|b| b.minor);
    let mut features = Vec::new();
    if init["partner_config"]["disableBuyBonus"].as_bool() != Some(true) {
        if let Some(coeffs) = cfg["buyBonusCoeffs"].as_array() {
            for (i, c) in coeffs.iter().filter_map(|c| c.as_f64()).enumerate() {
                let (id, name) = if i < 2 { (format!("level:{}", i + 1), format!("Spin level {}", i + 1)) } else { (format!("buy:{}", i - 1), format!("Buy bonus {}", i - 1)) };
                features.push(Feature { id, name: format!("{name} (×{})", short(c)), cost_mult: c, rtp: None });
            }
        } else if let Some(m) = cfg["bonusCostMultiplier"].as_f64() {
            features.push(Feature { id: "buy".into(), name: format!("Buy bonus (×{})", short(m)), cost_mult: m, rtp: None });
        }
    }
    let balance = cfg["balance"].as_f64().map(|b| (b * s.scale).round() as i64);
    *s.balance.get_mut() = balance;
    let currency = cfg["currency"].as_str().or(init["currency"].as_str()).unwrap_or("XXX").to_uppercase();
    s.info = SessionInfo { currency, decimals, bet_levels, default_bet, features, balance };
    Ok(s)
}

fn short(c: f64) -> String {
    if c.fract().abs() < 1e-6 {
        format!("{}", c.round() as i64)
    } else {
        format!("{c}")
    }
}

/// Free spins still to play after this reply.
fn in_free_spins(r: &Value) -> bool {
    let fs = &r["freespins"];
    (fs["freespinsTriggered"].as_bool() == Some(true) || fs["isFreespins"].as_bool() == Some(true)) && fs["isLastFreespin"].as_bool() != Some(true)
}

impl OnlyplaySession {
    async fn call(&self, path: &str, body: Value) -> Result<Value, SlotError> {
        let url = format!("{}/{}?{}", self.api, path, self.query);
        let res = self.http.post(&url).header("Content-Type", "application/json").body(body.to_string()).send().await.map_err(|e| SlotError::Bridge(format!("Onlyplay: {e}")))?;
        let status = res.status().as_u16();
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        let text = res.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != 200 || v["success"].as_bool() == Some(false) {
            let msg = v["message"].as_str().unwrap_or("").to_string();
            let lower = msg.to_ascii_lowercase();
            if v["code"].as_i64() == Some(9001) || lower.contains("insufficient") || lower.contains("balance") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || status == 401 || status == 403 {
                return Err(SlotError::SessionExpired);
            }
            let detail = if msg.is_empty() { text.chars().take(160).collect() } else { msg };
            return Err(SlotError::Provider(format!("Onlyplay HTTP {status}: {detail}")));
        }
        Ok(v)
    }
}

#[async_trait]
impl SlotSession for OnlyplaySession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        // the bet goes by its index in the game's list
        let index = self
            .bets
            .iter()
            .enumerate()
            .min_by_key(|(_, b)| ((**b * self.scale).round() as i64 - bet).abs())
            .map(|(i, _)| i)
            .ok_or_else(|| SlotError::Provider("This game offers no bets".into()))?;
        let bet_minor = (self.bets[index] * self.scale).round() as i64;
        let mult = feature.and_then(|f| self.info.features.iter().find(|x| x.id == f)).map(|f| f.cost_mult).unwrap_or(1.0);
        let cost = (bet_minor as f64 * mult).round() as i64;
        let level = |prefix: &str| feature.and_then(|f| f.strip_prefix(prefix)).and_then(|n| n.parse::<u64>().ok());
        let first = match (feature, level("level:"), level("buy:")) {
            (Some("buy"), _, _) => json!({ "ver": "1.0", "id": "buy", "betIndex": index }),
            (_, Some(n), _) => json!({ "ver": "1.0", "id": "spin", "betIndex": index, "spinLevel": n }),
            (_, _, Some(n)) => json!({ "ver": "1.0", "id": "buy", "betIndex": index, "buyBonusLevel": n }),
            (Some(f), _, _) => return Err(SlotError::Provider(format!("Unknown feature {f}"))),
            (None, _, _) => json!({ "ver": "1.0", "id": "spin", "betIndex": index, "spinLevel": null }),
        };
        let before = *self.balance.lock().await;
        let mut r = self.call("game_request", first).await?;
        let mut steps = 1usize;
        // free spins to play: leave them for the browser
        let held = hold && in_free_spins(&r);
        let mut free_spins = held;
        while !held && in_free_spins(&r) && steps < 500 {
            free_spins = true;
            r = self.call("game_request", json!({ "ver": "1.0", "id": "spin", "betIndex": index, "spinLevel": null })).await?;
            steps += 1;
        }
        let balance = r["balance"].as_f64().map(|b| (b * self.scale).round() as i64);
        *self.balance.lock().await = balance;
        // one call: the reply says what it won; a free-spin round: the replies' win fields
        // differ per game, but the balance (which holds the win) doesn't lie
        let reported = r["finalWin"].as_f64().filter(|w| *w > 0.0).or(r["spin"]["totalWin"].as_f64()).map(|w| (w * self.scale).round() as i64);
        let win = match (steps, reported, before, balance) {
            (1, Some(w), _, _) => w,
            (_, _, Some(b0), Some(b1)) => (b1 - b0 + cost).max(0),
            (_, w, _, _) => w.unwrap_or(0),
        };
        Ok(SpinResult {
            round_id: String::new(),
            cost,
            win,
            multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
            balance,
            pending: held,
            free_spins,
            held,
            events: steps,
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn close(&self) {}
}
