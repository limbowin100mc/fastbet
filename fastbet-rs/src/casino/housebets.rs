//! Housebets.com: the account over the site's API (`https://www.housebets.com/api/v1`, signed in
//! with the site's cookies), the House Games (Limbo, Dice, Keno, Mines) over their gateway
//! (`https://www.housebets.com/housegames`).
//!
//! The login is the site's cookies, read by the login window: `__Host-sid` (the session),
//! `__Host-csrf` (sent back as `X-CSRF-Token` on every write), `__Secure-ref` (for the site's
//! own refresh) and Cloudflare's `cf_clearance`, with the window's User Agent (Cloudflare checks
//! a signed-in request against the browser that passed its check, and turns away HTTP/1.1
//! header names written in lowercase). A House Games session comes from `POST /_api/housegames/launch`: its
//! `direct_bootstrap` carries the gateway URL, the coin's id there and a token the gateway takes
//! as `X-Token`. It plays in the coin selected on the account (`PUT /api/v1/wallet/currency`
//! switches it); `/_api/housegames/refresh` renews the token with a cookie the launch sets.
//! Bets of 0 are allowed.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use reqwest::cookie::{CookieStore, Jar};
use reqwest::Method;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::money::{f64_from_json, to_f64};
use crate::originals::{mines_state, Game, Round, Step, Tables};

const SITE: &str = "https://www.housebets.com";
/// The cookies that make the login, as the login window hands them over (the first one must be
/// there), and where each is sent.
pub const LOGIN_COOKIES: &[&str] = &["__Host-sid", "__Host-csrf", "__Secure-ref", "cf_clearance", "__cf_bm"];
pub const LOGIN_URLS: &[&str] = &["https://www.housebets.com/", "https://www.housebets.com/api/v1/auth/refresh"];
/// Amounts travel as numbers in the coin; 8 decimals, as the gateway counts them.
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const REFRESH: Duration = Duration::from_secs(10);
const RATES_EVERY: u32 = 12;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const EXPIRED: &str = "Your Housebets login has expired. Log in again in Settings.";
/// The House Games every seed change renews (each has its own seed pair).
const SEEDED: &[&str] = &["limbo", "dice", "keno2", "mines2"];
/// Mines' board: 5 × 5.
const GRID: u64 = 25;

enum CallError {
    Unauthenticated,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Unauthenticated => CasinoError::Server(EXPIRED.into()),
            CallError::Casino(e) => e,
        }
    }
}

/// A House Games session.
#[derive(Clone, Debug)]
struct Session {
    coin: String,
    /// The gateway's URL (`…/housegames`).
    api: String,
    /// The coin's id at the gateway (USDT is 1).
    code: u64,
    token: String,
}

/// A Mines round in play.
#[derive(Clone, Debug)]
struct MinesPlay {
    round: String,
    seq: u64,
    count: u32,
    coin: String,
    amount: Units,
}

/// The login as saved: `{"cookies": "name=value; …", "ua": "<the login window's User Agent>"}`
/// (a bare cookie line works too).
#[derive(Clone, Debug, PartialEq)]
struct Login {
    cookies: String,
    ua: Option<String>,
}

impl Login {
    fn parse(token: &str) -> Option<Login> {
        let t = token.trim();
        let (cookies, ua) = match serde_json::from_str::<Value>(t) {
            Ok(v) if v.is_object() => (v["cookies"].as_str().unwrap_or("").to_string(), v["ua"].as_str().map(str::to_string).filter(|u| !u.is_empty())),
            _ => (t.to_string(), None),
        };
        let parts: Vec<&str> = cookies.split(';').map(str::trim).filter(|kv| kv.split_once('=').is_some_and(|(k, v)| LOGIN_COOKIES.contains(&k) && !v.is_empty())).collect();
        parts.iter().any(|kv| kv.starts_with("__Host-sid=")).then(|| Login { cookies: parts.join("; "), ua })
    }

    fn to_token(&self) -> String {
        json!({ "cookies": self.cookies, "ua": self.ua }).to_string()
    }
}

pub struct Housebets {
    login: Mutex<Option<Login>>,
    jar: Mutex<Arc<Jar>>,
    client: Mutex<reqwest::Client>,
    wake: Notify,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// The coin selected on the account: the one House Games play in.
    selected: Mutex<String>,
    session: Mutex<Option<Session>>,
    launching: tokio::sync::Mutex<()>,
    /// The gateway takes one bet at a time per account ("duplicate request: please wait before
    /// placing another bet"), so bets queue here instead of bouncing off it.
    betting: tokio::sync::Mutex<()>,
    mines: Mutex<Option<MinesPlay>>,
    mines_epoch: AtomicU64,
    inflight: AtomicUsize,
    settled: AtomicU64,
}

impl Housebets {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let login = setup.token.as_deref().and_then(Login::parse);
        let (jar, client) = client_for(login.as_ref());
        let me = Arc::new(Self {
            login: Mutex::new(login),
            jar: Mutex::new(jar),
            client: Mutex::new(client),
            wake: Notify::new(),
            alive: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            events,
            market: Mutex::new(Market::default()),
            published: Mutex::new(Market::default()),
            selected: Mutex::new(String::new()),
            session: Mutex::new(None),
            launching: tokio::sync::Mutex::new(()),
            betting: tokio::sync::Mutex::new(()),
            mines: Mutex::new(None),
            mines_epoch: AtomicU64::new(0),
            inflight: AtomicUsize::new(0),
            settled: AtomicU64::new(0),
        });
        tokio::spawn(me.clone().run());
        me
    }

    fn status(&self, s: ConnStatus, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!("housebets: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            if self.login.lock().is_none() {
                self.status(ConnStatus::NoToken, "");
                self.wake.notified().await;
                continue;
            }
            self.status(ConnStatus::Connecting, "Opening housebets.com");
            match self.sign_in().await {
                Ok(()) => {
                    retries = 0;
                    self.serve().await;
                }
                Err(CallError::Unauthenticated) => {
                    self.status(ConnStatus::AuthFailed, EXPIRED);
                    self.wake.notified().await;
                }
                Err(e) => {
                    if !self.alive() {
                        break;
                    }
                    self.status(ConnStatus::Offline, CasinoError::from(e).to_string());
                    tokio::select! {
                        _ = tokio::time::sleep(backoff(&mut retries)) => {}
                        _ = self.wake.notified() => {}
                    }
                }
            }
        }
    }

    /// The player, the coins and their prices, the balances, then the seeds.
    async fn sign_in(&self) -> Result<(), CallError> {
        self.status(ConnStatus::Authenticating, "");
        let me = self.site(Method::GET, "/api/v1/auth/me", None).await?;
        let picked = me.get("selected_crypto_currency").or(me.get("selected_currency")).and_then(Value::as_str).unwrap_or("usdt").to_ascii_lowercase();
        *self.selected.lock() = picked.clone();
        {
            let mut m = self.market.lock();
            m.username = me.get("username").and_then(Value::as_str).map(str::to_string);
            m.site_currency = Some(picked);
        }
        if let Ok(cfg) = self.site(Method::GET, "/api/v1/config", None).await {
            let mut m = self.market.lock();
            for c in cfg.pointer("/wallet/currencies").and_then(Value::as_array).into_iter().flatten() {
                let crypto = c.get("currency_type").and_then(Value::as_str) == Some("crypto");
                let enabled = c.get("enabled").and_then(Value::as_bool) != Some(false);
                if let (Some(code), true, true) = (c.get("currency").and_then(Value::as_str), crypto, enabled) {
                    m.currencies.entry(code.to_ascii_lowercase()).or_insert(CurrencyInfo { decimals: DECIMALS, price: 0.0, hidden: false, fiat: false });
                }
            }
        }
        self.refresh_rates().await;
        self.refresh_balances().await?;
        self.status(ConnStatus::Ready, "");
        match self.seeds().await {
            Ok(st) => {
                let _ = self.events.send(CasinoEvent::Provable(st));
            }
            Err(e) => tracing::info!("housebets: seeds: {e}"),
        }
        Ok(())
    }

    async fn serve(&self) {
        let mut failures = 0;
        let mut ticks = 0u32;
        loop {
            tokio::select! {
                _ = self.wake.notified() => {
                    // a new login: start over with it
                    return;
                }
                _ = tokio::time::sleep(REFRESH) => {
                    ticks += 1;
                    if ticks % RATES_EVERY == 0 {
                        self.refresh_rates().await;
                    }
                    match self.refresh_balances().await {
                        Ok(()) => failures = 0,
                        Err(CallError::Unauthenticated) => {
                            self.status(ConnStatus::AuthFailed, EXPIRED);
                            self.wake.notified().await;
                            return;
                        }
                        Err(CallError::Casino(CasinoError::ConnectionLost | CasinoError::Timeout)) => failures += 1,
                        Err(_) => {}
                    }
                    if failures >= 3 {
                        return;
                    }
                }
            }
            if !self.alive() {
                return;
            }
        }
    }

    /// Dollars per coin (`/api/fx/rates`: the quote currencies' prices in dollars).
    async fn refresh_rates(&self) {
        let coins: Vec<String> = self.market.lock().currencies.keys().map(|c| c.to_ascii_uppercase()).collect();
        if coins.is_empty() {
            return;
        }
        let Ok(data) = self.site(Method::GET, &format!("/api/fx/rates?base=USD&quotes={}", coins.join(",")), None).await else { return };
        let Some(rates) = data.get("rates").and_then(Value::as_object) else { return };
        {
            let mut m = self.market.lock();
            for (code, price) in rates {
                if let (Some(info), Some(p)) = (m.currencies.get_mut(&code.to_ascii_lowercase()), f64_from_json(price)) {
                    info.price = p;
                }
            }
        }
        self.publish();
    }

    /// Re-reads the balances, unless a bet ran meanwhile (its local update is newer).
    async fn refresh_balances(&self) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let data = self.site(Method::GET, "/api/v1/wallet/balances", None).await?;
        if !(idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled) {
            return Ok(());
        }
        {
            let mut m = self.market.lock();
            let coins: Vec<String> = m.currencies.keys().cloned().collect();
            for c in coins {
                m.wallets.entry(c).or_insert(0);
            }
            for w in data.get("currencies").and_then(Value::as_array).into_iter().flatten() {
                let Some(code) = w.get("currency").and_then(Value::as_str).map(str::to_ascii_lowercase) else { continue };
                let coins = w.get("playable_balance").or(w.get("cash_balance")).and_then(f64_from_json).unwrap_or(0.0);
                m.currencies.entry(code.clone()).or_insert(CurrencyInfo { decimals: DECIMALS, price: 0.0, hidden: false, fiat: false });
                m.wallets.insert(code, units(coins));
            }
        }
        self.publish();
        Ok(())
    }

    fn publish(&self) {
        let m = self.market.lock().clone();
        let mut last = self.published.lock();
        if *last != m {
            *last = m.clone();
            let _ = self.events.send(CasinoEvent::Market(m));
        }
    }

    fn credit(&self, coin: &str, delta: Units) {
        self.settled.fetch_add(1, Ordering::SeqCst);
        if delta == 0 {
            return;
        }
        *self.market.lock().wallets.entry(coin.to_string()).or_insert(0) += delta;
        self.publish();
    }

    /// The login's `__Host-csrf`, as the jar holds it now.
    fn csrf(&self) -> String {
        let jar = self.jar.lock().clone();
        let url = reqwest::Url::parse(SITE).expect("site url");
        jar.cookies(&url)
            .and_then(|h| h.to_str().ok().map(str::to_string))
            .and_then(|h| h.split(';').map(str::trim).find_map(|kv| kv.strip_prefix("__Host-csrf=").map(str::to_string)))
            .unwrap_or_default()
    }

    /// One request to the site's API, writes carrying the CSRF token. A refused session is
    /// renewed once (`/api/v1/auth/refresh`) before giving up.
    async fn site(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, CallError> {
        let mut attempt = 0;
        loop {
            let client = self.client.lock().clone();
            let mut req = client.request(method.clone(), format!("{SITE}{path}")).timeout(RPC_TIMEOUT).header(reqwest::header::CONTENT_TYPE, "application/json");
            if method != Method::GET {
                req = req.header("X-CSRF-Token", self.csrf()).header("X-Correlation-ID", uuid());
            }
            if let Some(b) = &body {
                req = req.json(b);
            }
            let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
            let res = req.send().await.map_err(lost)?;
            let status = res.status().as_u16();
            let text = res.text().await.map_err(lost)?;
            if !(200..300).contains(&status) {
                tracing::warn!("housebets: {path} -> HTTP {status}: {}", text.chars().take(200).collect::<String>());
            }
            match parse_site(status, &text) {
                Err(CallError::Unauthenticated) if attempt == 0 && path != "/api/v1/auth/refresh" => {
                    attempt += 1;
                    let client = self.client.lock().clone();
                    let renewed = client
                        .post(format!("{SITE}/api/v1/auth/refresh"))
                        .timeout(RPC_TIMEOUT)
                        .header("X-CSRF-Token", self.csrf())
                        .header(reqwest::header::CONTENT_TYPE, "application/json")
                        .body("{}")
                        .send()
                        .await
                        .is_ok_and(|r| r.status().is_success());
                    if !renewed {
                        return Err(CallError::Unauthenticated);
                    }
                    self.save_login();
                }
                r => return r,
            }
        }
    }

    /// After the site renewed its cookies: the login from the jar goes to the app to save.
    fn save_login(&self) {
        let jar = self.jar.lock().clone();
        let mut parts: Vec<(String, String)> = vec![];
        for url in LOGIN_URLS {
            let Ok(u) = reqwest::Url::parse(url) else { continue };
            let Some(h) = jar.cookies(&u) else { continue };
            for kv in h.to_str().unwrap_or("").split(';').map(str::trim) {
                if let Some((k, v)) = kv.split_once('=') {
                    if LOGIN_COOKIES.contains(&k) && !parts.iter().any(|(n, _)| n == k) {
                        parts.push((k.to_string(), v.to_string()));
                    }
                }
            }
        }
        if parts.iter().any(|(k, _)| k == LOGIN_COOKIES[0]) {
            let cookies = parts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
            let mut cur = self.login.lock();
            let next = Login { cookies, ua: cur.as_ref().and_then(|l| l.ua.clone()) };
            if cur.as_ref() != Some(&next) {
                *cur = Some(next.clone());
                let _ = self.events.send(CasinoEvent::Token(next.to_token()));
            }
        }
    }

    /// The House Games session for `coin`: the account's selected coin is switched to it first
    /// when it differs, since a session plays in that one.
    async fn session(&self, coin: &str) -> Result<Session, CasinoError> {
        let coin = coin.to_ascii_lowercase();
        if let Some(s) = self.session.lock().clone().filter(|s| s.coin == coin) {
            return Ok(s);
        }
        let _one = self.launching.lock().await;
        if let Some(s) = self.session.lock().clone().filter(|s| s.coin == coin) {
            return Ok(s);
        }
        if *self.selected.lock() != coin {
            let r = self.site(Method::PUT, "/api/v1/wallet/currency", Some(json!({ "currency": coin, "correlation_id": uuid() }))).await?;
            let now = r.get("selected_currency").and_then(Value::as_str).unwrap_or(&coin).to_ascii_lowercase();
            if now != coin {
                return Err(CasinoError::Server(format!("Housebets didn't switch the account to {}", coin.to_ascii_uppercase())));
            }
            *self.selected.lock() = now.clone();
            self.market.lock().site_currency = Some(now);
        }
        let body = json!({
            "slug": "housegames-limbo",
            "locale": "en",
            "play_mode": "real",
            "is_mobile": false,
            "return_url": SITE,
            "deposit_url": format!("{SITE}/deposit?locale=en"),
            "display_currency": "USD",
            "display_in_fiat": false,
            "direct_bootstrap": true,
        });
        let client = self.client.lock().clone();
        let res = client
            .post(format!("{SITE}/_api/housegames/launch"))
            .timeout(RPC_TIMEOUT)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header("X-CSRF-Token", self.csrf())
            .header("Idempotency-Key", uuid())
            .json(&body)
            .send()
            .await
            .map_err(|e| if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost })?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|_| CasinoError::ConnectionLost)?;
        let d = parse_site(status, &text).map_err(CasinoError::from)?;
        let boot = d.get("direct_bootstrap").ok_or_else(|| CasinoError::BadReply(text.chars().take(200).collect()))?;
        let api = boot.get("api_url").and_then(Value::as_str).unwrap_or(&format!("{SITE}/housegames")).trim_end_matches('/').to_string();
        let token = boot.pointer("/session/token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| CasinoError::BadReply("no House Games session in the launch".into()))?;
        let code = boot.pointer("/currently_currency/code").and_then(Value::as_u64).ok_or_else(|| CasinoError::BadReply("no coin in the launch".into()))?;
        // only the gateway's own addresses get the token
        if !api.starts_with(&format!("{SITE}/")) {
            return Err(CasinoError::BadReply(format!("unexpected gateway {api}")));
        }
        let s = Session { coin, api, code, token: token.to_string() };
        tracing::info!("housebets: House Games session in {} (id {})", s.coin.to_ascii_uppercase(), s.code);
        *self.session.lock() = Some(s.clone());
        Ok(s)
    }

    /// A new token for a session the gateway refused, or none (open a new session then).
    async fn renew(&self, s: &Session) -> bool {
        let client = self.client.lock().clone();
        let body = json!({ "apiUrl": s.api, "intent": "refresh", "token": s.token, "selectorToken": s.token });
        let res = client.post(format!("{SITE}/_api/housegames/refresh")).timeout(RPC_TIMEOUT).header(reqwest::header::CONTENT_TYPE, "application/json").json(&body).send().await;
        let token = match res {
            Ok(r) if r.status().is_success() => r.json::<Value>().await.ok().and_then(|v| v.get("token").and_then(Value::as_str).map(str::to_string)),
            _ => None,
        };
        let mut cur = self.session.lock();
        match token.filter(|t| !t.trim().is_empty()) {
            Some(t) => {
                if let Some(c) = cur.as_mut().filter(|c| c.token == s.token) {
                    c.token = t;
                }
                true
            }
            None => {
                *cur = None;
                false
            }
        }
    }

    /// One call to the gateway in `coin`'s session: `path` with a body is a POST, without one a
    /// GET. A refused token is renewed (or the session opened again) and the call made once more.
    async fn game(&self, coin: &str, path: &str, body: Option<Value>) -> Result<Value, CasinoError> {
        // bets and steps one at a time; reads don't wait
        let _turn = match &body {
            Some(_) => Some(self.betting.lock().await),
            None => None,
        };
        let mut attempt = 0;
        loop {
            let s = self.session(coin).await?;
            let client = self.client.lock().clone();
            let url = format!("{}{path}", s.api);
            let req = match &body {
                Some(b) => client.post(&url).json(b),
                None => client.get(&url),
            };
            let lost = |e: reqwest::Error| if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost };
            let res = req.timeout(RPC_TIMEOUT).header("X-Token", &s.token).header(reqwest::header::CONTENT_TYPE, "application/json").send().await.map_err(lost)?;
            let status = res.status().as_u16();
            let text = res.text().await.map_err(lost)?;
            if status == 429 {
                tracing::debug!("housebets: {path} busy: {}", text.chars().take(200).collect::<String>());
            } else if !(200..300).contains(&status) {
                tracing::warn!("housebets: {path} -> HTTP {status}: {}", text.chars().take(400).collect::<String>());
            }
            if status == 401 && attempt == 0 {
                attempt += 1;
                if !self.renew(&s).await {
                    tracing::info!("housebets: House Games session ended, opening a new one");
                }
                continue;
            }
            return parse_game(status, &text);
        }
    }

    fn seed_coin(&self) -> String {
        let c = self.selected.lock().clone();
        if c.is_empty() {
            "usdt".into()
        } else {
            c
        }
    }

    /// Limbo's seed pair (every game has its own; Limbo stands for them).
    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let d = self.game(&self.seed_coin(), "/seeds?gameType=limbo", None).await?;
        Ok(ProvableState::from_json(d.get("seed").unwrap_or(&d)))
    }

    fn set_mines(&self, play: Option<MinesPlay>) {
        *self.mines.lock() = play;
        self.mines_epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// A Mines reply (`{game: {…}}`) -> the round; an open one is remembered for its steps.
    fn mines_round(&self, d: &Value, coin: &str, amount: Option<Units>, tiles: &[i64]) -> Result<Round, CasinoError> {
        let g = d.get("game").unwrap_or(d);
        let id = g.get("roundId").and_then(Value::as_str).ok_or_else(|| CasinoError::BadReply(d.to_string().chars().take(200).collect()))?.to_string();
        let ints = |k: &str| -> Vec<i64> { g.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() };
        let count = g.get("mines").and_then(Value::as_u64).unwrap_or(3) as u32;
        let active = g.get("active").and_then(Value::as_bool).unwrap_or(false);
        let mut picks = ints("opened");
        if picks.is_empty() && !tiles.is_empty() {
            picks = tiles.to_vec();
        }
        // the tile that hit a mine comes last
        if let Some(hit) = g.get("hit").and_then(Value::as_i64) {
            if !picks.contains(&hit) {
                picks.push(hit);
            }
        }
        let stake = amount.or_else(|| g.get("stake").and_then(coin_units)).unwrap_or(0);
        let payout = g.get("payout").and_then(coin_units).unwrap_or(0);
        let multiplier = if active { 0.0 } else { g.get("multiplier").and_then(f64_from_json).unwrap_or(0.0) };
        if active {
            self.set_mines(Some(MinesPlay { round: id.clone(), seq: g.get("seq").and_then(Value::as_u64).unwrap_or(0), count, coin: coin.to_string(), amount: stake }));
        } else {
            if self.mines.lock().as_ref().is_some_and(|p| p.round == id) {
                self.set_mines(None);
            }
            self.credit(coin, payout - stake);
        }
        Ok(Round {
            game: Game::Mines,
            id,
            active,
            currency: coin.to_string(),
            amount: stake,
            amount_mult: 1.0,
            payout,
            multiplier,
            state: mines_state(count, &picks, &ints("field")),
        })
    }
}

#[async_trait]
impl Casino for Housebets {
    fn id(&self) -> CasinoId {
        CasinoId::Housebets
    }

    fn set_token(&self, token: Option<String>) {
        let login = token.as_deref().and_then(Login::parse);
        let (jar, client) = client_for(login.as_ref());
        *self.login.lock() = login;
        *self.jar.lock() = jar;
        *self.client.lock() = client;
        *self.session.lock() = None;
        self.wake.notify_one();
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.ready.store(false, Ordering::SeqCst);
        self.wake.notify_one();
    }

    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let coin = req.currency.to_ascii_lowercase();
        let code = self.session(&coin).await?.code;
        let body = json!({ "betNumber": req.target, "amount": to_f64(req.amount, DECIMALS), "currency": code, "generation": uuid() });
        let d = self.game(&coin, "/limbo/bet", Some(body)).await?;
        let outcome = d.pointer("/game/outcome").and_then(f64_from_json).ok_or_else(|| CasinoError::BadReply(d.to_string().chars().take(200).collect()))?;
        let payout = d.pointer("/round/payout").or(d.pointer("/game/game/payout")).and_then(coin_units).unwrap_or(0);
        self.credit(&coin, payout - req.amount);
        Ok(BetReply {
            outcome,
            amount: Some(req.amount),
            winnings: Some(payout),
            win: Some(outcome >= req.target),
            nonce: d.pointer("/game/game/nonce").and_then(Value::as_u64),
            id: None,
        })
    }

    /// A new client seed for every House Game (each keeps its own pair).
    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let coin = self.seed_coin();
        for (i, game) in SEEDED.iter().enumerate() {
            let r = self.game(&coin, "/change_seed", Some(json!({ "gameType": game, "clientSeed": client_seed }))).await;
            if i == 0 {
                r?;
            } else if let Err(e) = r {
                tracing::info!("housebets: {game} seed: {e}");
            }
        }
        let mut st = self.seeds().await.unwrap_or_default();
        if st.client_seed.is_none() {
            st.client_seed = Some(client_seed.to_string());
        }
        Ok(st)
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        self.seeds().await
    }

    async fn list_slots(&self, _f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _id: &str, _demo: bool, _cur: &str, _fiat: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots on Housebets aren't in FastBet yet".into()))
    }

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let coin = currency.to_ascii_lowercase();
        let code = self.session(&coin).await?.code;
        let stake = to_f64(amount, DECIMALS);
        match game {
            Game::Dice => {
                let target = params["target"].as_f64().ok_or_else(|| CasinoError::Server("Pick a target".into()))?;
                let side = if params["condition"].as_str() == Some("below") { "under" } else { "over" };
                let body = json!({ "betNumber": target, "direction": side, "mode": side, "amount": stake, "currency": code, "generation": uuid() });
                let d = self.game(&coin, "/dice/bet", Some(body)).await?;
                let g = d.get("game").unwrap_or(&d);
                let result = g.get("outcome").and_then(f64_from_json).ok_or_else(|| CasinoError::BadReply(d.to_string().chars().take(200).collect()))?;
                let won = if side == "over" { result > target } else { result < target };
                let chance = g.get("winChance").and_then(f64_from_json).unwrap_or(if side == "over" { 100.0 - target } else { target });
                let payout = d.pointer("/round/payout").and_then(coin_units).unwrap_or(0);
                self.credit(&coin, payout - amount);
                Ok(Round {
                    game,
                    id: d.pointer("/round/roundId").or(g.pointer("/game/roundId")).and_then(Value::as_str).unwrap_or_default().to_string(),
                    active: false,
                    currency: coin,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier: if won && chance > 0.0 { 99.0 / chance } else { 0.0 },
                    state: json!({ "result": result, "target": target, "condition": if side == "over" { "above" } else { "below" } }),
                })
            }
            Game::Keno => {
                let picks: Vec<i64> = params["numbers"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                if picks.is_empty() {
                    return Err(CasinoError::Server("Pick 1 to 10 numbers".into()));
                }
                let risk = params["risk"].as_str().unwrap_or("classic").to_ascii_lowercase();
                let body = json!({ "generation": uuid(), "amount": stake, "risk": risk, "spots": picks, "currency": code });
                let d = self.game(&coin, "/keno2/bet", Some(body)).await?;
                let g = d.get("game").unwrap_or(&d);
                let payout = g.get("payout").and_then(coin_units).unwrap_or(0);
                self.credit(&coin, payout - amount);
                Ok(Round {
                    game,
                    id: g.get("roundId").and_then(Value::as_str).unwrap_or_default().to_string(),
                    active: false,
                    currency: coin,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier: g.get("multiplier").and_then(f64_from_json).unwrap_or(0.0),
                    state: json!({ "risk": risk, "drawnNumbers": g.get("balls").cloned().unwrap_or(json!([])), "selectedNumbers": picks }),
                })
            }
            Game::Mines => {
                let count = params["minesCount"].as_u64().unwrap_or(3).clamp(1, 24);
                let mut body = json!({ "generation": uuid(), "amount": stake, "grid": GRID, "mines": count, "currency": code });
                let mut tiles: Vec<i64> = vec![];
                if auto {
                    // the tiles go up front and the round settles in this one request
                    tiles = params["fields"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                    if tiles.is_empty() {
                        return Err(CasinoError::Server("Pick the tiles every bet reveals".into()));
                    }
                    body["tiles"] = json!(tiles);
                }
                let d = self.game(&coin, "/mines2/bet", Some(body)).await?;
                self.mines_round(&d, &coin, Some(amount), &tiles)
            }
            _ => Err(CasinoError::Server(format!("{} isn't on Housebets in FastBet", game.name()))),
        }
    }

    async fn original_step(&self, game: Game, step: Step, params: Value) -> Result<Round, CasinoError> {
        if game != Game::Mines {
            return Err(CasinoError::Server(format!("{} has no steps", game.name())));
        }
        let _inflight = InFlight::new(&self.inflight);
        let current = self.mines.lock().clone();
        let play = match current {
            Some(p) => p,
            None => {
                self.original_active(Game::Mines).await?;
                let resumed = self.mines.lock().clone();
                resumed.ok_or(CasinoError::Server("No Mines round is open".into()))?
            }
        };
        let mut body = json!({ "generation": uuid(), "roundId": play.round, "seq": play.seq });
        match step {
            Step::Next => {
                let tile = params["fields"].get(0).and_then(Value::as_i64).ok_or_else(|| CasinoError::Server("Pick a tile".into()))?;
                body["action"] = json!("reveal");
                body["tile"] = json!(tile);
            }
            Step::Cashout => body["action"] = json!("cashout"),
        }
        let d = self.game(&play.coin, "/mines2/action", Some(body)).await?;
        self.mines_round(&d, &play.coin, Some(play.amount), &[])
    }

    /// The Mines round left open, if any (the gateway's last game of the account).
    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        if game != Game::Mines {
            return Ok(None);
        }
        let coin = self.seed_coin();
        let epoch = self.mines_epoch.load(Ordering::SeqCst);
        let d = self.game(&coin, "/mines2/game", None).await?;
        if self.mines_epoch.load(Ordering::SeqCst) != epoch {
            let play = self.mines.lock().clone();
            return Ok(play.map(|p| Round {
                game: Game::Mines,
                id: p.round.clone(),
                active: true,
                currency: p.coin.clone(),
                amount: p.amount,
                amount_mult: 1.0,
                payout: 0,
                multiplier: 0.0,
                state: mines_state(p.count, &[], &[]),
            }));
        }
        let open = d.get("game").and_then(|g| g.get("active")).and_then(Value::as_bool) == Some(true);
        if !open {
            return Ok(None);
        }
        self.mines_round(&d, &coin, None, &[]).map(Some)
    }

    /// Housebets' own Keno paytable; the others are Stake's.
    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        let mut t = Tables::builtin();
        if let Ok(keno) = serde_json::from_str(include_str!("../../assets/housebets-keno.json")) {
            t.keno = keno;
        }
        Ok(t)
    }

    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        // the site's tiles for its House Games
        let data = self.site(Method::GET, "/api/v1/casino/search", None).await?;
        let games = data.get("sections").and_then(Value::as_array).into_iter().flatten().flat_map(|s| s.get("games").and_then(Value::as_array).into_iter().flatten());
        let mut out = vec![];
        for g in games {
            let slug = g.get("slug").and_then(Value::as_str).unwrap_or("");
            let ours = match slug {
                "housegames-limbo" => "limbo",
                "housegames-dice" => "dice",
                "housegames-keno" | "housegames-keno2" => "keno",
                "housegames-mines" | "housegames-mines2" => "mines",
                _ => continue,
            };
            if out.iter().any(|x: &crate::slots::SlotGame| x.id == ours) {
                continue;
            }
            out.push(crate::slots::SlotGame {
                id: ours.into(),
                name: g.get("name").and_then(Value::as_str).unwrap_or(ours).to_string(),
                producer: "housebets".into(),
                image: g.get("thumbnail_url").and_then(Value::as_str).unwrap_or("").to_string(),
                demo: false,
                rtp: 99.0,
                provider_game_id: None,
                studio: None,
            });
        }
        Ok(out)
    }
}

/// A cookie jar holding the login, and a client that sends it as the login window's browser.
fn client_for(login: Option<&Login>) -> (Arc<Jar>, reqwest::Client) {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let jar = Arc::new(Jar::default());
    if let Some(login) = login {
        let site = reqwest::Url::parse(SITE).expect("site url");
        let refresh = reqwest::Url::parse("https://www.housebets.com/api/v1/auth/refresh").expect("refresh url");
        for kv in login.cookies.split(';').map(str::trim).filter(|kv| kv.contains('=')) {
            if kv.starts_with("__Secure-ref=") {
                jar.add_cookie_str(&format!("{kv}; Path=/api/v1/auth/refresh; Secure"), &refresh);
            } else if kv.starts_with("cf_clearance=") || kv.starts_with("__cf_bm=") {
                jar.add_cookie_str(&format!("{kv}; Domain=.housebets.com; Path=/; Secure"), &site);
            } else {
                jar.add_cookie_str(&format!("{kv}; Path=/; Secure"), &site);
            }
        }
    }
    let ua = login.and_then(|l| l.ua.clone()).unwrap_or_else(|| USER_AGENT.to_string());
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_static(SITE));
    headers.insert(REFERER, HeaderValue::from_static("https://www.housebets.com/"));
    let client = reqwest::Client::builder()
        .user_agent(ua)
        .default_headers(headers)
        .cookie_provider(jar.clone())
        // header names as browsers write them: Cloudflare turns away lowercase ones over HTTP/1.1
        .http1_title_case_headers()
        .gzip(true)
        .build()
        .unwrap_or_default();
    (jar, client)
}

fn uuid() -> String {
    let mut b: [u8; 16] = rand::random();
    // version 4, variant 1
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Coins -> base units, rounded down.
fn units(coins: f64) -> Units {
    (coins.max(0.0) * 10f64.powi(DECIMALS as i32) + 1e-6).floor() as Units
}

fn coin_units(v: &Value) -> Option<Units> {
    f64_from_json(v).filter(|f| f.is_finite()).map(units)
}

/// The site's API: the object itself, or `{error}` / `{message}`.
fn parse_site(status: u16, body: &str) -> Result<Value, CallError> {
    let v = serde_json::from_str::<Value>(body).ok();
    if (200..300).contains(&status) {
        return Ok(v.unwrap_or(Value::Null));
    }
    if status == 401 {
        return Err(CallError::Unauthenticated);
    }
    let msg = v
        .as_ref()
        .and_then(|v| ["message", "error_description", "error", "detail"].iter().find_map(|k| v.get(*k).and_then(Value::as_str)).map(str::to_string))
        .unwrap_or_else(|| {
            if body.contains("challenge-platform") || body.contains("Just a moment") {
                "Cloudflare stopped the request; try again in a moment".into()
            } else {
                format!("HTTP {status}: {}", body.chars().take(160).collect::<String>())
            }
        });
    if status == 429 {
        return Err(CallError::Casino(CasinoError::Busy(msg)));
    }
    Err(CallError::Casino(CasinoError::Server(readable(&msg))))
}

/// The gateway: the object itself, or an error in some shape. "The round has moved on; here
/// it is as it stands" (409, code 19) carries the round as it is now: that is the answer.
fn parse_game(status: u16, body: &str) -> Result<Value, CasinoError> {
    let v = serde_json::from_str::<Value>(body).ok();
    if (200..300).contains(&status) {
        return v.ok_or_else(|| CasinoError::BadReply(body.chars().take(200).collect()));
    }
    if let Some(v) = v.as_ref().filter(|v| status == 409 && v.get("game").is_some_and(Value::is_object)) {
        return Ok(v.clone());
    }
    let msg = v
        .as_ref()
        .and_then(|v| {
            ["message", "error", "detail", "code"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str))
                .or_else(|| v.pointer("/error/message").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("HTTP {status}: {}", body.chars().take(160).collect::<String>()));
    let lower = msg.to_ascii_lowercase();
    if lower.contains("finish") && lower.contains("round") || lower.contains("active round") || lower.contains("round in progress") || lower.contains("already active") {
        return Err(CasinoError::RoundOpen);
    }
    if status == 429 || lower.contains("too many") || lower.contains("too fast") || lower.contains("in progress") || lower.contains("try again") {
        return Err(CasinoError::Busy(msg));
    }
    Err(CasinoError::Server(readable(&msg)))
}

/// Codes like `insufficient_balance` read as words, so the app tells them apart.
fn readable(msg: &str) -> String {
    if msg.contains(' ') {
        msg.to_string()
    } else {
        let s = msg.replace(['_', '-'], " ");
        let mut c = s.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }
}

struct InFlight<'a>(&'a AtomicUsize);

impl<'a> InFlight<'a> {
    fn new(n: &'a AtomicUsize) -> Self {
        n.fetch_add(1, Ordering::SeqCst);
        Self(n)
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins() {
        let l = Login::parse("__Host-sid=a; _ga=x; __Host-csrf=b; __Secure-ref=c").unwrap();
        assert_eq!((l.cookies.as_str(), l.ua.as_deref()), ("__Host-sid=a; __Host-csrf=b; __Secure-ref=c", None));
        assert!(Login::parse("__Host-csrf=b").is_none());
        let j = Login::parse(r#"{"cookies":"__Host-sid=a; cf_clearance=z","ua":"Mozilla/5.0 Edg/141"}"#).unwrap();
        assert_eq!((j.cookies.as_str(), j.ua.as_deref()), ("__Host-sid=a; cf_clearance=z", Some("Mozilla/5.0 Edg/141")));
        assert_eq!(Login::parse(&j.to_token()), Some(j));
        let (jar, _) = client_for(Login::parse("__Host-sid=a; __Host-csrf=tok; __Secure-ref=c").as_ref());
        let site = reqwest::Url::parse(SITE).unwrap();
        let sent = jar.cookies(&site).unwrap().to_str().unwrap().to_string();
        assert!(sent.contains("__Host-sid=a") && sent.contains("__Host-csrf=tok") && !sent.contains("__Secure-ref"), "{sent}");
        let refresh = reqwest::Url::parse("https://www.housebets.com/api/v1/auth/refresh").unwrap();
        assert!(jar.cookies(&refresh).unwrap().to_str().unwrap().contains("__Secure-ref=c"));
        let u = uuid();
        assert_eq!((u.len(), &u[14..15]), (36, "4"));
    }

    #[test]
    fn replies() {
        assert!(matches!(parse_site(401, r#"{"error":"unauthorized"}"#), Err(CallError::Unauthenticated)));
        assert!(matches!(parse_site(200, r#"{"username":"sx"}"#), Ok(v) if v["username"] == "sx"));
        assert!(matches!(parse_game(400, r#"{"error":"insufficient_balance"}"#), Err(e) if e.is_balance()));
        assert!(matches!(parse_game(409, r#"{"message":"Finish the active round first"}"#), Err(CasinoError::RoundOpen)));
        assert!(matches!(parse_game(429, "{}"), Err(e) if e.is_busy()));
        assert!(matches!(parse_game(429, r#"{"code":8,"error":"another bet in progress, please retry"}"#), Err(e) if e.is_busy()));
        let moved = parse_game(409, r#"{"code":19,"error":"the round has moved on; here it is as it stands","game":{"roundId":"r","active":false,"seq":2}}"#).ok().unwrap();
        assert_eq!(moved["game"]["seq"], 2);
        assert_eq!(units(0.344307013092576794), 34_430_701);
    }

    #[test]
    fn keno_paytable_ships() {
        let t: std::collections::HashMap<String, Vec<Vec<f64>>> = serde_json::from_str(include_str!("../../assets/housebets-keno.json")).unwrap();
        assert_eq!(t["classic"][2][1], 0.999586);
        assert_eq!(t["high"][2][3], 81.51);
    }
}
