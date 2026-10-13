//! Slotmill driver. The launcher lands on `<game>.<host>/?currency=..&org=..&key=..&server=..`
//! (an empty `key` is free play). Protocol (from the game client), all
//! `POST {server}/game.web/service?<query>` with an empty body and JSON replies `{code, data}`:
//!   fn=gameinit&org&lang&reference&client=<game>&(currency&multiplier=100 | key)
//!     -> user{sessid, balance{cash}}, game{gameid}, betAmounts{amounts[], defaultAmount,
//!        buyBonus[{id, multiplier, amounts[]}]}, pending{data[]}
//!   fn=play&minimal=Y&sessid&gameid&currency&amount&prepaid=no&buyBonus=<id|0>&burstMode=false&reference&extraBet=false
//!     -> wager{wagerid, status Finished|Pending, bets[{betid, step, betamount, wonamount}]},
//!        buyBal{cash} (after the bet), resultBal{cash} (after the win)
//! A Pending wager is finished with fn=play&amount=0&cmd=C (collect) on its wagerid / betid /
//! next step. A bonus buy sends the buy's own amount (multiplier × bet). Amounts are decimal
//! strings in currency units. Plain HTTPS, no browser needed.

use async_trait::async_trait;
use serde_json::Value;

use super::*;

#[derive(Clone, Debug)]
struct Buy {
    id: String,
    /// buy price (minor units) for each base bet (minor units), by position
    amounts: Vec<i64>,
}

pub struct SlotmillSession {
    http: reqwest::Client,
    server: String,
    org: String,
    sessid: String,
    gameid: String,
    currency: String,
    buys: Vec<Buy>,
    info: SessionInfo,
}

fn minor(v: &Value) -> Option<i64> {
    let f = v.as_str().and_then(|s| s.parse::<f64>().ok()).or(v.as_f64())?;
    Some((f * 100.0).round() as i64)
}

fn units(m: i64) -> String {
    format!("{}.{:02}", m / 100, m % 100)
}

pub async fn open(launch_url: &str) -> Result<SlotmillSession, SlotError> {
    let url = resolve_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let map = hacksaw::query_map(&url);
    let (Some(server), Some(org)) = (map.get("server"), map.get("org")) else {
        return Err(SlotError::Provider(format!("Could not read the launch parameters from {url}")));
    };
    // the game's own subdomain is its client id: tiki-thunder.<host>
    let client = url.split("://").nth(1).and_then(|h| h.split(['.', '/']).next()).unwrap_or("").to_string();
    let currency = map.get("currency").cloned().unwrap_or_else(|| "EUR".into());
    let http = reqwest::Client::builder().user_agent(UA).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let mut s = SlotmillSession {
        http,
        server: server.trim_end_matches('/').to_string(),
        org: org.clone(),
        sessid: String::new(),
        gameid: String::new(),
        currency: currency.clone(),
        buys: vec![],
        info: SessionInfo::default(),
    };
    let reference = avatarux::uuid_v4();
    let mut q: Vec<(&str, String)> = vec![("fn", "gameinit".into()), ("org", org.clone()), ("lang", "en".into()), ("reference", reference), ("client", client)];
    match map.get("key").filter(|k| !k.is_empty()) {
        Some(key) => q.push(("key", key.clone())),
        None => {
            q.push(("currency", currency.clone()));
            q.push(("multiplier", "100".into()));
        }
    }
    let d = s.call(&q).await?;
    s.sessid = d["user"]["sessid"].as_str().unwrap_or_default().to_string();
    s.gameid = d["game"]["gameid"].as_i64().map(|g| g.to_string()).or_else(|| d["game"]["gameid"].as_str().map(str::to_string)).unwrap_or_default();
    if let Some(c) = d["user"]["balance"]["currency"].as_str().or(d["user"]["currency"].as_str()) {
        s.currency = c.to_string();
    }
    let amounts = &d["betAmounts"];
    let bet_levels: Vec<BetLevel> = amounts["amounts"].as_array().map(|a| a.iter().filter_map(|v| Some(BetLevel { raw: v.as_str()?.to_string(), minor: minor(v)? })).collect()).unwrap_or_default();
    let mut features = Vec::new();
    for b in amounts["buyBonus"].as_array().into_iter().flatten() {
        let Some(id) = b["id"].as_i64().map(|i| i.to_string()) else { continue };
        let mult = b["multiplier"].as_f64().unwrap_or(1.0);
        let prices: Vec<i64> = b["amounts"].as_array().map(|a| a.iter().filter_map(minor).collect()).unwrap_or_default();
        features.push(Feature { id: id.clone(), name: format!("Buy bonus {} (×{})", b["name"].as_str().unwrap_or(&id), if mult.fract() == 0.0 { format!("{}", mult as i64) } else { format!("{mult}") }), cost_mult: mult, rtp: None });
        s.buys.push(Buy { id, amounts: prices });
    }
    s.info = SessionInfo {
        currency: s.currency.to_uppercase(),
        decimals: amounts["roundDecimals"].as_u64().unwrap_or(2).min(2) as u32,
        default_bet: minor(&amounts["defaultAmount"]),
        bet_levels,
        features,
        balance: minor(&d["user"]["balance"]["cash"]),
    };
    // a round left open is collected first
    if let Some(w) = d["pending"]["data"].as_array().and_then(|a| a.first()).cloned() {
        if w["status"].as_str() == Some("Pending") {
            let _ = s.collect(&w).await;
        }
    }
    Ok(s)
}

impl SlotmillSession {
    async fn call(&self, query: &[(&str, String)]) -> Result<Value, SlotError> {
        let url = format!("{}/game.web/service", self.server);
        let res = self
            .http
            .post(&url)
            .query(query)
            .header("Accept", "application/json, text/javascript, */*")
            .header("Content-Length", "0")
            .send()
            .await
            .map_err(|e| SlotError::Bridge(format!("Slotmill: {e}")))?;
        let status = res.status().as_u16();
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        let text = res.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let code = v["code"].as_i64().unwrap_or(-1);
        if status != 200 || code != 0 {
            let msg = v["msg"].as_str().unwrap_or("").to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("insufficient") || lower.contains("balance") || lower.contains("funds") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || status == 401 || status == 403 {
                return Err(SlotError::SessionExpired);
            }
            let detail = if msg.is_empty() { text.chars().take(160).collect() } else { msg };
            return Err(SlotError::Provider(format!("Slotmill HTTP {status} code {code}: {detail}")));
        }
        Ok(v["data"].clone())
    }

    /// Finishes a Pending wager (collect); returns the last reply.
    async fn collect(&self, wager: &Value) -> Result<Value, SlotError> {
        let mut w = wager.clone();
        let mut last_reply = Value::Null;
        for _ in 0..20 {
            if w["status"].as_str() != Some("Pending") {
                break;
            }
            let last = w["bets"].as_array().and_then(|b| b.last()).cloned().unwrap_or(Value::Null);
            let q = vec![
                ("fn", "play".to_string()),
                ("amount", "0".into()),
                ("minimal", "true".into()),
                ("org", self.org.clone()),
                ("lang", "en".into()),
                ("gameid", self.gameid.clone()),
                ("currency", self.currency.clone()),
                ("sessid", self.sessid.clone()),
                ("wagerid", w["wagerid"].as_str().unwrap_or("").to_string()),
                ("betid", last["betid"].as_i64().unwrap_or(1).to_string()),
                ("step", (last["step"].as_i64().unwrap_or(0) + 1).to_string()),
                ("reference", avatarux::uuid_v4()),
                ("cmd", "C".into()),
            ];
            last_reply = self.call(&q).await?;
            w = last_reply["wager"].clone();
        }
        Ok(last_reply)
    }
}

#[async_trait]
impl SlotSession for SlotmillSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let (amount, buy) = match feature {
            Some(f) => {
                let b = self.buys.iter().find(|b| b.id == f).ok_or_else(|| SlotError::Provider(format!("Unknown feature {f}")))?;
                // buy prices line up with the bet sizes
                let idx = self.info.bet_levels.iter().position(|l| l.minor == bet).ok_or_else(|| SlotError::Provider("Pick one of the game's bet sizes".into()))?;
                let price = *b.amounts.get(idx).ok_or_else(|| SlotError::Provider("This buy isn't offered at this bet size; pick another bet".into()))?;
                (price, b.id.clone())
            }
            None => (bet, "0".to_string()),
        };
        let q = vec![
            ("fn", "play".to_string()),
            ("minimal", "Y".into()),
            ("sessid", self.sessid.clone()),
            ("gameid", self.gameid.clone()),
            ("currency", self.currency.clone()),
            ("amount", units(amount)),
            ("prepaid", "no".into()),
            ("buyBonus", buy),
            ("burstMode", "false".into()),
            ("reference", avatarux::uuid_v4()),
            ("extraBet", "false".into()),
        ];
        let first = self.call(&q).await?;
        let after_bet = minor(&first["buyBal"]["cash"]);
        let steps = first["wager"]["bets"].as_array().map(|b| b.len()).unwrap_or(1);
        let free_spins = feature.is_some()
            || first["wager"]["bets"].as_array().is_some_and(|b| b.iter().any(|x| x["eventdata"]["freeSpinsTotal"].as_i64().unwrap_or(0) > 0 || x["eventdata"]["freeSpins"].as_i64().unwrap_or(0) > 0));
        let open = first["wager"]["status"].as_str() == Some("Pending");
        // the bonus is decided already; left uncollected, the browser shows it
        let held = hold && free_spins && open;
        let last = if open && !held { self.collect(&first["wager"]).await? } else { first.clone() };
        let balance = minor(&last["resultBal"]["cash"]).or(minor(&first["resultBal"]["cash"]));
        // the server's own balances: after the bet, and after the round paid out
        let win = if held {
            // the browser shows the bonus and what it pays
            0
        } else {
            match (after_bet, balance) {
                (Some(a), Some(b)) => (b - a).max(0),
                _ => last["wager"]["bets"].as_array().map(|b| b.iter().filter_map(|x| minor(&x["wonamount"])).sum()).unwrap_or(0),
            }
        };
        Ok(SpinResult {
            round_id: first["wager"]["wagerid"].as_str().unwrap_or("").to_string(),
            cost: amount,
            win,
            multiplier: if amount > 0 { win as f64 / amount as f64 } else { 0.0 },
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
