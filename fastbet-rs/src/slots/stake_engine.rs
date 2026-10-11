//! Stake Engine driver: the game server (RGS) every Stake Engine game runs on.
//! The protocol is Stake Engine's public one (its web SDK's `rgs-requests`):
//!   POST https://{rgs_url}/wallet/authenticate {sessionID, language}
//!     -> balance{amount, currency}, config{betLevels[], defaultBetLevel,
//!        gameModes[{mode, costMultiplier, maxBet}]}, round (one still open)
//!   POST /wallet/play {sessionID, amount, currency, mode}
//!     -> balance, round{betID, amount, payout, payoutMultiplier, active, mode, state[]}
//!   POST /wallet/end-round {sessionID} -> balance
//! The whole round, free spins included, comes back from one play. A round that
//! pays stays `active` until end-round settles it, and the next play is refused
//! ("player has active round") until then. Amounts are integers with six
//! decimals (1_000_000 = 1 USD); a mode costs `amount × costMultiplier`.
//! Errors come back as HTTP 4xx `{error: "ERR_…", message}`.
//!
//! The RGS sits behind Cloudflare, which turns plain HTTP clients away and drops
//! calls that come too fast: the fetch fails outright and the server stays shut
//! for a few seconds. Measured from one PC it allowed a short burst and then
//! about one or two calls a second, and got stricter the more often it was
//! tripped. So calls are spaced out, the spacing grows on every refusal and
//! shrinks slowly while calls go through, and a refused call is retried here
//! after a pause (a refused end-round must not cost us the round's result).

use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};

use super::*;
use crate::bridge::{Bridge, BridgeRequest};

/// RGS amounts carry six decimals.
pub const DECIMALS: u32 = 6;
const TIMEOUT: Duration = Duration::from_secs(20);
/// Spacing between calls: where it starts, its floor and its ceiling.
const GAP_START: Duration = Duration::from_millis(250);
const GAP_MIN: Duration = Duration::from_millis(150);
const GAP_MAX: Duration = Duration::from_millis(2500);
/// Calls that have to go through before the spacing shrinks a step.
const RELAX_AFTER: u32 = 60;
/// Pauses before retrying a refused call; after the last one the refusal is reported.
const COOLDOWNS_MS: &[u64] = &[2000, 3000, 5000, 8000];

/// Launch parameters from the game URL Stake hands out:
/// `https://<studio>.live.engine.io/<game>/<version>/?sessionID=…&rgs_url=rgs2.engine.io&currency=USD…`
#[derive(Clone, Debug)]
pub struct Launch {
    /// RGS origin, `https://rgs2.engine.io` (demo sessions use `rgsd`).
    pub rgs: String,
    pub session: String,
    pub currency: String,
    pub language: String,
}

impl Launch {
    pub fn from_url(url: &str) -> Option<Self> {
        let map = super::hacksaw::query_map(url);
        let session = map.get("sessionID").filter(|s| !s.is_empty())?.clone();
        let host = map.get("rgs_url").map(|h| h.trim().trim_end_matches('/')).filter(|h| !h.is_empty())?;
        let rgs = if host.starts_with("http") { host.to_string() } else { format!("https://{host}") };
        Some(Self {
            rgs,
            session,
            currency: map.get("currency").cloned().unwrap_or_else(|| "USD".into()),
            language: map.get("lang").cloned().unwrap_or_else(|| "en".into()),
        })
    }
}

/// Spaces calls out and learns how close together the server accepts them.
struct Pacer {
    next: Instant,
    gap: Duration,
    ok: u32,
}

impl Pacer {
    fn new() -> Self {
        Self { next: Instant::now(), gap: GAP_START, ok: 0 }
    }

    /// Books the next slot and returns when it starts.
    fn slot(&mut self) -> Instant {
        let at = self.next.max(Instant::now());
        self.next = at + self.gap;
        at
    }

    fn went_through(&mut self) {
        self.ok += 1;
        if self.ok >= RELAX_AFTER {
            self.ok = 0;
            self.gap = self.gap.saturating_sub(Duration::from_millis(25)).max(GAP_MIN);
        }
    }

    /// The server shut us out: wider spacing, and nothing goes out for `pause`.
    fn refused(&mut self, pause: Duration) {
        self.ok = 0;
        self.gap = (self.gap.mul_f64(1.5) + Duration::from_millis(100)).min(GAP_MAX);
        self.next = self.next.max(Instant::now() + pause);
    }
}

/// The bridge parked on the RGS, with the session it plays.
struct Rgs {
    bridge: Bridge,
    launch: Launch,
    pacer: Mutex<Pacer>,
}

impl Rgs {
    /// One call, paced, retried after a pause while Cloudflare refuses it.
    async fn call(&self, path: &str, body: &Value) -> Result<Value, SlotError> {
        self.timed(path, body).await.map(|(v, _)| v)
    }

    /// [`Rgs::call`], also returning how long the call that went through took (pauses excluded).
    async fn timed(&self, path: &str, body: &Value) -> Result<(Value, Duration), SlotError> {
        let mut refusals = 0usize;
        loop {
            let at = self.pacer.lock().slot();
            tokio::time::sleep_until(at.into()).await;
            let t0 = Instant::now();
            match self.once(path, body).await.map(|v| (v, t0.elapsed())) {
                Err(SlotError::RateLimited) => {
                    let Some(ms) = COOLDOWNS_MS.get(refusals) else { return Err(SlotError::RateLimited) };
                    refusals += 1;
                    tracing::info!("stake engine: {path} refused, pausing {ms} ms");
                    self.pacer.lock().refused(Duration::from_millis(*ms));
                }
                res => {
                    if res.is_ok() {
                        self.pacer.lock().went_through();
                    }
                    return res;
                }
            }
        }
    }

    async fn once(&self, path: &str, body: &Value) -> Result<Value, SlotError> {
        let req = BridgeRequest {
            method: "POST".into(),
            url: format!("{}{path}", self.launch.rgs),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: Some(body.to_string()),
        };
        let res = match self.bridge.request_within(req, TIMEOUT).await {
            Ok(r) => r,
            // Cloudflare's refusal never reaches the page as a response, only as a failed fetch
            Err(e) if e.contains("Failed to fetch") => return Err(SlotError::RateLimited),
            Err(e) => return Err(SlotError::Bridge(e)),
        };
        if res.status != 200 {
            return Err(rgs_error(res.status, &res.body));
        }
        serde_json::from_str(&res.body).map_err(|_| SlotError::Provider(format!("Bad reply: {}", snippet(&res.body))))
    }

    /// Settles the open round; the balance after it (`None` when there was no round) and the call's time.
    async fn end_round(&self) -> Result<(Option<i64>, Duration), SlotError> {
        match self.timed("/wallet/end-round", &json!({ "sessionID": self.launch.session })).await {
            Ok((v, took)) => Ok((v["balance"]["amount"].as_i64(), took)),
            Err(SlotError::Provider(m)) if no_round(&m) => Ok((None, Duration::ZERO)),
            Err(e) => Err(e),
        }
    }
}

pub struct StakeEngineSession {
    rgs: Rgs,
    /// The base mode as the game spells it ("base", "BASE").
    base: String,
    info: SessionInfo,
}

impl StakeEngineSession {
    pub async fn open(launch: Launch) -> Result<Self, SlotError> {
        // Same-origin with the RGS: its Cloudflare turns plain HTTP clients away.
        let bridge = Bridge::open(&format!("{}/", launch.rgs)).await.map_err(SlotError::Bridge)?;
        let rgs = Rgs { bridge, launch, pacer: Mutex::new(Pacer::new()) };
        match Self::authenticate(&rgs).await {
            Ok((base, info)) => Ok(Self { rgs, base, info }),
            Err(e) => {
                rgs.bridge.close();
                Err(match e {
                    SlotError::RateLimited => SlotError::Provider("Stake Engine's server is refusing requests from this connection for now. Wait a minute and retry.".into()),
                    e => e,
                })
            }
        }
    }

    async fn authenticate(rgs: &Rgs) -> Result<(String, SessionInfo), SlotError> {
        let launch = &rgs.launch;
        let auth = rgs.call("/wallet/authenticate", &json!({ "sessionID": launch.session, "language": launch.language })).await?;
        let config = &auth["config"];
        let modes = modes(config);
        let base = modes.iter().find(|(m, _)| m.eq_ignore_ascii_case("base")).map(|(m, _)| m.clone()).unwrap_or_else(|| "base".into());
        // the base mode's ceiling applies to the bet level itself
        let max = config["gameModes"]
            .as_array()
            .and_then(|a| a.iter().find(|m| m["mode"].as_str().is_some_and(|s| s.eq_ignore_ascii_case(&base))))
            .and_then(|m| m["maxBet"].as_i64())
            .or(config["maxBet"].as_i64())
            .filter(|m| *m > 0);
        let min = config["minBet"].as_i64().unwrap_or(0);
        let bet_levels: Vec<BetLevel> = config["betLevels"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).filter(|v| *v > 0 && *v >= min && max.is_none_or(|m| *v <= m)).map(|v| BetLevel { raw: v.to_string(), minor: v }).collect())
            .unwrap_or_default();
        if bet_levels.is_empty() {
            return Err(SlotError::Provider("The game offered no bet sizes".into()));
        }
        let default_bet = config["defaultBetLevel"].as_i64().filter(|d| bet_levels.iter().any(|l| l.minor == *d));
        let features = modes
            .iter()
            .filter(|(m, _)| *m != base)
            .map(|(m, cost)| Feature { id: m.clone(), name: mode_name(m, *cost), cost_mult: *cost, rtp: None })
            .collect();
        let currency = auth["balance"]["currency"].as_str().map(str::to_string).unwrap_or_else(|| launch.currency.clone()).to_ascii_uppercase();
        let mut balance = auth["balance"]["amount"].as_i64();
        // a round left open by an earlier session blocks every play until it is settled
        if auth["round"]["active"].as_bool() == Some(true) {
            if let (Some(b), _) = rgs.end_round().await? {
                balance = Some(b);
            }
        }
        let info = SessionInfo { currency, decimals: DECIMALS, bet_levels, default_bet, features, balance };
        Ok((base, info))
    }

    fn cost_mult(&self, mode: &str) -> f64 {
        self.info.features.iter().find(|f| f.id == mode).map(|f| f.cost_mult).unwrap_or(1.0)
    }
}

/// Bet modes with their cost multiplier. Newer servers send `gameModes`, the SDK's schema `betModes`.
fn modes(config: &Value) -> Vec<(String, f64)> {
    let mut out: Vec<(String, f64)> = Vec::new();
    if let Some(list) = config["gameModes"].as_array() {
        for m in list {
            if let Some(name) = m["mode"].as_str() {
                out.push((name.to_string(), m["costMultiplier"].as_f64().unwrap_or(1.0)));
            }
        }
    } else if let Some(map) = config["betModes"].as_object() {
        for (key, m) in map {
            let name = m["mode"].as_str().unwrap_or(key);
            out.push((name.to_string(), m["costMultiplier"].as_f64().unwrap_or(1.0)));
        }
    }
    out.retain(|(_, cost)| cost.is_finite() && *cost > 0.0);
    out
}

/// "superbonus", 1000 -> "Superbonus (×1000)"; "free_spins" -> "Free spins (×100)".
fn mode_name(mode: &str, cost: f64) -> String {
    let words = mode.replace(['_', '-'], " ").to_ascii_lowercase();
    let mut chars = words.chars();
    let name = match chars.next() {
        Some(c) => c.to_ascii_uppercase().to_string() + chars.as_str(),
        None => mode.to_string(),
    };
    let cost = if cost.fract() == 0.0 { format!("{}", cost as i64) } else { format!("{cost}") };
    format!("{name} (×{cost})")
}

fn active_round(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains("has active round")
}

fn no_round(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains("not have active round")
}

fn snippet(body: &str) -> String {
    body.trim().chars().take(160).collect()
}

/// Maps an RGS error reply (status codes from the Stake Engine schema) to a slot error.
fn rgs_error(status: u16, body: &str) -> SlotError {
    if status == 429 {
        return SlotError::RateLimited;
    }
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        // an unknown session is refused before it reaches the game: plain "Bad Request"
        return match status {
            400 | 401 => SlotError::SessionExpired,
            403 => SlotError::Provider(format!("The game server refused the request (HTTP 403): {}", snippet(body))),
            _ => SlotError::Provider(format!("HTTP {status}: {}", snippet(body))),
        };
    };
    let code = v["error"].as_str().or(v["status"]["statusCode"].as_str()).unwrap_or("").to_ascii_uppercase();
    let msg = v["message"].as_str().or(v["status"]["statusMessage"].as_str()).unwrap_or("").to_string();
    let lower = msg.to_ascii_lowercase();
    match code.as_str() {
        "ERR_IPB" => SlotError::Balance,
        "ERR_IS" | "ERR_ATE" => SlotError::SessionExpired,
        "ERR_GLE" => SlotError::Provider("Your gambling limit on Stake was reached".into()),
        _ if lower.contains("insufficient") || lower.contains("balance") => SlotError::Balance,
        _ if lower.contains("session") && (lower.contains("invalid") || lower.contains("expired") || lower.contains("timeout")) => SlotError::SessionExpired,
        _ => SlotError::Provider(match (msg.is_empty(), code.is_empty()) {
            (false, _) => msg,
            (true, false) => code,
            (true, true) => format!("HTTP {status}"),
        }),
    }
}

/// A free-spin round, from the math SDK's book events ("freegame" reveals, freeSpin* events).
fn has_free_spins(state: &[Value]) -> bool {
    state.iter().any(|e| {
        let kind = e["type"].as_str().unwrap_or("").to_ascii_lowercase();
        let game = e["gameType"].as_str().unwrap_or("").to_ascii_lowercase();
        kind.starts_with("freespin") || game.starts_with("free")
    })
}

/// Integer amount; some servers send whole numbers as floats.
fn amount(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64))
}

#[async_trait]
impl SlotSession for StakeEngineSession {
    fn info(&self) -> &SessionInfo {
        &self.info
    }

    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError> {
        let mode = feature.unwrap_or(&self.base);
        let body = json!({ "sessionID": self.rgs.launch.session, "amount": bet, "currency": self.info.currency, "mode": mode });
        let (res, mut took) = match self.rgs.timed("/wallet/play", &body).await {
            // a round we never saw settle (a reply lost on the way): settle it, then play
            Err(SlotError::Provider(m)) if active_round(&m) => {
                self.rgs.end_round().await?;
                self.rgs.timed("/wallet/play", &body).await?
            }
            other => other?,
        };
        let round = &res["round"];
        let mut balance = res["balance"]["amount"].as_i64();
        let win = amount(&round["payout"]).unwrap_or(0);
        let charged = (round["amount"].as_i64().unwrap_or(bet) as f64 * self.cost_mult(mode)).round() as i64;
        if round["active"].as_bool() == Some(true) {
            let (settled, end_took) = self.rgs.end_round().await?;
            took += end_took;
            if let Some(b) = settled {
                balance = Some(b);
            }
        }
        // server time only: the pacing between calls isn't latency
        let ms = took.as_secs_f64() * 1000.0;
        let state = round["state"].as_array().map(Vec::as_slice).unwrap_or(&[]);
        Ok(SpinResult {
            round_id: round["betID"].as_i64().map(|i| i.to_string()).or_else(|| round["betID"].as_str().map(str::to_string)).unwrap_or_default(),
            cost: charged,
            win,
            multiplier: if charged > 0 { win as f64 / charged as f64 } else { 0.0 },
            balance,
            pending: false,
            free_spins: has_free_spins(state),
            events: state.len(),
            ms,
        })
    }

    fn close(&self) {
        self.rgs.bridge.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_url() {
        let l = Launch::from_url("https://valkyrie.live.engine.io/odins-vault/v13/?sessionID=abc-_DEF&rgs_url=rgs2.engine.io&lang=en&currency=USD&device=desktop&social=false&demo=false").unwrap();
        assert_eq!(l.rgs, "https://rgs2.engine.io");
        assert_eq!(l.session, "abc-_DEF");
        assert_eq!(l.currency, "USD");
        assert!(Launch::from_url("https://valkyrie.live.engine.io/odins-vault/v13/?rgs_url=rgs2.engine.io").is_none());
    }

    #[test]
    fn modes_and_names() {
        let config = json!({"gameModes": [{"mode": "base", "costMultiplier": 1.0}, {"mode": "ante", "costMultiplier": 3.0}, {"mode": "bonus", "costMultiplier": 200.0}]});
        assert_eq!(modes(&config), vec![("base".into(), 1.0), ("ante".into(), 3.0), ("bonus".into(), 200.0)]);
        let old = json!({"betModes": {"BASE": {"mode": "BASE", "costMultiplier": 1}}});
        assert_eq!(modes(&old), vec![("BASE".into(), 1.0)]);
        assert_eq!(mode_name("superbonus", 1000.0), "Superbonus (×1000)");
        assert_eq!(mode_name("free_spins", 2.5), "Free spins (×2.5)");
    }

    #[test]
    fn errors() {
        assert!(matches!(rgs_error(429, "Too Many Requests"), SlotError::RateLimited));
        assert!(matches!(rgs_error(400, "Bad Request"), SlotError::SessionExpired));
        assert!(matches!(rgs_error(400, r#"{"error":"ERR_IPB","message":"insufficient balance"}"#), SlotError::Balance));
        assert!(matches!(rgs_error(400, r#"{"error":"ERR_IS","message":"session timeout"}"#), SlotError::SessionExpired));
        assert!(matches!(rgs_error(400, r#"{"error":"ERR_VAL","message":"player has active round"}"#), SlotError::Provider(m) if active_round(&m)));
        assert!(matches!(rgs_error(400, r#"{"error":"ERR_VAL","message":"player does not have active round"}"#), SlotError::Provider(m) if no_round(&m) && !active_round(&m)));
        assert!(matches!(rgs_error(400, r#"{"error":"ERR_VAL","message":"invalid amount","round":{}}"#), SlotError::Provider(m) if m == "invalid amount"));
    }

    #[test]
    fn free_spin_rounds() {
        let base = vec![json!({"index": 0, "type": "reveal", "gameType": "basegame"}), json!({"type": "finalWin"})];
        assert!(!has_free_spins(&base));
        let fs = vec![json!({"type": "reveal", "gameType": "basegame"}), json!({"type": "freeSpinTrigger"}), json!({"type": "reveal", "gameType": "freegame"})];
        assert!(has_free_spins(&fs));
    }

    #[test]
    fn pacing() {
        let mut p = Pacer::new();
        let a = p.slot();
        let b = p.slot();
        assert_eq!(b - a, GAP_START);
        p.refused(Duration::from_secs(2));
        assert!(p.gap > GAP_START);
        assert!(p.slot() >= Instant::now() + Duration::from_millis(1900));
        for _ in 0..30 {
            p.refused(Duration::ZERO);
        }
        assert_eq!(p.gap, GAP_MAX);
        let wide = p.gap;
        for _ in 0..RELAX_AFTER {
            p.went_through();
        }
        assert_eq!(p.gap, wide - Duration::from_millis(25));
        for _ in 0..RELAX_AFTER * 200 {
            p.went_through();
        }
        assert_eq!(p.gap, GAP_MIN);
    }
}
