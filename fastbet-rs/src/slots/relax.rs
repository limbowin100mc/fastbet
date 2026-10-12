//! Relax Gaming driver. The launcher lands on `<cdn>/casino/launcher.html?gameid=..&partnerid=..
//! &jurisdiction=..&channel=..&moneymode=fun|real&ticket=..`. Protocol (from the game client),
//! plain HTTPS JSON:
//!   GET  <cdn>/casino/apiconfig.json                         -> clientapiroot
//!   GET  {clientapiroot}/capi/2.0/casino/games/getclientconfig?gameref&jurisdiction&partnerid&channel
//!        -> gameServerApi, tokenApi
//!   fun:  POST {gameServerApi}/funmoneylogin {userid:null, mode:"dev", partnerid, channel, clientid:"",
//!         g, immediatePayouts:false, clientversion, jurisdiction}
//!   real: POST {tokenApi} {gameref, ticket, partnerid, mode, channel, clientid, clientversion, lang}
//!         -> response{token, userid}; POST {gameServerApi}/rmlogin {token, userid, mode, partnerid,
//!         channel, clientid, g, immediatePayouts:false, acceptFreespinsMidsession:true, clientversion}
//!     -> sid, stats{b, currency}, limits{legalBets[], defaultBet}, config{buyFeatureCost?, buyFeatureRtp?}
//!   POST {gameServerApi}/play {g, sid, ga:"spin", ba, restoreState, restoreVersion, buyFeature?}
//!     -> ended, win, ba, stats{b}; a round goes on with more "spin" plays while `ended` is false
//!   POST {gameServerApi}/gamefinished {g, sid, restoreVersion}  -> {b}
//! Amounts are minor units. A bonus buy plays `ba` = bet × buyFeatureCost with buyFeature:true.
//! The casino-client API (config, token) sits behind a bot filter that only lets browsers in, so
//! those two calls go through the browser bridge, parked on the launcher's origin like the game;
//! the game server itself is plain HTTPS.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::*;
use crate::bridge::{Bridge, BridgeRequest};

const CLIENT_VERSION: &str = "1.0.0";

pub struct RelaxSession {
    http: reqwest::Client,
    server: String,
    game: String,
    sid: String,
    version: AtomicU64,
    balance: Mutex<Option<i64>>,
    info: SessionInfo,
}

pub async fn open(launch_url: &str) -> Result<RelaxSession, SlotError> {
    let url = resolve_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let map = hacksaw::query_map(&url);
    let Some(game) = map.get("gameid").cloned() else {
        return Err(SlotError::Provider(format!("Could not read the launch parameters from {url}")));
    };
    let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
    let partner = map.get("partnerid").cloned().unwrap_or_else(|| "1".into());
    let jurisdiction = map.get("jurisdiction").cloned().unwrap_or_default();
    let channel = map.get("channel").cloned().unwrap_or_else(|| "web".into());
    let real = map.get("moneymode").map(|m| m == "real").unwrap_or(false) || map.contains_key("ticket");
    let http = reqwest::Client::builder().user_agent(UA).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let api: Value = http
        .get(format!("{origin}/casino/apiconfig.json"))
        .send()
        .await
        .map_err(|e| SlotError::Bridge(format!("Relax: {e}")))?
        .json()
        .await
        .map_err(|e| SlotError::Provider(format!("Relax config: {e}")))?;
    let root = api["clientapiroot"].as_str().unwrap_or("https://casino-client.api.relaxg.com").trim_end_matches('/').to_string();
    let bridge = Bridge::open(&format!("{origin}/casino/apiconfig.json")).await.map_err(SlotError::Bridge)?;
    let browser = |method: &str, url: String, body: Option<Value>| {
        let req = BridgeRequest {
            method: method.into(),
            url,
            headers: if body.is_some() { vec![("Content-Type".into(), "application/json".into())] } else { vec![] },
            body: body.map(|b| b.to_string()),
        };
        let bridge = &bridge;
        async move {
            let r = bridge.request(req).await.map_err(SlotError::Bridge)?;
            serde_json::from_str::<Value>(&r.body).map_err(|_| SlotError::Provider(format!("Relax casino API HTTP {}: {}", r.status, r.body.chars().take(120).collect::<String>())))
        }
    };
    let cfg = match browser("GET", format!("{root}/capi/2.0/casino/games/getclientconfig?gameref={game}&jurisdiction={jurisdiction}&partnerid={partner}&channel={channel}"), None).await {
        Ok(c) => c,
        Err(e) => {
            bridge.close();
            return Err(e);
        }
    };
    let server = cfg["gameServerApi"].as_str().map(|s| s.trim_end_matches('/').to_string());
    let Some(server) = server else {
        bridge.close();
        return Err(SlotError::Provider("Relax didn't name its game server".into()));
    };

    let mut s = RelaxSession { http, server, game: game.clone(), sid: String::new(), version: AtomicU64::new(0), balance: Mutex::new(None), info: SessionInfo::default() };
    let token = if real {
        let ticket = map.get("ticket").cloned().unwrap_or_default();
        let token_api = cfg["tokenApi"].as_str().map(str::to_string).unwrap_or_else(|| format!("{root}/capi/2.0/casino/token/gettoken"));
        Some(browser("POST", token_api, Some(json!({ "gameref": game, "ticket": ticket, "partnerid": partner, "mode": "dev", "channel": channel, "clientid": "", "clientversion": CLIENT_VERSION, "lang": map.get("lang").cloned().unwrap_or_else(|| "en_US".into()) }))).await)
    } else {
        None
    };
    // the browser was only needed for those two calls
    bridge.close();
    let login = if let Some(t) = token {
        let t = t?;
        if t["status"].as_str() != Some("ok") {
            return Err(SlotError::Provider(format!("Relax refused the session: {}", t["status"].as_str().unwrap_or("?"))));
        }
        s.post("rmlogin", json!({ "token": t["response"]["token"], "userid": t["response"]["userid"], "mode": "dev", "partnerid": partner, "channel": channel, "clientid": "", "g": game, "immediatePayouts": false, "acceptFreespinsMidsession": true, "clientversion": CLIENT_VERSION })).await?
    } else {
        s.post("funmoneylogin", json!({ "userid": null, "mode": "dev", "partnerid": partner, "channel": channel, "clientid": "", "g": game, "immediatePayouts": false, "clientversion": CLIENT_VERSION, "jurisdiction": jurisdiction })).await?
    };
    s.sid = login["sid"].as_str().ok_or_else(|| SlotError::Provider("Relax login gave no session".into()))?.to_string();
    let bets: Vec<i64> = login["limits"]["legalBets"].as_array().or(login["config"]["betSizeOptions"].as_array()).map(|a| a.iter().filter_map(|v| v.as_i64()).collect()).unwrap_or_default();
    let mut features = Vec::new();
    if let Some(cost) = login["config"]["buyFeatureCost"].as_f64().filter(|c| *c > 0.0) {
        let rtp = login["config"]["buyFeatureRtp"].as_f64().map(|r| if r <= 1.0 { r * 100.0 } else { r });
        features.push(Feature { id: "buy".into(), name: format!("Buy bonus (×{})", if cost.fract() == 0.0 { format!("{}", cost as i64) } else { format!("{cost}") }), cost_mult: cost, rtp });
    }
    let balance = login["stats"]["b"].as_i64();
    *s.balance.get_mut() = balance;
    s.info = SessionInfo {
        currency: login["stats"]["currency"].as_str().unwrap_or("FUN").to_uppercase(),
        decimals: 2,
        bet_levels: bets.iter().map(|b| BetLevel { raw: b.to_string(), minor: *b }).collect(),
        default_bet: login["limits"]["defaultBet"].as_i64(),
        features,
        balance,
    };
    Ok(s)
}

impl RelaxSession {
    async fn post(&self, action: &str, body: Value) -> Result<Value, SlotError> {
        self.post_url(&format!("{}/{action}", self.server), body).await
    }

    async fn post_url(&self, url: &str, body: Value) -> Result<Value, SlotError> {
        let res = self.http.post(url).header("Content-Type", "application/json").body(body.to_string()).send().await.map_err(|e| SlotError::Bridge(format!("Relax: {e}")))?;
        let status = res.status().as_u16();
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        let text = res.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        // errors come back as 200s with an errorCode
        let code = v["errorCode"].as_i64().or(v["errorcode"].as_i64());
        if status != 200 || code.is_some() {
            let msg = v["errorMessage"].as_str().or(v["errormessage"].as_str()).unwrap_or("").to_string();
            let lower = msg.to_ascii_lowercase();
            if lower.contains("insufficient") || lower.contains("balance") || lower.contains("funds") {
                return Err(SlotError::Balance);
            }
            if lower.contains("session") || lower.contains("sid") || status == 401 || status == 403 {
                return Err(SlotError::SessionExpired);
            }
            let detail = if msg.is_empty() { text.chars().take(160).collect() } else { msg };
            return Err(SlotError::Provider(format!("Relax {} ({}): {detail}", code.map(|c| c.to_string()).unwrap_or_else(|| format!("HTTP {status}")), url.rsplit('/').next().unwrap_or(""))));
        }
        Ok(v)
    }

    fn next_version(&self) -> u64 {
        self.version.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[async_trait]
impl SlotSession for RelaxSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let mult = feature.and_then(|f| self.info.features.iter().find(|x| x.id == f)).map(|f| f.cost_mult).unwrap_or(1.0);
        let ba = (bet as f64 * mult).round() as i64;
        let before = *self.balance.lock().await;
        let mut body = json!({ "g": self.game, "sid": self.sid, "ga": "spin", "ba": ba, "restoreState": "{}", "restoreVersion": self.next_version() });
        if feature.is_some() {
            body["buyFeature"] = json!(true);
        }
        let mut r = self.post("play", body).await?;
        let mut steps = 1usize;
        // a round that isn't over goes on with more spins (respins, bonus rounds)
        while r["ended"].as_bool() == Some(false) && steps < 500 {
            r = self.post("play", json!({ "g": self.game, "sid": self.sid, "ga": "spin", "ba": ba, "restoreState": "{}", "restoreVersion": self.next_version() })).await?;
            steps += 1;
        }
        let finished = self.post("gamefinished", json!({ "g": self.game, "sid": self.sid, "restoreVersion": self.next_version() })).await.ok();
        let balance = finished.as_ref().and_then(|f| f["b"].as_i64()).or(r["stats"]["b"].as_i64());
        *self.balance.lock().await = balance;
        let reported = r["win"].as_i64().unwrap_or(0);
        let win = match (steps, before, balance) {
            (1, _, _) => reported,
            // over several plays the balance tells the whole round's win
            (_, Some(b0), Some(b1)) => (b1 - b0 + ba).max(reported),
            _ => reported,
        };
        let free_spins = feature.is_some() || steps > 1 || r["subgameTriggered"].as_bool() == Some(true) || r["freespins"].is_object() || r["freeSpinAwardedCount"].as_i64().unwrap_or(0) > 0;
        Ok(SpinResult {
            round_id: r["roundId"].as_i64().filter(|i| *i > 0).map(|i| i.to_string()).unwrap_or_default(),
            cost: ba,
            win,
            multiplier: if ba > 0 { win as f64 / ba as f64 } else { 0.0 },
            balance,
            pending: false,
            free_spins,
            events: steps,
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }

    fn close(&self) {}
}
