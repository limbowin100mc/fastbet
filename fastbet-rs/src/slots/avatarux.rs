//! AvatarUX driver (their own RGS, reached through Hub88). Protocol, captured
//! from the game client:
//!   POST {server}/authenticate  {wallet, operator, key, provider, game}   (header x-session-id)
//!     -> {token, balance, currency, currencyDecimals}
//!   POST {server}/game/info     {game, provider}                           (Bearer token)
//!     -> {bets: {main|ante|bonus-buy-N: {available[], default, coin}}, config{rtpPerFeature}, betLimits}
//!   POST {server}/game/play     {game, provider, action, complete:false, asyncWin:false, bet}
//!     -> {roundId, wager{win, data[{totalWin, bonus}], next}, balance}
//! Amounts are in currency units (floats); we keep minor units internally.

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use serde_json::{json, Value};

use super::*;
use crate::bridge::Bridge;

#[derive(Clone, Debug)]
pub struct Launch {
    pub server: String,
    pub cdn_page: String,
    pub game: String,
    pub provider: String,
    pub wallet: String,
    pub operator: String,
    pub key: String,
    pub currency: String,
}

impl Launch {
    /// Parses the game's index.html URL: `?game=..&wallet=..&operator=..&key=..&server=..&provider=..`
    pub fn from_url(url: &str) -> Option<Self> {
        let map = super::hacksaw::query_map(url);
        let server = map.get("server")?.clone();
        Some(Self {
            server,
            cdn_page: url.to_string(),
            game: map.get("game")?.clone(),
            provider: map.get("provider").cloned().unwrap_or_else(|| "avatarux".into()),
            wallet: map.get("wallet").cloned().unwrap_or_else(|| "demo".into()),
            operator: map.get("operator").cloned().unwrap_or_else(|| "b2b".into()),
            key: map.get("key").cloned().filter(|k| !k.is_empty()).unwrap_or_else(|| format!("{}_{}", now_ms(), 30)),
            currency: map.get("currency").cloned().unwrap_or_default(),
        })
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub struct AvataruxSession {
    bridge: Bridge,
    launch: Launch,
    token: String,
    session_id: String,
    info: SessionInfo,
    seq: AtomicU64,
}

impl AvataruxSession {
    pub async fn open(launch: Launch) -> Result<Self, SlotError> {
        // Park on the RGS origin itself so every call is same-origin: rate limits and
        // errors then come back as readable status codes instead of opaque fetch failures.
        let bridge = Bridge::open(&format!("{}/", launch.server)).await.map_err(SlotError::Bridge)?;
        let _ = &launch.cdn_page;
        let session_id = uuid_v4();
        let auth_body = json!({ "wallet": launch.wallet, "operator": launch.operator, "key": launch.key, "provider": launch.provider, "game": launch.game });
        let (status, auth) = match bridge.post_json(&format!("{}/authenticate", launch.server), &[("x-session-id", &session_id)], &auth_body).await {
            Ok(v) => v,
            Err(e) => {
                bridge.close();
                return Err(SlotError::Bridge(e));
            }
        };
        if status != 200 {
            let msg = auth["error"]["message"].as_str().unwrap_or("rejected").to_string();
            bridge.close();
            return Err(SlotError::Provider(format!("authenticate HTTP {status}: {msg}")));
        }
        let token = auth["token"].as_str().ok_or_else(|| SlotError::Provider("no token".into()))?.to_string();
        let decimals = auth["currencyDecimals"].as_u64().unwrap_or(2) as u32;
        let currency = auth["currency"].as_str().map(|c| c.to_uppercase()).unwrap_or_else(|| launch.currency.to_uppercase());
        let scale = 10f64.powi(decimals as i32);
        let balance = auth["balance"].as_f64().map(|b| (b * scale).round() as i64);

        let bearer = format!("Bearer {token}");
        let info_body = json!({ "game": launch.game, "provider": launch.provider });
        let (status, info) = match bridge.post_json(&format!("{}/game/info", launch.server), &[("x-session-id", &session_id), ("Authorization", &bearer)], &info_body).await {
            Ok(v) => v,
            Err(e) => {
                bridge.close();
                return Err(SlotError::Bridge(e));
            }
        };
        if status != 200 {
            bridge.close();
            return Err(SlotError::Provider(format!("game/info HTTP {status}")));
        }
        let main = &info["bets"]["main"];
        let bet_levels: Vec<BetLevel> = main["available"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_f64()).map(|b| BetLevel { raw: b.to_string(), minor: (b * scale).round() as i64 }).collect())
            .unwrap_or_default();
        let default_bet = main["default"].as_f64().map(|b| (b * scale).round() as i64);
        // every other bet mode is a feature; its cost multiple is coin/mainCoin
        let main_coin = main["coin"].as_f64().unwrap_or(1.0).max(1e-9);
        let mut features = Vec::new();
        if let Some(modes) = info["bets"].as_object() {
            for (name, m) in modes {
                if name == "main" {
                    continue;
                }
                let coin = m["coin"].as_f64().unwrap_or(main_coin);
                let cost = coin / main_coin;
                let rtp = info["config"]["rtpPerFeature"][name].as_f64();
                features.push(Feature { id: name.clone(), name: feature_name(name, cost), cost_mult: cost, rtp });
            }
        }
        features.sort_by(|a, b| a.cost_mult.partial_cmp(&b.cost_mult).unwrap_or(std::cmp::Ordering::Equal));
        // the "recover" call the client makes clears any stale round state
        let _ = bridge
            .post_json(&format!("{}/game/recover", launch.server), &[("x-session-id", &session_id), ("Authorization", &bearer)], &json!({ "game": launch.game, "provider": launch.provider, "complete": false }))
            .await;
        Ok(Self { bridge, launch, token, session_id, info: SessionInfo { currency, decimals, bet_levels, default_bet, features, balance }, seq: AtomicU64::new(0) })
    }
}

fn feature_name(id: &str, cost: f64) -> String {
    let base = match id {
        "ante" => "Ante bet".to_string(),
        "bonus-buy-1" => "Bonus buy 1".to_string(),
        "bonus-buy-2" => "Bonus buy 2".to_string(),
        "bonus-buy-3" => "Bonus buy 3".to_string(),
        other => other.replace('-', " "),
    };
    format!("{base} (×{})", if cost.fract().abs() < 1e-6 { format!("{}", cost.round() as i64) } else { format!("{cost:.2}") })
}

pub fn uuid_v4() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}", h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7], h[8], h[9], h[10], h[11], h[12], h[13], h[14], h[15])
}

#[async_trait]
impl SlotSession for AvataruxSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let scale = 10f64.powi(self.info.decimals as i32);
        let action = feature.unwrap_or("main");
        // feature modes have their own bet lists: the same index as the main level
        let bet_units = if action == "main" {
            bet as f64 / scale
        } else {
            let mult = self.info.features.iter().find(|f| f.id == action).map(|f| f.cost_mult).unwrap_or(1.0);
            (bet as f64 / scale) * mult
        };
        let bet_units = (bet_units * scale).round() / scale;
        self.seq.fetch_add(1, Ordering::SeqCst);
        let body = json!({ "game": self.launch.game, "provider": self.launch.provider, "action": action, "complete": false, "asyncWin": false, "bet": bet_units });
        let bearer = format!("Bearer {}", self.token);
        let t0 = std::time::Instant::now();
        let mut attempt = 0;
        let (status, res) = loop {
            let r = match self.bridge.post_json(&format!("{}/game/play", self.launch.server), &[("x-session-id", &self.session_id), ("Authorization", &bearer)], &body).await {
                Ok(v) => v,
                Err(e) if e.starts_with("HTTP 429") => return Err(SlotError::RateLimited),
                Err(e) => return Err(SlotError::Bridge(e)),
            };
            // the RGS answers the first play of a session with a transient "Application Error";
            // the real client simply retries
            if r.0 == 400 && r.1["error"]["code"].as_str() == Some("APPLICATION_ERROR") && attempt < 4 {
                attempt += 1;
                tracing::info!("avatarux play rejected (APPLICATION_ERROR), retry {attempt}");
                tokio::time::sleep(std::time::Duration::from_millis(700 * attempt as u64)).await;
                continue;
            }
            break r;
        };
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        if status == 401 || status == 403 {
            return Err(SlotError::SessionExpired);
        }
        if status != 200 {
            let msg = res["error"]["message"].as_str().unwrap_or("").to_string();
            let code = res["error"]["code"].as_str().unwrap_or("").to_string();
            if msg.to_ascii_lowercase().contains("balance") || code.to_ascii_lowercase().contains("balance") || code.to_ascii_lowercase().contains("funds") {
                return Err(SlotError::Balance);
            }
            return Err(SlotError::Provider(format!("play HTTP {status}: {code} {msg}")));
        }
        let wager = &res["wager"];
        let win_units = wager["win"].as_f64().or_else(|| wager["data"].as_array().and_then(|d| d.last()).and_then(|s| s["totalWin"].as_f64())).unwrap_or(0.0);
        let win = (win_units * scale).round() as i64;
        let cost = (bet_units * scale).round() as i64;
        let free_spins = wager["data"].as_array().map(|d| d.len() > 1 || d.iter().any(|s| s["bonus"].as_bool().unwrap_or(false))).unwrap_or(false);
        let pending = !wager["next"].is_null();
        let round_id = res["roundId"].as_str().unwrap_or("").to_string();
        let mut balance = res["balance"].as_f64().map(|b| (b * scale).round() as i64);
        // The round stays open server-side until the client closes it; the next play is
        // rejected otherwise. The close reply carries the settled balance.
        if !round_id.is_empty() {
            let close = json!({ "game": self.launch.game, "provider": self.launch.provider, "roundId": round_id });
            match self.bridge.post_json(&format!("{}/game/complete", self.launch.server), &[("x-session-id", &self.session_id), ("Authorization", &bearer)], &close).await {
                Ok((200, c)) => {
                    if let Some(b) = c["balance"].as_f64() {
                        balance = Some((b * scale).round() as i64);
                    }
                }
                Ok((st, c)) => tracing::warn!("avatarux game/complete HTTP {st}: {}", c),
                Err(e) => return Err(SlotError::Bridge(e)),
            }
        }
        Ok(SpinResult {
            round_id,
            cost,
            win,
            multiplier: if cost > 0 { win as f64 / cost as f64 } else { 0.0 },
            balance,
            pending,
            free_spins,
            events: wager["data"].as_array().map(|d| d.len()).unwrap_or(0),
            ms,
        })
    }

    fn close(&self) {
        self.bridge.close();
    }
}

#[allow(dead_code)]
fn _unused(_: Value) {}
