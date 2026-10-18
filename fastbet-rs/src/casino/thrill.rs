//! Thrill.com: the account and wallets over the site's API (`https://api.thrill.com`), the
//! originals (Limbo, Keno, Mines) over their own API (`originals-instant-api.thrill-games.com`).
//!
//! The login is the HttpOnly cookie `token` the login window reads: a Stytch session JWT that
//! lives five minutes. The API still takes an expired one while the session lives (30 days) and
//! sends a fresh cookie back, which is kept and saved. An original is played through a game
//! session the site's API opens (`/v2/games/original/launch` → `sessionId`, one per game and
//! coin); the game API answers `{data, responseCode, message}`. Bets of 0 are allowed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use reqwest::Method;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::money::{f64_from_json, parse_units, to_f64};
use crate::originals::{mines_state, Game, Round, Step, Tables};

const API: &str = "https://api.thrill.com";
const SITE: &str = "https://thrill.com";
const GAMES: &str = "https://originals-instant-api.thrill-games.com/v1";
const UTILS: &str = "https://games-utils-api.thrill-games.com/v1";
/// The games' tiles: `<ART>/thrill-<game>_<version>.webp`.
const ART: &str = "https://rushcdn.net/thumbnails";
pub const TOKEN_COOKIE: &str = "token";
/// Amounts travel as numbers in the coin; 8 decimals cover every coin's bet sizes.
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// Balances are re-read this often while idle; bets update them locally.
const REFRESH: Duration = Duration::from_secs(10);
/// Plain HTTPS presents itself as desktop Chrome.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
const EXPIRED: &str = "Your Thrill login has expired. Log in again in Settings.";
/// Coins worth a dollar each.
const STABLE: &[&str] = &["usdt", "usdc", "usd", "dai", "busd"];
/// Mines' board: 5 × 5.
const EDGE: u64 = 5;

enum CallError {
    /// Thrill's bot check (Fastly) answered instead of the API.
    Challenge,
    /// The login was refused.
    Unauthenticated,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Challenge => CasinoError::Server("Thrill's bot check stopped the request; try again in a moment".into()),
            CallError::Unauthenticated => CasinoError::Server(EXPIRED.into()),
            CallError::Casino(e) => e,
        }
    }
}

/// How a game API call failed.
enum GameError {
    /// The game session is gone: open a new one.
    Session,
    Casino(CasinoError),
}

/// A Mines round in play: what the board needs to draw it.
#[derive(Clone, Debug)]
struct MinesPlay {
    round: String,
    count: u32,
    picks: Vec<i64>,
    amount: Units,
    currency: String,
}

pub struct Thrill {
    token: Mutex<Option<String>>,
    client: reqwest::Client,
    /// `X-BROWSER-SESSION`, one per run of the app (the site makes one per tab).
    browser: String,
    wake: Notify,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// Game sessions by (game, coin): what the game API takes as `X-PLAYER-TOKEN`.
    sessions: Mutex<HashMap<(&'static str, String), String>>,
    /// One session opened at a time, so parallel bets share it.
    launching: tokio::sync::Mutex<()>,
    /// The Mines round in play, if any, and how many times bets and steps changed it (a reply
    /// to an older look at the open round mustn't undo a newer bet).
    mines: Mutex<Option<MinesPlay>>,
    mines_epoch: AtomicU64,
    /// Bets in flight and bets settled, so a balance poll never overwrites a fresher local balance.
    inflight: AtomicUsize,
    settled: AtomicU64,
}

impl Thrill {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let me = Arc::new(Self {
            token: Mutex::new(clean_token(setup.token)),
            client: client(),
            browser: uuid(),
            wake: Notify::new(),
            alive: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            events,
            market: Mutex::new(Market::default()),
            published: Mutex::new(Market::default()),
            sessions: Mutex::new(HashMap::new()),
            launching: tokio::sync::Mutex::new(()),
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
        tracing::info!("thrill: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            if self.token.lock().is_none() {
                self.status(ConnStatus::NoToken, "");
                self.wake.notified().await;
                continue;
            }
            self.status(ConnStatus::Connecting, "Opening thrill.com");
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

    /// The player, the coin picked on the site, the wallets, then the seeds.
    async fn sign_in(&self) -> Result<(), CallError> {
        self.status(ConnStatus::Authenticating, "");
        let player = self.api(Method::GET, "/v1/player", None).await?;
        let name = ["username", "nickname", "displayName", "name"].iter().find_map(|k| player.get(*k).or(player.pointer(&format!("/player/{k}"))).and_then(Value::as_str)).map(str::to_string);
        self.market.lock().username = name;
        if let Ok(s) = self.api(Method::GET, "/v1/player/settings", None).await {
            let picked = ["selectedCrypto", "cryptoCurrency", "currentCrypto"].iter().find_map(|k| s.get(*k).and_then(Value::as_str)).map(str::to_ascii_lowercase);
            if picked.is_some() {
                self.market.lock().site_currency = picked;
            }
        }
        self.refresh_wallets().await?;
        self.status(ConnStatus::Ready, "");
        match self.seeds().await {
            Ok(st) => {
                let _ = self.events.send(CasinoEvent::Provable(st));
            }
            Err(e) => tracing::info!("thrill: seeds: {e}"),
        }
        Ok(())
    }

    /// Keeps balances fresh until the login or the network goes.
    async fn serve(&self) {
        let mut failures = 0;
        loop {
            tokio::select! {
                _ = self.wake.notified() => {
                    if !self.alive() {
                        return;
                    }
                    // a new login: start over with it
                    self.sessions.lock().clear();
                    return;
                }
                _ = tokio::time::sleep(REFRESH) => {
                    match self.refresh_wallets().await {
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

    /// Re-reads the wallets. They are only taken when no bet ran meanwhile, so a reply older
    /// than the latest local update never wins.
    async fn refresh_wallets(&self) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let data = self.api(Method::GET, "/v1/wallets?fiatCurrency=USD", None).await?;
        let fresh = idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled;
        {
            let mut m = self.market.lock();
            for w in wallets(&data) {
                let known = m.currencies.get(&w.currency).map(|i| i.price).unwrap_or(0.0);
                let price = w.price.unwrap_or(known);
                m.currencies.insert(w.currency.clone(), CurrencyInfo { decimals: DECIMALS, price, hidden: w.hidden, fiat: false });
                if fresh {
                    m.wallets.insert(w.currency, w.balance);
                }
            }
            if m.site_currency.is_none() {
                m.site_currency = m.wallets.contains_key("usdt").then(|| "usdt".to_string()).or_else(|| m.wallets.keys().next().cloned());
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

    /// Books a settled bet into the local wallet.
    fn credit(&self, cur: &str, delta: Units) {
        self.settled.fetch_add(1, Ordering::SeqCst);
        if delta == 0 {
            return;
        }
        *self.market.lock().wallets.entry(cur.to_string()).or_insert(0) += delta;
        self.publish();
    }

    /// A fresh `token` cookie in a reply replaces the one in use, and goes to the app to save.
    fn take_cookie(&self, res: &reqwest::Response, sent: &str) {
        let fresh = res.headers().get_all(reqwest::header::SET_COOKIE).iter().filter_map(|v| v.to_str().ok()).find_map(token_cookie);
        let Some(fresh) = fresh else { return };
        if fresh == sent {
            return;
        }
        {
            let mut cur = self.token.lock();
            // a new login arrived meanwhile: it wins
            if cur.as_deref() != Some(sent) {
                return;
            }
            *cur = Some(fresh.clone());
        }
        tracing::debug!("thrill: login renewed");
        let _ = self.events.send(CasinoEvent::Token(fresh));
    }

    /// One request to the site's API with the login cookie. The very first login JWT is now and
    /// then refused for a moment ("User with current JWT not valid"), so that is tried once more.
    async fn api(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value, CallError> {
        let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
        let mut attempt = 0;
        loop {
            let token = self.token.lock().clone().ok_or(CallError::Unauthenticated)?;
            let mut req = self.client.request(method.clone(), format!("{API}{path}")).timeout(RPC_TIMEOUT).header(reqwest::header::COOKIE, format!("{TOKEN_COOKIE}={token}"));
            if let Some(b) = &body {
                req = req.json(b);
            }
            let res = req.send().await.map_err(lost)?;
            self.take_cookie(&res, &token);
            let status = res.status().as_u16();
            let challenged = res.headers().get("fastly-mitigated").and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("challenge"));
            let text = res.text().await.map_err(lost)?;
            if challenged {
                return Err(CallError::Challenge);
            }
            match parse_api(status, &text) {
                Err(CallError::Unauthenticated) if attempt == 0 && text.contains("invalid_user_error") => {
                    attempt += 1;
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                r => return r,
            }
        }
    }

    /// The game session of `game` (limbo, keno, mines) for `currency`, opened when there is none.
    async fn session(&self, game: &'static str, currency: &str) -> Result<String, CasinoError> {
        let key = (game, currency.to_ascii_lowercase());
        if let Some(s) = self.sessions.lock().get(&key) {
            return Ok(s.clone());
        }
        let _one = self.launching.lock().await;
        if let Some(s) = self.sessions.lock().get(&key) {
            return Ok(s.clone());
        }
        let cur = currency.to_ascii_uppercase();
        let launch = |visible: &str| json!({ "gameKey": format!("thrill-{game}"), "underlyingCurrency": cur, "visibleCurrency": visible, "isDemoMode": false, "platform": "desktop" });
        let data = match self.api(Method::POST, "/v2/games/original/launch", Some(launch(&cur))).await {
            Ok(d) => d,
            // the site shows amounts in dollars when the player picked that: try it that way
            Err(CallError::Casino(CasinoError::Server(m))) if m.to_ascii_lowercase().contains("currency") => self.api(Method::POST, "/v2/games/original/launch", Some(launch("USD"))).await?,
            Err(e) => return Err(e.into()),
        };
        let id = data.get("sessionId").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| CasinoError::BadReply(data.to_string().chars().take(200).collect()))?;
        tracing::info!("thrill: {game} session for {cur}");
        self.sessions.lock().insert(key, id.to_string());
        Ok(id.to_string())
    }

    /// One call to `game`'s API in `currency`'s session: `path` with a body is a POST, without
    /// one a GET. A session that lapsed is opened again and the call made once more.
    async fn play(&self, game: &'static str, currency: &str, path: &str, body: Option<Value>) -> Result<Value, CasinoError> {
        self.call_game(&format!("{GAMES}/{game}/{path}"), game, currency, body).await
    }

    async fn call_game(&self, url: &str, game: &'static str, currency: &str, body: Option<Value>) -> Result<Value, CasinoError> {
        let mut attempt = 0;
        loop {
            let session = self.session(game, currency).await?;
            let req = match &body {
                Some(b) => self.client.post(url).json(b),
                None => self.client.get(url),
            };
            let req = req
                .timeout(RPC_TIMEOUT)
                .header("X-PLAYER-TOKEN", &session)
                .header("X-PRODUCT-NAME", format!("thrill-{game}"))
                .header("X-BROWSER-SESSION", &self.browser);
            let lost = |e: reqwest::Error| if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost };
            let res = req.send().await.map_err(lost)?;
            let status = res.status().as_u16();
            let text = res.text().await.map_err(lost)?;
            if !(200..300).contains(&status) {
                tracing::warn!("thrill: {url} -> HTTP {status}: {}", text.chars().take(400).collect::<String>());
            }
            tracing::debug!("thrill: {url} -> {status}: {}", text.chars().take(1500).collect::<String>());
            match parse_game(status, &text) {
                Ok(v) => return Ok(v),
                Err(GameError::Session) if attempt == 0 => {
                    attempt += 1;
                    self.sessions.lock().remove(&(game, currency.to_ascii_lowercase()));
                }
                Err(GameError::Session) => return Err(CasinoError::Server("Thrill closed the game session; try again".into())),
                Err(GameError::Casino(e)) => return Err(e),
            }
        }
    }

    /// The coin the seeds are read in: the one picked on the site.
    fn seed_currency(&self) -> String {
        self.market.lock().site_currency.clone().unwrap_or_else(|| "usdt".into())
    }

    /// The seeds shared by every original (`[active, next]`).
    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let data = self.call_game(&format!("{UTILS}/seed/activeandnext"), "limbo", &self.seed_currency(), None).await?;
        let active = data.get(0).or(data.get("active")).unwrap_or(&data);
        Ok(ProvableState::from_json(active))
    }

    /// The fields every bet carries: the coin, the stake, and what a dollar is worth in it.
    fn stake_fields(&self, cur: &str, amount: Units) -> Result<serde_json::Map<String, Value>, CasinoError> {
        let coins = to_f64(amount, DECIMALS);
        let price = self.price(cur);
        let rate = if price > 0.0 {
            1.0 / price
        } else if amount == 0 {
            1.0
        } else {
            return Err(CasinoError::Server(format!("Thrill hasn't told the app what {} is worth yet; it shows once the wallet holds some", cur.to_ascii_uppercase())));
        };
        let v = json!({
            "walletCurrencyIsoCode": cur.to_ascii_uppercase(),
            "anchorCurrencyIsoCode": "USD",
            "anchorToWalletExchangeRate": rate,
            "inputInAnchorCurrency": false,
            "walletNetStakeAmount": coins,
            "anchorNetStakeAmount": coins / rate,
        });
        Ok(v.as_object().cloned().unwrap_or_default())
    }

    /// Dollars per coin: from the wallets, a dollar for stablecoins.
    fn price(&self, cur: &str) -> f64 {
        let cur = cur.to_ascii_lowercase();
        if STABLE.contains(&cur.as_str()) {
            return 1.0;
        }
        self.market.lock().info(&cur).map(|i| i.price).unwrap_or(0.0)
    }

    /// A finished Mines round from a reply that carries the board (`tilesStatus`).
    fn mines_done(&self, play: &MinesPlay, d: &Value) -> Round {
        let (mines, revealed) = tiles(&d["tilesStatus"]);
        let picks = if play.picks.is_empty() { revealed } else { play.picks.clone() };
        let multiplier = d.get("wonMultiplier").and_then(f64_from_json).unwrap_or(0.0);
        let payout = d.get("wonWalletGrossAmount").and_then(coin_units).unwrap_or_else(|| crate::money::mul_f(play.amount, multiplier));
        self.credit(&play.currency, payout - play.amount);
        Round {
            game: Game::Mines,
            id: play.round.clone(),
            active: false,
            currency: play.currency.clone(),
            amount: play.amount,
            amount_mult: 1.0,
            payout,
            multiplier,
            state: mines_state(play.count, &picks, &mines),
        }
    }

    fn set_mines(&self, play: Option<MinesPlay>) {
        *self.mines.lock() = play;
        self.mines_epoch.fetch_add(1, Ordering::SeqCst);
    }

    fn mines_open(play: &MinesPlay) -> Round {
        Round {
            game: Game::Mines,
            id: play.round.clone(),
            active: true,
            currency: play.currency.clone(),
            amount: play.amount,
            amount_mult: 1.0,
            payout: 0,
            multiplier: 0.0,
            state: mines_state(play.count, &play.picks, &[]),
        }
    }
}

#[async_trait]
impl Casino for Thrill {
    fn id(&self) -> CasinoId {
        CasinoId::Thrill
    }

    fn set_token(&self, token: Option<String>) {
        *self.token.lock() = clean_token(token);
        self.sessions.lock().clear();
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
        let mut body = self.stake_fields(&cur, req.amount)?;
        body.insert("payoutMultiplier".into(), json!(req.target));
        body.insert("playoutTimeMilliseconds".into(), json!(0));
        let d = self.play("limbo", &cur, "bet", Some(Value::Object(body))).await?;
        let bad = || CasinoError::BadReply(d.to_string().chars().take(200).collect());
        let outcome = d.get("result").and_then(f64_from_json).ok_or_else(bad)?;
        let won = d.get("wonMultiplier").and_then(f64_from_json).unwrap_or(0.0);
        let payout = d.get("wonWalletGrossAmount").and_then(coin_units).unwrap_or_else(|| crate::money::mul_f(req.amount, won));
        self.credit(&cur, payout - req.amount);
        // a bet of 0 pays 0 either way: the roll against the target decides it
        let win = if req.amount == 0 { outcome >= req.target } else { won > 0.0 || outcome >= req.target };
        Ok(BetReply { outcome, amount: Some(req.amount), winnings: Some(payout), win: Some(win), nonce: d.get("nonce").and_then(Value::as_u64) })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        self.call_game(&format!("{UTILS}/seed/rotate"), "limbo", &self.seed_currency(), Some(json!({ "newClientSeed": client_seed }))).await?;
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

    async fn list_slots(&self, _f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _id: &str, _demo: bool, _cur: &str, _fiat: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots on Thrill aren't in FastBet yet".into()))
    }

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let cur = currency.to_ascii_lowercase();
        let mut body = self.stake_fields(&cur, amount)?;
        match game {
            Game::Keno => {
                let picks: Vec<i64> = params["numbers"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                if picks.is_empty() {
                    return Err(CasinoError::Server("Pick 1 to 10 numbers".into()));
                }
                let risk = params["risk"].as_str().unwrap_or("classic").to_ascii_lowercase();
                body.insert("riskProfile".into(), json!(risk_up(&risk)));
                // the board counts 0–39, Thrill 1–40
                body.insert("spots".into(), json!(picks.iter().map(|n| n + 1).collect::<Vec<_>>()));
                body.insert("playoutTimeMilliseconds".into(), json!(0));
                let d = self.play("keno", &cur, "bet", Some(Value::Object(body))).await?;
                let multiplier = d.get("wonMultiplier").and_then(f64_from_json).unwrap_or(0.0);
                let payout = d.get("wonWalletGrossAmount").and_then(coin_units).unwrap_or_else(|| crate::money::mul_f(amount, multiplier));
                self.credit(&cur, payout - amount);
                let drawn: Vec<i64> = d["result"].as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|n| n - 1).collect()).unwrap_or_default();
                Ok(Round {
                    game,
                    id: d.get("roundId").and_then(Value::as_str).unwrap_or_default().to_string(),
                    active: false,
                    currency: cur,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier,
                    state: json!({ "risk": risk, "drawnNumbers": drawn, "selectedNumbers": picks }),
                })
            }
            Game::Mines => {
                let count = params["minesCount"].as_u64().unwrap_or(3).clamp(1, 24);
                body.insert("numberOfMines".into(), json!(count));
                body.insert("edgeSize".into(), json!(EDGE));
                body.insert("insurancePercent".into(), json!(0));
                if auto {
                    // the tiles go up front and the round settles in this one request
                    let picks: Vec<i64> = params["fields"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                    if picks.is_empty() {
                        return Err(CasinoError::Server("Pick the tiles every bet reveals".into()));
                    }
                    body.insert("selectedTiles".into(), json!(picks));
                    let d = self.play("mines", &cur, "multi-select-bet", Some(Value::Object(body))).await?;
                    let play = MinesPlay { round: d.get("roundId").and_then(Value::as_str).unwrap_or_default().to_string(), count: count as u32, picks, amount, currency: cur };
                    return Ok(self.mines_done(&play, &d));
                }
                let d = self.play("mines", &cur, "bet", Some(Value::Object(body))).await?;
                let round = d.get("roundId").and_then(Value::as_str).ok_or_else(|| CasinoError::BadReply(d.to_string().chars().take(200).collect()))?;
                let play = MinesPlay { round: round.to_string(), count: count as u32, picks: vec![], amount, currency: cur };
                self.set_mines(Some(play.clone()));
                Ok(Self::mines_open(&play))
            }
            _ => Err(CasinoError::Server(format!("{} isn't on Thrill in FastBet", game.name()))),
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
                let d = self.play("mines", &play.currency, "reveal", Some(json!({ "tileIndex": tile, "roundId": play.round }))).await?;
                let mut play = play;
                play.picks.push(tile);
                let over = d.get("isConcluded").and_then(Value::as_bool).unwrap_or(false);
                if over {
                    self.set_mines(None);
                    // a mine without the board in the reply: at least the one hit
                    let mut done = d.clone();
                    if !done["tilesStatus"].is_array() && d.get("revealedTile").and_then(Value::as_str).is_some_and(|t| t.eq_ignore_ascii_case("mine")) {
                        let mut board = vec![Value::Null; 25];
                        board[tile.clamp(0, 24) as usize] = json!({ "isGem": false, "revealOrder": play.picks.len() });
                        done["tilesStatus"] = Value::Array(board);
                    }
                    return Ok(self.mines_done(&play, &done));
                }
                self.set_mines(Some(play.clone()));
                Ok(Self::mines_open(&play))
            }
            Step::Cashout => {
                let d = self.play("mines", &play.currency, "cashout", Some(json!({ "roundId": play.round }))).await?;
                self.set_mines(None);
                Ok(self.mines_done(&play, &d))
            }
        }
    }

    /// The Mines round left open, from the game's `init` (in the coin picked on the site).
    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        if game != Game::Mines {
            return Ok(None);
        }
        let cur = self.seed_currency();
        let epoch = self.mines_epoch.load(Ordering::SeqCst);
        let d = self.play("mines", &cur, &format!("init?walletCurrencyIsoCode={}&anchorCurrencyIsoCode=USD", cur.to_ascii_uppercase()), None).await?;
        // a bet or step went out meanwhile: what it knows is newer than this
        if self.mines_epoch.load(Ordering::SeqCst) != epoch {
            return Ok(self.mines.lock().as_ref().map(Self::mines_open));
        }
        let Some(open) = d.get("currentRound").filter(|r| r.is_object()) else {
            *self.mines.lock() = None;
            return Ok(None);
        };
        let (_, picks) = tiles(&open["tilesStatus"]);
        let count = open.get("numberOfMines").or(open.get("mineCount")).or(d.get("mineCount")).and_then(Value::as_u64).unwrap_or(3) as u32;
        let play = MinesPlay {
            round: open.get("roundId").and_then(Value::as_str).unwrap_or_default().to_string(),
            count,
            picks,
            amount: open.get("walletNetStakeAmount").or(open.get("walletStakeNetAmount")).and_then(coin_units).unwrap_or(0),
            currency: cur,
        };
        *self.mines.lock() = Some(play.clone());
        Ok(Some(Self::mines_open(&play)))
    }

    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        Ok(Tables::builtin())
    }

    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok([("limbo", "Limbo"), ("keno", "Keno"), ("mines", "Mines")]
            .iter()
            .map(|(slug, name)| crate::slots::SlotGame {
                id: slug.to_string(),
                name: name.to_string(),
                producer: "thrill".into(),
                image: format!("{ART}/thrill-{slug}_1.webp"),
                demo: false,
                rtp: 99.0,
                provider_game_id: None,
                studio: None,
            })
            .collect())
    }
}

/// Plain HTTPS to the APIs, looking like the site's own requests.
fn client() -> reqwest::Client {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_static(SITE));
    headers.insert(REFERER, HeaderValue::from_static("https://thrill.com/"));
    reqwest::Client::builder().user_agent(USER_AGENT).default_headers(headers).gzip(true).build().unwrap_or_default()
}

/// A random id in the uuid shape, like the site's browser session.
fn uuid() -> String {
    let b: [u8; 16] = rand::random();
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// The saved login: the bare cookie value (a pasted `token=…` works too).
fn clean_token(token: Option<String>) -> Option<String> {
    let t = token?.trim().to_string();
    let t = t.strip_prefix("token=").map(str::to_string).unwrap_or(t);
    let t = t.split(';').next().unwrap_or("").trim().to_string();
    (!t.is_empty()).then_some(t)
}

/// `token=<jwt>; Path=/; …` -> the jwt (an emptied cookie is no login).
fn token_cookie(set_cookie: &str) -> Option<String> {
    let (name, value) = set_cookie.split(';').next()?.split_once('=')?;
    (name.trim() == TOKEN_COOKIE && !value.trim().is_empty()).then(|| value.trim().to_string())
}

/// The site's API replies with the object itself; errors carry a message.
fn parse_api(status: u16, body: &str) -> Result<Value, CallError> {
    let v = serde_json::from_str::<Value>(body).ok();
    if (200..300).contains(&status) {
        return Ok(v.unwrap_or(Value::Null));
    }
    let msg = v
        .as_ref()
        .and_then(|v| {
            ["message", "error_message", "description", "detail"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str))
                .or_else(|| v.pointer("/error/message").and_then(Value::as_str))
                .or_else(|| v.get("error").and_then(Value::as_str))
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("HTTP {status}: {}", body.chars().take(160).collect::<String>()));
    let lower = format!("{msg} {body}").to_ascii_lowercase();
    if status == 401 || (status == 403 && ["jwt", "invalid_user", "unauthor", "session", "token"].iter().any(|k| lower.contains(k))) {
        return Err(CallError::Unauthenticated);
    }
    if status == 429 {
        return Err(CallError::Casino(CasinoError::Busy(msg)));
    }
    Err(CallError::Casino(CasinoError::Server(msg)))
}

/// A game API reply `{data, responseCode, message}` -> the data, or what went wrong.
fn parse_game(status: u16, body: &str) -> Result<Value, GameError> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        if status == 401 || status == 403 {
            return Err(GameError::Session);
        }
        if status == 429 {
            return Err(GameError::Casino(CasinoError::Busy("Too many requests".into())));
        }
        return Err(GameError::Casino(CasinoError::BadReply(format!("HTTP {status}: {}", body.chars().take(160).collect::<String>()))));
    };
    let code = v.get("responseCode").and_then(Value::as_str).unwrap_or("");
    let msg = v.get("message").and_then(Value::as_str).unwrap_or(code).to_string();
    if (200..300).contains(&status) && (code.is_empty() || code.eq_ignore_ascii_case("ok")) {
        return Ok(v.get("data").cloned().unwrap_or(Value::Null));
    }
    let lower = format!("{code} {msg}").to_ascii_lowercase();
    if code == "GameRoundAlreadyInProgress" || lower.contains("already in progress") {
        return Err(GameError::Casino(CasinoError::RoundOpen));
    }
    // a step sent right after the bet ("The request is too fast"), or two calls at once
    // ("RequestStillProcessing: Another request is still processing")
    if lower.contains("too fast") || lower.contains("still processing") || status == 429 {
        return Err(GameError::Casino(CasinoError::Busy(msg)));
    }
    if status == 401 || status == 403 || ["token", "session", "unauthor", "expired"].iter().any(|k| lower.contains(k)) {
        return Err(GameError::Session);
    }
    if lower.contains("insufficient") || lower.contains("balance") && lower.contains("not enough") {
        return Err(GameError::Casino(CasinoError::Server("Insufficient balance".into())));
    }
    Err(GameError::Casino(CasinoError::Server(if msg.is_empty() { format!("Thrill refused the bet (HTTP {status})") } else { msg })))
}

/// One crypto wallet: its coin, balance and (when it holds some) dollars per coin.
struct Wallet {
    currency: String,
    balance: Units,
    price: Option<f64>,
    /// Hidden in the site's wallet list.
    hidden: bool,
}

/// `/v1/wallets` (a list, or `{items}`) -> the crypto wallets.
fn wallets(data: &Value) -> Vec<Wallet> {
    let list = data.as_array().or_else(|| ["items", "wallets", "data"].iter().find_map(|k| data.get(*k).and_then(Value::as_array))).map(Vec::as_slice).unwrap_or_default();
    list.iter()
        .filter_map(|w| {
            let crypto = w.get("cryptoBalance")?;
            let currency = crypto.get("currency").and_then(|c| c.as_str().or_else(|| c.get("cryptoCurrency").and_then(Value::as_str))).or_else(|| w.get("currency").and_then(Value::as_str))?.to_ascii_lowercase();
            // `value` counts the smallest unit (`decimals` of them, 18) as a whole-number string;
            // `balance` is the same in coins ("0.002804571057048899", "1.5E-7")
            let coins = w.get("balance").and_then(f64_from_json).or_else(|| minor(crypto)).unwrap_or(0.0);
            // rounded down: never more than the wallet holds
            let balance = (coins.max(0.0) * 10f64.powi(DECIMALS as i32) + 1e-6).floor() as Units;
            let price = if STABLE.contains(&currency.as_str()) {
                Some(1.0)
            } else {
                match (minor(crypto), w.get("fiatBalance").and_then(minor)) {
                    (Some(c), Some(usd)) if c > 0.0 && usd > 0.0 => Some(usd / c),
                    _ => None,
                }
            };
            let hidden = w.get("visible").and_then(Value::as_bool) == Some(false);
            Some(Wallet { currency, balance, price, hidden })
        })
        .collect()
}

/// `{value: "<smallest units>", decimals}` -> the amount.
fn minor(b: &Value) -> Option<f64> {
    let v = b.get("value").and_then(f64_from_json)?;
    let d = b.get("decimals").and_then(Value::as_u64).unwrap_or(0) as i32;
    Some(v / 10f64.powi(d))
}

/// Mines' `tilesStatus` (one entry per tile: null, or `{isGem, revealOrder}`) -> the mines, and
/// the tiles the player revealed in the order they were.
fn tiles(board: &Value) -> (Vec<i64>, Vec<i64>) {
    let Some(list) = board.as_array() else { return (vec![], vec![]) };
    let mines = list.iter().enumerate().filter(|(_, t)| t.get("isGem").and_then(Value::as_bool) == Some(false)).map(|(i, _)| i as i64).collect();
    let mut revealed: Vec<(i64, i64)> = list.iter().enumerate().filter_map(|(i, t)| Some((t.get("revealOrder").and_then(Value::as_i64)?, i as i64))).collect();
    revealed.sort_unstable();
    (mines, revealed.into_iter().map(|(_, i)| i).collect())
}

/// Our risk names -> Thrill's ("classic" -> "Classic").
fn risk_up(risk: &str) -> &'static str {
    match risk {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        _ => "Classic",
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
    fn logins_and_cookies() {
        assert_eq!(clean_token(Some(" token=abc.def; Path=/ ".into())).as_deref(), Some("abc.def"));
        assert_eq!(clean_token(Some("abc".into())).as_deref(), Some("abc"));
        assert_eq!(clean_token(Some("  ".into())), None);
        assert_eq!(token_cookie("token=eyJ.x.y; Path=/; Domain=.thrill.com; HttpOnly; Secure").as_deref(), Some("eyJ.x.y"));
        assert_eq!(token_cookie("token=; Max-Age=0"), None);
        assert_eq!(token_cookie("thrill-visitor-id=1; Path=/"), None);
        assert_eq!(uuid().len(), 36);
    }

    #[test]
    fn site_replies() {
        assert!(matches!(parse_api(200, r#"{"username":"sx"}"#), Ok(v) if v["username"] == "sx"));
        assert!(matches!(parse_api(403, r#"{"error_type":"invalid_user_error","message":"User with current JWT not valid"}"#), Err(CallError::Unauthenticated)));
        assert!(matches!(parse_api(401, ""), Err(CallError::Unauthenticated)));
        assert!(matches!(parse_api(400, r#"{"message":"Bad currency"}"#), Err(CallError::Casino(CasinoError::Server(m))) if m == "Bad currency"));
        assert!(matches!(parse_api(429, "{}"), Err(CallError::Casino(CasinoError::Busy(_)))));
    }

    #[test]
    fn game_replies() {
        // the live Mines multi-select reply at 0
        let ok = parse_game(200, r#"{"data":{"roundId":"01a1","walletStakeNetAmount":0,"nextWinMultiplier":1.125,"balanceAfterBet":null},"responseCode":"OK","message":"OK"}"#).ok().unwrap();
        assert_eq!(ok["nextWinMultiplier"], 1.125);
        assert!(matches!(parse_game(400, r#"{"data":null,"responseCode":"GameRoundAlreadyInProgress","message":"x"}"#), Err(GameError::Casino(CasinoError::RoundOpen))));
        assert!(matches!(parse_game(400, r#"{"data":null,"responseCode":"Error","message":"The request is too fast. Please try again."}"#), Err(GameError::Casino(e)) if e.is_busy()));
        assert!(matches!(parse_game(401, "Unauthorized"), Err(GameError::Session)));
        assert!(matches!(parse_game(400, r#"{"data":null,"responseCode":"RequestStillProcessing","message":"Another request is still processing. Please try again later."}"#), Err(GameError::Casino(e)) if e.is_busy()));
        assert!(matches!(parse_game(400, r#"{"data":null,"responseCode":"InsufficientBalance","message":"Insufficient balance"}"#), Err(GameError::Casino(e)) if e.is_balance()));
    }

    #[test]
    fn wallets_and_boards() {
        // trimmed from the live reply (2026-10-18)
        let data = json!({"items": [
            {"balance": "0.002804571057048899", "cryptoBalance": {"currency": {"cryptoCurrency": "USDT"}, "decimals": 18, "value": "2804571057048899"}, "currency": "USDT", "fiatBalance": {"currency": {"fiatCurrency": "USD"}, "decimals": 18, "value": "2804571057048899"}, "visible": true},
            {"balance": "0.000079255209429999", "cryptoBalance": {"currency": {"cryptoCurrency": "LINK"}, "decimals": 18, "value": "79255209429999"}, "currency": "LINK", "fiatBalance": {"currency": {"fiatCurrency": "USD"}, "decimals": 18, "value": "931703753490920"}, "visible": true},
            {"balance": "1.51308082725E-7", "cryptoBalance": {"currency": {"cryptoCurrency": "ETH"}, "decimals": 18, "value": "151308082725"}, "currency": "ETH", "fiatBalance": {"currency": {"fiatCurrency": "USD"}, "decimals": 18, "value": "357118954049317"}, "visible": true},
            {"balance": "0E-18", "cryptoBalance": {"currency": {"cryptoCurrency": "EOS"}, "decimals": 18, "value": "0"}, "currency": "EOS", "fiatBalance": {"currency": {"fiatCurrency": "USD"}, "decimals": 18, "value": "0"}, "visible": false}
        ], "totalAmount": {"value": "1"}});
        let w = wallets(&data);
        assert_eq!(w.len(), 4);
        assert_eq!((w[0].currency.as_str(), w[0].balance, w[0].price), ("usdt", 280_457, Some(1.0)));
        assert_eq!((w[1].currency.as_str(), w[1].balance), ("link", 7_925));
        assert!((w[1].price.unwrap() - 11.7557).abs() < 0.001, "{:?}", w[1].price);
        assert_eq!((w[2].currency.as_str(), w[2].balance), ("eth", 15));
        assert!((w[2].price.unwrap() - 2360.2).abs() < 1.0, "{:?}", w[2].price);
        assert!(w[3].price.is_none() && w[3].hidden && w[3].balance == 0);
        // a lost board: the mine hit was the second reveal
        let mut board = vec![Value::Null; 25];
        board[4] = json!({"isGem": true, "revealOrder": 1});
        board[9] = json!({"isGem": false, "revealOrder": 2});
        board[20] = json!({"isGem": false, "revealOrder": null});
        let (mines, picks) = tiles(&Value::Array(board));
        assert_eq!((mines, picks), (vec![9, 20], vec![4, 9]));
        assert_eq!(risk_up("medium"), "Medium");
    }
}
