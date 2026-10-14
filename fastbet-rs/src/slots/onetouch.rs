//! OneTouch driver. The launcher lands on `files.onetouch.io/game/<slug>/latest/index.html`
//! with `session_id` and `engine_base_url` in the query. Protocol (from the game wrapper):
//!   POST {engine}/api/v2/login/platform {sessionId}      -> userSession{gameCategory}; the
//!        `x-auth-token` response header is the token every later call sends back
//!   GET  {engine}/api/v2/slots/<game>/get_state           -> balance, nextSceneState,
//!        config{currency, betSizes[], betSizeIndex, featureBuyInData{TYPE: {buyInCost}}}
//!   POST {engine}/api/v2/slots/<game>/play {betSize, coinValue:1, type}
//!        -> balance, totalBet, roundWin, nextSceneState{sceneId}
//! A round goes on (free spins) with type "SPIN" until `nextSceneState.sceneId` is "main"
//! again. Amounts are in currency units (floats); <game> is the game category in lower case
//! without underscores (GRAB_ME_GOLD -> grabmegold). Plain HTTPS, no browser needed.
//! Some casinos' setups (Shuffle's) price a spin in coins: `betSizes` are coin counts and
//! `coinValues` what a coin is worth, a spin costing betSize × coinValue; they also name a
//! `config_id` in the launch URL, which every call carries as `configId`.

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::*;

pub struct OneTouchSession {
    http: reqwest::Client,
    base: String,
    game: String,
    token: Mutex<String>,
    config_id: Option<String>,
    /// (betSize, coinValue) to send for each bet level, by its minor units.
    combos: HashMap<i64, (f64, f64)>,
    info: SessionInfo,
}

pub async fn open(launch_url: &str) -> Result<OneTouchSession, SlotError> {
    let url = resolve_launcher(launch_url).await.map_err(SlotError::Bridge)?;
    let map = hacksaw::query_map(&url);
    let (Some(session), Some(base)) = (map.get("session_id"), map.get("engine_base_url")) else {
        return Err(SlotError::Provider(format!("Could not read the launch parameters from {url}")));
    };
    let http = reqwest::Client::builder().user_agent(UA).build().map_err(|e| SlotError::Bridge(e.to_string()))?;
    let base = base.trim_end_matches('/').to_string();
    let config_id = map.get("config_id").cloned().filter(|c| !c.is_empty());
    let mut s = OneTouchSession { http, base, game: String::new(), token: Mutex::new(String::new()), config_id, combos: HashMap::new(), info: SessionInfo::default() };

    let mut login_body = json!({ "sessionId": session });
    if let Some(c) = &s.config_id {
        login_body["configId"] = json!(c);
    }
    let login = s.call("POST", "api/v2/login/platform", Some(login_body)).await?;
    let category = login["userSession"]["gameCategory"].as_str().unwrap_or_default();
    s.game = category.to_ascii_lowercase().replace('_', "");
    if s.game.is_empty() {
        return Err(SlotError::Provider("OneTouch didn't say which game this is".into()));
    }
    let query = s.config_id.as_ref().map(|c| format!("?configId={c}")).unwrap_or_default();
    let state = s.call("GET", &format!("api/v2/slots/{}/get_state{query}", s.game), None).await?;
    let cfg = &state["config"];
    let floats = |k: &str| cfg[k].as_array().map(|a| a.iter().filter_map(Value::as_f64).collect::<Vec<f64>>()).unwrap_or_default();
    let sizes = floats("betSizes");
    let mut coins = floats("coinValues");
    if coins.is_empty() {
        coins.push(1.0);
    }
    let pick = |list: &[f64], k: &str| cfg[k].as_u64().and_then(|i| list.get(i as usize).copied());
    let default_coin = pick(&coins, "coinValueIndex").unwrap_or(coins[0]);
    let default_bet = pick(&sizes, "betSizeIndex").map(|b| minor(b * default_coin));
    // the game's own coin value first, so a bet it can make keeps it
    for coin in std::iter::once(default_coin).chain(coins.iter().copied()) {
        for size in &sizes {
            s.combos.entry(minor(size * coin)).or_insert((*size, coin));
        }
    }
    let mut bet_levels: Vec<BetLevel> = s.combos.keys().map(|m| BetLevel { raw: format!("{:.2}", *m as f64 / 100.0), minor: *m }).collect();
    bet_levels.sort_by_key(|b| b.minor);
    let mut features: Vec<Feature> = cfg["featureBuyInData"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(id, d)| {
                    let cost = d["buyInCost"].as_f64()?;
                    Some(Feature { id: id.clone(), name: feature_name(id, cost), cost_mult: cost, rtp: None })
                })
                .collect()
        })
        .unwrap_or_default();
    if cfg["disableBuyIn"].as_bool() == Some(true) {
        features.retain(|f| !f.id.starts_with("BUY_IN"));
    }
    features.sort_by(|a, b| a.cost_mult.partial_cmp(&b.cost_mult).unwrap_or(std::cmp::Ordering::Equal));
    s.info = SessionInfo {
        currency: cfg["currency"].as_str().unwrap_or("FUN").to_uppercase(),
        decimals: 2,
        bet_levels,
        default_bet,
        features,
        balance: state["balance"].as_f64().map(minor),
    };
    // a round left open (free spins in progress) is played out first
    if state["nextSceneState"]["sceneId"].as_str().is_some_and(|sc| sc != "main") {
        let bet = s.info.default_bet.unwrap_or(0);
        let _ = s.finish(bet, "SPIN", false, std::time::Instant::now()).await?;
    }
    Ok(s)
}

fn minor(units: f64) -> i64 {
    (units * 100.0).round() as i64
}

/// BUY_IN -> "Buy free spins", BUY_IN_SUPER -> "Buy super free spins", SPIN_SIDE -> "Side bet".
fn feature_name(id: &str, cost: f64) -> String {
    let base = match id {
        "BUY_IN" => "Buy free spins".to_string(),
        "BUY_IN_SUPER" => "Buy super free spins".to_string(),
        "SPIN_SIDE" => "Side bet".to_string(),
        other => {
            let s = other.replace('_', " ").to_ascii_lowercase();
            format!("{}{}", s.get(0..1).unwrap_or("").to_uppercase(), s.get(1..).unwrap_or(""))
        }
    };
    format!("{base} (×{})", if cost.fract().abs() < 1e-6 { format!("{}", cost.round() as i64) } else { format!("{cost}") })
}

impl OneTouchSession {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Result<Value, SlotError> {
        let url = format!("{}/{}", self.base, path);
        let token = self.token.lock().await.clone();
        let mut req = if method == "POST" { self.http.post(&url) } else { self.http.get(&url) };
        req = req.header("Content-Type", "application/json");
        if !token.is_empty() {
            req = req.header("x-auth-token", token);
        }
        if let Some(b) = body {
            req = req.body(b.to_string());
        }
        let res = req.send().await.map_err(|e| SlotError::Bridge(format!("OneTouch: {e}")))?;
        let status = res.status().as_u16();
        if let Some(t) = res.headers().get("x-auth-token").and_then(|v| v.to_str().ok()).filter(|t| !t.is_empty()) {
            *self.token.lock().await = t.to_string();
        }
        let text = res.text().await.unwrap_or_default();
        if status == 429 {
            return Err(SlotError::RateLimited);
        }
        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if status != 200 {
            let code = v["errors"][0]["code"].as_str().unwrap_or("").to_ascii_lowercase();
            if code.contains("insufficient") || code.contains("balance") || code.contains("funds") {
                return Err(SlotError::Balance);
            }
            if code.contains("session") || status == 401 || status == 403 {
                return Err(SlotError::SessionExpired);
            }
            let detail = if code.is_empty() { text.chars().take(160).collect() } else { code };
            return Err(SlotError::Provider(format!("OneTouch HTTP {status}: {detail}")));
        }
        Ok(v)
    }

    /// Plays `kind` and then the rest of the round; returns the last reply plus the first's cost.
    async fn finish(&self, bet: i64, kind: &str, hold: bool, t0: std::time::Instant) -> Result<SpinResult, SlotError> {
        let path = format!("api/v2/slots/{}/play", self.game);
        let (size, coin) = *self.combos.get(&bet).ok_or_else(|| SlotError::Provider("Pick one of the game's bet sizes".into()))?;
        let mut cost = 0i64;
        let mut steps = 0usize;
        let mut kind = kind.to_string();
        let mut free_spins = false;
        loop {
            let mut body = json!({ "betSize": size, "coinValue": coin, "type": kind });
            if let Some(c) = &self.config_id {
                body["configId"] = json!(c);
            }
            let r = self.call("POST", &path, Some(body)).await?;
            if steps == 0 {
                cost = r["totalBet"].as_f64().map(minor).unwrap_or(bet);
            }
            steps += 1;
            let scene = r["nextSceneState"]["sceneId"].as_str().unwrap_or("main");
            // free spins to play: leave them for the browser
            let held = hold && scene != "main";
            if scene == "main" || steps > 500 || held {
                let win = r["roundWin"].as_f64().map(minor).unwrap_or(0);
                return Ok(SpinResult {
                    round_id: r["gameId"].as_str().unwrap_or("").to_string(),
                    cost,
                    win,
                    multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
                    balance: r["balance"].as_f64().map(minor),
                    pending: scene != "main",
                    free_spins: free_spins || held,
                    held,
                    events: steps,
                    ms: t0.elapsed().as_secs_f64() * 1000.0,
                });
            }
            // a feature scene: the free spins are played as "SPIN" calls at no cost
            free_spins = true;
            kind = "SPIN".into();
        }
    }
}

#[async_trait]
impl SlotSession for OneTouchSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        self.finish(bet, feature.unwrap_or("SPIN"), hold, std::time::Instant::now()).await
    }

    fn close(&self) {}
}
