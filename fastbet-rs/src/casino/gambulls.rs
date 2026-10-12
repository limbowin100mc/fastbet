//! Gambulls (gambulls.com). Its originals are Jackpot Studio games: the site hands out a session
//! per game (`getGameLaunchUrl`, a Next.js server action on gambulls.com) and the game talks to
//! Jackpot Studio's server (`https://rgs.jackpot.studio`) with it as `Authorization: Bearer …`:
//! `POST /game/play {message}`, `GET /game/balance`, `POST /session/refresh` (a new session, the
//! old one keeps working), `POST /seed/current`, `POST /seed/rotate {clientSeed}`.
//!
//! Gambulls has no API keys. Its login is the NextAuth cookie `__Secure-authjs.session-token`,
//! which the login window reads from its browser. gambulls.com sits behind a Cloudflare check, so
//! the site's own calls go through the browser bridge, holding that cookie; Jackpot Studio answers
//! plain HTTPS. Server actions are called by an id that changes with every build of the site, so
//! the ids are read from the site's scripts next to the action's name. The site renews its login
//! cookie as it is used; the bridge's copy is read back and saved.
//!
//! `FASTBET_GAMBULLS_DEMO=1` plays with the site's demo money instead, no login needed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::bridge::{Bridge, BridgeCookie, BridgeRequest, BridgeSetup};
use crate::money::{f64_from_json, format_units, parse_units};

const SITE: &str = "https://gambulls.com";
const RGS: &str = "https://rgs.jackpot.studio";
pub const SESSION_COOKIE: &str = "__Secure-authjs.session-token";
/// The bridge's own browser profile; the account's login cookie is put into it.
const PROFILE: &str = "gambulls-bridge";
/// Jackpot Studio plays in the account's currency, dollars for every account seen so far.
const CURRENCY: &str = "usd";
/// Bets go down to 0.00000001 and payouts carry more digits still; 8 decimals are kept.
const DECIMALS: u32 = 8;
const LIMBO: &str = "originals-limbo";
const LAUNCH: &str = "getGameLaunchUrl";
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// The balance is re-read this often while idle; bets update it locally.
const REFRESH: Duration = Duration::from_secs(10);
/// A game session lasts about 4 hours; it is renewed after this.
const SESSION_RENEW: Duration = Duration::from_secs(60 * 60);
/// How often the site is asked who is logged in, which also keeps its login cookie fresh.
const SITE_CHECK: Duration = Duration::from_secs(10 * 60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(90);
/// The bridge window shows itself when the Cloudflare check takes longer than this.
const SHOW_CHECK_AFTER: Duration = Duration::from_secs(20);
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

const EXPIRED: &str = "Your Gambulls login has expired. Log in again in Settings.";

enum CallError {
    /// Cloudflare answered instead of gambulls.com.
    Cloudflare,
    /// gambulls.com doesn't see a login.
    LoggedOut,
    /// Jackpot Studio no longer knows the game session.
    SessionGone,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Cloudflare => CasinoError::Server("Cloudflare turned the app away from gambulls.com".into()),
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

#[derive(Clone, Debug, PartialEq)]
struct User {
    id: String,
    name: Option<String>,
}

#[derive(Clone)]
struct Session {
    token: String,
    since: Instant,
}

pub struct Gambulls {
    token: Mutex<Option<String>>,
    demo: bool,
    client: reqwest::Client,
    bridge: Mutex<Option<Bridge>>,
    /// Server action ids by name, read from the site's scripts.
    actions: Mutex<HashMap<&'static str, String>>,
    user: Mutex<Option<User>>,
    /// Jackpot Studio sessions by game slug.
    sessions: Mutex<HashMap<&'static str, Session>>,
    /// One game launch at a time; bets that need the same session wait for it.
    launching: tokio::sync::Mutex<()>,
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

impl Gambulls {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let me = Arc::new(Self {
            token: Mutex::new(setup.token.filter(|t| !t.trim().is_empty())),
            demo: std::env::var("FASTBET_GAMBULLS_DEMO").is_ok_and(|v| !v.is_empty() && v != "0"),
            client: client(),
            bridge: Mutex::new(None),
            actions: Mutex::new(HashMap::new()),
            user: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            launching: tokio::sync::Mutex::new(()),
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
        tracing::info!("gambulls: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn close_bridge(&self) {
        if let Some(b) = self.bridge.lock().take() {
            b.close();
        }
    }

    fn bridge(&self) -> Result<Bridge, CallError> {
        self.bridge.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            let token = self.token.lock().clone();
            if token.is_none() && !self.demo {
                self.status(ConnStatus::NoToken, "");
                self.wake.notified().await;
                continue;
            }
            let stop = match self.connect(token.as_deref()).await {
                Ok(()) => {
                    retries = 0;
                    self.serve().await
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
            self.close_bridge();
            match stop {
                Stop::Wake => {}
                Stop::LoggedOut => {
                    self.status(ConnStatus::AuthFailed, EXPIRED);
                    self.wake.notified().await;
                }
                Stop::Lost => {
                    tokio::select! {
                        _ = tokio::time::sleep(backoff(&mut retries)) => {}
                        _ = self.wake.notified() => {}
                    }
                }
            }
        }
        self.close_bridge();
    }

    /// Opens the bridge on gambulls.com with the login cookie, checks who is logged in and
    /// opens the Limbo session.
    async fn connect(&self, token: Option<&str>) -> Result<(), CallError> {
        self.sessions.lock().clear();
        self.status(ConnStatus::Connecting, "Opening gambulls.com");
        let cookies = token.map(|t| vec![BridgeCookie { name: SESSION_COOKIE.into(), value: t.to_string(), domain: "gambulls.com".into() }]).unwrap_or_default();
        let bridge = Bridge::open_with(&format!("{SITE}/en"), Some(PROFILE), &BridgeSetup { user_agent: None, cookies }).await.map_err(|e| CallError::Casino(CasinoError::Server(e)))?;
        if let Some(old) = self.bridge.lock().replace(bridge.clone()) {
            old.close();
        }

        // the bridge's page has to get through the Cloudflare check first
        let t0 = Instant::now();
        let mut shown = false;
        let rsc = loop {
            if !self.alive() {
                return Err(CallError::Casino(CasinoError::NotConnected));
            }
            match site_get(&bridge, "/en", true).await {
                Ok(body) => break body,
                Err(CallError::Cloudflare) => {
                    if t0.elapsed() > CHECK_TIMEOUT {
                        return Err(CallError::Casino(CasinoError::Server("Cloudflare didn't let the app through to gambulls.com".into())));
                    }
                    if !shown && t0.elapsed() > SHOW_CHECK_AFTER {
                        self.status(ConnStatus::Connecting, "Complete the security check in the window that opened");
                        bridge.show(true);
                        shown = true;
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(e) => return Err(e),
            }
        };
        if shown {
            bridge.show(false);
        }

        self.status(ConnStatus::Authenticating, "");
        let user = if self.demo { None } else { Some(user_of(&rsc).ok_or(CallError::LoggedOut)?) };
        {
            let mut m = self.market.lock();
            m.currencies.insert(CURRENCY.into(), CurrencyInfo { decimals: DECIMALS, price: 1.0, hidden: false });
            m.site_currency = Some(CURRENCY.into());
            m.username = if self.demo { Some("Demo".into()) } else { user.as_ref().and_then(|u| u.name.clone()) };
        }
        *self.user.lock() = user;
        self.save_cookie(&bridge).await;
        self.session(LIMBO).await?;
        if let Err(e) = self.refresh_balance().await {
            tracing::info!("gambulls: balance: {}", CasinoError::from(e));
        }
        self.publish();
        self.status(ConnStatus::Ready, "");
        if let Ok(st) = self.seeds().await {
            let _ = self.events.send(CasinoEvent::Provable(st));
        }
        Ok(())
    }

    /// Keeps the balance, the game sessions and the login fresh until something needs a new connection.
    async fn serve(&self) -> Stop {
        let mut site_at = Instant::now();
        let mut failures = 0;
        loop {
            tokio::select! {
                _ = self.wake.notified() => return Stop::Wake,
                _ = tokio::time::sleep(REFRESH) => {}
            }
            if !self.alive() {
                return Stop::Wake;
            }
            self.renew_sessions().await;
            if site_at.elapsed() > SITE_CHECK {
                site_at = Instant::now();
                match self.site_check().await {
                    Ok(()) => {}
                    Err(CallError::LoggedOut) => return Stop::LoggedOut,
                    Err(e) => tracing::info!("gambulls: site check: {}", CasinoError::from(e)),
                }
            }
            match self.refresh_balance().await {
                Ok(()) => failures = 0,
                Err(CallError::Casino(CasinoError::ConnectionLost | CasinoError::Timeout)) => failures += 1,
                Err(_) => {}
            }
            // the network is gone: start over
            if failures >= 3 {
                return Stop::Lost;
            }
        }
    }

    /// Asks the site who is logged in (which also renews its login cookie) and saves a renewed cookie.
    async fn site_check(&self) -> Result<(), CallError> {
        let bridge = self.bridge()?;
        let rsc = site_get(&bridge, "/en", true).await?;
        if !self.demo && user_of(&rsc).is_none() {
            return Err(CallError::LoggedOut);
        }
        self.save_cookie(&bridge).await;
        Ok(())
    }

    /// The site hands out a new login cookie as it is used: the bridge's copy goes to the settings.
    async fn save_cookie(&self, bridge: &Bridge) {
        if self.demo {
            return;
        }
        let Ok(list) = bridge.cookies(&format!("{SITE}/")).await else { return };
        let Some(fresh) = list.into_iter().find(|c| c.name == SESSION_COOKIE).map(|c| c.value).filter(|v| !v.is_empty()) else { return };
        {
            let mut cur = self.token.lock();
            if cur.as_deref() == Some(fresh.as_str()) {
                return;
            }
            *cur = Some(fresh.clone());
        }
        tracing::info!("gambulls: login renewed");
        let _ = self.events.send(CasinoEvent::Token(fresh));
    }

    /// The Jackpot Studio session of a game, opened through the site when there is none yet.
    async fn session(&self, slug: &'static str) -> Result<String, CallError> {
        if let Some(s) = self.sessions.lock().get(slug) {
            return Ok(s.token.clone());
        }
        let _one = self.launching.lock().await;
        if let Some(s) = self.sessions.lock().get(slug) {
            return Ok(s.token.clone());
        }
        let token = self.launch(slug).await?;
        self.sessions.lock().insert(slug, Session { token: token.clone(), since: Instant::now() });
        Ok(token)
    }

    /// Drops a session Jackpot Studio no longer knows and opens a new one (unless another call already did).
    async fn reopen(&self, slug: &'static str, stale: &str) -> Result<String, CallError> {
        {
            let mut s = self.sessions.lock();
            if s.get(slug).is_some_and(|x| x.token == stale) {
                s.remove(slug);
            }
        }
        self.session(slug).await
    }

    /// `getGameLaunchUrl` on the site -> the game's session token.
    async fn launch(&self, slug: &str) -> Result<String, CallError> {
        let user = self.user.lock().as_ref().map(|u| u.id.clone());
        let args = json!([{
            "gameSlug": slug,
            "demo": self.demo,
            "platform": "GPL_DESKTOP",
            "deviceType": 1,
            "lang": "en",
            "gameCurrency": "$undefined",
            "countryCode": "$undefined",
            "lobbyUrl": SITE,
            "depositUrl": format!("{SITE}/en?cashier=deposit"),
        }, user.as_deref().unwrap_or("guest")]);
        let res = self.call_action(LAUNCH, &args, &format!("/en/casino/games/{slug}")).await?;
        if res.get("success").and_then(Value::as_bool) != Some(true) {
            let msg = res.get("message").and_then(Value::as_str).unwrap_or("Gambulls didn't open the game").to_string();
            let low = msg.to_ascii_lowercase();
            if ["unauthor", "login", "not logged", "unauthenticated", "session"].iter().any(|k| low.contains(k)) {
                return Err(CallError::LoggedOut);
            }
            return Err(CallError::Casino(CasinoError::Server(msg)));
        }
        let token = res.pointer("/responseObject/sessionToken").and_then(Value::as_str).filter(|t| !t.is_empty());
        let token = token.ok_or_else(|| CallError::Casino(CasinoError::BadReply(res.to_string().chars().take(200).collect())))?;
        tracing::info!("gambulls: {slug} session opened");
        Ok(token.to_string())
    }

    /// Calls the site's server action `name` from the bridge's page; answers its result object.
    async fn call_action(&self, name: &'static str, args: &Value, path: &str) -> Result<Value, CallError> {
        for attempt in 0..2 {
            let id = self.action_id(name).await?;
            let bridge = self.bridge()?;
            let req = BridgeRequest {
                method: "POST".into(),
                url: path.into(),
                headers: vec![
                    ("Accept".into(), "text/x-component".into()),
                    ("Next-Action".into(), id.clone()),
                    ("Content-Type".into(), "text/plain;charset=UTF-8".into()),
                ],
                body: Some(args.to_string()),
            };
            let res = bridge.request_within(req, RPC_TIMEOUT).await.map_err(bridge_error)?;
            if is_challenge(res.status, &res.body) {
                return Err(CallError::Cloudflare);
            }
            // the site was rebuilt since the ids were read: read them again
            if attempt == 0 && (res.status == 404 || res.body.contains("Server action not found") || res.body.contains("Failed to find Server Action")) {
                self.actions.lock().clear();
                continue;
            }
            return action_result(&res.body).ok_or_else(|| CallError::Casino(CasinoError::BadReply(format!("HTTP {}: {}", res.status, res.body.chars().take(160).collect::<String>()))));
        }
        Err(CallError::Casino(CasinoError::Server(format!("Gambulls changed its site: {name} wasn't found"))))
    }

    /// The id of server action `name`, read from the scripts the homepage loads.
    async fn action_id(&self, name: &'static str) -> Result<String, CallError> {
        if let Some(id) = self.actions.lock().get(name) {
            return Ok(id.clone());
        }
        let bridge = self.bridge()?;
        let html = site_get(&bridge, "/en", false).await?;
        let mut scripts = chunk_paths(&html);
        // the shared chunks hold most actions; the page and layout chunks go last
        scripts.sort_by_key(|p| p.contains("/app/"));
        for path in scripts {
            let js = match self.client.get(format!("{SITE}{path}")).timeout(RPC_TIMEOUT).send().await {
                Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                // static files usually skip the Cloudflare check; when not, the bridge fetches them
                _ => match bridge.request_within(BridgeRequest { method: "GET".into(), url: path.clone(), headers: vec![], body: None }, RPC_TIMEOUT).await {
                    Ok(r) if r.status == 200 => r.body,
                    _ => continue,
                },
            };
            if let Some(id) = action_in(&js, name) {
                tracing::info!("gambulls: {name} is {id}");
                self.actions.lock().insert(name, id.clone());
                return Ok(id);
            }
        }
        Err(CallError::Casino(CasinoError::Server(format!("Gambulls changed its site: {name} wasn't found"))))
    }

    /// One request to Jackpot Studio with a game session.
    async fn rgs(&self, method: reqwest::Method, path: &str, body: Option<Value>, session: &str) -> Result<Value, CallError> {
        let mut req = self.client.request(method, format!("{RGS}{path}")).timeout(RPC_TIMEOUT).bearer_auth(session);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
        let res = req.send().await.map_err(lost)?;
        let status = res.status().as_u16();
        let text = res.text().await.map_err(lost)?;
        parse_rgs(status, &text)
    }

    /// A request in a game's session; a session Jackpot Studio dropped is opened again once.
    async fn in_session(&self, slug: &'static str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, CallError> {
        let session = self.session(slug).await?;
        match self.rgs(method.clone(), path, body.clone(), &session).await {
            Err(CallError::SessionGone) => {
                let fresh = self.reopen(slug, &session).await?;
                self.rgs(method, path, body, &fresh).await
            }
            res => res,
        }
    }

    /// Trades game sessions older than an hour for new ones; a dropped one is opened again when next needed.
    async fn renew_sessions(&self) {
        let old: Vec<(&'static str, String)> = self.sessions.lock().iter().filter(|(_, s)| s.since.elapsed() > SESSION_RENEW).map(|(k, s)| (*k, s.token.clone())).collect();
        for (slug, token) in old {
            match self.rgs(reqwest::Method::POST, "/session/refresh", Some(json!({})), &token).await {
                Ok(v) => {
                    if let Some(fresh) = v.get("sessionToken").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                        self.sessions.lock().insert(slug, Session { token: fresh.to_string(), since: Instant::now() });
                    }
                }
                Err(CallError::SessionGone) => {
                    let mut s = self.sessions.lock();
                    if s.get(slug).is_some_and(|x| x.token == token) {
                        s.remove(slug);
                    }
                }
                Err(e) => tracing::info!("gambulls: session renewal: {}", CasinoError::from(e)),
            }
        }
    }

    /// Re-reads the balance. It is only taken when no bet ran meanwhile, so a reply older than
    /// the latest local update never wins.
    async fn refresh_balance(&self) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let v = self.in_session(LIMBO, reqwest::Method::GET, "/game/balance", None).await?;
        let balance = v.get("balance").and_then(amount_units).ok_or_else(|| CallError::Casino(CasinoError::BadReply(v.to_string().chars().take(200).collect())))?;
        if let Some(cur) = v.get("currency").and_then(Value::as_str).filter(|c| !c.eq_ignore_ascii_case(CURRENCY)) {
            tracing::warn!("gambulls: the game session is in {cur}, shown as dollars");
        }
        if idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled {
            self.market.lock().wallets.insert(CURRENCY.into(), balance);
            self.publish();
        }
        Ok(())
    }

    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let v = self.in_session(LIMBO, reqwest::Method::POST, "/seed/current", Some(json!({}))).await?;
        Ok(seed_state(&v))
    }

    /// Books a settled bet into the local wallet.
    fn credit(&self, delta: Units) {
        if delta == 0 {
            return;
        }
        *self.market.lock().wallets.entry(CURRENCY.into()).or_insert(0) += delta;
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
}

#[async_trait]
impl Casino for Gambulls {
    fn id(&self) -> CasinoId {
        CasinoId::Gambulls
    }

    fn set_token(&self, token: Option<String>) {
        let token = token.filter(|t| !t.trim().is_empty());
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
        self.close_bridge();
    }

    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        if req.amount <= 0 {
            return Err(CasinoError::Server("Gambulls doesn't take bets of 0".into()));
        }
        let _inflight = InFlight::new(&self.inflight);
        let message = json!({ "bet": { "amount": format_units(req.amount, DECIMALS, DECIMALS), "target": target_str(req.target) } });
        let v = self.in_session(LIMBO, reqwest::Method::POST, "/game/play", Some(json!({ "message": message }))).await?;
        let bad = || CasinoError::BadReply(v.to_string().chars().take(200).collect());
        let bet = v.pointer("/message/bet").ok_or_else(bad)?;
        let outcome = bet.get("outcome").and_then(f64_from_json).ok_or_else(bad)?;
        let payout = bet.pointer("/result/payout").and_then(amount_units).unwrap_or(0);
        self.credit(payout - req.amount);
        self.settled.fetch_add(1, Ordering::SeqCst);
        Ok(BetReply { outcome, amount: Some(req.amount), winnings: Some(payout), win: Some(payout > 0), nonce: None })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let v = self.in_session(LIMBO, reqwest::Method::POST, "/seed/rotate", Some(json!({ "clientSeed": client_seed }))).await?;
        let mut st = seed_state(v.get("active").unwrap_or(&v));
        st.client_seed.get_or_insert_with(|| client_seed.to_string());
        st.nonce.get_or_insert(0);
        Ok(st)
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        self.seeds().await
    }

    async fn list_slots(&self, _filter: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _catalog_id: &str, _demo: bool, _currency: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots aren't available on Gambulls yet".into()))
    }
}

/// `fastbet --gambulls-test [bets]`: connects the way the app does and prints the account, the
/// balance and the seeds. With `FASTBET_GAMBULLS_DEMO=1` it also plays `bets` Limbo rounds of
/// 0.01 demo dollars at 2×; with a login (`FASTBET_GAMBULLS_TOKEN`, the session cookie) it never bets.
pub async fn self_test(bets: usize) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = std::env::var("FASTBET_GAMBULLS_TOKEN").ok().filter(|t| !t.is_empty());
    let casino = Gambulls::spawn(Setup { token, ..Default::default() }, tx);
    let demo = std::env::var("FASTBET_GAMBULLS_DEMO").is_ok_and(|v| !v.is_empty() && v != "0");
    let t0 = Instant::now();
    let show = |e: CasinoEvent| match e {
        CasinoEvent::Status(s, d) => println!("status: {} {d}", s.short()),
        CasinoEvent::Market(m) => println!("account: {:?}, balance: {:?}", m.username, m.wallets.get(CURRENCY).map(|v| format_units(*v, DECIMALS, DECIMALS))),
        CasinoEvent::Provable(p) => println!("seeds: client {:?}, server hash {:?}, nonce {:?}", p.client_seed, p.server_seed_hash, p.nonce),
        CasinoEvent::Token(t) => println!("login renewed ({} characters)", t.len()),
    };
    loop {
        match tokio::time::timeout(Duration::from_secs(150), rx.recv()).await {
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
    if demo {
        for i in 0..bets {
            let t = Instant::now();
            match casino.bet_limbo(&BetRequest { currency: CURRENCY.into(), amount: 1_000_000, target: 2.0 }).await {
                Ok(r) => println!("bet {i}: {:.4}× win {:?} paid {:?} ({} ms)", r.outcome, r.win, r.winnings.map(|w| format_units(w, DECIMALS, DECIMALS)), t.elapsed().as_millis()),
                Err(e) => println!("bet {i}: {e}"),
            }
        }
        match casino.rotate_seed("fastbet-test").await {
            Ok(p) => println!("new seed: {:?} nonce {:?}", p.client_seed, p.nonce),
            Err(e) => println!("new seed: {e}"),
        }
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    while let Ok(e) = rx.try_recv() {
        show(e);
    }
    casino.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// A page of the site through the bridge; `rsc` asks for the React data instead of the HTML.
async fn site_get(bridge: &Bridge, path: &str, rsc: bool) -> Result<String, CallError> {
    let headers = if rsc { vec![("RSC".into(), "1".into())] } else { vec![] };
    let res = bridge.request_within(BridgeRequest { method: "GET".into(), url: path.into(), headers, body: None }, RPC_TIMEOUT).await.map_err(bridge_error)?;
    if is_challenge(res.status, &res.body) {
        return Err(CallError::Cloudflare);
    }
    if res.status != 200 {
        return Err(CallError::Casino(CasinoError::BadReply(format!("gambulls.com answered HTTP {}", res.status))));
    }
    Ok(res.body)
}

fn bridge_error(e: String) -> CallError {
    CallError::Casino(if e.contains("timeout") { CasinoError::Timeout } else { CasinoError::ConnectionLost })
}

fn is_challenge(status: u16, body: &str) -> bool {
    let head: String = body.chars().take(4000).collect();
    ["Just a moment", "challenge-platform", "Attention Required", "cf-chl"].iter().any(|k| head.contains(k)) || (status == 403 && !head.trim_start().starts_with('{'))
}

/// The logged-in user from the site's page data: the session object carries `rawToken` (the
/// login cookie), with the user's `name` and `id` before it. None: nobody is logged in.
fn user_of(rsc: &str) -> Option<User> {
    let at = rsc.find("\"rawToken\"")?;
    let head = &rsc[..at];
    // the object holding rawToken starts at the first `{` back that isn't closed before it
    let mut depth = 0;
    let start = head.char_indices().rev().find_map(|(i, c)| {
        match c {
            '}' => depth += 1,
            '{' if depth == 0 => return Some(i),
            '{' => depth -= 1,
            _ => {}
        }
        None
    })?;
    let obj = &head[start..];
    let field = |key: &str| -> Option<String> {
        let pat = format!("\"{key}\":\"");
        let from = obj.find(&pat)? + pat.len();
        let len = obj[from..].find('"')?;
        Some(obj[from..from + len].to_string()).filter(|v| !v.is_empty())
    };
    Some(User { id: field("id")?, name: field("name") })
}

/// The scripts a page loads, as site paths.
fn chunk_paths(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (at, _) in html.match_indices("/_next/static/chunks/") {
        let rest = &html[at..];
        let end = rest.find(|c: char| !(c.is_ascii_alphanumeric() || "/_.-%[]()~".contains(c))).unwrap_or(rest.len());
        let path = &rest[..end];
        if path.ends_with(".js") && !out.iter().any(|p| p == path) {
            out.push(path.to_string());
        }
    }
    out
}

/// `createServerReference)("<id>",x.callServer,void 0,x.findSourceMapURL,"<name>")` -> the id.
fn action_in(js: &str, name: &str) -> Option<String> {
    const CALL: &str = "createServerReference)(\"";
    let tail = format!("\"{name}\")");
    js.match_indices(&tail).find_map(|(at, _)| {
        let head = &js[at.saturating_sub(300)..at];
        let start = head.rfind(CALL)? + CALL.len();
        let id: String = head[start..].chars().take_while(char::is_ascii_hexdigit).collect();
        (id.len() >= 40 && head[start + id.len()..].starts_with('"')).then_some(id)
    })
}

/// A server action's reply (`0:{"a":"$@1",…}` / `1:{"success":…}` lines) -> the result object.
fn action_result(body: &str) -> Option<Value> {
    body.lines().find_map(|line| {
        let (_, json) = line.split_once(':')?;
        serde_json::from_str::<Value>(json).ok().filter(|v| v.get("success").is_some())
    })
}

/// Jackpot Studio's reply -> the JSON, or its error (`{status, code, detail, error: {message}}`).
fn parse_rgs(status: u16, body: &str) -> Result<Value, CallError> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        if status == 401 {
            return Err(CallError::SessionGone);
        }
        if status == 429 {
            return Err(CallError::Casino(CasinoError::Busy("Too many requests".into())));
        }
        if is_challenge(status, body) {
            return Err(CallError::Cloudflare);
        }
        let snippet: String = body.chars().take(160).collect();
        return Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status}: {snippet}"))));
    };
    if (200..300).contains(&status) {
        return Ok(v);
    }
    if status == 401 {
        return Err(CallError::SessionGone);
    }
    let code = v.get("code").and_then(Value::as_str).unwrap_or("");
    let detail = &v["detail"];
    let money = |k: &str| detail.get(k).and_then(f64_from_json).map(|x| format!("${}", fmt_amount(x)));
    let msg = match code {
        "bet_below_min" => format!("Below Gambulls' minimum bet{}", money("minBet").map(|m| format!(" of {m}")).unwrap_or_default()),
        "bet_limit_exceeded" => format!("Above Gambulls' maximum bet{}", money("maxBet").map(|m| format!(" of {m}")).unwrap_or_default()),
        "win_limit_exceeded" => "The win would be above Gambulls' maximum win".into(),
        "game_param_out_of_range" => "Gambulls refused the bet: a setting is out of range".into(),
        _ => v
            .pointer("/error/message")
            .and_then(Value::as_str)
            .or_else(|| detail.as_str())
            .or_else(|| v.get("title").and_then(Value::as_str))
            .unwrap_or("Gambulls refused the bet")
            .to_string(),
    };
    if status == 429 || code.contains("too_many") || code.contains("rate") {
        return Err(CallError::Casino(CasinoError::Busy(msg)));
    }
    Err(CallError::Casino(CasinoError::Server(msg)))
}

fn fmt_amount(x: f64) -> String {
    let s = format!("{x:.8}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    s.to_string()
}

/// `/seed/current` (or the `active` part of `/seed/rotate`) -> the seed state.
fn seed_state(v: &Value) -> ProvableState {
    ProvableState {
        client_seed: v.get("clientSeed").and_then(Value::as_str).map(str::to_string),
        server_seed_hash: v.get("serverHash").and_then(Value::as_str).map(str::to_string),
        nonce: v.get("nonce").and_then(|n| n.as_u64().or_else(|| n.as_str().and_then(|s| s.parse().ok()))),
    }
}

/// "0.12469135689" (or a number) -> base units; digits past the 8th decimal are dropped.
fn amount_units(v: &Value) -> Option<Units> {
    match v {
        Value::String(s) => parse_units(s, DECIMALS),
        Value::Number(_) => parse_units(&format!("{:.8}", f64_from_json(v).filter(|f| f.is_finite() && *f >= 0.0)?), DECIMALS),
        _ => None,
    }
}

/// 2.0 -> "2", 1.5 -> "1.5": the target as the game sends it.
fn target_str(t: f64) -> String {
    fmt_amount((t * 100.0).round() / 100.0)
}

/// Plain HTTPS, looking like the game's own requests.
fn client() -> reqwest::Client {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json, text/plain, */*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_static("https://gambulls.jackpot.studio"));
    headers.insert(REFERER, HeaderValue::from_static("https://gambulls.jackpot.studio/"));
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
    fn site_data() {
        // trimmed from the logged-in page data
        let rsc = r#"9:{"slogan":"Play","name":"Online Casino"}
a:["$","$L1",null,{"session":{"user":{"name":"sxseth","email":"x@y","image":"https://i","id":"cmecl6abc","sessionId":"592fb","isActive":true,"currency":"USD","selfExcludedUntil":null,"rawToken":"$42"},"expires":"2026-10-12T23:32:08.175Z"}}]"#;
        assert_eq!(user_of(rsc), Some(User { id: "cmecl6abc".into(), name: Some("sxseth".into()) }));
        assert_eq!(user_of(r#"{"session":null,"name":"Online Casino","id":"x"}"#), None);
        // fields of another object don't count
        assert_eq!(user_of(r#"{"name":"Promo","id":"p1"},{"user":{"email":"x","rawToken":"$4"}}"#), None);

        let html = r#"<script src="/_next/static/chunks/1044-a5db909455c2163e.js" async=""></script><script src="/_next/static/chunks/app/%5Blng%5D/layout-2ad02a0723e31770.js" async=""></script>"#;
        assert_eq!(chunk_paths(html), vec!["/_next/static/chunks/1044-a5db909455c2163e.js", "/_next/static/chunks/app/%5Blng%5D/layout-2ad02a0723e31770.js"]);

        let js = r#"let i=(0,n.createServerReference)("609c52d3ffca88911522660c97ce0ffdba8163300e",n.callServer,void 0,n.findSourceMapURL,"getGameLaunchUrl"),s=(0,n.createServerReference)("00c2ffacbff0b67cd9237eba504b406b1eb410be8f",n.callServer,void 0,n.findSourceMapURL,"resolveGameLaunchCountry")"#;
        assert_eq!(action_in(js, "getGameLaunchUrl").as_deref(), Some("609c52d3ffca88911522660c97ce0ffdba8163300e"));
        assert_eq!(action_in(js, "resolveGameLaunchCountry").as_deref(), Some("00c2ffacbff0b67cd9237eba504b406b1eb410be8f"));
        assert_eq!(action_in(js, "getUserWallet"), None);

        let body = "0:{\"a\":\"$@1\",\"f\":\"\",\"b\":\"3QDLyZ8FSEEoPvc3exmTD\"}\n1:{\"success\":true,\"message\":\"OK\",\"responseObject\":{\"source\":3,\"sessionToken\":\"abce00e1\"}}\n";
        assert_eq!(action_result(body).and_then(|v| v.pointer("/responseObject/sessionToken").cloned()), Some(json!("abce00e1")));
    }

    #[test]
    fn game_replies() {
        // from demo rounds
        let ok = parse_rgs(200, r#"{"message":{"metadata":{"balance":"999","roundId":"44e9"},"bet":{"outcome":"1.436037713749783008","result":{"payout":"0","multiplier":"0"}}}}"#).ok().unwrap();
        assert_eq!(ok.pointer("/message/bet/outcome").and_then(f64_from_json), Some(1.436037713749783));
        assert!(matches!(parse_rgs(401, r#"{"status":401,"title":"Unauthorized","code":"session_not_found_or_expired"}"#), Err(CallError::SessionGone)));
        assert!(matches!(parse_rgs(400, r#"{"status":400,"code":"game_invalid_amount","detail":"wager must be positive","error":{"message":"wager must be positive"}}"#), Err(CallError::Casino(CasinoError::Server(m))) if m == "wager must be positive"));
        assert!(matches!(parse_rgs(400, r#"{"status":400,"code":"bet_below_min","detail":{"minBet":"0.1"}}"#), Err(CallError::Casino(CasinoError::Server(m))) if m == "Below Gambulls' minimum bet of $0.1"));
        assert!(matches!(parse_rgs(403, "<html>Just a moment...</html>"), Err(CallError::Cloudflare)));
        assert_eq!(amount_units(&json!("0.12469135689")), Some(12_469_135));
        assert_eq!(amount_units(&json!("1000")), Some(100_000_000_000));
        assert_eq!((target_str(2.0), target_str(1.5), target_str(1.01), target_str(1_000_000.0)), ("2".into(), "1.5".into(), "1.01".into(), "1000000".into()));
        assert_eq!(format_units(1_000_000, DECIMALS, DECIMALS), "0.01");
        let st = seed_state(&json!({ "serverHash": "04017f", "clientSeed": "c9d1", "nonce": 18 }));
        assert_eq!((st.client_seed.as_deref(), st.server_seed_hash.as_deref(), st.nonce), (Some("c9d1"), Some("04017f"), Some(18)));
    }
}
