//! Hacksaw Gaming driver. Protocol (captured from the game client):
//!   POST {env}/play/authenticate {seq, partner, gameId, gameVersion, currency,
//!        languageCode, mode, branding, channel, userAgent, token, currencyMultiplier}
//!     -> sessionUuid, betLevels[], defaultBetLevel, bonusGames[], accountBalance
//!   POST {env}/play/bet {seq, sessionUuid, bets:[{betAmount, bonusGameId?}], ...}
//!     -> round{status, roundId, events[{etn, bc, awa, ...}]}, accountBalance{balance}
//! Amounts are strings in minor units of the session currency.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::json;

use super::*;
use crate::bridge::Bridge;

const STATIC: &str = "https://static-live.hacksawgaming.com";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36 Edg/154.0.0.0";

/// Launch parameters as found in the game's index.html URL.
#[derive(Clone, Debug)]
pub struct Launch {
    pub game_id: u64,
    pub partner: String,
    pub token: String,
    /// 1 = real money, 2 = demo
    pub mode: u8,
    pub env: String,
    pub currency: String,
    pub language: String,
}

impl Launch {
    pub fn demo(game_id: u64) -> Self {
        Self {
            game_id,
            partner: "demo".into(),
            token: "demo".into(),
            mode: 2,
            env: "https://rgs-demo.hacksawgaming.com/api".into(),
            currency: "EUR".into(),
            language: "en".into(),
        }
    }

    /// Parses the query string of a static-launcher / index.html URL.
    pub fn from_url(url: &str, currency_hint: &str) -> Option<Self> {
        let map = query_map(url);
        let game_id = map.get("gameid")?.parse().ok()?;
        let mode_s = map.get("mode").cloned().unwrap_or_default();
        let mode = if mode_s == "demo" || mode_s == "2" { 2 } else { 1 };
        let partner = map.get("partner").cloned().unwrap_or_else(|| "demo".into());
        let token = map.get("token").cloned().unwrap_or_else(|| "demo".into());
        let env = map
            .get(if mode == 1 { "realmoneyenv" } else { "env" })
            .or(map.get("env"))
            .cloned()
            .unwrap_or_else(|| if mode == 2 { "https://rgs-demo.hacksawgaming.com/api".into() } else { "https://rgs.hacksawgaming.com/api".into() });
        Some(Self {
            game_id,
            partner,
            token,
            mode,
            env,
            currency: map.get("currency").cloned().unwrap_or_else(|| currency_hint.to_string()),
            language: map.get("language").cloned().unwrap_or_else(|| "en".into()),
        })
    }
}

/// Query string of a URL as a map, percent-decoded (`#fragment` params included).
pub fn query_map(url: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    let after = url.split_once('?').map(|x| x.1).unwrap_or("");
    for part in after.split(['&', '#']) {
        if let Some((k, v)) = part.split_once('=') {
            map.insert(k.to_string(), urldecode(v));
        }
    }
    map
}

/// The live RGS for a partner, from the launcher's per-partner config.
pub async fn live_env(partner: &str) -> String {
    let fallback = "https://rgs.hacksawgaming.com/api".to_string();
    let url = format!("{STATIC}/launcher/configs/{partner}.json");
    let Ok(client) = reqwest::Client::builder().user_agent(UA).build() else { return fallback };
    match client.get(&url).send().await {
        Ok(r) if r.status().is_success() => match r.json::<serde_json::Value>().await {
            Ok(v) => v["env"].as_str().map(str::to_string).unwrap_or(fallback),
            Err(_) => fallback,
        },
        _ => fallback,
    }
}

fn urldecode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub struct HacksawSession {
    bridge: Bridge,
    launch: Launch,
    session: String,
    seq: AtomicU64,
    info: SessionInfo,
}

impl HacksawSession {
    pub async fn open(launch: Launch) -> Result<Self, SlotError> {
        // The game version comes from the static CDN (plain HTTP is fine there).
        let ver = reqwest::Client::builder()
            .user_agent(UA)
            .build()
            .map_err(|e| SlotError::Bridge(e.to_string()))?
            .get(format!("{STATIC}/{}/version.json", launch.game_id))
            .send()
            .await
            .map_err(|e| SlotError::Bridge(format!("version: {e}")))?
            .json::<serde_json::Value>()
            .await
            .map(|v| v["version"].as_str().unwrap_or("1.0.0").to_string())
            .unwrap_or_else(|_| "1.0.0".to_string());
        // Park the bridge on the RGS origin itself: every call is then same-origin and a
        // Cloudflare challenge page, if any, is solved by the browser engine on load.
        let page = format!("{}/meta/gameInfo?gameId={}&currency={}&rm=96&jurisdiction=curacao&partner={}", launch.env, launch.game_id, launch.currency, launch.partner);
        let bridge = Bridge::open(&page).await.map_err(SlotError::Bridge)?;

        let body = json!({
            "seq": 1,
            "partner": launch.partner,
            "gameId": launch.game_id,
            "gameVersion": ver,
            "currency": launch.currency,
            "languageCode": format!("{}-us", launch.language),
            "mode": launch.mode,
            "branding": "default",
            "channel": 1,
            "userAgent": UA,
            "token": launch.token,
            "currencyMultiplier": 1,
        });
        let (status, auth) = match bridge.post_json(&format!("{}/play/authenticate", launch.env), &[("Access-Control-Allow-Origin", "*")], &body).await {
            Ok(v) => v,
            Err(e) if e.starts_with("HTTP 429") => {
                bridge.close();
                return Err(SlotError::Provider("The provider is rate-limiting new sessions. Wait a minute and try again.".into()));
            }
            Err(e) => {
                bridge.close();
                return Err(SlotError::Bridge(e));
            }
        };
        if status != 200 {
            bridge.close();
            return Err(SlotError::Provider(format!("authenticate HTTP {status}")));
        }
        if let Some(code) = auth.get("statusCode").and_then(|c| c.as_i64()) {
            if code != 0 {
                return Err(SlotError::Provider(format!("authenticate: {}", auth.get("statusMessage").and_then(|m| m.as_str()).unwrap_or("rejected"))));
            }
        }
        let session = auth["sessionUuid"].as_str().ok_or_else(|| SlotError::Provider("no sessionUuid".into()))?.to_string();
        let bet_levels: Vec<BetLevel> = auth["betLevels"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).filter_map(|s| s.parse::<i64>().ok().map(|m| BetLevel { raw: s.to_string(), minor: m })).collect())
            .unwrap_or_default();
        let features = auth["bonusGames"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|b| {
                        let id = b["bonusGameId"].as_str()?.to_string();
                        let cost: f64 = b["betCostMultiplier"].as_str().and_then(|s| s.parse().ok()).or(b["betCostMultiplier"].as_f64()).unwrap_or(1.0);
                        Some(Feature { name: feature_name(&id, cost), id, cost_mult: cost, rtp: b["expectedRtp"].as_f64() })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let balance = auth["accountBalance"]["balance"].as_str().and_then(|s| s.parse().ok());
        let currency = auth["accountBalance"]["currencyCode"].as_str().map(str::to_string).unwrap_or_else(|| launch.currency.clone());
        let info = SessionInfo {
            currency,
            decimals: 2,
            default_bet: auth["defaultBetLevel"].as_str().and_then(|s| s.parse().ok()),
            bet_levels,
            features,
            balance,
        };
        Ok(Self { bridge, launch, session, seq: AtomicU64::new(1), info })
    }
}

fn feature_name(id: &str, cost: f64) -> String {
    let base = match id {
        "fs" => "Free spins".to_string(),
        "fs_2" => "Super free spins".to_string(),
        "mod_bonus" => "Bonus booster".to_string(),
        "mod_boost" => "Feature boost".to_string(),
        other => other.replace('_', " "),
    };
    format!("{base} (×{})", if cost.fract() == 0.0 { format!("{}", cost as i64) } else { format!("{cost}") })
}

#[async_trait]
impl SlotSession for HacksawSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let mut b = json!({ "betAmount": bet.to_string() });
        if let Some(f) = feature {
            b["bonusGameId"] = json!(f);
        }
        let body = json!({ "seq": seq, "sessionUuid": self.session, "bets": [b], "offerId": null, "promotionId": null, "autoplay": true });
        let t0 = std::time::Instant::now();
        let (status, res) = match self.bridge.post_json(&format!("{}/play/bet", self.launch.env), &[("Access-Control-Allow-Origin", "*")], &body).await {
            Ok(v) => v,
            Err(e) if e.starts_with("HTTP 429") => return Err(SlotError::RateLimited),
            Err(e) => return Err(SlotError::Bridge(e)),
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if status != 200 {
            return Err(SlotError::Provider(format!("bet HTTP {status}")));
        }
        Self::check_status(&res, "bet")?;
        let round = &res["round"];
        let events = round["events"].as_array().cloned().unwrap_or_default();
        let win: i64 = events.last().and_then(|e| e["awa"].as_str()).and_then(|s| s.parse().ok()).unwrap_or(0);
        let charged: i64 = events.first().and_then(|e| e["bc"].as_str()).and_then(|s| s.parse().ok()).filter(|c| *c > 0).unwrap_or_else(|| {
            let mult = feature.and_then(|f| self.info.features.iter().find(|x| x.id == f)).map(|f| f.cost_mult).unwrap_or(1.0);
            (bet as f64 * mult).round() as i64
        });
        let round_id = round["roundId"].as_str().unwrap_or("").to_string();
        let mut status_s = round["status"].as_str().unwrap_or("").to_string();
        let free_spins = events.iter().any(|e| {
            let n = e["etn"].as_str().unwrap_or("");
            n.contains("fs") || n.contains("free") || n.contains("bonus")
        });
        let mut balance = res["accountBalance"]["balance"].as_str().and_then(|s| s.parse().ok());

        // A winning round waits for the client to say the win presentation is over
        // before it is settled; the next bet is rejected until then.
        if status_s == "wfwpc" && !round_id.is_empty() {
            let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
            let confirm = json!({
                "seq": seq,
                "sessionUuid": self.session,
                "roundId": round_id,
                "continueInstructions": { "action": "win_presentation_complete" }
            });
            match self.bridge.post_json(&format!("{}/play/bet", self.launch.env), &[("Access-Control-Allow-Origin", "*")], &confirm).await {
                Ok((_, c)) => {
                    Self::check_status(&c, "confirm")?;
                    if let Some(b) = c["accountBalance"]["balance"].as_str().and_then(|s| s.parse().ok()) {
                        balance = Some(b);
                    }
                    status_s = c["round"]["status"].as_str().unwrap_or("completed").to_string();
                }
                Err(e) if e.starts_with("HTTP 429") => return Err(SlotError::RateLimited),
                Err(e) => return Err(SlotError::Bridge(e)),
            }
        }
        // Some games pause for a player choice (pick a chest...). We cannot answer those.
        let choices = round["possibleActions"].as_array().map(|a| a.len()).unwrap_or(0);
        if choices > 0 && status_s != "completed" {
            return Err(SlotError::Provider("This round needs a player choice, which autoplay cannot make".into()));
        }
        Ok(SpinResult {
            round_id,
            cost: charged,
            win,
            multiplier: if charged > 0 { win as f64 / charged as f64 } else { 0.0 },
            balance,
            pending: !(status_s == "completed" || status_s.is_empty()),
            free_spins,
            events: events.len(),
            ms,
        })
    }

    fn close(&self) {
        self.bridge.close();
    }
}

impl HacksawSession {
    fn check_status(res: &serde_json::Value, what: &str) -> Result<(), SlotError> {
        let code = res.get("statusCode").and_then(|c| c.as_i64().or_else(|| c.as_str().and_then(|s| s.parse().ok()))).unwrap_or(0);
        if code == 0 {
            return Ok(());
        }
        let msg = res.get("statusMessage").and_then(|m| m.as_str()).unwrap_or("").to_string();
        let lower = msg.to_ascii_lowercase();
        if code == 10 || lower.contains("session timeout") || lower.contains("session expired") || lower.contains("invalid session") {
            return Err(SlotError::SessionExpired);
        }
        if lower.contains("balance") || lower.contains("funds") {
            return Err(SlotError::Balance);
        }
        Err(SlotError::Provider(format!("{what} rejected ({code}): {msg}")))
    }

}

/// `fastbet --bridge-test [gameId]`: authenticate the demo and spin a few times.
pub async fn self_test(game_id: u64, spins: usize) {
    let t0 = std::time::Instant::now();
    let s = match HacksawSession::open(Launch::demo(game_id)).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("open failed: {e}");
            return;
        }
    };
    println!("opened in {} ms: currency {} bets {:?} default {:?} features {:?} balance {:?}", t0.elapsed().as_millis(), s.info.currency, s.info.bet_levels.iter().map(|b| b.minor).collect::<Vec<_>>(), s.info.default_bet, s.info.features.iter().map(|f| format!("{}={}", f.id, f.cost_mult)).collect::<Vec<_>>(), s.info.balance);
    let bet = s.info.default_bet.unwrap_or(100);
    for i in 0..spins {
        match s.spin(bet, None).await {
            Ok(r) => println!("spin {} round {} cost {} win {} x{:.2} balance {:?} pending {} fs {} events {} {:.0} ms", i + 1, r.round_id, r.cost, r.win, r.multiplier, r.balance, r.pending, r.free_spins, r.events, r.ms),
            Err(e) => println!("spin {} failed: {e}", i + 1),
        }
    }
    if let Some(f) = s.info.features.first().cloned() {
        match s.spin(bet, Some(&f.id)).await {
            Ok(r) => println!("feature {} cost {} win {} x{:.2} balance {:?} pending {} events {}", f.id, r.cost, r.win, r.multiplier, r.balance, r.pending, r.events),
            Err(e) => println!("feature {} failed: {e}", f.id),
        }
    }
    s.close();
}
