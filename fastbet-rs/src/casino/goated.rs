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
use std::time::{Duration, Instant, SystemTime};

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
/// The games of the providers asked for (`?providers=a,b`), grouped by category (public).
const GAMES: &str = "/main/games?providers=";
/// A game session's URL: `{currency, platform, fiat, game_id, demo_mode, locale}` (Hub88).
const LAUNCH: &str = "/hub88/create";
/// How long the slot list is kept before it is fetched again.
const SLOTS_TTL: Duration = Duration::from_secs(600);
/// Goated's (Hub88's) provider codes with slots -> the producer names the drivers know; a code
/// without one stays itself and shows as Untested.
pub const PROVIDERS: &[(&str, &str)] = &[
    ("pgp", "pragmaticplay"),
    ("hsg", "hacksawgaming"),
    ("spm", "spinomenal"),
    ("png", "play'ngo"),
    ("nlc", "nolimitcity"),
    ("bgm", "bgaming"),
    ("rlx", "relaxgaming"),
    ("rtg", "redtigergaming"),
    ("avx", "avatarux"),
    ("ntn", "netent"),
    ("bmg", "boominggames"),
    ("wzn", "voltent"),
    ("end", "endorphina"),
    ("pgs", "pgsoft"),
    ("trf", "truelab"),
    ("popl", "popiplay"),
    ("oak", "3oaks"),
    ("oct", "penguinking"),
    ("pets", "peter&sons"),
    ("slm", "slotmillgames"),
    ("big", "bigtimegaming"),
    ("bst", "betsoft"),
    ("shdla", "shadylady"),
    ("bckg", "backseatgaming"),
    ("bpt", "blueprintgaming"),
    ("hp", "happyduck"),
    ("hbo", "habanero"),
    ("snk", "sneakyslots"),
    ("rgg", "retrogaming"),
];
const VIP_REWARDS: &str = "/user/vip-rewards";
const DAILY_BONUS: &str = "/user/daily-bonus";
const DAILY_BONUS_CLAIM: &str = "/user/daily-bonus/claim";
/// Where the VIP rewards are claimed: Goated asks for a reCAPTCHA there.
const VIP_PAGE: &str = "https://www.goated.com/vip-program";
/// Refresh ticks between profile reads (for the VIP progress).
const VIP_EVERY: u32 = 6;
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
    last_vip: Mutex<Option<VipStatus>>,
    /// Every slot Goated lists, and when it was fetched (the API has no search or paging).
    slots: Mutex<Option<(Instant, Vec<crate::slots::SlotGame>)>>,
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
            last_vip: Mutex::new(None),
            slots: Mutex::new(None),
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
        let mut ticks: u32 = 0;
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
                        // the profile carries the VIP progress: now and then
                        ticks += 1;
                        if ticks % VIP_EVERY == 0 {
                            if let Ok(user) = self.signed(reqwest::Method::GET, USER, None).await {
                                self.apply_vip(&user);
                            }
                        }
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
                self.apply_vip(&user);
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

    /// Every slot of the providers in `PROVIDERS`, from the cache while it is fresh.
    async fn slot_list(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        if let Some((at, list)) = self.slots.lock().as_ref() {
            if at.elapsed() < SLOTS_TTL && !list.is_empty() {
                return Ok(list.clone());
            }
        }
        let codes: Vec<&str> = PROVIDERS.iter().map(|(c, _)| *c).collect();
        let data = self.call(reqwest::Method::GET, &format!("{GAMES}{}", codes.join(",")), None, None).await?;
        let list = slots_of(&data);
        if !list.is_empty() {
            *self.slots.lock() = Some((Instant::now(), list.clone()));
        }
        Ok(list)
    }

    /// The profile's `vip_progress`: the rank, the next one and the XP each starts at.
    fn apply_vip(&self, user: &Value) {
        if let Some(vip) = vip_status(user) {
            send_vip(&self.events, &self.last_vip, vip);
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
                m.currencies.insert(name, CurrencyInfo { decimals: DECIMALS, price: f64_from_json(price).unwrap_or(0.0), hidden, fiat: false });
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

    /// The VIP rewards waiting (claimed on Goated, which asks for a reCAPTCHA) and the daily bonus
    /// (claimed here).
    async fn rewards(&self) -> Result<Vec<Reward>, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let (vip, daily) = tokio::join!(self.signed(reqwest::Method::GET, VIP_REWARDS, None), self.signed(reqwest::Method::GET, DAILY_BONUS, None));
        let mut out = rewards_of(&vip?, SystemTime::now());
        if let Some(d) = daily.ok().and_then(|d| daily_of(&d, SystemTime::now())) {
            out.push(d);
        }
        Ok(out)
    }

    async fn claim_reward(&self, id: &str, _currency: &str) -> Result<String, CasinoError> {
        if id != "daily" {
            return Err(CasinoError::Server("Claim this one on Goated: it asks for a captcha there".into()));
        }
        let data = self.signed(reqwest::Method::POST, DAILY_BONUS_CLAIM, Some(json!({}))).await?;
        let _ = self.refresh_balances().await;
        Ok(match data.get("amount").and_then(f64_from_json) {
            Some(usd) => format!("Claimed {}", crate::money::fmt_usd(usd)),
            None => "Claimed".into(),
        })
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

    /// Goated hands out every game of the providers asked for in one go, so the list is fetched
    /// once (and again every few minutes) and searched and paged here.
    async fn list_slots(&self, f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let all = self.slot_list().await?;
        let q = f.query.as_deref().map(|q| q.trim().to_ascii_lowercase()).filter(|q| !q.is_empty());
        Ok(all
            .into_iter()
            .filter(|g| f.producer.as_deref().is_none_or(|p| g.producer == p))
            .filter(|g| q.as_deref().is_none_or(|q| g.name.to_ascii_lowercase().contains(q)))
            .skip(f.skip)
            .take(f.limit.max(1))
            .collect())
    }

    async fn launch_slot(&self, catalog_id: &str, demo: bool, currency: &str, fiat: &str) -> Result<String, CasinoError> {
        let cur = currency.trim().to_ascii_uppercase();
        if !demo && (cur.is_empty() || cur == "POINTS") {
            return Err(CasinoError::Server("Pick a coin in the wallet to play for real (points can't pay for slots)".into()));
        }
        let fiat = if fiat.is_empty() { "USD".to_string() } else { fiat.to_ascii_uppercase() };
        let body = json!({ "currency": if cur.is_empty() || cur == "POINTS" { "USDT".to_string() } else { cur }, "platform": "GPL_DESKTOP", "fiat": fiat, "game_id": catalog_id, "demo_mode": demo, "locale": "en" });
        let data = self.signed(reqwest::Method::POST, LAUNCH, Some(body)).await?;
        data.get("url").and_then(Value::as_str).map(str::to_string).ok_or_else(|| CasinoError::BadReply(data.to_string().chars().take(200).collect()))
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
/// `/main/games`' `SLOTS` category -> the slots, as Goated lists them (its most popular first).
fn slots_of(data: &Value) -> Vec<crate::slots::SlotGame> {
    let groups = data.as_array().map(Vec::as_slice).unwrap_or_default();
    let slots = groups.iter().find(|g| g.get("category").and_then(Value::as_str) == Some("SLOTS"));
    slots
        .and_then(|g| g.get("games"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|g| {
            let code = g.get("pr").and_then(Value::as_str)?;
            let producer = PROVIDERS.iter().find(|(c, _)| *c == code).map_or(code, |(_, ours)| *ours);
            let known = crate::slots::Provider::display_producer(producer) != producer;
            Some(crate::slots::SlotGame {
                id: g.get("id")?.as_str()?.to_string(),
                name: g.get("ti").or(g.get("name")).and_then(Value::as_str)?.trim().to_string(),
                producer: producer.to_string(),
                // `p` is the tile's path, already escaped
                image: g.get("p").and_then(Value::as_str).map(|p| format!("https://firebasestorage.googleapis.com/v0/b/g3casino.appspot.com/o/hub88gametiles{p}?alt=media")).unwrap_or_default(),
                demo: true,
                rtp: g.get("rtp").and_then(f64_from_json).unwrap_or(0.0),
                provider_game_id: None,
                studio: if known { None } else { g.get("pn").and_then(Value::as_str).map(str::to_string) },
            })
        })
        .collect()
}

/// `/user/vip-rewards`' `claimable` list -> the reward cards, all claimed on Goated's VIP page.
/// Rakeback is always shown, empty or not.
fn rewards_of(data: &Value, now: SystemTime) -> Vec<Reward> {
    let mut out = Vec::new();
    for r in data.get("claimable").and_then(Value::as_array).into_iter().flatten() {
        if r["claimed"].as_bool() == Some(true) {
            continue;
        }
        let expires = r.get("expires_at").and_then(f64_from_json).filter(|e| *e > 0.0).map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms as u64));
        if expires.is_some_and(|e| e <= now) {
            continue;
        }
        let ty = r.get("reward_type").and_then(Value::as_str).unwrap_or("");
        let kind = match ty {
            "RAKEBACK_SELF" => RewardKind::Rakeback,
            "DAILY_BONUS" => RewardKind::Daily,
            "WEEKLY_BONUS" => RewardKind::Weekly,
            "MONTHLY_BONUS" => RewardKind::Monthly,
            t if t.contains("LEVEL_UP") || t.starts_with("TIER_UP") => RewardKind::LevelUp,
            t if t.contains("RELOAD") => RewardKind::Reload,
            _ => RewardKind::Bonus,
        };
        let coins: Vec<(String, f64)> = r
            .get("amount")
            .and_then(Value::as_object)
            .map(|o| o.iter().filter_map(|(c, a)| Some((c.clone(), f64_from_json(a)?))).filter(|(_, a)| *a > 0.0).collect())
            .unwrap_or_default();
        out.push(Reward {
            title: r.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| kind.name().to_string()),
            link: Some(VIP_PAGE.into()),
            usd: r.get("amount_usd").and_then(f64_from_json),
            detail: if coins.is_empty() { "Claimed on Goated, which asks for a captcha".into() } else { coins_text(&coins) },
            claimable: true,
            expires,
            ..Reward::new(r.get("id").and_then(Value::as_str).unwrap_or(ty), kind)
        });
    }
    if !out.iter().any(|r| r.kind == RewardKind::Rakeback) {
        out.insert(0, Reward { usd: Some(0.0), detail: "Nothing waiting yet".into(), link: Some(VIP_PAGE.into()), ..Reward::new("RAKEBACK_SELF", RewardKind::Rakeback) });
    }
    out
}

/// `/user/daily-bonus` -> the daily bonus card, when Goated runs one for the account.
fn daily_of(d: &Value, now: SystemTime) -> Option<Reward> {
    if d["enabled"].as_bool() != Some(true) {
        return None;
    }
    let ready = d["eligible"].as_bool() == Some(true) && d["claimed_today"].as_bool() != Some(true);
    let next = d.get("next_claim_at").and_then(f64_from_json).filter(|ms| *ms > 0.0).map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms as u64)).filter(|t| *t > now);
    let streak = d.get("streak_day").and_then(Value::as_u64).unwrap_or(0);
    let detail = match d.get("ineligible_reason").and_then(Value::as_str) {
        Some(why) if !ready && d["eligible"].as_bool() != Some(true) => why.to_string(),
        _ if streak > 0 => format!("Day {streak} of the streak"),
        _ => "Comes back every day".into(),
    };
    Some(Reward { detail, claimable: ready, next_at: if ready { None } else { next }, ..Reward::new("daily", RewardKind::Daily) })
}

/// `/user/`'s `vip_progress {xp, current_rank, next_rank, current_rank_xp, next_rank_xp}`, with
/// `stats.total_wagered` for the XP per dollar.
fn vip_status(user: &Value) -> Option<VipStatus> {
    let v = user.get("vip_progress").filter(|v| v.is_object())?;
    let xp = v.get("xp").and_then(f64_from_json)?;
    let rank = v.get("current_rank").and_then(Value::as_str).unwrap_or("Unranked").to_string();
    let start = v.get("current_rank_xp").and_then(f64_from_json).unwrap_or(0.0);
    let next = v.get("next_rank").and_then(Value::as_str).filter(|n| !n.is_empty()).and_then(|n| Some((n.to_string(), v.get("next_rank_xp").and_then(f64_from_json)?)));
    let wagered = user.pointer("/stats/total_wagered").and_then(f64_from_json).unwrap_or(0.0);
    Some(VipStatus::ranked(rank, xp, start, next, Some(xp_rate(xp, wagered))))
}

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
    fn slot_list() {
        let data = json!([{"category": "ORIGINALS", "games": []}, {"category": "SLOTS", "games": [
            {"id": "hsg_levampire", "ti": "Le Vampire", "pr": "hsg", "pn": "Hacksaw Gaming", "p": "%2Fhacksaw-gaming%2Fhsg_levampire.png", "c": "bd8d35", "xx": "US"},
            {"id": "bmg_x", "ti": "X", "pr": "bmg", "pn": "Booming Games", "p": "%2Fbooming%2Fx.png"}]}]);
        let list = slots_of(&data);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].id.as_str(), list[0].name.as_str(), list[0].producer.as_str()), ("hsg_levampire", "Le Vampire", "hacksawgaming"));
        assert_eq!(list[0].image, "https://firebasestorage.googleapis.com/v0/b/g3casino.appspot.com/o/hub88gametiles%2Fhacksaw-gaming%2Fhsg_levampire.png?alt=media");
        assert!(list[0].studio.is_none());
        // a provider without a driver keeps its own name to show
        assert_eq!(list[1].studio.as_deref(), Some("Booming Games"));
    }

    #[test]
    fn rewards_lists() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_792_148_400);
        // the account's claimable list that day: some rakeback
        let data = json!({"claimable": [{"id": "RAKEBACK_SELF", "reward_type": "RAKEBACK_SELF", "name": "Rakeback", "amount": {"USDC": 0.0000172, "USDT": 0.0000084}, "amount_usd": 0.0000256, "created_at": 1792147464856u64, "expires_at": -1, "claimed": false}]});
        let list = rewards_of(&data, now);
        assert_eq!(list.len(), 1);
        assert!(list[0].claimable && list[0].link.as_deref() == Some(VIP_PAGE) && list[0].usd == Some(0.0000256) && list[0].expires.is_none());
        assert_eq!(list[0].detail, "0.0000172 USDC + 0.0000084 USDT");
        // nothing waiting: an empty rakeback card
        let list = rewards_of(&json!({"claimable": []}), now);
        assert!(list.len() == 1 && !list[0].claimable && list[0].kind == RewardKind::Rakeback);
        // the daily bonus is off for this account
        assert!(daily_of(&json!({"enabled": false, "eligible": false}), now).is_none());
        let d = daily_of(&json!({"enabled": true, "eligible": true, "claimed_today": false, "streak_day": 3, "next_claim_at": 0}), now).unwrap();
        assert!(d.claimable && d.link.is_none() && d.detail == "Day 3 of the streak");
    }

    #[test]
    fn vip_from_profile() {
        let user = json!({"vip_progress": {"xp": 1872, "level": 1, "current_rank": "Bronze 1", "next_rank": "Bronze 2", "current_rank_xp": 1000, "next_rank_xp": 2000},
            "stats": {"total_wagered": 1872.764068956265}});
        let v = vip_status(&user).unwrap();
        assert_eq!(v.rank, "Bronze 1");
        assert_eq!(v.next_rank, Some(("Bronze 2".to_string(), 2000.0)));
        assert!((v.rank_progress() - 0.872).abs() < 1e-9);
        assert!((v.xp_per_usd.unwrap() - 1872.0 / 1872.764068956265).abs() < 1e-9);
        assert!(vip_status(&json!({"stats": {}})).is_none());
    }

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
