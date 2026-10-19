//! Degen.com: the account and the house games (Limbo, Dice, Keno, Mines) over the site's API
//! (`https://api.degen.com/v1`), signed in with the site's cookies.
//!
//! The login is the API's cookies, read by the login window: `access_token` (short-lived),
//! `refresh_token` (sent only to `/v1/auth/refresh`, which sets a new pair) and `device_id`, with
//! Cloudflare's `cf_clearance` and the window's User Agent. Bets carry no coin: they play in the
//! account's primary wallet (`PATCH /balance/primary` switches it), so the driver switches it to
//! the coin of the bet first. Amounts travel as strings with 8 decimals. Bets of 0 are allowed.

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
use crate::money::{f64_from_json, parse_units};
use crate::originals::{mines_state, Game, Round, Step, Tables};

const API: &str = "https://api.degen.com/v1";
const API_HOST: &str = "https://api.degen.com/";
const SITE: &str = "https://degen.com";
/// The cookies that make the login, as the login window hands them over (the first one must be
/// there), and where each is sent.
pub const LOGIN_COOKIES: &[&str] = &["access_token", "refresh_token", "device_id", "cf_clearance"];
pub const LOGIN_URLS: &[&str] = &["https://api.degen.com/", "https://api.degen.com/v1/auth/refresh", "https://degen.com/"];
const REFRESH_URL: &str = "https://api.degen.com/v1/auth/refresh";
/// Amounts travel as strings in the coin with 8 decimals ("0.00991435").
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
const REFRESH: Duration = Duration::from_secs(10);
const RATES_EVERY: u32 = 12;
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const EXPIRED: &str = "Your Degen login has expired. Log in again in Settings.";
/// Keno's board here: 40 numbers, 10 drawn.
const KENO_VARIANT: &str = "keno_40";

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

/// A Mines round in play.
#[derive(Clone, Debug)]
struct MinesPlay {
    id: String,
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
        // an expired access token renews from the refresh token alone
        parts.iter().any(|kv| kv.starts_with("access_token=") || kv.starts_with("refresh_token=")).then(|| Login { cookies: parts.join("; "), ua })
    }

    fn to_token(&self) -> String {
        json!({ "cookies": self.cookies, "ua": self.ua }).to_string()
    }
}

pub struct Degen {
    login: Mutex<Option<Login>>,
    jar: Mutex<Arc<Jar>>,
    client: Mutex<reqwest::Client>,
    wake: Notify,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// The account's primary wallet: the coin bets play in.
    primary: Mutex<String>,
    switching: tokio::sync::Mutex<()>,
    /// One token renewal at a time: the site hands out a new refresh token with each, so two at
    /// once would spend the same one twice. Counts the renewals done.
    renewing: tokio::sync::Mutex<()>,
    renewals: AtomicU64,
    mines: Mutex<Option<MinesPlay>>,
    mines_epoch: AtomicU64,
    inflight: AtomicUsize,
    settled: AtomicU64,
}

impl Degen {
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
            primary: Mutex::new(String::new()),
            switching: tokio::sync::Mutex::new(()),
            renewing: tokio::sync::Mutex::new(()),
            renewals: AtomicU64::new(0),
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
        tracing::info!("degen: {} {detail}", s.short());
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
            self.status(ConnStatus::Connecting, "Opening degen.com");
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
        let me = self.api(Method::GET, "/users/profile", None).await?;
        self.market.lock().username = me.get("username").and_then(Value::as_str).map(str::to_string);
        if let Ok(assets) = self.api(Method::GET, "/payments/available-assets", None).await {
            let mut m = self.market.lock();
            for a in assets.get("assets").and_then(Value::as_array).into_iter().flatten() {
                if let Some(code) = a.get("symbol").and_then(Value::as_str) {
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
            Err(e) => tracing::info!("degen: seeds: {e}"),
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

    /// Dollars per coin (`/payments/currency-rates`).
    async fn refresh_rates(&self) {
        let Ok(data) = self.api(Method::GET, "/payments/currency-rates", None).await else { return };
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

    /// Re-reads the balances and the primary wallet, unless a bet ran meanwhile (its local
    /// update is newer).
    async fn refresh_balances(&self) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let data = self.api(Method::GET, "/balance/wallets", None).await?;
        let wallets = data.as_array().cloned().unwrap_or_default();
        // the primary wallet is the account's, changed on the site too: always taken
        if let Some(p) = wallets.iter().find(|w| w.get("isPrimary").and_then(Value::as_bool) == Some(true)).and_then(|w| w.get("asset").and_then(Value::as_str)) {
            let p = p.to_ascii_lowercase();
            *self.primary.lock() = p.clone();
            self.market.lock().site_currency = Some(p);
        }
        if !(idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled) {
            return Ok(());
        }
        {
            let mut m = self.market.lock();
            let coins: Vec<String> = m.currencies.keys().cloned().collect();
            for c in coins {
                m.wallets.entry(c).or_insert(0);
            }
            for w in &wallets {
                let Some(code) = w.get("asset").and_then(Value::as_str).map(str::to_ascii_lowercase) else { continue };
                let bal = w.get("balance").and_then(coin_units).unwrap_or(0);
                m.currencies.entry(code.clone()).or_insert(CurrencyInfo { decimals: DECIMALS, price: 0.0, hidden: false, fiat: false });
                m.wallets.insert(code, bal);
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

    /// One request to the API. A refused access token is renewed once (`/auth/refresh`)
    /// before giving up.
    async fn api(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, CallError> {
        let mut attempt = 0;
        loop {
            let seen = self.renewals.load(Ordering::SeqCst);
            let client = self.client.lock().clone();
            let mut req = client.request(method.clone(), format!("{API}{path}")).timeout(RPC_TIMEOUT);
            if let Some(b) = &body {
                req = req.json(b);
            }
            let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
            let res = req.send().await.map_err(lost)?;
            let status = res.status().as_u16();
            let text = res.text().await.map_err(lost)?;
            if status == 429 {
                tracing::debug!("degen: {path} busy: {}", text.chars().take(200).collect::<String>());
            } else if !(200..300).contains(&status) {
                tracing::warn!("degen: {path} -> HTTP {status}: {}", text.chars().take(300).collect::<String>());
            }
            match parse_reply(status, &text) {
                Err(CallError::Unauthenticated) if attempt == 0 => {
                    attempt += 1;
                    self.renew(seen).await?;
                }
                r => return r,
            }
        }
    }

    /// New tokens from the refresh token, unless another call renewed them since `seen`.
    async fn renew(&self, seen: u64) -> Result<(), CallError> {
        let _one = self.renewing.lock().await;
        if self.renewals.load(Ordering::SeqCst) != seen {
            return Ok(());
        }
        let client = self.client.lock().clone();
        let res = client.post(REFRESH_URL).timeout(RPC_TIMEOUT).send().await.map_err(|e| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost }))?;
        let status = res.status().as_u16();
        if !(200..300).contains(&status) {
            let text = res.text().await.unwrap_or_default();
            tracing::warn!("degen: token renewal -> HTTP {status}: {}", text.chars().take(200).collect::<String>());
            return Err(match status {
                400 | 401 | 403 => CallError::Unauthenticated,
                _ => CallError::Casino(CasinoError::Server(format!("Degen didn't renew the login (HTTP {status})"))),
            });
        }
        self.renewals.fetch_add(1, Ordering::SeqCst);
        tracing::info!("degen: login renewed");
        self.save_login();
        Ok(())
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
        let cookies = parts.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
        let mut cur = self.login.lock();
        let Some(next) = Login::parse(&cookies).map(|l| Login { ua: cur.as_ref().and_then(|c| c.ua.clone()), ..l }) else { return };
        if cur.as_ref() != Some(&next) {
            *cur = Some(next.clone());
            let _ = self.events.send(CasinoEvent::Token(next.to_token()));
        }
    }

    /// Makes `coin` the primary wallet, the one bets play in.
    async fn use_coin(&self, coin: &str) -> Result<(), CasinoError> {
        if *self.primary.lock() == coin {
            return Ok(());
        }
        let _one = self.switching.lock().await;
        if *self.primary.lock() == coin {
            return Ok(());
        }
        self.api(Method::PATCH, "/balance/primary", Some(json!({ "asset": coin.to_ascii_uppercase() }))).await?;
        tracing::info!("degen: bets play in {} now", coin.to_ascii_uppercase());
        *self.primary.lock() = coin.to_string();
        self.market.lock().site_currency = Some(coin.to_string());
        self.publish();
        Ok(())
    }

    /// A bet in `coin`: the reply says which coin it played in. Another one means the primary
    /// wallet changed on the site meanwhile; the bet went through in that coin.
    fn played_in(&self, d: &Value, coin: &str) -> Result<String, CasinoError> {
        let asset = d.get("asset").and_then(Value::as_str).map(str::to_ascii_lowercase).unwrap_or_else(|| coin.to_string());
        if asset == coin {
            return Ok(asset);
        }
        *self.primary.lock() = asset.clone();
        Err(CasinoError::Server(format!("Degen played that bet in {} (its wallet was switched on the site). Check the bet on degen.com.", asset.to_ascii_uppercase())))
    }

    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let d = self.api(Method::GET, "/games/provably-fair/seed-info", None).await?;
        Ok(ProvableState::from_json(&d))
    }

    fn set_mines(&self, play: Option<MinesPlay>) {
        *self.mines.lock() = play;
        self.mines_epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// A Mines game -> the round; an open one is remembered for its steps. `tried` are the
    /// tiles this request asked for (a lost round's mine may not be among the revealed ones).
    fn mines_round(&self, g: &Value, coin: &str, amount: Option<Units>, tried: &[i64]) -> Result<Round, CasinoError> {
        let id = g.get("id").and_then(Value::as_str).ok_or_else(|| CasinoError::BadReply(g.to_string().chars().take(200).collect()))?.to_string();
        let ints = |k: &str| -> Vec<i64> { g.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() };
        let count = g.get("minesCount").and_then(Value::as_u64).unwrap_or(3) as u32;
        let status = g.get("status").and_then(Value::as_str).unwrap_or("").to_ascii_uppercase();
        let active = status == "ACTIVE";
        let busted = status == "BUSTED";
        let mut picks = ints("revealedTiles");
        let mines = ints("minePositions");
        if busted && !picks.iter().any(|t| mines.contains(t)) {
            if let Some(hit) = tried.iter().find(|t| mines.contains(t)) {
                picks.push(*hit);
            }
        }
        let stake = amount.or_else(|| g.get("betAmount").and_then(coin_units)).unwrap_or(0);
        let payout = g.get("finalPayout").and_then(coin_units).unwrap_or(0);
        let multiplier = if active || busted { 0.0 } else { g.get("currentMultiplier").and_then(f64_from_json).unwrap_or(0.0) };
        if active {
            self.set_mines(Some(MinesPlay { id: id.clone(), count, coin: coin.to_string(), amount: stake }));
        } else {
            if self.mines.lock().as_ref().is_some_and(|p| p.id == id) {
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
            state: mines_state(count, &picks, &mines),
        })
    }
}

#[async_trait]
impl Casino for Degen {
    fn id(&self) -> CasinoId {
        CasinoId::Degen
    }

    fn set_token(&self, token: Option<String>) {
        let login = token.as_deref().and_then(Login::parse);
        let (jar, client) = client_for(login.as_ref());
        *self.login.lock() = login;
        *self.jar.lock() = jar;
        *self.client.lock() = client;
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
        self.use_coin(&coin).await?;
        let body = json!({ "betAmount": amount_str(req.amount), "targetMultiplier": req.target });
        let d = self.api(Method::POST, "/games/limbo/bet", Some(body)).await?;
        let outcome = d.get("resultMultiplier").or(d.get("crashPoint")).and_then(f64_from_json).ok_or_else(|| CasinoError::BadReply(d.to_string().chars().take(200).collect()))?;
        let payout = d.get("winAmount").and_then(coin_units).unwrap_or(0);
        let played = self.played_in(&d, &coin);
        self.credit(played.as_deref().unwrap_or(&coin), payout - req.amount);
        played?;
        Ok(BetReply {
            outcome,
            amount: Some(req.amount),
            winnings: Some(payout),
            win: Some(d.get("status").and_then(Value::as_str).map(|s| s.eq_ignore_ascii_case("won")).unwrap_or(outcome >= req.target)),
            nonce: d.get("nonce").and_then(|n| n.as_u64().or_else(|| n.as_str().and_then(|s| s.parse().ok()))),
        })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let d = self.api(Method::PATCH, "/games/provably-fair/client-seed", Some(json!({ "clientSeed": client_seed }))).await?;
        tracing::info!("degen: new seed pair (old nonce {})", d.get("oldNonce").and_then(|n| n.as_str().map(str::to_string).or_else(|| n.as_u64().map(|n| n.to_string()))).unwrap_or_default());
        Ok(ProvableState {
            client_seed: Some(d.get("newClientSeed").and_then(Value::as_str).unwrap_or(client_seed).to_string()),
            server_seed_hash: d.get("newSeedHash").and_then(Value::as_str).map(str::to_string),
            nonce: Some(0),
        })
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        self.seeds().await
    }

    async fn list_slots(&self, _f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _id: &str, _demo: bool, _cur: &str, _fiat: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots on Degen aren't in FastBet yet".into()))
    }

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let coin = currency.to_ascii_lowercase();
        self.use_coin(&coin).await?;
        let stake = amount_str(amount);
        match game {
            Game::Dice => {
                let target = params["target"].as_f64().ok_or_else(|| CasinoError::Server("Pick a target".into()))?;
                let target = (target * 100.0).round() / 100.0;
                let over = params["condition"].as_str() != Some("below");
                let body = json!({ "gameSessionId": uuid(), "betAmount": stake, "betType": if over { "ROLL_OVER" } else { "ROLL_UNDER" }, "targetNumber": target });
                let d = self.api(Method::POST, "/games/dice/bet", Some(body)).await?;
                let result = d.get("rollResult").and_then(f64_from_json).ok_or_else(|| CasinoError::BadReply(d.to_string().chars().take(200).collect()))?;
                tracing::debug!("degen: dice {result} status {:?} win {:?}", d.get("status"), d.get("winAmount"));
                // the site's verdict where it gives one
                let ruled = if over { result > target } else { result < target };
                let verdict = d.get("status").and_then(Value::as_str).unwrap_or("");
                let won = if verdict.is_empty() { ruled } else { verdict.eq_ignore_ascii_case("won") };
                if won != ruled {
                    let side = if over { "over" } else { "under" };
                    tracing::warn!("degen: dice {result} vs {target} ({side}) came back {verdict}");
                }
                let chance = if over { 100.0 - target } else { target };
                let payout = d.get("winAmount").and_then(coin_units).unwrap_or(0);
                let played = self.played_in(&d, &coin);
                self.credit(played.as_deref().unwrap_or(&coin), payout - amount);
                played?;
                Ok(Round {
                    game,
                    id: d.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    active: false,
                    currency: coin,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier: if won && chance > 0.0 { 99.0 / chance } else { 0.0 },
                    state: json!({ "result": result, "target": target, "condition": if over { "above" } else { "below" } }),
                })
            }
            Game::Keno => {
                let picks: Vec<i64> = params["numbers"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                if picks.is_empty() {
                    return Err(CasinoError::Server("Pick 1 to 10 numbers".into()));
                }
                let risk = params["risk"].as_str().unwrap_or("classic").to_ascii_lowercase();
                // the board counts 0–39, Degen 1–40
                let body = json!({
                    "betAmount": stake,
                    "variantId": KENO_VARIANT,
                    "riskLevel": risk.to_ascii_uppercase(),
                    "selectedNumbers": picks.iter().map(|n| n + 1).collect::<Vec<_>>(),
                });
                let d = self.api(Method::POST, "/games/keno/bet", Some(body)).await?;
                let payout = d.get("winAmount").and_then(coin_units).unwrap_or(0);
                let played = self.played_in(&d, &coin);
                self.credit(played.as_deref().unwrap_or(&coin), payout - amount);
                played?;
                let drawn: Vec<i64> = d["drawnNumbers"].as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|n| n - 1).collect()).unwrap_or_default();
                Ok(Round {
                    game,
                    id: d.get("id").and_then(Value::as_str).unwrap_or_default().to_string(),
                    active: false,
                    currency: coin,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier: d.get("payoutMultiplier").and_then(f64_from_json).unwrap_or(0.0),
                    state: json!({ "risk": risk, "drawnNumbers": drawn, "selectedNumbers": picks }),
                })
            }
            Game::Mines => {
                let count = params["minesCount"].as_u64().unwrap_or(3).clamp(1, 24);
                if auto {
                    // the tiles go up front and the round settles in this one request
                    let tiles: Vec<i64> = params["fields"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                    if tiles.is_empty() {
                        return Err(CasinoError::Server("Pick the tiles every bet reveals".into()));
                    }
                    let body = json!({ "gameId": uuid(), "betAmount": stake, "minesCount": count, "gameSessionId": uuid(), "tilePositions": tiles });
                    let d = self.api(Method::POST, "/games/mines/autoplay", Some(body)).await?;
                    return self.mines_round(&d, &coin, Some(amount), &tiles);
                }
                let body = json!({ "betAmount": stake, "minesCount": count, "gameSessionId": uuid() });
                let d = self.api(Method::POST, "/games/mines/start", Some(body)).await?;
                self.mines_round(&d, &coin, Some(amount), &[])
            }
            _ => Err(CasinoError::Server(format!("{} isn't on Degen in FastBet", game.name()))),
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
        match step {
            Step::Next => {
                let tile = params["fields"].get(0).and_then(Value::as_i64).ok_or_else(|| CasinoError::Server("Pick a tile".into()))?;
                let d = self.api(Method::POST, "/games/mines/reveal", Some(json!({ "gameId": play.id, "tilePosition": tile }))).await?;
                self.mines_round(&d, &play.coin, Some(play.amount), &[tile])
            }
            Step::Cashout => {
                let d = self.api(Method::POST, "/games/mines/cashout", Some(json!({ "gameId": play.id }))).await?;
                self.mines_round(&d, &play.coin, Some(play.amount), &[])
            }
        }
    }

    /// The Mines round left open, if any.
    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        if game != Game::Mines {
            return Ok(None);
        }
        let epoch = self.mines_epoch.load(Ordering::SeqCst);
        let d = self.api(Method::GET, "/games/mines/active", None).await?;
        if self.mines_epoch.load(Ordering::SeqCst) != epoch {
            // a bet started or ended a round meanwhile: that one is newer than this answer
            let play = self.mines.lock().clone();
            return Ok(play.map(|p| Round {
                game: Game::Mines,
                id: p.id.clone(),
                active: true,
                currency: p.coin.clone(),
                amount: p.amount,
                amount_mult: 1.0,
                payout: 0,
                multiplier: 0.0,
                state: mines_state(p.count, &[], &[]),
            }));
        }
        let open = d.get("status").and_then(Value::as_str).is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE"));
        if !open {
            return Ok(None);
        }
        let coin = d.get("asset").and_then(Value::as_str).map(str::to_ascii_lowercase).unwrap_or_else(|| self.primary.lock().clone());
        self.mines_round(&d, &coin, None, &[]).map(Some)
    }

    /// Degen's Keno paytables (`/games/keno/config`); the others are Stake's.
    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        let mut t = Tables::builtin();
        match self.api(Method::GET, "/games/keno/config", None).await {
            Ok(cfg) => {
                let keno = keno_tables(&cfg);
                if !keno.is_empty() {
                    t.keno = keno;
                }
            }
            Err(e) => tracing::info!("degen: keno tables: {}", CasinoError::from(e)),
        }
        Ok(t)
    }

    /// The site's covers for its house games.
    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok([("limbo", "Limbo"), ("dice", "Dice"), ("keno", "Keno"), ("mines", "Mines")]
            .into_iter()
            .map(|(id, name)| crate::slots::SlotGame {
                id: id.into(),
                name: name.into(),
                producer: "degen".into(),
                image: format!("https://assets.degen.com/degen/degen/{id}.webp"),
                demo: false,
                rtp: 99.0,
                provider_game_id: None,
                studio: None,
            })
            .collect())
    }

    async fn vault_deposit(&self, currency: &str, amount: Units) -> Result<(), CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        self.api(Method::POST, "/balance/vault/deposit", Some(json!({ "asset": currency.to_ascii_uppercase(), "amount": amount_str(amount) }))).await?;
        let _ = self.refresh_balances().await;
        Ok(())
    }
}

/// `/games/keno/config`'s tables -> risk -> [picks - 1] -> payout per number of hits.
fn keno_tables(cfg: &Value) -> std::collections::HashMap<String, Vec<Vec<f64>>> {
    let mut out = std::collections::HashMap::new();
    let Some(tables) = cfg.get("multiplierTables").and_then(Value::as_object) else { return out };
    for (risk, by_picks) in tables {
        let rows: Vec<Vec<f64>> = (1..=10).map(|p| by_picks.get(p.to_string()).and_then(Value::as_array).map(|a| a.iter().filter_map(f64_from_json).collect()).unwrap_or_default()).collect();
        if rows.iter().all(|r| !r.is_empty()) {
            out.insert(risk.to_ascii_lowercase(), rows);
        }
    }
    out
}

/// A cookie jar holding the login, and a client that sends it as the login window's browser.
fn client_for(login: Option<&Login>) -> (Arc<Jar>, reqwest::Client) {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let jar = Arc::new(Jar::default());
    if let Some(login) = login {
        let api = reqwest::Url::parse(API_HOST).expect("api url");
        let refresh = reqwest::Url::parse(REFRESH_URL).expect("refresh url");
        for kv in login.cookies.split(';').map(str::trim).filter(|kv| kv.contains('=')) {
            if kv.starts_with("refresh_token=") {
                jar.add_cookie_str(&format!("{kv}; Path=/v1/auth/refresh; Secure"), &refresh);
            } else if kv.starts_with("cf_clearance=") {
                jar.add_cookie_str(&format!("{kv}; Domain=.degen.com; Path=/; Secure"), &api);
            } else {
                jar.add_cookie_str(&format!("{kv}; Path=/; Secure"), &api);
            }
        }
    }
    let ua = login.and_then(|l| l.ua.clone()).unwrap_or_else(|| USER_AGENT.to_string());
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_static(SITE));
    headers.insert(REFERER, HeaderValue::from_static("https://degen.com/"));
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

/// Base units -> the amount as the site writes it: "0.00100000".
fn amount_str(v: Units) -> String {
    let v = v.max(0);
    let one = 10i128.pow(DECIMALS);
    format!("{}.{:0width$}", v / one, v % one, width = DECIMALS as usize)
}

/// "0.00991435" (or a number) -> base units, rounded down.
fn coin_units(v: &Value) -> Option<Units> {
    match v {
        Value::String(s) => parse_units(s.trim().trim_start_matches('-'), DECIMALS).map(|u| if s.trim().starts_with('-') { -u } else { u }),
        _ => f64_from_json(v).filter(|f| f.is_finite()).map(|f| (f * 10f64.powi(DECIMALS as i32) + 1e-6).floor() as Units),
    }
}

/// The API's answer, or its error: `{statusCode, message}` where the message is text, a list
/// of texts or `{message, errorCode}`.
fn parse_reply(status: u16, body: &str) -> Result<Value, CallError> {
    let v = serde_json::from_str::<Value>(body).ok();
    if (200..300).contains(&status) {
        // an empty answer is "nothing" (no Mines round open)
        return Ok(v.unwrap_or(Value::Null));
    }
    if status == 401 {
        return Err(CallError::Unauthenticated);
    }
    let text = |m: &Value| -> Option<String> {
        match m {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => Some(a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).filter(|s| !s.is_empty()),
            Value::Object(o) => o.get("message").and_then(Value::as_str).or(o.get("errorCode").and_then(Value::as_str)).map(str::to_string),
            _ => None,
        }
    };
    let msg = v
        .as_ref()
        .and_then(|v| v.get("message").and_then(text).or_else(|| v.get("error").and_then(text)))
        .unwrap_or_else(|| {
            if body.contains("challenge-platform") || body.contains("Just a moment") {
                "Cloudflare stopped the request; try again in a moment".into()
            } else {
                format!("HTTP {status}: {}", body.chars().take(160).collect::<String>())
            }
        });
    let lower = msg.to_ascii_lowercase();
    if lower.contains("active game") || lower.contains("game in progress") || lower.contains("already have an active") || lower.contains("finish") && lower.contains("game") {
        return Err(CallError::Casino(CasinoError::RoundOpen));
    }
    if status == 429 || lower.contains("too many") || lower.contains("too fast") {
        return Err(CallError::Casino(CasinoError::Busy(msg)));
    }
    if status == 451 {
        return Err(CallError::Casino(CasinoError::Server(format!("Degen isn't available in your region: {msg}"))));
    }
    Err(CallError::Casino(CasinoError::Server(msg)))
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
        let l = Login::parse("access_token=a; _fbp=x; refresh_token=r; device_id=d").unwrap();
        assert_eq!((l.cookies.as_str(), l.ua.as_deref()), ("access_token=a; refresh_token=r; device_id=d", None));
        assert!(Login::parse("device_id=d; cf_clearance=c").is_none());
        let j = Login::parse(r#"{"cookies":"refresh_token=r; cf_clearance=z","ua":"Mozilla/5.0 Edg/141"}"#).unwrap();
        assert_eq!((j.cookies.as_str(), j.ua.as_deref()), ("refresh_token=r; cf_clearance=z", Some("Mozilla/5.0 Edg/141")));
        assert_eq!(Login::parse(&j.to_token()), Some(j));
        let (jar, _) = client_for(Login::parse("access_token=a; refresh_token=r; device_id=d; cf_clearance=c").as_ref());
        let api = reqwest::Url::parse("https://api.degen.com/v1/games/limbo/bet").unwrap();
        let sent = jar.cookies(&api).unwrap().to_str().unwrap().to_string();
        assert!(sent.contains("access_token=a") && sent.contains("device_id=d") && sent.contains("cf_clearance=c") && !sent.contains("refresh_token"), "{sent}");
        let refresh = reqwest::Url::parse(REFRESH_URL).unwrap();
        assert!(jar.cookies(&refresh).unwrap().to_str().unwrap().contains("refresh_token=r"));
    }

    #[test]
    fn amounts() {
        assert_eq!(amount_str(0), "0.00000000");
        assert_eq!(amount_str(100_000), "0.00100000");
        assert_eq!(amount_str(1_234_500_000), "12.34500000");
        assert_eq!(coin_units(&json!("0.00991435")), Some(991_435));
        assert_eq!(coin_units(&json!("-0.5")), Some(-50_000_000));
        assert_eq!(coin_units(&json!(0.0001)), Some(10_000));
    }

    #[test]
    fn replies() {
        assert!(matches!(parse_reply(401, r#"{"statusCode":401,"message":{"message":"Unauthorized","statusCode":401}}"#), Err(CallError::Unauthenticated)));
        assert!(matches!(parse_reply(200, ""), Ok(Value::Null)));
        let e = CasinoError::from(parse_reply(400, r#"{"statusCode":400,"message":"Insufficient balance"}"#).err().unwrap());
        assert!(e.is_balance(), "{e}");
        let e = CasinoError::from(parse_reply(400, r#"{"statusCode":400,"message":["betAmount must be a string"]}"#).err().unwrap());
        assert_eq!(e.to_string().contains("betAmount must be a string"), true);
        assert!(matches!(parse_reply(429, "{}"), Err(CallError::Casino(CasinoError::Busy(_)))));
        assert!(matches!(parse_reply(400, r#"{"message":"You already have an active game"}"#), Err(CallError::Casino(CasinoError::RoundOpen))));
    }

    #[test]
    fn keno_paytables() {
        let cfg = json!({ "multiplierTables": { "HIGH": {
            "0": [], "1": ["0.0", "3.96"], "2": ["0.0", "0.0", "17.1"], "3": ["0.0", "0.0", "0.0", "81.5"], "4": ["0"], "5": ["0"],
            "6": ["0"], "7": ["0"], "8": ["0"], "9": ["0"], "10": ["0.0", "0.0", "0.0", "0.0", "3.5", "8.0", "13.0", "63.0", "500.0", "800.0", "1000.0"]
        } } });
        let t = keno_tables(&cfg);
        assert_eq!(t["high"][2][3], 81.5);
        assert_eq!(t["high"][9][10], 1000.0);
    }
}
