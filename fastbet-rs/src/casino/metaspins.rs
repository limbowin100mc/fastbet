//! Metaspins (metaspins.com). The site talks to its backend over Azure Web PubSub: `GET
//! https://services.metaspins.com/api/negotiate` (with the login cookie) hands out a websocket URL,
//! events go out as `{type:"event", event, data:{correlationId, message}}` and come back as
//! `{type:"message", from:"server"|"group", data:{eventName, success, message, correlationId}}`.
//! A guest gets a `connected` event saying `InvalidSession`; a login gets `user-sync` (the user,
//! with the active coin). `get-accounts` lists the wallets, `game-launch {currency, gameIdentifier}`
//! opens a game paid from that coin's wallet.
//!
//! The originals are Origami games (`origami/Limbo`). The launch URL carries the game's settings
//! (base64 JSON: Origami environment, currency, bet limits) and an Origami token; the game then
//! talks to Origami's server (`https://eu-api.betorigami.com/agg01/provider/v1` for Metaspins) over
//! plain HTTPS with it as `Authorization: Bearer …`: `POST /games/limbo/play {amount,
//! targetMultiplier}` (the target × 100), `GET /users/balance`, `GET /provably-fair/my-info`,
//! `PATCH /provably-fair/client-seed {clientSeed}`. Origami plays in dollars whatever coin pays
//! (`launchCurrency: "USD"`), so the app bets and shows the game's own dollar balance: the money in
//! the coin that is active on the site.
//!
//! Metaspins has no API keys. Its login is the HttpOnly cookie `usersession` on metaspins.com,
//! which the login window reads from its browser once the site says it is logged in (guests get
//! one too). Every negotiate moves its expiry a week on.
//!
//! `FASTBET_METASPINS_DUMP=1` prints the socket's frames and the launch settings to stderr.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderValue, SEC_WEBSOCKET_PROTOCOL};
use tokio_tungstenite::tungstenite::Message;

use super::creds::base64url;
use super::*;
use crate::money::{f64_from_json, format_units, parse_units};

const SERVICES: &str = "https://services.metaspins.com";
const SITE: &str = "https://metaspins.com";
pub const SESSION_COOKIE: &str = "usersession";
const LIMBO: &str = "origami/Limbo";
/// Azure Web PubSub's JSON protocol, the one the site's client speaks.
const SUBPROTOCOL: &str = "json.webpubsub.azure.v1";
/// Amounts travel as decimal strings; 8 decimals cover every coin's bet sizes.
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the server gets to say whether the cookie is a login.
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// Balances are re-read this often while idle; bets update them locally.
const REFRESH: Duration = Duration::from_secs(10);
/// The app says what it is: Cloudflare in front of metaspins.com challenges a Chrome user agent
/// that doesn't come with Chrome's TLS handshake, but lets an honest client through.
const USER_AGENT: &str = concat!("FastBet/", env!("CARGO_PKG_VERSION"));

const EXPIRED: &str = "Your Metaspins login has expired. Log in again in Settings.";

enum CallError {
    /// Metaspins doesn't see a login.
    LoggedOut,
    /// Origami no longer knows the game's token.
    SessionGone,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::LoggedOut => CasinoError::Server(EXPIRED.into()),
            CallError::SessionGone => CasinoError::Server("The game session ended; it is opened again on the next bet".into()),
            CallError::Casino(e) => e,
        }
    }
}

/// Why serving a connection stopped.
enum Stop {
    /// A new login, or the app is closing: connect again (or not).
    Wake,
    LoggedOut,
    Lost,
}

/// An Origami game session for one of the account's coins.
#[derive(Clone, Debug)]
struct Game {
    token: String,
    base: String,
    /// The currency the game plays in, as Origami writes it ("USD", "BTC").
    currency: String,
    min_bet: Option<String>,
}

/// A server event answering one of ours (or pushed).
struct Reply {
    success: bool,
    message: Value,
}

/// A wallet from `get-accounts`.
#[derive(Clone, Debug, PartialEq)]
struct Account {
    /// As the site writes it ("BTC", "USDT").
    code: String,
    active: bool,
}

pub struct Metaspins {
    token: Mutex<Option<String>>,
    client: reqwest::Client,
    dump: bool,
    out: Mutex<Option<mpsc::UnboundedSender<Message>>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Reply>>>,
    /// The server's word on the login after each connect: `user-sync` (yes) or `connected` (no).
    hello: Mutex<Option<oneshot::Sender<Reply>>>,
    seq: AtomicU64,
    /// The coin active on the site, which pays for the game ("USDT").
    coin: Mutex<Option<String>>,
    /// Origami sessions by paying coin.
    games: Mutex<HashMap<String, Game>>,
    /// One game launch at a time; bets that need the same session wait for it.
    launching: tokio::sync::Mutex<()>,
    /// `X-Origami-Trace`: browser id, window id and a counter, like the game's own requests.
    trace: (String, String),
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

impl Metaspins {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        Self::start(setup, events)
    }

    fn start(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<Self> {
        let me = Arc::new(Self {
            token: Mutex::new(setup.token.map(|t| clean_token(&t)).filter(|t| !t.is_empty())),
            client: client(),
            dump: std::env::var("FASTBET_METASPINS_DUMP").is_ok_and(|v| !v.is_empty() && v != "0"),
            out: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            hello: Mutex::new(None),
            seq: AtomicU64::new(0),
            coin: Mutex::new(None),
            games: Mutex::new(HashMap::new()),
            launching: tokio::sync::Mutex::new(()),
            trace: (uuid(), uuid()),
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
        tracing::info!("metaspins: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            let token = self.token.lock().clone();
            let Some(token) = token else {
                self.status(ConnStatus::NoToken, "");
                self.wake.notified().await;
                continue;
            };
            let stop = match self.connect(&token).await {
                Ok(mut socket) => {
                    retries = 0;
                    let stop = self.serve(&mut socket).await;
                    socket.abort();
                    stop
                }
                Err(CallError::LoggedOut) => Stop::LoggedOut,
                Err(e) => {
                    if self.alive() {
                        self.status(ConnStatus::Offline, CasinoError::from(e).to_string());
                    }
                    Stop::Lost
                }
            };
            self.ready.store(false, Ordering::SeqCst);
            self.disconnect();
            match stop {
                Stop::Wake => {}
                Stop::LoggedOut => {
                    self.status(ConnStatus::AuthFailed, EXPIRED);
                    self.wake.notified().await;
                }
                Stop::Lost => {
                    if self.alive() {
                        self.status(ConnStatus::Offline, "");
                    }
                    tokio::select! {
                        _ = tokio::time::sleep(backoff(&mut retries)) => {}
                        _ = self.wake.notified() => {}
                    }
                }
            }
        }
        self.disconnect();
    }

    /// Drops the socket and fails whatever still waits on it.
    fn disconnect(&self) {
        *self.out.lock() = None;
        self.hello.lock().take();
        let pending: Vec<_> = self.pending.lock().drain().collect();
        drop(pending);
    }

    /// Opens the site's socket with the login cookie, checks the login, finds the active coin and
    /// opens its Limbo session. Answers the socket's task.
    async fn connect(self: &Arc<Self>, token: &str) -> Result<tokio::task::JoinHandle<()>, CallError> {
        self.games.lock().clear();
        self.status(ConnStatus::Connecting, "Opening metaspins.com");
        let url = self.negotiate(token).await?;
        let mut req = url.as_str().into_client_request().map_err(|e| CallError::Casino(CasinoError::BadReply(e.to_string())))?;
        req.headers_mut().insert(SEC_WEBSOCKET_PROTOCOL, HeaderValue::from_static(SUBPROTOCOL));
        let ws = match tokio::time::timeout(RPC_TIMEOUT, tokio_tungstenite::connect_async(req)).await {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                tracing::warn!("metaspins: socket: {e}");
                return Err(CallError::Casino(CasinoError::ConnectionLost));
            }
            Err(_) => return Err(CallError::Casino(CasinoError::Timeout)),
        };
        let (mut sink, mut stream) = ws.split();
        let (otx, mut orx) = mpsc::unbounded_channel::<Message>();
        *self.out.lock() = Some(otx);
        let (htx, hrx) = oneshot::channel();
        *self.hello.lock() = Some(htx);
        let me = self.clone();
        let socket = tokio::spawn(async move {
            let writer = async {
                while let Some(m) = orx.recv().await {
                    if sink.send(m).await.is_err() {
                        break;
                    }
                }
                let _ = sink.close().await;
            };
            let reader = async {
                while let Some(msg) = stream.next().await {
                    match msg {
                        Ok(Message::Text(t)) => me.on_frame(&t),
                        Ok(Message::Binary(b)) => {
                            if let Ok(t) = std::str::from_utf8(&b) {
                                me.on_frame(t)
                            }
                        }
                        Ok(Message::Close(c)) => {
                            tracing::info!("metaspins: socket closed {c:?}");
                            break;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::info!("metaspins: socket error: {e}");
                            break;
                        }
                    }
                }
            };
            tokio::select! {
                _ = writer => {}
                _ = reader => {}
            }
        });

        let fail = |socket: &tokio::task::JoinHandle<()>, e: CallError| {
            socket.abort();
            e
        };
        self.status(ConnStatus::Authenticating, "");
        match tokio::time::timeout(HELLO_TIMEOUT, hrx).await {
            Ok(Ok(r)) if !r.success => {
                let e = if session_error(&r.message) { CallError::LoggedOut } else { CallError::Casino(CasinoError::Server(text_of(&r.message, "Metaspins refused the connection"))) };
                return Err(fail(&socket, e));
            }
            Ok(Ok(r)) => self.on_user(&r.message),
            // no word: the wallets tell
            _ => {}
        }
        let opened = async {
            self.refresh_coin().await?;
            self.refresh_balance(true).await
        };
        if let Err(e) = opened.await {
            return Err(fail(&socket, e));
        }
        self.status(ConnStatus::Ready, "");
        let me = self.clone();
        tokio::spawn(async move {
            if let Ok(st) = me.seeds().await {
                let _ = me.events.send(CasinoEvent::Provable(st));
            }
        });
        Ok(socket)
    }

    /// Keeps the active coin and the balance fresh until something needs a new connection.
    async fn serve(&self, socket: &mut tokio::task::JoinHandle<()>) -> Stop {
        loop {
            tokio::select! {
                _ = self.wake.notified() => return Stop::Wake,
                _ = &mut *socket => return Stop::Lost,
                _ = tokio::time::sleep(REFRESH) => {}
            }
            if !self.alive() {
                return Stop::Wake;
            }
            let res = match self.refresh_coin().await {
                Ok(()) => self.refresh_balance(false).await,
                e => e,
            };
            match res {
                Ok(()) | Err(CallError::SessionGone) => {}
                Err(CallError::LoggedOut) => return Stop::LoggedOut,
                Err(CallError::Casino(CasinoError::NotConnected)) => return Stop::Lost,
                Err(CallError::Casino(e)) => tracing::info!("metaspins: balance: {e}"),
            }
        }
    }

    /// `GET /api/session`, then `GET /api/negotiate`, the way the site connects: the session check
    /// hands out a short-lived `kycvalidation` cookie (a minute) that negotiate wants next to the
    /// login cookie. Answers the socket's URL; a renewed login cookie is saved.
    async fn negotiate(&self, token: &str) -> Result<String, CallError> {
        let mut jar = vec![(SESSION_COOKIE.to_string(), token.to_string())];
        let (status, set, _) = self.services(&format!("{SERVICES}/api/session"), &jar).await?;
        tracing::info!("metaspins: session check HTTP {status}, cookies {:?}", set.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>());
        merge_cookies(&mut jar, set);
        let (status, set, text) = self.services(&format!("{SERVICES}/api/negotiate"), &jar).await?;
        merge_cookies(&mut jar, set);
        // a cookie the server doesn't know (gone, or never a session) gets an empty 400
        if matches!(status, 400 | 401) {
            tracing::info!("metaspins: negotiate HTTP {status} {}", text.chars().take(160).collect::<String>());
            return Err(CallError::LoggedOut);
        }
        let url = serde_json::from_str::<Value>(&text).ok().and_then(|v| v.get("url").and_then(Value::as_str).map(str::to_string));
        let url = url.ok_or_else(|| CallError::Casino(CasinoError::BadReply(format!("HTTP {status}: {}", text.chars().take(160).collect::<String>()))))?;
        let fresh = jar.iter().find(|(k, _)| k == SESSION_COOKIE).map(|(_, v)| v.clone()).filter(|v| v != token);
        if let Some(fresh) = fresh {
            *self.token.lock() = Some(fresh.clone());
            tracing::info!("metaspins: login renewed");
            let _ = self.events.send(CasinoEvent::Token(fresh));
        }
        Ok(url)
    }

    /// A GET on one of the site's services with cookies -> status, the cookies it sets, body.
    async fn services(&self, url: &str, jar: &[(String, String)]) -> Result<(u16, Vec<(String, String)>, String), CallError> {
        let cookie = jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
        let res = self.client.get(url).header(reqwest::header::COOKIE, cookie).timeout(RPC_TIMEOUT).send().await.map_err(lost)?;
        let status = res.status().as_u16();
        let set = res.headers().get_all(reqwest::header::SET_COOKIE).iter().filter_map(|h| h.to_str().ok()).filter_map(set_cookie).collect();
        let text = res.text().await.map_err(lost)?;
        if is_challenge(status, &text) {
            return Err(CallError::Casino(CasinoError::Server("Cloudflare turned the app away from metaspins.com".into())));
        }
        if status == 503 {
            return Err(CallError::Casino(CasinoError::Server("Metaspins is down for maintenance".into())));
        }
        Ok((status, set, text))
    }

    /// Sends event `name` and waits for the server's answer to it.
    async fn request(&self, name: &str, message: Option<Value>) -> Result<Value, CallError> {
        let out = self.out.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let id = format!("fb{n:x}{:08x}", rand::random::<u32>());
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.clone(), tx);
        let mut data = json!({ "correlationId": id });
        if let Some(m) = message {
            data["message"] = m;
        }
        let frame = json!({ "type": "event", "event": name, "dataType": "json", "data": data, "ackId": n });
        if self.dump {
            eprintln!("metaspins -> {frame}");
        }
        if out.send(Message::Text(frame.to_string().into())).is_err() {
            self.pending.lock().remove(&id);
            return Err(CallError::Casino(CasinoError::NotConnected));
        }
        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(r)) if r.success => Ok(r.message),
            Ok(Ok(r)) if session_error(&r.message) => Err(CallError::LoggedOut),
            Ok(Ok(r)) => Err(CallError::Casino(CasinoError::Server(text_of(&r.message, &format!("Metaspins refused {name}"))))),
            Ok(Err(_)) => Err(CallError::Casino(CasinoError::ConnectionLost)),
            Err(_) => {
                self.pending.lock().remove(&id);
                Err(CallError::Casino(CasinoError::Timeout))
            }
        }
    }

    fn on_frame(&self, raw: &str) {
        if self.dump {
            eprintln!("metaspins <- {}", raw.chars().take(2000).collect::<String>());
        }
        let Ok(v) = serde_json::from_str::<Value>(raw) else { return };
        match v.get("type").and_then(Value::as_str) {
            Some("message") => {
                let d = &v["data"];
                let reply = Reply { success: d.get("success").and_then(Value::as_bool).unwrap_or(false), message: d.get("message").cloned().unwrap_or(Value::Null) };
                if let Some(id) = d.get("correlationId").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    if let Some(tx) = self.pending.lock().remove(id) {
                        let _ = tx.send(reply);
                        return;
                    }
                }
                match d.get("eventName").and_then(Value::as_str).unwrap_or("") {
                    "connected" => {
                        if let Some(tx) = self.hello.lock().take() {
                            let _ = tx.send(reply);
                        }
                    }
                    "user-sync" if reply.success => {
                        self.on_user(&reply.message);
                        if let Some(c) = reply.message.get("currency").and_then(Value::as_str).filter(|c| !c.is_empty()) {
                            self.set_coin(c);
                        }
                        if reply.message.get("userid").is_some_and(|u| !u.is_null()) {
                            if let Some(tx) = self.hello.lock().take() {
                                let _ = tx.send(reply);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Some("system") if v.get("event").and_then(Value::as_str) == Some("disconnected") => {
                let why = v.get("message").map(|m| m.to_string()).unwrap_or_default();
                tracing::info!("metaspins: the server dropped the socket: {why}");
            }
            Some("ack") if v.get("success").and_then(Value::as_bool) == Some(false) => {
                let why = v.get("error").map(|m| m.to_string()).unwrap_or_default();
                tracing::info!("metaspins: event refused: {why}");
            }
            _ => {}
        }
    }

    /// The account's name from `connected` / `user-sync` / `get-account-details`.
    fn on_user(&self, m: &Value) {
        let name = ["username", "userName", "nickname", "displayName"].iter().find_map(|k| m.get(k).and_then(Value::as_str)).map(str::trim).filter(|s| !s.is_empty());
        if let Some(name) = name {
            let changed = {
                let mut mk = self.market.lock();
                let changed = mk.username.as_deref() != Some(name);
                mk.username = Some(name.to_string());
                changed
            };
            if changed {
                self.publish();
            }
        }
    }

    /// The coin that pays, as the site switches it; the game of another coin is opened on next use.
    fn set_coin(&self, code: &str) {
        let mut coin = self.coin.lock();
        if coin.as_deref() != Some(code) {
            tracing::info!("metaspins: playing with {code}");
            *coin = Some(code.to_string());
        }
    }

    fn coin(&self) -> Result<String, CallError> {
        self.coin.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))
    }

    /// The active coin from `get-accounts` (which also tells whether the login still holds).
    async fn refresh_coin(&self) -> Result<(), CallError> {
        let v = self.request("get-accounts", None).await?;
        let accounts = accounts_of(&v);
        let active = accounts.iter().find(|a| a.active).or(accounts.first());
        let active = active.ok_or_else(|| CallError::Casino(CasinoError::BadReply(format!("no wallets: {}", v.to_string().chars().take(200).collect::<String>()))))?;
        self.set_coin(&active.code);
        Ok(())
    }

    /// Re-reads the game's balance (the active coin's money, in the game's currency). It is only
    /// taken when no bet ran meanwhile, so a reply older than the latest local update never wins.
    async fn refresh_balance(&self, force: bool) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let coin = self.coin()?;
        let game = self.game(&coin).await?;
        let path = format!("/users/balance?currency={}&origamiGame=LIMBO", urlencode(&game.currency));
        let v = self.in_game(&coin, reqwest::Method::GET, &path, None).await?;
        let balance = v.get("balance").and_then(amount_units).ok_or_else(|| CallError::Casino(CasinoError::BadReply(v.to_string().chars().take(200).collect())))?;
        if force || (idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled) {
            self.book(&game.currency, balance);
        }
        Ok(())
    }

    /// The game's currency becomes the market's only one, holding `balance`.
    fn book(&self, currency: &str, balance: Units) {
        let key = currency.to_ascii_lowercase();
        {
            let mut m = self.market.lock();
            m.currencies.retain(|k, _| *k == key);
            m.wallets.retain(|k, _| *k == key);
            let price = if key == "usd" { 1.0 } else { 0.0 };
            m.currencies.entry(key.clone()).or_insert(CurrencyInfo { decimals: DECIMALS, price, hidden: false, fiat: false });
            m.wallets.insert(key.clone(), balance);
            m.site_currency = Some(key);
        }
        self.publish();
    }

    /// The Origami session of coin `cur`, opened through the site when there is none yet.
    async fn game(&self, cur: &str) -> Result<Game, CallError> {
        if let Some(g) = self.games.lock().get(cur) {
            return Ok(g.clone());
        }
        let _one = self.launching.lock().await;
        if let Some(g) = self.games.lock().get(cur) {
            return Ok(g.clone());
        }
        let g = self.launch(cur).await?;
        self.games.lock().insert(cur.to_string(), g.clone());
        Ok(g)
    }

    /// Drops a session Origami no longer knows and opens a new one (unless another call already did).
    async fn reopen(&self, cur: &str, stale: &str) -> Result<Game, CallError> {
        {
            let mut g = self.games.lock();
            if g.get(cur).is_some_and(|x| x.token == stale) {
                g.remove(cur);
            }
        }
        self.game(cur).await
    }

    /// `game-launch` on the site, paid from coin `code` -> the game's Origami token and settings.
    async fn launch(&self, code: &str) -> Result<Game, CallError> {
        let v = self.request("game-launch", Some(json!({ "currency": code, "gameIdentifier": LIMBO }))).await?;
        let bad = || CallError::Casino(CasinoError::BadReply(v.to_string().chars().take(200).collect()));
        // a demo launch: the site played it as a guest
        if v.get("launchCurrency").and_then(Value::as_str).is_some_and(|c| c.eq_ignore_ascii_case("FUN")) {
            return Err(CallError::LoggedOut);
        }
        let url = v.get("gameurl").and_then(Value::as_str).ok_or_else(bad)?;
        let launch = parse_launch(url).ok_or_else(bad)?;
        if self.dump {
            eprintln!("metaspins launch: settings {} token {} session {}", launch.settings, launch.auth_token.is_some(), launch.session.is_some());
        }
        let env = launch.settings.get("environment").and_then(Value::as_str).unwrap_or("");
        let base = origami_base(env).ok_or_else(|| CallError::Casino(CasinoError::Server(format!("Metaspins opened Limbo on an Origami server FastBet doesn't know ({env})"))))?;
        let (token, settings) = match (launch.auth_token, launch.session) {
            (Some(t), _) => (t, launch.settings),
            (None, Some(s)) => {
                let v = self.origami(reqwest::Method::POST, base, &format!("/auth/session/{}", urlencode(&s)), None, None).await?;
                let t = v.get("authToken").and_then(Value::as_str).filter(|t| !t.is_empty()).ok_or_else(|| CallError::Casino(CasinoError::BadReply(v.to_string().chars().take(200).collect())))?;
                (t.to_string(), v.get("settings").filter(|s| s.is_object()).cloned().unwrap_or(launch.settings))
            }
            (None, None) => return Err(CallError::LoggedOut),
        };
        // the launch says what the game plays in ("USD"); the settings may too
        let currency = settings.get("currency").and_then(Value::as_str).or_else(|| v.get("launchCurrency").and_then(Value::as_str)).unwrap_or(code).to_string();
        let min_bet = settings.get("minBet").and_then(|m| m.as_str().map(str::to_string).or_else(|| f64_from_json(m).map(|f| fmt_amount(f))));
        tracing::info!("metaspins: Limbo session opened for {code} (plays in {currency})");
        Ok(Game { token, base: base.to_string(), currency, min_bet })
    }

    /// One request to Origami with a game's token.
    async fn origami(&self, method: reqwest::Method, base: &str, path: &str, body: Option<Value>, game: Option<&Game>) -> Result<Value, CallError> {
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let mut req = self.client.request(method, format!("{base}{path}")).timeout(RPC_TIMEOUT).header("X-Origami-Trace", format!("{}-{}-{ms}-{n}", self.trace.0, self.trace.1));
        if let Some(g) = game {
            req = req.bearer_auth(&g.token);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.map_err(lost)?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(lost)?;
        parse_origami(status, &text, game.and_then(|g| g.min_bet.as_deref()), game.map(|g| g.currency.as_str()))
    }

    /// A request in coin `cur`'s game session; a session Origami dropped is opened again once.
    async fn in_game(&self, cur: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, CallError> {
        let game = self.game(cur).await?;
        match self.origami(method.clone(), &game.base, path, body.clone(), Some(&game)).await {
            Err(CallError::SessionGone) => {
                let fresh = self.reopen(cur, &game.token).await?;
                self.origami(method, &fresh.base, path, body, Some(&fresh)).await
            }
            res => res,
        }
    }

    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let coin = self.coin()?;
        let v = self.in_game(&coin, reqwest::Method::GET, "/provably-fair/my-info", None).await?;
        Ok(ProvableState::from_json(&v))
    }

    fn publish(&self) {
        let m = self.market.lock().clone();
        let mut last = self.published.lock();
        if *last != m {
            *last = m.clone();
            let _ = self.events.send(CasinoEvent::Market(m));
        }
    }
}

#[async_trait]
impl Casino for Metaspins {
    fn id(&self) -> CasinoId {
        CasinoId::Metaspins
    }

    fn set_token(&self, token: Option<String>) {
        let token = token.map(|t| clean_token(&t)).filter(|t| !t.is_empty());
        {
            let mut cur = self.token.lock();
            // the same login while connected: nothing to redo
            if *cur == token && token.is_some() && self.ready.load(Ordering::SeqCst) {
                let _ = self.events.send(CasinoEvent::Status(ConnStatus::Ready, String::new()));
                return;
            }
            *cur = token;
        }
        self.wake.notify_one();
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.ready.store(false, Ordering::SeqCst);
        self.wake.notify_one();
        self.disconnect();
    }

    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        if req.amount <= 0 {
            return Err(CasinoError::Server("Metaspins doesn't take bets of 0".into()));
        }
        let _inflight = InFlight::new(&self.inflight);
        let coin = self.coin()?;
        let game = self.game(&coin).await?;
        if !req.currency.eq_ignore_ascii_case(&game.currency) {
            return Err(CasinoError::Server(format!("Metaspins' Limbo plays in {}: pick it in the wallet", game.currency)));
        }
        let body = json!({ "amount": format_units(req.amount, DECIMALS, DECIMALS), "targetMultiplier": target_hundredths(req.target) });
        let v = self.in_game(&coin, reqwest::Method::POST, "/games/limbo/play", Some(body)).await?;
        let bet = limbo_result(&v).ok_or_else(|| CasinoError::BadReply(v.to_string().chars().take(200).collect()))?;
        if let Some(after) = bet.after {
            self.book(&game.currency, after);
        }
        self.settled.fetch_add(1, Ordering::SeqCst);
        Ok(BetReply { outcome: bet.outcome, amount: Some(bet.amount.unwrap_or(req.amount)), winnings: Some(bet.payout), win: Some(bet.payout > 0), nonce: None })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let coin = self.coin()?;
        let v = self.in_game(&coin, reqwest::Method::PATCH, "/provably-fair/client-seed", Some(json!({ "clientSeed": client_seed }))).await?;
        let mut st = match self.seeds().await {
            Ok(st) => st,
            Err(_) => ProvableState::from_json(&v),
        };
        st.client_seed.get_or_insert_with(|| client_seed.to_string());
        Ok(st)
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        self.seeds().await
    }

    async fn list_slots(&self, _filter: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _catalog_id: &str, _demo: bool, _currency: &str, _fiat: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots aren't available on Metaspins yet".into()))
    }
}

/// `fastbet --metaspins-test`: connects the way the app does with the login cookie in
/// `FASTBET_METASPINS_TOKEN` and prints the account, the paying coin, the Limbo session, the
/// balance and the seeds.
/// It never bets.
pub async fn self_test() {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = std::env::var("FASTBET_METASPINS_TOKEN").ok().filter(|t| !t.is_empty());
    let casino = Metaspins::start(Setup { token, ..Default::default() }, tx);
    let t0 = Instant::now();
    let base = origami_base("SOFTSWISS_EU_PROD").unwrap_or_default();
    match casino.origami(reqwest::Method::GET, base, "/provably-fair/public-key", None, None).await {
        Ok(v) => println!("origami reachable: {v}"),
        Err(e) => println!("origami: {}", CasinoError::from(e)),
    }
    let show = |e: CasinoEvent| match e {
        CasinoEvent::Status(s, d) => println!("status: {} {d}", s.short()),
        CasinoEvent::Market(m) => {
            let wallets: Vec<String> = m.wallets.iter().map(|(c, v)| format!("{c} {} (${:.2})", format_units(*v, DECIMALS, DECIMALS), crate::money::to_f64(*v, DECIMALS) * m.info(c).map(|i| i.price).unwrap_or(0.0))).collect();
            println!("account: {:?}, wallet {:?}: {}", m.username, m.site_currency, wallets.join(", "));
        }
        CasinoEvent::Provable(p) => println!("seeds: client {:?}, server hash {:?}, nonce {:?}", p.client_seed, p.server_seed_hash, p.nonce),
        CasinoEvent::Token(t) => println!("login renewed ({} characters)", t.len()),
    };
    loop {
        match tokio::time::timeout(Duration::from_secs(60), rx.recv()).await {
            Ok(Some(CasinoEvent::Status(s, d))) => {
                println!("status: {} {d} ({} ms)", s.short(), t0.elapsed().as_millis());
                if s.is_ready() {
                    break;
                }
                if matches!(s, ConnStatus::AuthFailed | ConnStatus::NoToken) {
                    casino.shutdown();
                    return;
                }
            }
            Ok(Some(e)) => show(e),
            _ => {
                println!("no answer");
                casino.shutdown();
                return;
            }
        }
    }
    let coin = casino.coin.lock().clone().unwrap_or_default();
    match casino.game(&coin).await {
        Ok(g) => println!("Limbo session paid in {coin}: plays in {}, min bet {:?}, server {}", g.currency, g.min_bet, g.base),
        Err(e) => println!("Limbo session paid in {coin}: {}", CasinoError::from(e)),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    while let Ok(e) = rx.try_recv() {
        show(e);
    }
    casino.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// `Set-Cookie: name=value; …` -> (name, value).
fn set_cookie(h: &str) -> Option<(String, String)> {
    let (k, v) = h.split(';').next()?.split_once('=')?;
    Some((k.trim().to_string(), v.trim().to_string())).filter(|(k, _)| !k.is_empty())
}

/// Takes the cookies a reply set into the jar; an emptied one is dropped.
fn merge_cookies(jar: &mut Vec<(String, String)>, set: Vec<(String, String)>) {
    for (k, v) in set {
        jar.retain(|(n, _)| *n != k);
        if !v.is_empty() {
            jar.push((k, v));
        }
    }
}

/// The cookie's value, also when pasted as `usersession=…`.
fn clean_token(t: &str) -> String {
    let t = t.trim();
    t.strip_prefix(&format!("{SESSION_COOKIE}=")).unwrap_or(t).split(';').next().unwrap_or("").trim().to_string()
}

fn lost(e: reqwest::Error) -> CallError {
    CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost })
}

fn is_challenge(status: u16, body: &str) -> bool {
    let head: String = body.chars().take(4000).collect();
    ["Just a moment", "challenge-platform", "Attention Required", "cf-chl"].iter().any(|k| head.contains(k)) || (status == 403 && !head.trim_start().starts_with('{'))
}

/// A refusal that means the cookie is no login (`InvalidSession`, a session-expired code).
fn session_error(m: &Value) -> bool {
    let text = match m {
        Value::String(s) => s.clone(),
        _ => m.get("codes").map(|c| c.to_string()).unwrap_or_default(),
    };
    let low = text.to_ascii_lowercase();
    low.contains("session") || low.contains("unauthor") || low.contains("not logged")
}

/// A server message's text: a plain string, or the first of its `codes`.
fn text_of(m: &Value, fallback: &str) -> String {
    match m {
        Value::String(s) if !s.is_empty() => s.clone(),
        _ => m
            .get("codes")
            .and_then(Value::as_array)
            .and_then(|c| c.first())
            .and_then(Value::as_str)
            .or_else(|| m.get("message").and_then(Value::as_str))
            .unwrap_or(fallback)
            .to_string(),
    }
}

/// `get-accounts` (a list, or an object holding one) -> the wallets.
fn accounts_of(v: &Value) -> Vec<Account> {
    let list: Vec<&Value> = match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => ["accounts", "items", "wallets", "data"].iter().find_map(|k| o.get(*k).and_then(Value::as_array)).map(|a| a.iter().collect()).unwrap_or_default(),
        _ => vec![],
    };
    list.into_iter()
        .filter_map(|a| {
            let code = a.get("currency").and_then(Value::as_str)?.trim().to_string();
            Some(Account { code, active: a.get("isActive").and_then(Value::as_bool).unwrap_or(false) }).filter(|a| !a.code.is_empty())
        })
        .collect()
}

/// What a launch URL carries.
#[derive(Debug, Default, PartialEq)]
struct Launch {
    settings: Value,
    auth_token: Option<String>,
    session: Option<String>,
}

/// The site's launch URL -> the game's settings and token. Metaspins wraps the game's own URL
/// in a launcher page whose `options` parameter is base64 JSON `{launch_options: {game_url}}`.
fn parse_launch(url: &str) -> Option<Launch> {
    let u = reqwest::Url::parse(url).ok()?;
    let q = |k: &str| u.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned()).filter(|v| !v.is_empty());
    let b64 = |s: String| -> Option<Value> { serde_json::from_slice(&base64url(&s.replace(' ', "+"))?).ok() };
    if let Some(opts) = q("options") {
        let inner = b64(opts)?.pointer("/launch_options/game_url")?.as_str()?.to_string();
        return (!inner.contains("options=")).then(|| parse_launch(&inner)).flatten();
    }
    Some(Launch { settings: q("settings").and_then(b64).unwrap_or(Value::Null), auth_token: q("authToken"), session: q("session") })
}

/// Origami's server for an environment named in the game's settings.
fn origami_base(env: &str) -> Option<&'static str> {
    Some(match env {
        "SOFTSWISS_EU_PROD" => "https://eu-api.betorigami.com/agg01/provider/v1",
        "GR8_EU_PROD" => "https://eu-api.betorigami.com/agg02/provider/v1",
        "HUB88_EU_PROD" => "https://eu-api.betorigami.com/hub88/provider/v1",
        "ST8_EU_PROD" => "https://eu-api.betorigami.com/st8/provider/v1",
        "SOFTSWISS_AS_PROD" => "https://as-api.betorigami.com/agg01/provider/v1",
        "GR8_AS_PROD" => "https://as-api.betorigami.com/agg02/provider/v1",
        "HUB88_AS_PROD" => "https://as-api.betorigami.com/hub88/provider/v1",
        "ST8_AS_PROD" => "https://as-api.betorigami.com/st8/provider/v1",
        _ => return None,
    })
}

/// Origami's reply -> the JSON, or its error (`{status, errorCode, message}`).
fn parse_origami(status: u16, body: &str, min_bet: Option<&str>, currency: Option<&str>) -> Result<Value, CallError> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        if status == 401 {
            return Err(CallError::SessionGone);
        }
        if status == 429 {
            return Err(CallError::Casino(CasinoError::Busy("Too many requests".into())));
        }
        if is_challenge(status, body) {
            return Err(CallError::Casino(CasinoError::Server("Cloudflare turned the app away from Origami's game server".into())));
        }
        let snippet: String = body.chars().take(160).collect();
        return Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status}: {snippet}"))));
    };
    if (200..300).contains(&status) {
        return Ok(v);
    }
    let code = v.get("errorCode").and_then(Value::as_str).unwrap_or("");
    if code == "INVALID_TOKEN" || status == 401 {
        return Err(CallError::SessionGone);
    }
    let msg = match code {
        "INSUFFICIENT_BALANCE" => "Insufficient balance".to_string(),
        "BET_AMOUNT_TOO_LOW" => match (min_bet, currency) {
            (Some(m), Some(c)) => format!("Below Metaspins' minimum bet of {m} {c}"),
            _ => "Below Metaspins' minimum bet".into(),
        },
        "LIMIT_REACHED" => "Above Metaspins' bet or win limit".into(),
        // Origami couldn't book the bet with Metaspins' wallet
        "WEBHOOK_ERROR" => "Metaspins' wallet didn't answer the game (WEBHOOK_ERROR)".into(),
        _ => v.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_string).unwrap_or_else(|| if code.is_empty() { format!("Metaspins refused the bet (HTTP {status})") } else { code.to_string() }),
    };
    if status == 429 || code.contains("TOO_MANY") || code.contains("RATE") {
        return Err(CallError::Casino(CasinoError::Busy(msg)));
    }
    Err(CallError::Casino(CasinoError::Server(msg)))
}

/// A settled Limbo round.
#[derive(Debug, PartialEq)]
struct LimboBet {
    outcome: f64,
    amount: Option<Units>,
    payout: Units,
    after: Option<Units>,
}

/// `/games/limbo/play`'s reply -> the round: the result is in the last action (`END_BET`).
fn limbo_result(v: &Value) -> Option<LimboBet> {
    let last = v.get("betActions")?.as_array()?.last()?;
    let outcome = last.pointer("/data/LIMBO/randomMultiplier").and_then(f64_from_json)?;
    Some(LimboBet {
        outcome,
        amount: v.get("amount").and_then(amount_units),
        payout: v.get("payout").and_then(amount_units).unwrap_or(0),
        after: v.get("afterBalance").and_then(amount_units),
    })
}

/// 2.0 -> 200: Origami takes the target in hundredths.
fn target_hundredths(t: f64) -> u64 {
    (t * 100.0).round().max(0.0) as u64
}

/// "0.12469135689" (or a number) -> base units; digits past the 8th decimal are dropped.
fn amount_units(v: &Value) -> Option<Units> {
    match v {
        Value::String(s) => parse_units(s, DECIMALS),
        Value::Number(_) => parse_units(&format!("{:.8}", f64_from_json(v).filter(|f| f.is_finite() && *f >= 0.0)?), DECIMALS),
        _ => None,
    }
}

fn fmt_amount(x: f64) -> String {
    let s = format!("{x:.8}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A random v4 UUID, the kind of id the game uses for its browser and window.
fn uuid() -> String {
    let b: [u8; 16] = rand::random();
    let h: String = b.iter().enumerate().map(|(i, x)| match i {
        6 => format!("{:02x}", (x & 0x0f) | 0x40),
        8 => format!("{:02x}", (x & 0x3f) | 0x80),
        _ => format!("{x:02x}"),
    }).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Plain HTTPS, looking like the site's own requests.
fn client() -> reqwest::Client {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_static(SITE));
    headers.insert(REFERER, HeaderValue::from_static("https://metaspins.com/"));
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        // header names as browsers write them: Cloudflare turns away lowercase ones over HTTP/1.1
        .http1_title_case_headers()
        .default_headers(headers)
        .gzip(true)
        .build()
        .unwrap_or_default()
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
    fn launch_urls() {
        // the demo launch the site answers a guest with
        let url = "https://s3.eu-central-1.amazonaws.com/ignition.button/round-2/index.html?options=eyJsYXVuY2hfb3B0aW9ucyI6eyJnYW1lX3VybCI6Imh0dHBzOi8vZ2FtZXMuYmV0b3JpZ2FtaS5jb20vP3NldHRpbmdzPWV5SnliM1ZzWlhSMFpWQnlaWFpsYm5SUGNIQnZjMmwwWlVKbGRITWlPblJ5ZFdVc0ltZGhiV1VpT2lKTVNVMUNUeUlzSW1WdWRtbHliMjV0Wlc1MElqb2lVMDlHVkZOWFNWTlRYMFZWWDFCU1QwUWlMQ0owYUdWdFpTSTZJbTFsZEdGemNHbHVjeTVqYzNNJTJGZGoweE56WTNOakE0T1RrMElpd2liRzluYjFWeWJDSTZiblZzYkN3aVptOXZkR1Z5VUc5emFYUnBiMjRpT2lKQlFrOVdSU0lzSW14aGJtZDFZV2RsSWpvaVpXNGlMQ0p0YVc1Q1pYUWlPaUl3TGpBeElpd2liV0Y0UW1WMElqb2lNakF3TUNJc0ltMWhlRkJoZVc5MWRDSTZJakV3TURBd01EQWlMQ0psWkdkbElqb3hMQ0pqZFhKeVpXNWplU0k2SWxWVFJDSXNJbU4xY25KbGJtTjVSR1ZqYVcxaGJITWlPaklzSW5CaGVXOTFkRVJsWTJsdFlXeHpJam94TWl3aWJHOWlZbmxWY213aU9pSm9kSFJ3Y3pvdkwySmhZMnRsYm1RdWJXVjBZWE53YVc1ekxtTnZiUzlsZUdsMFgybG1jbUZ0WlNJc0luSmxjWFZwY21WelJuVnNiRk5qY21WbGJpSTZabUZzYzJVc0luTm9iM2REZFhKeVpXNWplVUZ6VkdWNGRDSTZabUZzYzJWOSIsInN0cmF0ZWd5IjoiaWZyYW1lIn19";
        let l = parse_launch(url).unwrap();
        assert_eq!(l.settings["environment"], json!("SOFTSWISS_EU_PROD"));
        assert_eq!(l.settings["game"], json!("LIMBO"));
        assert_eq!(l.settings["minBet"], json!("0.01"));
        assert_eq!((l.auth_token, l.session), (None, None));
        assert_eq!(origami_base("SOFTSWISS_EU_PROD"), Some("https://eu-api.betorigami.com/agg01/provider/v1"));
        // the game's own URL with a token
        let l = parse_launch("https://games.betorigami.com/?settings=eyJnYW1lIjoiTElNQk8ifQ%3D%3D&authToken=abc.def").unwrap();
        assert_eq!((l.settings["game"].as_str(), l.auth_token.as_deref()), (Some("LIMBO"), Some("abc.def")));
    }

    #[test]
    fn socket_replies() {
        assert!(session_error(&json!("InvalidSession")));
        assert!(session_error(&json!({ "codes": ["session-expired"] })));
        assert!(!session_error(&json!({ "codes": ["game-not-found"] })));
        assert_eq!(text_of(&json!({ "codes": ["game-not-found"] }), "x"), "game-not-found");
        // trimmed from get-accounts
        let a = accounts_of(&json!([{ "usdAmount": 0.0081, "currency": "BTC", "amount": 0.0000001, "isActive": false, "isFiat": false }, { "usdAmount": 22.09, "currency": "USDT", "amount": 22.12721499, "isActive": true, "isFiat": false }]));
        assert_eq!(a, vec![Account { code: "BTC".into(), active: false }, Account { code: "USDT".into(), active: true }]);
        assert_eq!(clean_token("usersession=ABC; path=/"), "ABC");
        assert_eq!(clean_token(" ABC "), "ABC");
        let mut jar = vec![("usersession".to_string(), "A".to_string())];
        merge_cookies(&mut jar, vec![set_cookie("kycvalidation=K; expires=Wed, 14 Oct 2026 23:24:01 GMT; domain=metaspins.com").unwrap(), set_cookie("usersession=B; path=/").unwrap()]);
        assert_eq!(jar, vec![("kycvalidation".to_string(), "K".to_string()), ("usersession".to_string(), "B".to_string())]);
    }

    #[test]
    fn game_replies() {
        let v = json!({ "id": "b1", "amount": "0.01", "payout": "0.02", "multiplier": "2", "afterBalance": "10.01",
            "betActions": [{ "type": "START_BET", "data": {} }, { "type": "END_BET", "data": { "LIMBO": { "randomMultiplier": "3.47", "result": [3.47] } } }] });
        assert_eq!(limbo_result(&v), Some(LimboBet { outcome: 3.47, amount: Some(1_000_000), payout: 2_000_000, after: Some(1_001_000_000) }));
        assert!(matches!(parse_origami(401, r#"{"status":"ERR_UNAUTHORIZED","errorCode":"INVALID_TOKEN","message":"Invalid token"}"#, None, None), Err(CallError::SessionGone)));
        assert!(matches!(parse_origami(400, r#"{"errorCode":"BET_AMOUNT_TOO_LOW","message":"x"}"#, Some("0.01"), Some("USD")), Err(CallError::Casino(CasinoError::Server(m))) if m == "Below Metaspins' minimum bet of 0.01 USD"));
        assert!(matches!(parse_origami(400, r#"{"errorCode":"INSUFFICIENT_BALANCE"}"#, None, None), Err(CallError::Casino(CasinoError::Server(m))) if m == "Insufficient balance"));
        assert_eq!((target_hundredths(2.0), target_hundredths(1.01), target_hundredths(1_000_000.0)), (200, 101, 100_000_000));
        assert_eq!(uuid().len(), 36);
    }
}
