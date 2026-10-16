//! BGaming driver (Popiplay runs the same platform). The launcher redirects to an "interlayer" page whose
//! `_target` is the game page on demo./beta.bgaming-network.com; that page
//! embeds a JSON config with the per-session API endpoint (and a CSRF header Popiplay checks):
//!   POST {api} {"command":"init"}                       -> options.available_bets (minor units),
//!                                                          default_bet, currency{code, exponent}, balance.wallet
//!   POST {api} {"command":"spin","options":{"bet":N}}  -> outcome.win, balance.wallet, flow{state, available_actions}
//! Rounds with free spins continue with {"command":"freespin"} until flow.state is "closed".
//! Newer games live on their own host, `<game>.demo.bgaming-network.com/hyperhive?launch_token=…`,
//! whose page names a `play_token`; the game speaks JSON-RPC 2.0 to `<host>/api`:
//!   init {token, req:{}}        -> balance, currency_attributes{code, exponent}, config{bet_limits,
//!                                  default_bet}, state (an open bonus), state_lock
//!   play {token, req:{bet}, state_lock} -> final, balance, resp.round{id, win}, state_lock (sent
//!                                  back with the next call); some games want req {bet, bet_type:"bet", bid:0}
//! Their bonuses are each game's own (picks, gambles, buys by product), so a round that isn't
//! `final` is left for the game in the browser, which picks it up from `init`.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;
use crate::bridge::Bridge;

#[derive(Clone, Debug)]
pub struct Launch {
    /// The game page (demo.bgaming-network.com/games/<Game>/<CUR>?launch_token=…).
    pub page_url: String,
}

impl Launch {
    /// From the interlayer URL (or a direct game page URL).
    pub fn from_url(url: &str) -> Option<Self> {
        // the game page, or its newer per-game host's `/hyperhive` page (same embedded config)
        let page = url.contains("/games/") || url.contains("/hyperhive?");
        if page && (url.contains("bgaming-network.com/") || url.contains("popiplay.network/")) {
            return Some(Self { page_url: url.to_string() });
        }
        let map = super::hacksaw::query_map(url);
        let target = map.get("_target")?.clone();
        let base = if url.contains("beta-interl") || url.contains("bg-beta") {
            "https://beta.bgaming-network.com"
        } else {
            "https://demo.bgaming-network.com"
        };
        Some(Self { page_url: format!("{base}{target}") })
    }
}

pub struct BgamingSession {
    bridge: Bridge,
    api: String,
    /// (header, value) the page asks every API call to carry
    csrf: Option<(String, String)>,
    /// A hyperhive game: its play token and the state lock of the last reply.
    hive: Option<(String, tokio::sync::Mutex<String>)>,
    /// Which spin request the hyperhive game takes: 0 not known yet, 1 `{bet}`, 2 with bet_type and bid.
    shape: std::sync::atomic::AtomicU8,
    info: SessionInfo,
}

impl BgamingSession {
    pub async fn open(launch: Launch) -> Result<Self, SlotError> {
        // The game page carries the per-session API URL in its embedded config.
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| SlotError::Bridge(e.to_string()))?;
        let html = client.get(&launch.page_url).send().await.map_err(|e| SlotError::Bridge(format!("game page: {e}")))?.text().await.unwrap_or_default();
        if launch.page_url.contains("/hyperhive?") {
            return Self::open_hive(&launch.page_url, &html).await;
        }
        let api = extract_json_string(&html, "\"api\":\"").ok_or_else(|| SlotError::Provider("Could not find the game API in the game page".into()))?;
        let csrf = extract_json_string(&html, "\"csrfTokenHeaderValue\":\"").map(|v| (extract_json_string(&html, "\"csrfTokenHeaderName\":\"").unwrap_or_else(|| "X-CSRF-Token".into()), v));
        let origin = api.split('/').take(3).collect::<Vec<_>>().join("/");
        let bridge = Bridge::open(&format!("{origin}/robots.txt")).await.map_err(SlotError::Bridge)?;

        let headers: Vec<(&str, &str)> = csrf.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (status, init) = match bridge.post_json(&api, &headers, &json!({ "command": "init" })).await {
            Ok(v) => v,
            Err(e) => {
                bridge.close();
                return Err(SlotError::Bridge(e));
            }
        };
        if status != 200 {
            bridge.close();
            return Err(SlotError::Provider(format!("init HTTP {status}: {}", err_msg(&init))));
        }
        let opts = &init["options"];
        let currency = opts["currency"]["code"].as_str().unwrap_or("FUN").to_uppercase();
        let decimals = opts["currency"]["exponent"].as_u64().unwrap_or(2) as u32;
        let bet_levels: Vec<BetLevel> = opts["available_bets"].as_array().map(|a| a.iter().filter_map(|v| v.as_i64()).map(|m| BetLevel { raw: m.to_string(), minor: m }).collect()).unwrap_or_default();
        let info = SessionInfo {
            currency,
            decimals,
            bet_levels,
            default_bet: opts["default_bet"].as_i64(),
            features: vec![],
            balance: init["balance"]["wallet"].as_i64().map(|w| w + init["balance"]["game"].as_i64().unwrap_or(0)),
        };
        Ok(Self { bridge, api, csrf, hive: None, shape: Default::default(), info })
    }

    async fn open_hive(page_url: &str, html: &str) -> Result<Self, SlotError> {
        let token = extract_json_string(html, "\"play_token\":\"").ok_or_else(|| SlotError::Provider("Could not find the play token in the game page".into()))?;
        let origin = page_url.split('/').take(3).collect::<Vec<_>>().join("/");
        let api = format!("{origin}/api");
        let bridge = Bridge::open(&format!("{origin}/robots.txt")).await.map_err(SlotError::Bridge)?;
        let init = match rpc(&bridge, &api, "init", json!({ "token": token, "req": {} })).await {
            Ok(v) => v,
            Err(e) => {
                bridge.close();
                return Err(e);
            }
        };
        if !init["state"].is_null() {
            bridge.close();
            return Err(SlotError::Provider("This game has a bonus open: finish it in your browser first".into()));
        }
        let cur = &init["currency_attributes"];
        let info = SessionInfo {
            currency: cur["code"].as_str().or(init["currency"].as_str()).unwrap_or("FUN").to_uppercase(),
            decimals: cur["exponent"].as_u64().unwrap_or(2) as u32,
            bet_levels: init["config"]["bet_limits"].as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|m| BetLevel { raw: m.to_string(), minor: m }).collect()).unwrap_or_default(),
            default_bet: init["config"]["default_bet"].as_i64(),
            features: vec![],
            balance: init["balance"].as_i64(),
        };
        let lock = init["state_lock"].as_str().unwrap_or("").to_string();
        Ok(Self { bridge, api, csrf: None, hive: Some((token, tokio::sync::Mutex::new(lock))), shape: Default::default(), info })
    }

    /// One hyperhive spin; a round that doesn't end in it is left for the browser.
    async fn spin_hive(&self, token: &str, lock: &tokio::sync::Mutex<String>, bet: i64) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        use std::sync::atomic::Ordering;
        let mut lock = lock.lock().await;
        let known = self.shape.load(Ordering::Relaxed);
        let shapes: &[u8] = match known {
            0 => &[1, 2],
            1 => &[1],
            _ => &[2],
        };
        let mut r = Value::Null;
        for (i, &shape) in shapes.iter().enumerate() {
            let req = if shape == 1 { json!({ "bet": bet }) } else { json!({ "bet": bet, "bet_type": "bet", "bid": 0 }) };
            let mut params = json!({ "token": token, "req": req });
            if !lock.is_empty() {
                params["state_lock"] = json!(*lock);
            }
            match rpc(&self.bridge, &self.api, "play", params).await {
                Ok(v) => {
                    self.shape.store(shape, Ordering::Relaxed);
                    r = v;
                    break;
                }
                // the game wants the other request ("Something went wrong")
                Err(SlotError::Provider(_)) if i + 1 < shapes.len() => continue,
                Err(e) => return Err(e),
            }
        }
        if let Some(l) = r["state_lock"].as_str() {
            *lock = l.to_string();
        }
        let round = &r["resp"]["round"];
        let done = r["final"].as_bool() != Some(false);
        let amount = |v: &Value| v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok()));
        let win = if done { amount(&round["win"]).unwrap_or(0) } else { 0 };
        Ok(SpinResult {
            round_id: round["id"].as_str().unwrap_or("").to_string(),
            cost: bet,
            win,
            multiplier: if bet > 0 { win as f64 / bet as f64 } else { 0.0 },
            balance: r["balance"].as_i64(),
            pending: !done,
            free_spins: !done,
            held: !done,
            events: 1,
            ms: t0.elapsed().as_secs_f64() * 1000.0,
        })
    }
}

/// A JSON-RPC call to a hyperhive game -> its `result`.
async fn rpc(bridge: &Bridge, api: &str, method: &str, params: Value) -> Result<Value, SlotError> {
    let body = json!({ "id": "1", "jsonrpc": "2.0", "method": method, "params": params });
    let (status, res) = match bridge.post_json(api, &[], &body).await {
        Ok(v) => v,
        Err(e) if e.starts_with("HTTP 429") => return Err(SlotError::RateLimited),
        Err(e) => return Err(SlotError::Bridge(e)),
    };
    if status == 401 || status == 403 {
        return Err(SlotError::SessionExpired);
    }
    if let Some(err) = res.get("error").filter(|e| !e.is_null()) {
        let msg = err["message"].as_str().map(str::to_string).unwrap_or_else(|| err.to_string());
        let lower = msg.to_ascii_lowercase();
        if lower.contains("balance") || lower.contains("funds") {
            return Err(SlotError::Balance);
        }
        if lower.contains("session") || lower.contains("token") {
            return Err(SlotError::SessionExpired);
        }
        return Err(SlotError::Provider(format!("BGaming {method}: {msg}")));
    }
    if status != 200 {
        return Err(SlotError::Provider(format!("BGaming {method} HTTP {status}")));
    }
    Ok(res["result"].clone())
}

fn err_msg(v: &Value) -> String {
    v["error"].as_str().or(v["message"].as_str()).or(v["error"]["message"].as_str()).unwrap_or("").to_string()
}

/// Finds `"key":"value"` in an HTML/JSON blob and returns the unescaped value.
fn extract_json_string(hay: &str, key: &str) -> Option<String> {
    let start = hay.find(key)? + key.len();
    let rest = &hay[start..];
    let end = rest.find('"')?;
    Some(rest[..end].replace("\\/", "/").replace("\\u0026", "&"))
}

#[async_trait]
impl SlotSession for BgamingSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, _feature: Option<&str>, hold: bool) -> Result<SpinResult, SlotError> {
        if let Some((token, lock)) = &self.hive {
            return self.spin_hive(token, lock, bet).await;
        }
        let t0 = std::time::Instant::now();
        let mut body = json!({ "command": "spin", "options": { "bet": bet } });
        let mut total_win = 0i64;
        let mut balance = None;
        let mut round_id = String::new();
        let mut events = 0usize;
        let mut free_spins = false;
        let mut held = false;
        for _ in 0..400 {
            let headers: Vec<(&str, &str)> = self.csrf.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            let (status, res) = match self.bridge.post_json(&self.api, &headers, &body).await {
                Ok(v) => v,
                Err(e) if e.starts_with("HTTP 429") => return Err(SlotError::RateLimited),
                Err(e) => return Err(SlotError::Bridge(e)),
            };
            if status == 401 || status == 403 {
                return Err(SlotError::SessionExpired);
            }
            if status != 200 {
                let msg = err_msg(&res);
                let lower = msg.to_ascii_lowercase();
                if lower.contains("balance") || lower.contains("funds") {
                    return Err(SlotError::Balance);
                }
                if lower.contains("session") {
                    return Err(SlotError::SessionExpired);
                }
                return Err(SlotError::Provider(format!("spin HTTP {status}: {msg}")));
            }
            events += 1;
            total_win += res["outcome"]["win"].as_i64().unwrap_or(0);
            // the round's win sits in `game` until the next action moves it to the wallet
            if let Some(b) = res["balance"]["wallet"].as_i64() {
                balance = Some(b + res["balance"]["game"].as_i64().unwrap_or(0));
            }
            if round_id.is_empty() {
                round_id = res["flow"]["round_id"].as_i64().map(|r| r.to_string()).or_else(|| res["flow"]["round_id"].as_str().map(str::to_string)).unwrap_or_default();
            }
            let state = res["flow"]["state"].as_str().unwrap_or("closed");
            let actions: Vec<String> = res["flow"]["available_actions"].as_array().map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect()).unwrap_or_default();
            if state == "closed" || state == "ready" {
                break;
            }
            // a round still open: continue with whatever the server offers next
            let next = ["freespin", "respin", "collect", "cascade", "bonus"].iter().find(|a| actions.iter().any(|x| x == *a)).map(|s| s.to_string());
            match next {
                // free spins to play: leave them for the browser
                Some(cmd) if hold && cmd == "freespin" => {
                    free_spins = true;
                    held = true;
                    break;
                }
                Some(cmd) => {
                    free_spins |= cmd == "freespin";
                    body = json!({ "command": cmd });
                }
                None => break,
            }
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        Ok(SpinResult {
            round_id,
            cost: bet,
            win: total_win,
            multiplier: if bet > 0 { total_win as f64 / bet as f64 } else { 0.0 },
            balance,
            pending: held,
            free_spins,
            held,
            events,
            ms,
        })
    }

    fn close(&self) {
        self.bridge.close();
    }
}
