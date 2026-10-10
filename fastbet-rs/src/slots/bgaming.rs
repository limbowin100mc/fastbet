//! BGaming driver. The launcher redirects to an "interlayer" page whose
//! `_target` is the game page on demo./beta.bgaming-network.com; that page
//! embeds a JSON config with the per-session API endpoint:
//!   POST {api} {"command":"init"}                       -> options.available_bets (minor units),
//!                                                          default_bet, currency{code, exponent}, balance.wallet
//!   POST {api} {"command":"spin","options":{"bet":N}}  -> outcome.win, balance.wallet, flow{state, available_actions}
//! Rounds with free spins continue with {"command":"freespin"} until flow.state is "closed".

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
        if url.contains("bgaming-network.com/games/") {
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
        let api = extract_json_string(&html, "\"api\":\"").ok_or_else(|| SlotError::Provider("Could not find the game API in the game page".into()))?;
        let origin = api.split('/').take(3).collect::<Vec<_>>().join("/");
        let bridge = Bridge::open(&format!("{origin}/robots.txt")).await.map_err(SlotError::Bridge)?;

        let (status, init) = match bridge.post_json(&api, &[], &json!({ "command": "init" })).await {
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
            balance: init["balance"]["wallet"].as_i64(),
        };
        Ok(Self { bridge, api, info })
    }
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

    async fn spin(&self, bet: i64, _feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let t0 = std::time::Instant::now();
        let mut body = json!({ "command": "spin", "options": { "bet": bet } });
        let mut total_win = 0i64;
        let mut balance = None;
        let mut round_id = String::new();
        let mut events = 0usize;
        let mut free_spins = false;
        for _ in 0..400 {
            let (status, res) = match self.bridge.post_json(&self.api, &[], &body).await {
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
            if let Some(b) = res["balance"]["wallet"].as_i64() {
                balance = Some(b);
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
            pending: false,
            free_spins,
            events,
            ms,
        })
    }

    fn close(&self) {
        self.bridge.close();
    }
}
