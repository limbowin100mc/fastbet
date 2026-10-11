//! Stake over its GraphQL API (`/_api/graphql`), signed in with the user's
//! API token in the `x-access-token` header.
//!
//! Stake's domains sit behind Cloudflare checks that turn plain HTTP clients
//! away (stake.com answers them with its "blocked" page), so every call runs
//! through the browser bridge parked on the chosen mirror, same-origin with
//! the API. A fresh bridge clears the check by itself in about 15 s; if the
//! check wants a click, the bridge window is shown for the user. When it never
//! passes, the user can copy the `cf_clearance` cookie and User Agent from
//! their own browser; the bridge then presents both on that mirror. A cookie
//! Cloudflare refuses is reported, not turned into a check window: that window
//! would claim to be the user's browser, and such a check doesn't pass.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::bridge::{Bridge, BridgeCookie, BridgeRequest, BridgeSetup};
use crate::cloudflare::{self, Clearance};
use crate::money::{f64_from_json, parse_units, to_f64};

/// Stake sends every amount as a float with at most 8 decimals.
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// Balances are re-read this often while idle; bets update them locally.
const REFRESH: Duration = Duration::from_secs(10);
const RATES_EVERY: Duration = Duration::from_secs(120);
/// A hidden bridge gets this long to clear Cloudflare before we show it.
const SHOW_CHECK_AFTER: Duration = Duration::from_secs(20);
const CHECK_TIMEOUT: Duration = Duration::from_secs(150);
/// Browser profile of the bridge, so Stake's cookies stay apart from the slot providers'.
const PROFILE: &str = "stake";
/// Profile used with a cookie from the user's browser, so it never mixes with the bridge's own.
const PROFILE_MANUAL: &str = "stake-cf";

pub const DEFAULT_MIRROR: &str = "stake.com";

/// Stake and its official mirrors. Every one reaches the same account.
pub const MIRRORS: &[&str] = &[
    "stake.com", "stake.jp", "stake.bet", "stake.ceo", "stake.games", "stake.mba", "stake.krd", "stake.bz",
    "stake.ac", "stake.pet", "staketr.com", "stakeru8.com", "stakeru9.com", "stkmirror.com", "stake1017.com",
    "stake1020.com", "stake1021.com", "stake1022.com", "stake1034.com", "stake1035.com", "stake1036.com",
    "stake1037.com", "stake1038.com", "stake1039.com", "stake1043.com", "stake1048.com", "stake1052.com",
    "stake1057.com", "stake1061.com", "stake1066.com", "stake1067.com", "stake1068.com", "stake1069.com",
    "stake1070.com", "stake1071.com", "stake1072.com", "stake1073.com", "stake1074.com", "stake1075.com",
    "stake1076.com", "stake1077.com", "stake1078.com", "stake1079.com", "stake1080.com", "stake1081.com",
    "stake1082.com", "stake1083.com", "stake1084.com", "stake1085.com", "stake1086.com", "stake1087.com",
    "stake1088.com", "stake1089.com", "stake1090.com", "stake1091.com", "stake1092.com", "stake1093.com",
    "stake1094.com", "stake1095.com", "stake3000.com", "stake3001.com", "stake3002.com", "stake3003.com",
    "stake3004.com", "stake3005.com", "stake3006.com", "stake3007.com", "stake3008.com", "stake3009.com",
    "stake3010.com", "stake3011.com", "stake3012.com", "stake3013.com", "stake3014.com", "stake3015.com",
    "stake3016.com", "stake3017.com", "stake3018.com", "stake3019.com", "stake3020.com", "stake3026.com",
    "stake3027.com", "stake3028.com", "stake3029.com", "stake3030.com", "stake3033.com", "stake3035.com",
    "stake3039.com", "stake3040.com", "stake3041.com", "stake3043.com", "stake3045.com", "stake3046.com",
    "stake3047.com", "stake3048.com", "stake3050.com", "stake3051.com", "stake3053.com", "stake3056.com",
    "stake3058.com", "stake3061.com", "stake3062.com", "stake3064.com", "stake3065.com", "stake3067.com",
    "stake3069.com", "stake3070.com", "stake3072.com", "stake3075.com", "stake3077.com", "stake3079.com",
    "stake3080.com", "stake3082.com", "stake3083.com", "stake3084.com", "stake3085.com", "stake3086.com",
    "stake3087.com", "stake3088.com", "stake3090.com", "stake3094.com", "stake3098.com", "stake3099.com",
    "stake3103.com", "stake3104.com", "stake3107.com", "stake3109.com", "stake3112.com", "stake3114.com",
    "stake3116.com", "stake3119.com", "stake3199.com",
];

const ME: &str = "query FastbetMe { user { id name balances { available { amount currency } } \
    activeClientSeed { seed } activeServerSeed { seedHash nonce } } }";
const RATES: &str = "query FastbetRates { info { currencies { name usd: value(fiatCurrency: usd) } } }";
const LIMBO: &str = "mutation FastbetLimbo($amount: Float!, $multiplierTarget: Float!, $currency: CurrencyEnum!, $identifier: String!) { \
    limboBet(amount: $amount, currency: $currency, multiplierTarget: $multiplierTarget, identifier: $identifier) { \
    id nonce payoutMultiplier amount payout currency state { ... on CasinoGameLimbo { result multiplierTarget } } } }";
const ROTATE: &str = "mutation FastbetRotateSeed($seed: String!) { rotateSeedPair(seed: $seed) { clientSeed { user { id \
    activeClientSeed { id seed } activeServerSeed { id nonce seedHash nextSeedHash } } } } }";

const INVALID_TOKEN: &str = "Invalid API token";

/// Mirror as typed or saved -> bare host ("https://Stake.bet/" -> "stake.bet").
pub fn normalize_mirror(m: &str) -> String {
    let m = m.trim().to_ascii_lowercase();
    let m = m.strip_prefix("https://").or_else(|| m.strip_prefix("http://")).unwrap_or(&m);
    m.trim_end_matches('/').to_string()
}

enum CallError {
    /// Cloudflare answered instead of Stake: the bridge has to pass the check (again).
    Cloudflare,
    /// Stake's own "not available from your location" page.
    Blocked,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Cloudflare => CasinoError::Server("Cloudflare check, reconnecting".into()),
            CallError::Blocked => CasinoError::Server("This mirror is blocked from your location".into()),
            CallError::Casino(e) => e,
        }
    }
}

/// The bridge in use, with the origin it is parked on.
#[derive(Clone)]
struct Link {
    gen: u64,
    bridge: Bridge,
    origin: String,
}

pub struct Stake {
    token: Mutex<Option<String>>,
    mirror: Mutex<String>,
    clearance: Mutex<Option<Clearance>>,
    /// Bumped when the mirror or the Cloudflare cookie in use changes: the bridge is rebuilt.
    setup: AtomicU64,
    /// Token, mirror or cookie changed.
    wake: Notify,
    /// A call found the bridge dead or challenged; the run loop rebuilds it.
    broken: Notify,
    broken_flag: AtomicBool,
    link: Mutex<Option<Link>>,
    link_gen: AtomicU64,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// Bets in flight and bets settled, so a balance poll never overwrites a fresher local balance.
    inflight: AtomicUsize,
    settled: AtomicU64,
}

impl Stake {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let mirror = setup.mirror.map(|m| normalize_mirror(&m)).filter(|m| !m.is_empty()).unwrap_or_else(|| DEFAULT_MIRROR.into());
        let me = Arc::new(Self {
            token: Mutex::new(clean_token(setup.token)),
            mirror: Mutex::new(mirror),
            clearance: Mutex::new(setup.clearance.filter(Clearance::is_complete)),
            setup: AtomicU64::new(0),
            wake: Notify::new(),
            broken: Notify::new(),
            broken_flag: AtomicBool::new(false),
            link: Mutex::new(None),
            link_gen: AtomicU64::new(0),
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
        tracing::info!("stake: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// The mirror and Cloudflare cookie are still the ones the bridge was opened with.
    fn setup_is(&self, setup: u64) -> bool {
        self.setup.load(Ordering::SeqCst) == setup
    }

    /// The user's cookie, when it was issued on `mirror`.
    fn clearance_for(&self, mirror: &str) -> Option<Clearance> {
        self.clearance.lock().clone().filter(|c| c.mirror == mirror)
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            let setup = self.setup.load(Ordering::SeqCst);
            let mirror = self.mirror.lock().clone();
            let res = self.connect(&mirror, setup).await;
            if res.is_ok() {
                retries = 0;
                self.serve(setup).await;
            }
            self.drop_link();
            if !self.alive() {
                break;
            }
            if !self.setup_is(setup) {
                continue;
            }
            self.status(ConnStatus::Offline, res.err().unwrap_or_default());
            tokio::select! {
                _ = tokio::time::sleep(backoff(&mut retries)) => {}
                _ = self.wake.notified() => {}
            }
        }
        self.drop_link();
    }

    /// Opens the bridge on `mirror` and waits until Stake answers through it.
    async fn connect(&self, mirror: &str, setup: u64) -> Result<(), String> {
        self.status(ConnStatus::Connecting, format!("Opening {mirror}"));
        let origin = format!("https://{mirror}");
        let page = format!("{origin}/robots.txt");
        let manual = self.clearance_for(mirror);
        let bridge = match &manual {
            Some(c) => {
                let cookie = BridgeCookie { name: cloudflare::COOKIE.into(), value: c.cookie.clone(), domain: format!(".{mirror}") };
                let setup = BridgeSetup { user_agent: Some(c.user_agent.clone()), cookies: vec![cookie] };
                Bridge::open_with(&page, Some(PROFILE_MANUAL), &setup).await?
            }
            None => Bridge::open_profile(&page, Some(PROFILE)).await?,
        };
        let link = Link { gen: self.link_gen.fetch_add(1, Ordering::SeqCst) + 1, bridge, origin };
        self.broken_flag.store(false, Ordering::SeqCst);
        *self.link.lock() = Some(link.clone());

        let t0 = Instant::now();
        let mut shown = false;
        loop {
            if !self.alive() || !self.setup_is(setup) {
                return Err(String::new());
            }
            match self.post(&link, RATES, json!({}), false, Duration::from_secs(8)).await {
                Ok(data) => {
                    if shown {
                        link.bridge.show(false);
                    }
                    self.apply_rates(&data);
                    return Ok(());
                }
                Err(CallError::Cloudflare) => {
                    if manual.is_some() && t0.elapsed() > SHOW_CHECK_AFTER {
                        return Err(format!(
                            "Cloudflare didn't accept your cookie on {mirror}. Copy a fresh cookie and the User Agent from the same browser \
                             on this computer, or remove the cookie."
                        ));
                    }
                    if t0.elapsed() > CHECK_TIMEOUT {
                        return Err(format!(
                            "Cloudflare didn't let us through on {mirror}. Try another mirror, or set the Cloudflare cookie in Settings. \
                             On a server (VPS), Cloudflare may not trust its IP address at all."
                        ));
                    }
                    if manual.is_none() && !shown && t0.elapsed() > SHOW_CHECK_AFTER {
                        link.bridge.show(true);
                        shown = true;
                    }
                    let msg = match (shown, manual.is_some()) {
                        (true, _) => "Finish the Cloudflare check in the window that opened",
                        (false, true) => "Cloudflare didn't take your cookie, passing the check…",
                        (false, false) => "Passing the Cloudflare check…",
                    };
                    self.status(ConnStatus::Connecting, msg);
                }
                Err(CallError::Blocked) => return Err(format!("{mirror} is blocked from your location. Pick another mirror.")),
                Err(CallError::Casino(CasinoError::ConnectionLost)) => return Err(format!("The connection to {mirror} was closed")),
                // a challenge page navigating away drops the request; just ask again
                Err(CallError::Casino(e)) => {
                    if t0.elapsed() > Duration::from_secs(60) {
                        return Err(e.to_string());
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    /// Signs in and keeps balances fresh until the bridge breaks or the mirror or cookie changes.
    async fn serve(&self, setup: u64) {
        self.authenticate().await;
        let mut rates_at = Instant::now();
        loop {
            tokio::select! {
                _ = self.wake.notified() => {
                    if !self.alive() || !self.setup_is(setup) {
                        return;
                    }
                    self.authenticate().await;
                }
                _ = self.broken.notified() => {
                    // a permit left over from an earlier bridge is not a reason to rebuild this one
                    if self.broken_flag.swap(false, Ordering::SeqCst) {
                        return;
                    }
                }
                _ = tokio::time::sleep(REFRESH) => {
                    if rates_at.elapsed() > RATES_EVERY {
                        rates_at = Instant::now();
                        if let Ok(data) = self.gql(RATES, json!({}), false).await {
                            self.apply_rates(&data);
                        }
                    }
                    if self.ready.load(Ordering::SeqCst) {
                        self.refresh_me().await;
                    }
                }
            }
            if !self.alive() {
                return;
            }
        }
    }

    fn drop_link(&self) {
        self.ready.store(false, Ordering::SeqCst);
        if let Some(link) = self.link.lock().take() {
            link.bridge.close();
        }
    }

    async fn authenticate(&self) {
        if self.token.lock().is_none() {
            self.status(ConnStatus::NoToken, "");
            return;
        }
        self.status(ConnStatus::Authenticating, "");
        match self.gql(ME, json!({}), true).await {
            Ok(data) => match data.get("user").filter(|u| u.is_object()) {
                Some(user) => {
                    self.apply_me(user, true);
                    self.status(ConnStatus::Ready, "");
                    let _ = self.events.send(CasinoEvent::Provable(provable_from(user)));
                }
                None => self.status(ConnStatus::AuthFailed, "Stake returned no account for this token"),
            },
            Err(CallError::Casino(CasinoError::Server(m))) => self.status(ConnStatus::AuthFailed, m),
            // the bridge broke; serve() is about to rebuild it
            Err(e) => self.status(ConnStatus::Offline, CasinoError::from(e).to_string()),
        }
    }

    /// Re-reads the account. Balances are only taken when no bet ran meanwhile,
    /// so a reply older than the latest local update never wins.
    async fn refresh_me(&self) {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        match self.gql(ME, json!({}), true).await {
            Ok(data) => {
                if let Some(user) = data.get("user").filter(|u| u.is_object()) {
                    let fresh = idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled;
                    self.apply_me(user, fresh);
                }
            }
            Err(CallError::Casino(CasinoError::Server(m))) if m == INVALID_TOKEN => self.status(ConnStatus::AuthFailed, m),
            Err(_) => {}
        }
    }

    fn apply_me(&self, user: &Value, balances: bool) {
        {
            let mut m = self.market.lock();
            m.username = user.get("name").and_then(Value::as_str).map(str::to_string);
            if balances {
                if let Some(list) = user.get("balances").and_then(Value::as_array) {
                    m.wallets.clear();
                    for b in list {
                        let a = &b["available"];
                        if let (Some(cur), Some(amount)) = (a.get("currency").and_then(Value::as_str), a.get("amount").and_then(coin_units)) {
                            m.wallets.insert(cur.to_string(), amount);
                        }
                    }
                }
                // Stake keeps the selected coin in the browser, not the account; start on the biggest balance
                if m.site_currency.is_none() {
                    m.site_currency = richest(&m);
                }
            }
        }
        self.publish();
    }

    fn apply_rates(&self, data: &Value) {
        let Some(list) = data.pointer("/info/currencies").and_then(Value::as_array) else { return };
        {
            let mut m = self.market.lock();
            for c in list {
                let Some(name) = c.get("name").and_then(Value::as_str) else { continue };
                let price = c.get("usd").and_then(f64_from_json).unwrap_or(0.0);
                m.currencies.insert(name.to_string(), CurrencyInfo { decimals: DECIMALS, price, hidden: false });
            }
        }
        self.publish();
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

    async fn post(&self, link: &Link, query: &str, variables: Value, auth: bool, timeout: Duration) -> Result<Value, CallError> {
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if auth {
            let token = self.token.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
            headers.push(("x-access-token".into(), token));
        }
        let req = BridgeRequest {
            method: "POST".into(),
            url: format!("{}/_api/graphql", link.origin),
            headers,
            body: Some(json!({ "query": query, "variables": variables }).to_string()),
        };
        let res = link.bridge.request_within(req, timeout).await.map_err(|e| {
            CallError::Casino(if e.contains("timeout") { CasinoError::Timeout } else { CasinoError::ConnectionLost })
        })?;
        parse_reply(res.status, &res.body)
    }

    /// One API call over the current bridge. A dead or challenged bridge is flagged for the run loop.
    async fn gql(&self, query: &str, variables: Value, auth: bool) -> Result<Value, CallError> {
        let link = self.link.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
        let res = self.post(&link, query, variables, auth, RPC_TIMEOUT).await;
        let broken = matches!(res, Err(CallError::Cloudflare) | Err(CallError::Blocked) | Err(CallError::Casino(CasinoError::ConnectionLost)));
        if broken && link.gen == self.link_gen.load(Ordering::SeqCst) {
            self.ready.store(false, Ordering::SeqCst);
            self.broken_flag.store(true, Ordering::SeqCst);
            self.broken.notify_one();
        }
        res
    }
}

#[async_trait]
impl Casino for Stake {
    fn id(&self) -> CasinoId {
        CasinoId::Stake
    }

    fn set_token(&self, token: Option<String>) {
        *self.token.lock() = clean_token(token);
        self.wake.notify_one();
    }

    fn set_mirror(&self, mirror: &str) {
        let m = normalize_mirror(mirror);
        if m.is_empty() {
            return;
        }
        {
            let mut cur = self.mirror.lock();
            if *cur == m {
                return;
            }
            *cur = m;
            self.setup.fetch_add(1, Ordering::SeqCst);
        }
        self.wake.notify_one();
    }

    fn set_clearance(&self, clearance: Option<Clearance>) {
        let mirror = self.mirror.lock().clone();
        let before = self.clearance_for(&mirror);
        *self.clearance.lock() = clearance.filter(Clearance::is_complete);
        // a cookie for another mirror changes nothing about the bridge in use
        if self.clearance_for(&mirror) != before {
            self.setup.fetch_add(1, Ordering::SeqCst);
            self.wake.notify_one();
        }
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.drop_link();
        self.wake.notify_one();
    }

    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        struct InFlight<'a>(&'a AtomicUsize);
        impl Drop for InFlight<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let _inflight = InFlight(&self.inflight);

        let vars = json!({
            "amount": to_f64(req.amount, DECIMALS),
            "multiplierTarget": req.target,
            "currency": req.currency.to_ascii_lowercase(),
            "identifier": identifier(),
        });
        let data = self.gql(LIMBO, vars, true).await?;
        let bet = data.get("limboBet").filter(|b| b.is_object()).ok_or_else(|| CasinoError::BadReply(data.to_string()))?;
        let outcome = bet.pointer("/state/result").and_then(f64_from_json).ok_or_else(|| CasinoError::BadReply(bet.to_string()))?;
        let amount = bet.get("amount").and_then(coin_units);
        let payout = bet.get("payout").and_then(coin_units);
        if let (Some(a), Some(p)) = (amount, payout) {
            let cur = bet.get("currency").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| req.currency.to_ascii_lowercase());
            self.credit(&cur, p - a);
        }
        self.settled.fetch_add(1, Ordering::SeqCst);
        Ok(BetReply {
            outcome,
            amount,
            winnings: payout,
            win: bet.get("payoutMultiplier").and_then(f64_from_json).map(|m| m > 0.0),
            nonce: bet.get("nonce").and_then(Value::as_u64),
        })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let data = self.gql(ROTATE, json!({ "seed": client_seed }), true).await?;
        let user = data.pointer("/rotateSeedPair/clientSeed/user").ok_or_else(|| CasinoError::BadReply(data.to_string()))?;
        let mut st = provable_from(user);
        if st.client_seed.is_none() {
            st.client_seed = Some(client_seed.to_string());
        }
        if st.nonce.is_none() {
            st.nonce = Some(0);
        }
        Ok(st)
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        let data = self.gql(ME, json!({}), true).await?;
        let user = data.get("user").filter(|u| u.is_object()).ok_or_else(|| CasinoError::BadReply(data.to_string()))?;
        Ok(provable_from(user))
    }

    async fn list_slots(&self, _filter: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Err(CasinoError::Server("Slots aren't available on Stake yet".into()))
    }

    async fn launch_slot(&self, _catalog_id: &str, _demo: bool, _currency: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots aren't available on Stake yet".into()))
    }
}

/// Stake answers `{data, errors}`; anything that isn't JSON came from Cloudflare or a block page.
fn parse_reply(status: u16, body: &str) -> Result<Value, CallError> {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            if body.contains("ip-blocked") {
                return Err(CallError::Blocked);
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
        }
    };
    if let Some(e) = v.get("errors").and_then(Value::as_array).and_then(|a| a.first()) {
        let msg = e.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string();
        let kind = e.get("errorType").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
        return Err(CallError::Casino(match kind.as_str() {
            "invalidsession" => CasinoError::Server(INVALID_TOKEN.into()),
            k if k.contains("parallel") || k.contains("ratelimit") || k.contains("toomany") => CasinoError::Busy(msg),
            _ => CasinoError::Server(msg),
        }));
    }
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

fn provable_from(user: &Value) -> ProvableState {
    ProvableState {
        client_seed: user.pointer("/activeClientSeed/seed").and_then(Value::as_str).map(str::to_string),
        server_seed_hash: user.pointer("/activeServerSeed/seedHash").and_then(Value::as_str).map(str::to_string),
        nonce: user.pointer("/activeServerSeed/nonce").and_then(Value::as_u64),
    }
}

/// 0.00012345 -> 12345 base units.
fn coin_units(v: &Value) -> Option<Units> {
    let f = f64_from_json(v)?;
    if !f.is_finite() || f < 0.0 {
        return None;
    }
    parse_units(&format!("{f:.8}"), DECIMALS)
}

/// The coin holding the most value, else BTC.
fn richest(m: &Market) -> Option<String> {
    let usd = |cur: &str, v: Units| to_f64(v, DECIMALS) * m.info(cur).map(|i| i.price).unwrap_or(0.0);
    m.wallets
        .iter()
        .map(|(c, v)| (c, usd(c, *v)))
        .filter(|(_, u)| *u > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(c, _)| c.clone())
        .or_else(|| m.wallets.contains_key("btc").then(|| "btc".to_string()))
        .or_else(|| m.wallets.keys().next().cloned())
}

/// Per-bet idempotency key, like the site's own (21 url-safe characters).
fn identifier() -> String {
    use rand::Rng;
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789_-";
    let mut rng = rand::rng();
    (0..21).map(|_| ABC[rng.random_range(0..ABC.len())] as char).collect()
}

fn clean_token(token: Option<String>) -> Option<String> {
    token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirrors_are_unique_bare_hosts() {
        let mut seen = std::collections::HashSet::new();
        for m in MIRRORS {
            assert_eq!(normalize_mirror(m), *m, "{m} is not a bare lowercase host");
            assert!(seen.insert(*m), "{m} listed twice");
        }
        assert_eq!(MIRRORS[0], DEFAULT_MIRROR);
        assert_eq!(normalize_mirror(" https://Stake.BET/ "), "stake.bet");
    }

    #[test]
    fn replies() {
        let ok = parse_reply(200, r#"{"data":{"limboBet":{"amount":0.0001,"payout":0.0002}}}"#).ok().unwrap();
        assert_eq!(ok.pointer("/limboBet/amount").and_then(coin_units), Some(10_000));
        assert_eq!(ok.pointer("/limboBet/payout").and_then(coin_units), Some(20_000));

        let bad = r#"{"errors":[{"path":["user"],"message":"Your session is invalid. Please login again.","errorType":"invalidSession"}],"data":{"user":null}}"#;
        assert!(matches!(parse_reply(200, bad), Err(CallError::Casino(CasinoError::Server(m))) if m == INVALID_TOKEN));

        let busy = r#"{"errors":[{"message":"Slow down","errorType":"parallelCasinoBet"}],"data":null}"#;
        assert!(matches!(parse_reply(200, busy), Err(CallError::Casino(e)) if e.is_busy()));

        assert!(matches!(parse_reply(403, "<html><title>Just a moment...</title>"), Err(CallError::Cloudflare)));
        assert!(matches!(parse_reply(403, "<!-- saved from url=(0041)https://stake-errors.pages.dev/ip-blocked -->"), Err(CallError::Blocked)));
    }

    #[test]
    fn amounts_round_trip() {
        for units in [1, 10_000, 123_456_789, 100_000_000, 2_500_000_000_000] {
            let sent = to_f64(units, DECIMALS);
            assert_eq!(coin_units(&json!(sent)), Some(units), "{units}");
        }
    }
}
