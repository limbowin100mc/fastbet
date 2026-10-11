//! Goated over its REST API (`https://apis.goated.com`), signed in as the site is.
//!
//! Goated has no API keys. The site keeps its login as an access token (a JWT in
//! localStorage `g3_token`, sent as `Authorization: Bearer …`) and a
//! `refresh_token` cookie; posting that cookie to `/user/auth/token/refresh` gives
//! a new access token, which the site does an hour before the old one runs out.
//! The login window hands both over ([`Creds`]), or the user pastes them (Manual).
//! A renewed login goes back to the settings, cookie included when Goated sends a
//! new one. Replies are `{success, data}` or `{success: false, error, status}`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::creds::Creds;
use super::*;
use crate::money::{f64_from_json, parse_units, to_f64};
use crate::originals::{mines_state, Game, Round, Step, Tables};

const API: &str = "https://apis.goated.com";
const SITE: &str = "https://www.goated.com";
/// Amounts travel as numbers in the coin; 8 decimals cover every coin's bet sizes.
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// Balances are re-read this often while idle; bets update them locally.
const REFRESH: Duration = Duration::from_secs(10);
const RATES_EVERY: Duration = Duration::from_secs(120);
/// The access token is renewed this many seconds before it runs out (the site: an hour).
const RENEW_BEFORE: i64 = 3600;
/// Plain HTTPS presents itself as desktop Chrome.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
pub const REFRESH_COOKIE: &str = "refresh_token";

const RATES: &str = "/main/exchange-rates";
const USER: &str = "/user/";
const BALANCES: &str = "/user/wallet/balances";
const RENEW: &str = "/user/auth/token/refresh";
const LIMBO: &str = "/casino/limbo/bet";
const ACTIVE_SEED: &str = "/casino/provably-fair/active-seed";
const ROTATE_SEED: &str = "/casino/provably-fair/rotate-seed";
const DICE: &str = "/casino/dice/v2";
const MINES_BET: &str = "/casino/mines/bet";
const MINES_AUTO: &str = "/casino/mines/multi-select-bet";
const MINES_SELECT: &str = "/casino/mines/select";
const MINES_CASHOUT: &str = "/casino/mines/cashout";
const MINES_ACTIVE: &str = "/casino/mines/active";
/// The originals' artwork ships with the site's build under changing names, so the paths are
/// read from the homepage's own tiles (`<img alt="Dice on Goated" … url=%2F_next%2Fstatic%2Fmedia%2Fdice.….png`).
const ART_TILES: &[(&str, &str)] = &[("limbo", "Limbo"), ("dice", "Dice"), ("mines", "Mines")];

const EXPIRED: &str = "Your Goated login has expired. Log in again in Settings.";
/// Not money: Goated's points and staked tokens.
const HIDDEN: &[&str] = &["points", "goated-staked"];

enum CallError {
    /// Cloudflare answered instead of Goated.
    Cloudflare,
    /// The access token was refused.
    Unauthenticated,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Cloudflare => CasinoError::Server("Cloudflare turned the app away from Goated".into()),
            CallError::Unauthenticated => CasinoError::Server(EXPIRED.into()),
            CallError::Casino(e) => e,
        }
    }
}

pub struct Goated {
    creds: Mutex<Option<Creds>>,
    /// Refresh tokens already traded in: a stale copy handed back is ignored.
    spent: Mutex<HashSet<String>>,
    /// One renewal at a time; calls that saw the same token refused wait for it.
    renewing: tokio::sync::Mutex<()>,
    client: reqwest::Client,
    wake: Notify,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// Bets in flight and bets settled, so a balance poll never overwrites a fresher local balance.
    inflight: AtomicUsize,
    settled: AtomicU64,
}

impl Goated {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let me = Arc::new(Self {
            creds: Mutex::new(setup.token.as_deref().and_then(Creds::parse)),
            spent: Mutex::new(HashSet::new()),
            renewing: tokio::sync::Mutex::new(()),
            client: client(),
            wake: Notify::new(),
            alive: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            events,
            market: Mutex::new(Market::default()),
            published: Mutex::new(Market::default()),
            inflight: AtomicUsize::new(0),
            settled: AtomicU64::new(0),
        });
        tokio::spawn(me.clone().run());
        me
    }

    fn status(&self, s: ConnStatus, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!("goated: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            self.status(ConnStatus::Connecting, "Opening goated.com");
            match self.call(reqwest::Method::GET, RATES, None, None).await {
                Ok(data) => {
                    retries = 0;
                    self.apply_rates(&data);
                    self.serve().await;
                }
                Err(e) => {
                    if !self.alive() {
                        break;
                    }
                    self.ready.store(false, Ordering::SeqCst);
                    self.status(ConnStatus::Offline, CasinoError::from(e).to_string());
                    tokio::select! {
                        _ = tokio::time::sleep(backoff(&mut retries)) => {}
                        _ = self.wake.notified() => {}
                    }
                }
            }
        }
    }

    /// Signs in and keeps balances fresh until the connection drops.
    async fn serve(&self) {
        self.authenticate().await;
        let mut rates_at = Instant::now();
        let mut failures = 0;
        loop {
            tokio::select! {
                _ = self.wake.notified() => {
                    if !self.alive() {
                        return;
                    }
                    self.authenticate().await;
                }
                _ = tokio::time::sleep(REFRESH) => {
                    if rates_at.elapsed() > RATES_EVERY {
                        rates_at = Instant::now();
                        if let Ok(data) = self.call(reqwest::Method::GET, RATES, None, None).await {
                            self.apply_rates(&data);
                        }
                    }
                    if self.ready.load(Ordering::SeqCst) {
                        match self.refresh_balances().await {
                            Ok(()) => failures = 0,
                            Err(CallError::Casino(CasinoError::ConnectionLost | CasinoError::Timeout)) => failures += 1,
                            Err(_) => {}
                        }
                        // the network is gone: start over
                        if failures >= 3 {
                            return;
                        }
                    }
                }
            }
            if !self.alive() {
                return;
            }
        }
    }

    async fn authenticate(&self) {
        if self.creds.lock().is_none() {
            self.status(ConnStatus::NoToken, "");
            return;
        }
        self.status(ConnStatus::Authenticating, "");
        match self.signed(reqwest::Method::GET, USER, None).await {
            Ok(user) => {
                let name = ["username", "name", "display_name"].iter().find_map(|k| user.get(*k).and_then(Value::as_str)).map(str::to_string);
                self.market.lock().username = name;
                if let Err(e) = self.refresh_balances().await {
                    tracing::info!("goated: balances: {}", CasinoError::from(e));
                }
                self.status(ConnStatus::Ready, "");
                if let Ok(st) = self.seeds().await {
                    let _ = self.events.send(CasinoEvent::Provable(st));
                }
            }
            Err(CallError::Unauthenticated) => self.status(ConnStatus::AuthFailed, EXPIRED),
            Err(CallError::Casino(CasinoError::Server(m))) => self.status(ConnStatus::AuthFailed, m),
            Err(e) => self.status(ConnStatus::Offline, CasinoError::from(e).to_string()),
        }
    }

    /// Re-reads the balances. They are only taken when no bet ran meanwhile, so a reply
    /// older than the latest local update never wins.
    async fn refresh_balances(&self) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let data = match self.signed(reqwest::Method::GET, BALANCES, None).await {
            Ok(d) => d,
            Err(CallError::Unauthenticated) => {
                self.status(ConnStatus::AuthFailed, EXPIRED);
                return Err(CallError::Unauthenticated);
            }
            Err(e) => return Err(e),
        };
        if !(idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled) {
            return Ok(());
        }
        {
            let mut m = self.market.lock();
            m.wallets.clear();
            for (cur, amount) in balances(&data) {
                m.wallets.insert(cur, amount);
            }
            // the site keeps the picked coin in the browser, not the account; start on the biggest balance
            if m.site_currency.is_none() {
                m.site_currency = richest(&m);
            }
        }
        self.publish();
        Ok(())
    }

    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let data = self.signed(reqwest::Method::GET, ACTIVE_SEED, None).await?;
        let seed = ["seed", "active_seed", "activeSeed"].iter().find_map(|k| data.get(*k).filter(|v| v.is_object())).unwrap_or(&data);
        Ok(ProvableState::from_json(seed))
    }

    fn apply_rates(&self, data: &Value) {
        let Some(list) = data.get("crypto").and_then(Value::as_object) else { return };
        {
            let mut m = self.market.lock();
            for (name, price) in list {
                let name = name.to_ascii_lowercase();
                let hidden = HIDDEN.contains(&name.as_str());
                m.currencies.insert(name, CurrencyInfo { decimals: DECIMALS, price: f64_from_json(price).unwrap_or(0.0), hidden });
            }
        }
        self.publish();
    }

    /// An original's reply: the round, with the balance Goated reports after it.
    fn round(&self, game: Game, bet: &Value) -> Result<Round, CasinoError> {
        let round = round_of(game, bet).ok_or_else(|| CasinoError::BadReply(bet.to_string().chars().take(200).collect()))?;
        tracing::debug!("goated: {} -> open {} ×{} {}", game.name(), round.active, round.multiplier, round.state);
        if let Some(after) = bet.pointer(&format!("/updated_balance/{}", round.currency.to_ascii_uppercase())).and_then(coin_units) {
            self.market.lock().wallets.insert(round.currency.clone(), after);
            self.settled.fetch_add(1, Ordering::SeqCst);
            self.publish();
        }
        Ok(round)
    }

    /// Books a settled bet into the local wallet.
    fn credit(&self, cur: &str, delta: Units) {
        if delta == 0 {
            return;
        }
        *self.market.lock().wallets.entry(cur.to_string()).or_insert(0) += delta;
        self.publish();
    }

    fn publish(&self) {
        let m = self.market.lock().clone();
        let mut last = self.published.lock();
        if *last != m {
            *last = m.clone();
            let _ = self.events.send(CasinoEvent::Market(m));
        }
    }

    /// One request to the API; `access` signs it. Answers `data`.
    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>, access: Option<&str>) -> Result<Value, CallError> {
        let mut req = self.client.request(method, format!("{API}{path}")).timeout(RPC_TIMEOUT);
        if let Some(t) = access {
            req = req.bearer_auth(t);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
        let res = req.send().await.map_err(lost)?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(lost)?;
        parse_reply(status, &text)
    }

    /// A signed-in request. The access token is renewed when it is about to run out, or when
    /// the API refuses it, and then the request goes out once more.
    async fn signed(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, CallError> {
        let creds = self.creds.lock().clone().ok_or(CallError::Casino(CasinoError::Server("Not logged in".into())))?;
        let mut access = creds.access.clone();
        if let Some(left) = creds.expires_in().filter(|l| *l < RENEW_BEFORE && !creds.refresh.is_empty()) {
            match self.renew(&creds.access).await {
                Ok(fresh) => access = fresh,
                // still valid for a while: use it, the next call tries again
                Err(_) if left > 30 => {}
                Err(e) => return Err(e),
            }
        }
        match self.call(method.clone(), path, body.clone(), Some(&access)).await {
            Err(CallError::Unauthenticated) => {
                let fresh = self.renew(&access).await?;
                self.call(method, path, body, Some(&fresh)).await
            }
            res => res,
        }
    }

    /// Trades the refresh cookie for a new access token, once for every call that saw `stale`
    /// refused. The new login (with the new cookie, when Goated sends one) is handed to the app to save.
    async fn renew(&self, stale: &str) -> Result<String, CallError> {
        let _one = self.renewing.lock().await;
        let creds = self.creds.lock().clone().ok_or(CallError::Unauthenticated)?;
        if creds.access != stale {
            return Ok(creds.access);
        }
        if creds.refresh.is_empty() {
            return Err(CallError::Unauthenticated);
        }
        let res = self
            .client
            .post(format!("{API}{RENEW}"))
            .timeout(RPC_TIMEOUT)
            .bearer_auth(&creds.access)
            .header(reqwest::header::COOKIE, format!("{REFRESH_COOKIE}={}", creds.refresh))
            .json(&json!({}))
            .send()
            .await
            .map_err(|_| CallError::Casino(CasinoError::ConnectionLost))?;
        let rotated = res.headers().get_all(reqwest::header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(refresh_cookie);
        let status = res.status().as_u16();
        let text = res.text().await.map_err(|_| CallError::Casino(CasinoError::ConnectionLost))?;
        let data = match parse_reply(status, &text) {
            Ok(d) => d,
            // a refused refresh cookie: the login is over
            Err(CallError::Unauthenticated) | Err(CallError::Casino(CasinoError::Server(_))) => return Err(CallError::Casino(CasinoError::Server(EXPIRED.into()))),
            Err(e) => return Err(e),
        };
        let access = data.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| CallError::Casino(CasinoError::BadReply(text.chars().take(200).collect())))?;
        let next = Creds { access: access.to_string(), refresh: rotated.unwrap_or_else(|| creds.refresh.clone()) };
        if next.refresh != creds.refresh {
            self.spent.lock().insert(creds.refresh.clone());
        }
        {
            let mut cur = self.creds.lock();
            if cur.as_ref() != Some(&creds) {
                // a new login arrived meanwhile; it wins
                return cur.as_ref().map(|c| c.access.clone()).ok_or(CallError::Unauthenticated);
            }
            *cur = Some(next.clone());
        }
        tracing::info!("goated: login renewed");
        let _ = self.events.send(CasinoEvent::Token(next.to_token()));
        Ok(next.access)
    }
}

#[async_trait]
impl Casino for Goated {
    fn id(&self) -> CasinoId {
        CasinoId::Goated
    }

    fn set_token(&self, token: Option<String>) {
        let next = token.as_deref().and_then(Creds::parse);
        {
            let mut cur = self.creds.lock();
            let spent = next.as_ref().is_some_and(|n| !n.refresh.is_empty() && self.spent.lock().contains(&n.refresh));
            if !(spent && cur.is_some()) {
                *cur = next;
            }
        }
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
        let cur = req.currency.to_ascii_lowercase();
        let body = json!({
            "multiplier": req.target,
            "amount": to_f64(req.amount, DECIMALS),
            "currency": cur.to_ascii_uppercase(),
            "is_fiat": false,
        });
        let bet = self.signed(reqwest::Method::POST, LIMBO, Some(body)).await?;
        let bad = || CasinoError::BadReply(bet.to_string().chars().take(200).collect());
        let outcome = bet.get("multiplier_output").and_then(f64_from_json).ok_or_else(bad)?;
        let amount = bet.get("amount").and_then(coin_units);
        let payout = bet.get("payout").and_then(coin_units);
        if let (Some(a), Some(p)) = (amount, payout) {
            self.credit(&cur, p - a);
        }
        self.settled.fetch_add(1, Ordering::SeqCst);
        // a bet of 0 pays 0 either way: the result against the target decides it
        let win = if req.amount == 0 { Some(outcome >= req.target) } else { payout.map(|p| p > 0) };
        Ok(BetReply { outcome, amount, winnings: payout, win, nonce: bet.get("nonce").and_then(Value::as_u64) })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        self.signed(reqwest::Method::POST, ROTATE_SEED, Some(json!({ "client_seed": client_seed }))).await?;
        let mut st = self.seeds().await.unwrap_or_default();
        if st.client_seed.is_none() {
            st.client_seed = Some(client_seed.to_string());
        }
        if st.nonce.is_none() {
            st.nonce = Some(0);
        }
        Ok(st)
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        self.seeds().await
    }

    async fn list_slots(&self, _filter: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _catalog_id: &str, _demo: bool, _currency: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots aren't available on Goated yet".into()))
    }

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let (amount, currency) = (to_f64(amount, DECIMALS), currency.to_ascii_uppercase());
        let (path, body) = match game {
            Game::Dice => {
                let target = params["target"].as_f64().ok_or_else(|| CasinoError::Server("Pick a target".into()))?;
                let roll = if params["condition"].as_str() == Some("below") { "UNDER" } else { "OVER" };
                (DICE, json!({ "user_value": target, "roll_type": roll, "amount": amount, "currency": currency, "is_fiat": false }))
            }
            Game::Mines => {
                let mines = params["minesCount"].as_u64().unwrap_or(3);
                if auto {
                    (MINES_AUTO, json!({ "num_mines": mines, "amount": amount, "currency": currency, "locations": params["fields"], "is_fiat": false }))
                } else {
                    (MINES_BET, json!({ "num_mines": mines, "amount": amount, "currency": currency, "is_fiat": false }))
                }
            }
            _ => return Err(CasinoError::Server(format!("{} isn't on Goated", game.name()))),
        };
        let bet = self.signed(reqwest::Method::POST, path, Some(body)).await?;
        self.round(game, &bet)
    }

    async fn original_step(&self, game: Game, step: Step, params: Value) -> Result<Round, CasinoError> {
        if game != Game::Mines {
            return Err(CasinoError::Server(format!("{} has no steps", game.name())));
        }
        let _inflight = InFlight::new(&self.inflight);
        let bet = match step {
            Step::Next => {
                let tile = params["fields"].get(0).and_then(Value::as_u64).ok_or_else(|| CasinoError::Server("Pick a tile".into()))?;
                self.signed(reqwest::Method::PATCH, MINES_SELECT, Some(json!({ "location": tile }))).await?
            }
            Step::Cashout => self.signed(reqwest::Method::POST, MINES_CASHOUT, Some(json!({}))).await?,
        };
        self.round(game, &bet)
    }

    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        if game != Game::Mines {
            return Ok(None);
        }
        let bet = self.signed(reqwest::Method::GET, MINES_ACTIVE, None).await?;
        if !bet.is_object() {
            return Ok(None);
        }
        round_of(game, &bet).map(Some).ok_or_else(|| CasinoError::BadReply(bet.to_string().chars().take(200).collect()))
    }

    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        Ok(Tables::builtin())
    }

    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let html = self.client.get(SITE).timeout(RPC_TIMEOUT).send().await.map_err(|_| CasinoError::ConnectionLost)?.text().await.map_err(|_| CasinoError::ConnectionLost)?;
        Ok(ART_TILES
            .iter()
            .filter_map(|(slug, name)| {
                let image = tile_art(&html, name)?;
                Some(crate::slots::SlotGame {
                    id: slug.to_string(),
                    name: name.to_string(),
                    producer: "goated".into(),
                    image,
                    demo: false,
                    rtp: 99.0,
                    provider_game_id: None,
                    studio: None,
                })
            })
            .collect())
    }
}

/// A Goated reply -> a round in the state shape of Stake's board for the game.
fn round_of(game: Game, bet: &Value) -> Option<Round> {
    let num = |k: &str| bet.get(k).and_then(f64_from_json);
    let ints = |k: &str| -> Vec<i64> { bet.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default() };
    let (active, state) = match game {
        Game::Dice => {
            let condition = if bet.get("roll_type").and_then(Value::as_str) == Some("UNDER") { "below" } else { "above" };
            (false, json!({ "result": num("roll_value")?, "target": num("user_value")?, "condition": condition }))
        }
        Game::Mines => {
            let count = bet.get("num_mines").and_then(Value::as_u64).unwrap_or(0) as u32;
            (bet.get("is_active").and_then(Value::as_bool).unwrap_or(false), mines_state(count, &ints("picked_locations"), &ints("mine_locations")))
        }
        _ => return None,
    };
    Some(Round {
        game,
        id: bet.get("id")?.as_str()?.to_string(),
        active,
        currency: bet.get("currency").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase(),
        amount: bet.get("amount").and_then(coin_units).unwrap_or(0),
        amount_mult: 1.0,
        payout: bet.get("payout").and_then(coin_units).unwrap_or(0),
        // null while a round is open
        multiplier: num("multiplier").unwrap_or(0.0),
        state,
    })
}

/// The artwork of the homepage's tile `<img alt="<name> on Goated" … url=<path>&…>`, as a full URL.
fn tile_art(html: &str, name: &str) -> Option<String> {
    let at = html.find(&format!("alt=\"{name} on Goated\""))?;
    let tag = &html[at..at + html[at..].find('>')?];
    let start = tag.find("url=")? + 4;
    let encoded = &tag[start..start + tag[start..].find(['&', '"', ' '])?];
    let path = encoded.replace("%2F", "/").replace("%2f", "/");
    (path.starts_with("/_next/static/") && !path.contains("..")).then(|| format!("{SITE}{path}"))
}

/// Plain HTTPS to the API, looking like the site's own requests.
fn client() -> reqwest::Client {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_static(SITE));
    headers.insert(REFERER, HeaderValue::from_static("https://www.goated.com/"));
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        // header names as browsers write them: Cloudflare turns away lowercase ones over HTTP/1.1
        .http1_title_case_headers()
        .default_headers(headers)
        .gzip(true)
        .build()
        .unwrap_or_default()
}

/// `{success, data}` -> data; `{success: false, error, status}` -> the error.
fn parse_reply(status: u16, body: &str) -> Result<Value, CallError> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        if status == 401 {
            return Err(CallError::Unauthenticated);
        }
        if status == 429 {
            return Err(CallError::Casino(CasinoError::Busy("Too many requests".into())));
        }
        let challenge = ["challenge-platform", "Just a moment", "Attention Required", "cf-chl"].iter().any(|k| body.contains(k));
        if challenge || status == 403 || status == 503 {
            return Err(CallError::Cloudflare);
        }
        let snippet: String = body.chars().take(160).collect();
        return Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status}: {snippet}"))));
    };
    let code = v.get("status").and_then(Value::as_u64).unwrap_or(status as u64);
    if v.get("success").and_then(Value::as_bool) == Some(true) && (200..300).contains(&status) {
        return Ok(v.get("data").cloned().unwrap_or(Value::Null));
    }
    if code == 401 || status == 401 {
        return Err(CallError::Unauthenticated);
    }
    // the error comes as `error`, or as a string in `data`
    let msg = v.get("error").and_then(Value::as_str).or_else(|| v.get("data").and_then(Value::as_str)).or_else(|| v.get("message").and_then(Value::as_str)).unwrap_or("Goated refused the request").to_string();
    if code == 429 || status == 429 {
        return Err(CallError::Casino(CasinoError::Busy(msg)));
    }
    Err(CallError::Casino(CasinoError::Server(msg)))
}

/// `refresh_token=<value>; Path=…` -> the value.
fn refresh_cookie(set_cookie: &str) -> Option<String> {
    let (name, value) = set_cookie.split(';').next()?.split_once('=')?;
    (name.trim() == REFRESH_COOKIE && !value.trim().is_empty()).then(|| value.trim().to_string())
}

/// Balances as `{ "USDT": 1.5, … }`, or a list of `{currency, amount|balance}`.
fn balances(data: &Value) -> Vec<(String, Units)> {
    match data {
        Value::Object(map) => map.iter().filter_map(|(cur, v)| Some((cur.to_ascii_lowercase(), coin_units(v)?))).collect(),
        Value::Array(list) => list
            .iter()
            .filter_map(|b| {
                let cur = b.get("currency").and_then(Value::as_str)?;
                let amount = ["amount", "balance", "available"].iter().find_map(|k| b.get(*k).and_then(coin_units))?;
                Some((cur.to_ascii_lowercase(), amount))
            })
            .collect(),
        _ => vec![],
    }
}

/// 0.00012345 (number or string) -> 12345 base units. Digits past the 8th decimal are dropped.
fn coin_units(v: &Value) -> Option<Units> {
    match v {
        Value::String(s) => parse_units(s, DECIMALS),
        Value::Number(_) => {
            let f = f64_from_json(v).filter(|f| f.is_finite() && *f >= 0.0)?;
            parse_units(&format!("{f:.8}"), DECIMALS)
        }
        _ => None,
    }
}

/// The coin holding the most value, else USDT.
fn richest(m: &Market) -> Option<String> {
    let usd = |cur: &str, v: Units| to_f64(v, DECIMALS) * m.info(cur).map(|i| i.price).unwrap_or(0.0);
    m.wallets
        .iter()
        .map(|(c, v)| (c, usd(c, *v)))
        .filter(|(_, u)| *u > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c.clone())
        .or_else(|| m.wallets.contains_key("usdt").then(|| "usdt".to_string()))
        .or_else(|| m.wallets.keys().next().cloned())
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
    fn replies() {
        let ok = parse_reply(200, r#"{"success":true,"data":{"multiplier_output":2.5}}"#).ok().unwrap();
        assert_eq!(ok["multiplier_output"], 2.5);
        assert!(matches!(parse_reply(200, r#"{"success":false,"error":"Unauthorized","status":401}"#), Err(CallError::Unauthenticated)));
        assert!(matches!(parse_reply(401, "Unauthorized"), Err(CallError::Unauthenticated)));
        assert!(matches!(parse_reply(200, r#"{"success":false,"data":"Insufficient balance"}"#), Err(CallError::Casino(CasinoError::Server(m))) if m == "Insufficient balance"));
        assert!(matches!(parse_reply(403, "<html>Just a moment...</html>"), Err(CallError::Cloudflare)));
    }

    #[test]
    fn originals_replies() {
        // trimmed from live 0-amount rounds
        let dice = json!({ "id": "R08", "user_value": 98, "roll_type": "UNDER", "roll_value": 64, "payout": 0, "multiplier": 1.0102, "amount": 0, "currency": "USDT" });
        let r = round_of(Game::Dice, &dice).unwrap();
        assert_eq!((r.state["result"].as_f64(), r.state["condition"].as_str(), r.currency.as_str()), (Some(64.0), Some("below"), "usdt"));
        let open = json!({ "id": "PW", "is_active": true, "num_mines": 3, "picked_locations": [12], "mine_locations": [], "multiplier": null, "amount": 0, "currency": "USDT" });
        let r = round_of(Game::Mines, &open).unwrap();
        assert!(r.active && r.multiplier == 0.0);
        assert_eq!(r.state["rounds"][0]["payoutMultiplier"].as_f64(), Some(1.125));
        let lost = json!({ "id": "8U", "is_active": false, "num_mines": 20, "picked_locations": [0], "mine_locations": [9, 0, 1], "multiplier": 0, "payout": 0, "amount": 0, "currency": "USDT" });
        let r = round_of(Game::Mines, &lost).unwrap();
        assert!(!r.active && r.state["rounds"][0]["payoutMultiplier"].as_f64() == Some(0.0));
        let html = r#"<a><img alt="Dice on Goated" loading="lazy" src="/_next/image?url=%2F_next%2Fstatic%2Fmedia%2Fdice.120bc1e2.png&amp;w=1536&amp;q=75"></a>"#;
        assert_eq!(tile_art(html, "Dice").as_deref(), Some("https://www.goated.com/_next/static/media/dice.120bc1e2.png"));
        assert_eq!(tile_art(html, "Mines"), None);
    }

    #[test]
    fn cookies_and_balances() {
        assert_eq!(refresh_cookie("refresh_token=abc.def; Path=/; HttpOnly; Secure").as_deref(), Some("abc.def"));
        assert_eq!(refresh_cookie("other=1; Path=/"), None);
        let b = balances(&json!({ "USDT": 1.5, "BTC": "0.00012345" }));
        assert!(b.contains(&("usdt".into(), 150_000_000)) && b.contains(&("btc".into(), 12_345)));
        let b = balances(&json!([{ "currency": "ETH", "balance": 0.25 }]));
        assert_eq!(b, vec![("eth".into(), 25_000_000)]);
    }
}
