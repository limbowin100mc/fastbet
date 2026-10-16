//! Stake over its GraphQL API (`/_api/graphql`), signed in with the user's
//! API token in the `x-access-token` header.
//!
//! Stake's domains sit behind Cloudflare checks that turn plain HTTP clients
//! away (stake.com answers them with its "blocked" page), so every call runs
//! through the browser bridge parked on the chosen mirror, same-origin with
//! the API. A fresh bridge clears the check by itself in about 15 s; if the
//! check wants a click, the bridge window is shown for the user.
//!
//! When it never passes, the user copies their own browser's User Agent and
//! cookies for the mirror (see [`crate::cloudflare`]). The app then talks to
//! Stake directly, as that browser: plain HTTPS with the same User Agent and
//! cookies is let through (measured: `__cf_bm` plus its User Agent turns a 403
//! into a 200). If Cloudflare refuses that, the bridge tries the same values.
//! A refused cookie is reported, not turned into a check window: that window
//! would claim to be the user's browser, and such a check doesn't pass.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::bridge::{Bridge, BridgeCookie, BridgeRequest, BridgeSetup};
use crate::originals::{Game, Resume, Round, Step, Tables};
use crate::cloudflare::Clearance;
use crate::currency::is_fiat;
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

/// The VIP flag and how far to the next one (0–1), asked on its own so it can never break the account query.
const VIP: &str = "query FastbetVip { user { id flagProgress { flag progress } } }";
/// Stake's VIP flags, lowest first: (flag, name, dollars wagered it starts at).
const FLAGS: &[(&str, &str, f64)] = &[
    ("none", "Unranked", 0.0),
    ("bronze", "Bronze", 10e3),
    ("silver", "Silver", 50e3),
    ("gold", "Gold", 100e3),
    ("platinum", "Platinum I", 250e3),
    ("wagered(500k)", "Platinum II", 500e3),
    ("wagered(1m)", "Platinum III", 1e6),
    ("wagered(2.5m)", "Platinum IV", 2.5e6),
    ("wagered(5m)", "Platinum V", 5e6),
    ("wagered(10m)", "Platinum VI", 10e6),
    ("wagered(25m)", "Diamond I", 25e6),
    ("wagered(50m)", "Diamond II", 50e6),
    ("wagered(100m)", "Diamond III", 100e6),
    ("wagered(250m)", "Diamond IV", 250e6),
    ("wagered(500m)", "Diamond V", 500e6),
    ("wagered(1000m)", "Obsidian I", 1e9),
    ("wagered(2500m)", "Obsidian II", 2.5e9),
    ("wagered(5000m)", "Opal I", 5e9),
    ("wagered(10000m)", "Opal II", 10e9),
    ("wagered(25000m)", "Plutonium", 25e9),
];
/// Rakeback, the reload and the weekly / monthly bonuses waiting.
const REWARDS: &str = "query FastbetRewards { claimableSelfServeBonuses { bonusCodeId code category availableAt } \
    user { id rakeback { enabled balances { currency availableAmount } } reload: faucet { id active claimInterval lastClaim expireAt value } } }";
const CLAIM_RAKEBACK: &str = "mutation FastbetClaimRakeback { claimRakeback { id currency amount } }";
/// The open challenges (Stake refuses the list in some regions).
const CHALLENGES: &str = "query FastbetChallenges { challengeGlobalList(limit: 50, offset: 0, sort: prize) { id active targetMultiplier minBetUsd award currency \
    expireAt game { name slug thumbnailUrl providerGroup { name } } } }";
/// Refresh ticks between VIP reads.
const VIP_EVERY: u32 = 3;
const ME: &str = "query FastbetMe { user { id name balances { available { amount currency } } \
    activeClientSeed { seed } activeServerSeed { seedHash nonce } } }";
// The site's own currency setup: the USD rate of every coin and fiat (`baseRate`), the fiats it can
// show amounts in, and the fiats it has launched as wallets.
const RATES: &str = "query FastbetRates { currencyConfiguration(isAcp: false) { baseRates { currency baseRate } \
    launchedFiatCurrencies displayFiatCurrencies } }";
/// Stake.us sweepstakes coins: in the rates, but not money on stake.com.
const SOCIAL: &[&str] = &["gold", "sweeps"];
const LIMBO: &str = "mutation FastbetLimbo($amount: Float!, $multiplierTarget: Float!, $currency: CurrencyEnum!, $identifier: String!) { \
    limboBet(amount: $amount, currency: $currency, multiplierTarget: $multiplierTarget, identifier: $identifier) { \
    id nonce payoutMultiplier amount payout currency state { ... on CasinoGameLimbo { result multiplierTarget } } } }";
const ROTATE: &str = "mutation FastbetRotateSeed($seed: String!) { rotateSeedPair(seed: $seed) { clientSeed { user { id \
    activeClientSeed { id seed } activeServerSeed { id nonce seedHash nextSeedHash } } } } }";

// Slots: Stake Engine games only. They sit in one kurator group; a session is a game URL
// carrying the RGS session (see `slots::stake_engine`).
const ENGINE_GROUP: &str = "stake-engine";
const SLOTS_LIST: &str = "query FastbetEngineGames($slug: String!, $limit: Int!, $offset: Int!) { slugKuratorGroup(slug: $slug) { id \
    groupGamesList(limit: $limit, offset: $offset, sort: popular7d) { game { id name slug thumbnailUrl edge isDemoEnabled \
    providerGroup { name translation } } } } }";
const SLOTS_SEARCH: &str = "query FastbetEngineSearch($query: String!, $groupIds: [String!], $limit: Int, $offset: Int) { \
    kuratorGameQuery(query: $query, groupIds: $groupIds, limit: $limit, offset: $offset) { id name slug thumbnailUrl edge \
    isDemoEnabled providerGroup { name translation } groupGames { group { slug } } } }";
const ENGINE_GROUP_ID: &str = "query FastbetEngineGroup($slug: String!) { slugKuratorGroup(slug: $slug) { id } }";
const START_SLOT: &str = "mutation FastbetStartSlot($slug: String!, $source: CurrencyEnum!, $target: CurrencyEnum!) { \
    startThirdPartySession(slug: $slug, source: $source, target: $target) { config } }";
const START_DEMO: &str = "mutation FastbetStartDemo($slug: String!) { startThirdPartyDemoSession(slug: $slug) { config } }";
/// Stake Engine games run in fiat; Stake converts the bets from the coin the session is started with.
/// The player's display fiat (Stake takes any of its own: EUR, JPY and RUB measured), else USD.
const SLOT_FIAT: &str = "usd";
/// Thumbnail size for the lobby tiles (2x the tile), as JPEG for the image decoder.
const THUMB: &str = "w=264&h=346&fit=min&fm=jpg&q=85";

// Originals: the site posts them to `/_api/casino/<game>/<action>` (REST twins of the GraphQL
// mutations, same bodies); Flip only exists as GraphQL.
const ORIGINALS_GROUP: &str = "stake-originals";
const ORIGINALS_LIST: &str = "query FastbetOriginals($slug: String!) { slugKuratorGroup(slug: $slug) { \
    groupGamesList(limit: 50, offset: 0) { game { id name slug thumbnailUrl edge isDemoEnabled providerGroup { name translation } } } } }";
const TABLES: &str = "query FastbetTables { info { kenoPayouts { payouts risk } wheelPayouts { payouts risk rows } \
    plinkoPayouts { payouts rows risk } } }";
const FLIP_FIELDS: &str = "id active currency amountMultiplier payoutMultiplier amount payout \
    state { ... on CasinoGameFlip { currentRound payoutMultiplier playedRounds flips } }";

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

/// How calls reach Stake.
#[derive(Clone)]
enum Transport {
    /// The hidden browser window, parked on the mirror.
    Bridge(Bridge),
    /// Plain HTTPS carrying the User Agent and cookies of the user's own browser.
    Direct(reqwest::Client),
}

impl Transport {
    fn close(&self) {
        if let Transport::Bridge(b) = self {
            b.close();
        }
    }

    fn show(&self, on: bool) {
        if let Transport::Bridge(b) = self {
            b.show(on);
        }
    }
}

/// The way in use, with the origin it reaches.
#[derive(Clone)]
struct Link {
    gen: u64,
    transport: Transport,
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
    last_vip: Mutex<Option<VipStatus>>,
    /// Bets in flight and bets settled, so a balance poll never overwrites a fresher local balance.
    inflight: AtomicUsize,
    settled: AtomicU64,
    /// Id of the Stake Engine game group, for searching inside it.
    engine_group: Mutex<Option<String>>,
    /// Open rounds of originals and what they have staked so far (already taken off the wallet).
    open_rounds: Mutex<std::collections::HashMap<String, Units>>,
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
            last_vip: Mutex::new(None),
            inflight: AtomicUsize::new(0),
            settled: AtomicU64::new(0),
            engine_group: Mutex::new(None),
            open_rounds: Mutex::new(Default::default()),
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
        // a paste with only the login leaves Cloudflare to the app's own check
        let manual = self.clearance_for(mirror).filter(Clearance::has_cloudflare);
        // the user's browser first: straight to Stake with its User Agent and cookies
        if let Some(c) = &manual {
            self.status(ConnStatus::Connecting, format!("Opening {mirror} as your browser"));
            match self.connect_direct(mirror, c).await {
                Ok(()) => return Ok(()),
                Err(Some(e)) => return Err(e),
                Err(None) => self.status(ConnStatus::Connecting, "Cloudflare didn't take the cookies directly, trying them in the app's browser…"),
            }
        }
        let bridge = match &manual {
            Some(c) => {
                let cookies = c.cookies().into_iter().map(|(name, value)| BridgeCookie { name, value, domain: format!(".{mirror}") }).collect();
                let setup = BridgeSetup { user_agent: Some(c.user_agent.clone()), cookies };
                Bridge::open_with(&page, Some(PROFILE_MANUAL), &setup).await?
            }
            None => Bridge::open_profile(&page, Some(PROFILE)).await?,
        };
        let link = Link { gen: self.link_gen.fetch_add(1, Ordering::SeqCst) + 1, transport: Transport::Bridge(bridge), origin };
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
                        link.transport.show(false);
                    }
                    self.apply_rates(&data);
                    return Ok(());
                }
                Err(CallError::Cloudflare) => {
                    if manual.is_some() && t0.elapsed() > SHOW_CHECK_AFTER {
                        return Err(format!(
                            "Cloudflare didn't accept your browser's cookies on {mirror}. Open {mirror} in that browser, then copy its cookies \
                             and User Agent again (they expire, and only work from this computer's IP), or remove them."
                        ));
                    }
                    if t0.elapsed() > CHECK_TIMEOUT {
                        return Err(format!(
                            "Cloudflare didn't let us through on {mirror}. Try another mirror, or log in with your browser in Settings. \
                             On a server (VPS), Cloudflare may not trust its IP address at all."
                        ));
                    }
                    if manual.is_none() && !shown && t0.elapsed() > SHOW_CHECK_AFTER {
                        link.transport.show(true);
                        shown = true;
                    }
                    let msg = match (shown, manual.is_some()) {
                        (true, _) => "Finish the Cloudflare check in the window that opened",
                        (false, true) => "Cloudflare didn't take your cookies, passing the check…",
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
        let mut ticks: u32 = 0;
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
                        ticks += 1;
                        if ticks % VIP_EVERY == 0 {
                            self.refresh_vip().await;
                        }
                    }
                }
            }
            if !self.alive() {
                return;
            }
        }
    }

    /// Reads the VIP flag and its progress.
    async fn refresh_vip(&self) {
        match self.gql(VIP, json!({}), true).await {
            Ok(data) => match data.pointer("/user/flagProgress").and_then(vip_status) {
                Some(vip) => send_vip(&self.events, &self.last_vip, vip),
                None => tracing::info!("stake: no vip progress in {}", data.to_string().chars().take(200).collect::<String>()),
            },
            Err(e) => tracing::info!("stake: vip progress: {}", CasinoError::from(e)),
        }
    }

    /// A coin's USD price as the site gives it; dollar coins count as a dollar until prices are in.
    fn usd_price(&self, cur: &str) -> f64 {
        let cur = cur.to_ascii_lowercase();
        let price = self.market.lock().currencies.get(&cur).map(|i| i.price).unwrap_or(0.0);
        if price > 0.0 {
            price
        } else if matches!(cur.as_str(), "usdt" | "usdc" | "usd") {
            1.0
        } else {
            0.0
        }
    }

    fn drop_link(&self) {
        self.ready.store(false, Ordering::SeqCst);
        if let Some(link) = self.link.lock().take() {
            link.transport.close();
        }
    }

    /// Connects without the bridge, as the user's browser. `Err(None)`: Cloudflare (or the
    /// network) refused it, worth trying the bridge; `Err(Some(_))`: no point going on.
    async fn connect_direct(&self, mirror: &str, c: &Clearance) -> Result<(), Option<String>> {
        let client = direct_client(mirror, c).map_err(Some)?;
        let link = Link { gen: self.link_gen.fetch_add(1, Ordering::SeqCst) + 1, transport: Transport::Direct(client), origin: format!("https://{mirror}") };
        self.broken_flag.store(false, Ordering::SeqCst);
        *self.link.lock() = Some(link.clone());
        match self.post(&link, RATES, json!({}), false, Duration::from_secs(15)).await {
            Ok(data) => {
                tracing::info!("stake: connected directly with the browser's cookies");
                self.apply_rates(&data);
                Ok(())
            }
            Err(CallError::Blocked) => Err(Some(format!("{mirror} is blocked from your location. Pick another mirror."))),
            Err(e) => {
                tracing::info!("stake: direct connection refused: {}", CasinoError::from(e));
                self.link.lock().take();
                Err(None)
            }
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
                    self.refresh_vip().await;
                    // FASTBET_STAKE_GQL=<file>: run its queries (split by lines of ---) once logged in and log the replies (dev)
                    if let Ok(file) = std::env::var("FASTBET_STAKE_GQL") {
                        for q in std::fs::read_to_string(file).unwrap_or_default().split("\n---") {
                            let q = q.trim();
                            if q.is_empty() {
                                continue;
                            }
                            let reply = self.gql(q, json!({}), true).await.map_err(CasinoError::from);
                            tracing::info!("stake gql {} -> {}", q.chars().take(60).collect::<String>(), format!("{reply:?}").chars().take(3000).collect::<String>());
                        }
                    }
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
        let Some(cfg) = data.get("currencyConfiguration") else { return };
        let Some(list) = cfg.get("baseRates").and_then(Value::as_array) else { return };
        let codes = |key: &str| -> Vec<String> {
            let list = cfg.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            sorted(list.iter().filter_map(Value::as_str).map(str::to_ascii_lowercase).collect())
        };
        let (launched, display) = (codes("launchedFiatCurrencies"), codes("displayFiatCurrencies"));
        {
            let mut m = self.market.lock();
            for c in list {
                let Some(name) = c.get("currency").and_then(Value::as_str) else { continue };
                let price = c.get("baseRate").and_then(f64_from_json).unwrap_or(0.0);
                // the rates cover ~150 fiats; the wallet lists only coins, and fiats the player holds
                let fiat = is_fiat(name) || launched.iter().chain(&display).any(|f| f == name);
                let hidden = SOCIAL.contains(&name);
                m.currencies.insert(name.to_string(), CurrencyInfo { decimals: DECIMALS, price, hidden, fiat });
            }
            if !display.is_empty() {
                m.fiats = display;
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
        let body = json!({ "query": query, "variables": variables }).to_string();
        let (status, text) = self.send(link, "/_api/graphql", body, auth, timeout).await?;
        parse_reply(status, &text)
    }

    /// POSTs `body` to `path` on the mirror, through the bridge or directly.
    async fn send(&self, link: &Link, path: &str, body: String, auth: bool, timeout: Duration) -> Result<(u16, String), CallError> {
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string())];
        if auth {
            let token = self.token.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
            headers.push(("x-access-token".into(), token));
        }
        let url = format!("{}{path}", link.origin);
        match &link.transport {
            Transport::Bridge(bridge) => {
                let req = BridgeRequest { method: "POST".into(), url, headers, body: Some(body) };
                let res = bridge.request_within(req, timeout).await.map_err(|e| {
                    CallError::Casino(if e.contains("timeout") { CasinoError::Timeout } else { CasinoError::ConnectionLost })
                })?;
                Ok((res.status, res.body))
            }
            Transport::Direct(client) => {
                let mut req = client.post(url).timeout(timeout).body(body);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
                let res = req.send().await.map_err(lost)?;
                let status = res.status().as_u16();
                Ok((status, res.text().await.map_err(lost)?))
            }
        }
    }

    /// One REST call of the originals API (`/_api/casino/<path>`), signed in.
    async fn rest(&self, path: &str, body: Value) -> Result<Value, CallError> {
        let link = self.link.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
        let res = match self.send(&link, &format!("/_api/casino/{path}"), body.to_string(), true, RPC_TIMEOUT).await {
            Ok((status, text)) => parse_rest(status, &text),
            Err(e) => Err(e),
        };
        self.flag_broken(&link, &res);
        res
    }

    /// A call found the bridge dead or challenged: the run loop rebuilds it.
    fn flag_broken<T>(&self, link: &Link, res: &Result<T, CallError>) {
        let broken = matches!(res, Err(CallError::Cloudflare) | Err(CallError::Blocked) | Err(CallError::Casino(CasinoError::ConnectionLost)));
        if broken && link.gen == self.link_gen.load(Ordering::SeqCst) {
            self.ready.store(false, Ordering::SeqCst);
            self.broken_flag.store(true, Ordering::SeqCst);
            self.broken.notify_one();
        }
    }

    /// Books an original's reply into the local wallet: new stake comes off when the
    /// casino takes it, the payout goes on when the round settles.
    fn book(&self, r: &Round) {
        let staked = r.stake();
        let before = {
            let mut open = self.open_rounds.lock();
            let before = open.get(&r.id).copied().unwrap_or(0);
            if r.active {
                open.insert(r.id.clone(), staked);
            } else {
                open.remove(&r.id);
            }
            before
        };
        let mut delta = before - staked;
        if !r.active {
            delta += r.payout;
            self.settled.fetch_add(1, Ordering::SeqCst);
        }
        self.credit(&r.currency, delta);
    }

    /// A round from an original's reply (`{"minesBet": {...}}` or the bet object itself).
    fn round_of(game: Game, v: &Value) -> Result<Round, CasinoError> {
        let bet = if v.get("id").is_some() { Some(v) } else { v.as_object().and_then(|o| o.values().find(|x| x.is_object())) };
        bet.and_then(|b| round_from(game, b)).ok_or_else(|| CasinoError::BadReply(v.to_string().chars().take(200).collect()))
    }

    /// One API call over the current bridge. A dead or challenged bridge is flagged for the run loop.
    /// The Stake Engine group's id, looked up once.
    async fn engine_group_id(&self) -> Result<String, CasinoError> {
        if let Some(id) = self.engine_group.lock().clone() {
            return Ok(id);
        }
        let data = self.gql(ENGINE_GROUP_ID, json!({ "slug": ENGINE_GROUP }), false).await?;
        let id = data.pointer("/slugKuratorGroup/id").and_then(Value::as_str).ok_or_else(|| CasinoError::BadReply(data.to_string()))?.to_string();
        *self.engine_group.lock() = Some(id.clone());
        Ok(id)
    }

    async fn gql(&self, query: &str, variables: Value, auth: bool) -> Result<Value, CallError> {
        let link = self.link.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
        let res = self.post(&link, query, variables, auth, RPC_TIMEOUT).await;
        self.flag_broken(&link, &res);
        res
    }
}

#[async_trait]
impl Casino for Stake {
    fn id(&self) -> CasinoId {
        CasinoId::Stake
    }

    /// Rakeback claims here; the reload and the weekly / monthly bonuses only on Stake itself,
    /// which checks with Cloudflare Turnstile that a person claims them.
    async fn rewards(&self) -> Result<Vec<Reward>, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let data = self.gql(REWARDS, json!({}), true).await?;
        let link = format!("https://{}/?tab=rewards&modal=vip", self.mirror.lock());
        Ok(rewards_of(&data, &|c: &str| self.usd_price(c), SystemTime::now(), &link))
    }

    async fn claim_reward(&self, id: &str, _currency: &str) -> Result<String, CasinoError> {
        if id != "rakeback" {
            return Err(CasinoError::Server("Claim this one on Stake: it checks there that a person claims it".into()));
        }
        let data = self.gql(CLAIM_RAKEBACK, json!({}), true).await?;
        self.refresh_me().await;
        Ok(claimed_text(data.get("claimRakeback")))
    }

    async fn challenges(&self) -> Result<Vec<Challenge>, CasinoError> {
        let data = self.gql(CHALLENGES, json!({}), self.ready.load(Ordering::SeqCst)).await?;
        Ok(challenges_of(&data, &|c: &str| self.usd_price(c)))
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
        let _inflight = InFlight::new(&self.inflight);

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
        // a bet of 0 pays 0 either way: the result against the target decides it
        let win = if req.amount == 0 { Some(outcome >= req.target) } else { bet.get("payoutMultiplier").and_then(f64_from_json).map(|m| m > 0.0) };
        Ok(BetReply {
            outcome,
            amount,
            winnings: payout,
            win,
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

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let stake = to_f64(amount, DECIMALS);
        let mut body = match game {
            // the stake goes on the picked sides, or as chips on the table: no top-level amount
            Game::Baccarat => baccarat_body(&params, stake),
            Game::Roulette => roulette_body(&params, stake),
            _ => {
                let mut b = if params.is_object() { params } else { json!({}) };
                b["amount"] = json!(stake);
                b
            }
        };
        body["currency"] = json!(currency.to_ascii_lowercase());
        body["identifier"] = json!(identifier());
        let reply = if game == Game::Flip {
            let q = format!(
                "mutation FastbetFlip($amount: Float!, $currency: CurrencyEnum!, $identifier: String, $guesses: [FlipConditionEnum!]!) {{ \
                 flipBet(amount: $amount, currency: $currency, identifier: $identifier, guesses: $guesses) {{ {FLIP_FIELDS} }} }}"
            );
            self.gql(&q, body, true).await?
        } else {
            self.rest(game.bet_path(auto), body).await?
        };
        let round = Self::round_of(game, &reply)?;
        self.book(&round);
        Ok(round)
    }

    async fn original_step(&self, game: Game, step: Step, params: Value) -> Result<Round, CasinoError> {
        let _inflight = InFlight::new(&self.inflight);
        let mut body = if params.is_object() { params } else { json!({}) };
        let reply = if game == Game::Flip {
            let q = match step {
                Step::Next => format!("mutation FastbetFlipNext($condition: FlipConditionEnum!) {{ flipNext(condition: $condition) {{ {FLIP_FIELDS} }} }}"),
                Step::Cashout => {
                    body = json!({ "identifier": identifier() });
                    format!("mutation FastbetFlipCashout($identifier: String!) {{ flipCashout(identifier: $identifier) {{ {FLIP_FIELDS} }} }}")
                }
            };
            self.gql(&q, body, true).await?
        } else {
            let path = game.step_path(step).ok_or_else(|| CasinoError::Server(format!("{} has no cash out", game.name())))?;
            if step == Step::Cashout || game.next_takes_identifier() {
                body["identifier"] = json!(identifier());
            }
            self.rest(path, body).await?
        };
        let round = Self::round_of(game, &reply)?;
        self.book(&round);
        Ok(round)
    }

    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        let reply = match game.resume() {
            Resume::Graphql => {
                let q = format!("query FastbetFlipOpen {{ user {{ activeCasinoBet(game: flip) {{ {FLIP_FIELDS} }} }} }}");
                self.gql(&q, json!({}), true).await?
            }
            Resume::ActiveBet => self.rest(&format!("active-bet/{}", game.key()), json!({})).await?,
            // `{"chickenGetState": <bet or null>}`, the same shape as active-bet's
            Resume::GetState(action) => {
                let v = self.rest(&format!("{}/{action}", game.slug()), json!({})).await?;
                let bet = v.as_object().and_then(|o| o.values().next().cloned()).unwrap_or(Value::Null);
                json!({ "user": { "activeCasinoBet": bet } })
            }
        };
        let Some(bet) = reply.pointer("/user/activeCasinoBet").filter(|b| b.is_object()) else { return Ok(None) };
        let round = round_from(game, bet).ok_or_else(|| CasinoError::BadReply(bet.to_string()))?;
        // its stake left the wallet before we saw it, so only remember it for the settle
        if round.active {
            self.open_rounds.lock().insert(round.id.clone(), round.stake());
        }
        Ok(Some(round))
    }

    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        let data = self.gql(TABLES, json!({}), false).await?;
        let info = data.get("info").ok_or_else(|| CasinoError::BadReply(data.to_string()))?;
        let mut t = Tables::default();
        for k in info["kenoPayouts"].as_array().into_iter().flatten() {
            let (Some(risk), Some(p)) = (k["risk"].as_str(), payouts(&k["payouts"])) else { continue };
            t.keno.entry(risk.to_string()).or_default().push(p);
        }
        for (key, table) in [("wheelPayouts", &mut t.wheel), ("plinkoPayouts", &mut t.plinko)] {
            for w in info[key].as_array().into_iter().flatten() {
                let (Some(risk), Some(rows), Some(p)) = (w["risk"].as_str(), w["rows"].as_u64(), payouts(&w["payouts"])) else { continue };
                table.entry(risk.to_string()).or_default().insert(rows.to_string(), p);
            }
        }
        // keno lists come one per number of picks, in order
        for lists in t.keno.values_mut() {
            lists.sort_by_key(Vec::len);
        }
        Ok(t)
    }

    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let data = self.gql(ORIGINALS_LIST, json!({ "slug": ORIGINALS_GROUP }), false).await?;
        let list = data.pointer("/slugKuratorGroup/groupGamesList").and_then(Value::as_array).cloned().unwrap_or_default();
        Ok(list.iter().filter_map(|g| slot_game(&g["game"])).collect())
    }

    async fn list_slots(&self, filter: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        // Stake answers a bigger page with `numberLessEqual`
        let limit = filter.limit.clamp(1, 50);
        let auth = self.ready.load(Ordering::SeqCst);
        let query = filter.query.as_deref().map(str::trim).filter(|q| !q.is_empty());
        let Some(query) = query else {
            let vars = json!({ "slug": ENGINE_GROUP, "limit": limit, "offset": filter.skip });
            let data = self.gql(SLOTS_LIST, vars, auth).await?;
            let group = data.get("slugKuratorGroup").filter(|g| g.is_object()).ok_or_else(|| CasinoError::BadReply(data.to_string()))?;
            if let Some(id) = group.get("id").and_then(Value::as_str) {
                *self.engine_group.lock() = Some(id.to_string());
            }
            let list = group.get("groupGamesList").and_then(Value::as_array).cloned().unwrap_or_default();
            return Ok(list.iter().filter_map(|g| slot_game(&g["game"])).collect());
        };
        let group = self.engine_group_id().await?;
        let vars = json!({ "query": query, "groupIds": [group], "limit": limit, "offset": filter.skip });
        let data = self.gql(SLOTS_SEARCH, vars, auth).await?;
        let list = data.get("kuratorGameQuery").and_then(Value::as_array).cloned().unwrap_or_default();
        // the group filter is the server's; keep only Stake Engine games either way
        let in_group = |g: &&Value| {
            g["groupGames"].as_array().is_none_or(|a| a.iter().any(|x| x.pointer("/group/slug").and_then(Value::as_str) == Some(ENGINE_GROUP)))
        };
        Ok(list.iter().filter(in_group).filter_map(slot_game).collect())
    }

    async fn launch_slot(&self, catalog_id: &str, demo: bool, currency: &str, fiat: &str) -> Result<String, CasinoError> {
        let ready = self.ready.load(Ordering::SeqCst);
        let (key, data) = if demo {
            // demo sessions need no account; signed in, Stake ties them to it
            ("startThirdPartyDemoSession", self.gql(START_DEMO, json!({ "slug": catalog_id }), ready).await?)
        } else {
            if !ready {
                return Err(CasinoError::NotConnected);
            }
            let source = currency.trim().to_ascii_lowercase();
            if source.is_empty() {
                return Err(CasinoError::Server("Pick a currency first".into()));
            }
            // like the site: the game shows the fiat picked for display
            let fiat = fiat.trim().to_ascii_lowercase();
            let target = if fiat.is_empty() { SLOT_FIAT.to_string() } else { fiat };
            let vars = json!({ "slug": catalog_id, "source": source, "target": target });
            ("startThirdPartySession", self.gql(START_SLOT, vars, true).await?)
        };
        data.get(key)
            .and_then(|s| s.get("config"))
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://"))
            .map(str::to_string)
            .ok_or_else(|| CasinoError::BadReply(data.to_string()))
    }
}

/// A Stake Engine game from a kurator listing.
fn slot_game(g: &Value) -> Option<crate::slots::SlotGame> {
    let slug = g.get("slug")?.as_str()?;
    let name = g.get("name")?.as_str()?.trim();
    let pg = &g["providerGroup"];
    // `translation` is sometimes an untranslated key ("game_kurator_group.ovryx_gaming")
    let studio = [&pg["name"], &pg["translation"]]
        .into_iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|s| !s.is_empty() && !s.contains("game_kurator_group"))
        .map(str::to_string);
    let edge = g.get("edge").and_then(f64_from_json).filter(|e| *e > 0.0 && *e < 1.0);
    let image = g
        .get("thumbnailUrl")
        .and_then(Value::as_str)
        .map(|u| format!("{u}{}{THUMB}", if u.contains('?') { '&' } else { '?' }))
        .unwrap_or_default();
    Some(crate::slots::SlotGame {
        id: slug.to_string(),
        name: name.to_string(),
        producer: crate::slots::STAKE_ENGINE.into(),
        image,
        demo: g.get("isDemoEnabled").and_then(Value::as_bool).unwrap_or(true),
        rtp: edge.map(|e| (1.0 - e) * 100.0).unwrap_or(0.0),
        provider_game_id: g.get("id").and_then(Value::as_str).map(str::to_string),
        studio,
    })
}

/// A plain HTTPS client that presents itself as the user's browser: its User Agent, and its
/// cookies for the mirror in a jar that also keeps what Cloudflare and Stake set later.
fn direct_client(mirror: &str, c: &Clearance) -> Result<reqwest::Client, String> {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let jar = Arc::new(browser_jar(mirror, c)?);
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    let origin = HeaderValue::from_str(&format!("https://{mirror}")).map_err(|e| e.to_string())?;
    headers.insert(ORIGIN, origin);
    headers.insert(REFERER, HeaderValue::from_str(&format!("https://{mirror}/")).map_err(|e| e.to_string())?);
    reqwest::Client::builder()
        .user_agent(c.user_agent.clone())
        // header names as browsers write them: Cloudflare turns away lowercase ones over HTTP/1.1
        .http1_title_case_headers()
        .cookie_provider(jar)
        .default_headers(headers)
        .gzip(true)
        .build()
        .map_err(|e| format!("Could not set up the connection: {e}"))
}

/// The pasted cookies, stored the way Cloudflare sets its own (`Domain=<mirror>`), so the fresh
/// `__cf_bm` it hands out every few minutes replaces the pasted one instead of sitting beside it.
fn browser_jar(mirror: &str, c: &Clearance) -> Result<reqwest::cookie::Jar, String> {
    let url: reqwest::Url = format!("https://{mirror}/").parse().map_err(|e| format!("{e}"))?;
    let jar = reqwest::cookie::Jar::default();
    for (name, value) in c.cookies() {
        jar.add_cookie_str(&format!("{name}={value}; Domain={mirror}; Path=/; Secure"), &url);
    }
    Ok(jar)
}

/// Stake answers `{data, errors}`; anything that isn't JSON came from Cloudflare or a block page.
fn parse_reply(status: u16, body: &str) -> Result<Value, CallError> {
    let v = parse_json(status, body)?;
    check_errors(&v)?;
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

/// The originals' REST answers: `{"<game>Bet": {...}}`, or `{errors, data: null}` like GraphQL.
fn parse_rest(status: u16, body: &str) -> Result<Value, CallError> {
    if status == 404 {
        // an unknown route; its HTML page carries Cloudflare's script, which isn't a check
        return Err(CallError::Casino(CasinoError::BadReply("HTTP 404: no such game action".into())));
    }
    let v = parse_json(status, body)?;
    check_errors(&v)?;
    Ok(v)
}

fn parse_json(status: u16, body: &str) -> Result<Value, CallError> {
    match serde_json::from_str(body) {
        Ok(v) => Ok(v),
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
            // an error page (a 502 from Cloudflare …) has nothing to show beyond its status
            if body.trim_start().starts_with('<') {
                return Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status} error page"))));
            }
            let snippet: String = body.chars().take(160).collect();
            Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status}: {snippet}"))))
        }
    }
}

fn check_errors(v: &Value) -> Result<(), CallError> {
    let Some(e) = v.get("errors").and_then(Value::as_array).and_then(|a| a.first()) else { return Ok(()) };
    let msg = e.get("message").and_then(Value::as_str).unwrap_or("Unknown error").to_string();
    let kind = e.get("errorType").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    Err(CallError::Casino(match kind.as_str() {
        "invalidsession" => CasinoError::Server(INVALID_TOKEN.into()),
        "existinggame" => CasinoError::RoundOpen,
        k if k.contains("parallel") || k.contains("ratelimit") || k.contains("toomany") => CasinoError::Busy(msg),
        "insufficientbalance" => CasinoError::Server("Insufficient balance".into()),
        "notfound" => CasinoError::Server("No open round".into()),
        "nocashoutavailable" => CasinoError::Server("Nothing to cash out yet".into()),
        "request_invalid_schema" => CasinoError::Server("Stake refused these bet settings".into()),
        _ => CasinoError::Server(msg),
    }))
}

/// Baccarat's body: the stake on every picked side (`{"sides": ["banker", "tie"]}`).
fn baccarat_body(params: &Value, stake: f64) -> Value {
    let sides: Vec<&str> = params["sides"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    let on = |side: &str| if sides.contains(&side) { stake } else { 0.0 };
    json!({ "player": on("player"), "banker": on("banker"), "tie": on("tie") })
}

/// Roulette's body: chips (`{"chips": {"number17": 2, "colorRed": 1}}`, counts of the stake)
/// grouped the way the site sends them: `{"numbers": [{"value": "number17", "amount": …}], …}`.
fn roulette_body(params: &Value, stake: f64) -> Value {
    let mut body = json!({});
    for (key, count) in params["chips"].as_object().into_iter().flatten() {
        let n = count.as_f64().unwrap_or(0.0);
        if n <= 0.0 {
            continue;
        }
        let group = ["number", "color", "parity", "range", "row"].iter().find(|g| key.starts_with(*g)).map(|g| format!("{g}s"));
        let group = match group.as_deref() {
            Some("paritys") => "parities".to_string(),
            Some(g) => g.to_string(),
            None => continue,
        };
        let list = body[group.as_str()].as_array().cloned().unwrap_or_default();
        let mut list = list;
        list.push(json!({ "value": key, "amount": stake * n }));
        body[group.as_str()] = Value::Array(list);
    }
    body
}

/// A bet object of an original as a [`Round`].
fn round_from(game: Game, b: &Value) -> Option<Round> {
    Some(Round {
        game,
        id: b.get("id")?.as_str()?.to_string(),
        active: b.get("active").and_then(Value::as_bool).unwrap_or(false),
        currency: b.get("currency").and_then(Value::as_str).unwrap_or("").to_string(),
        amount: b.get("amount").and_then(coin_units).unwrap_or(0),
        amount_mult: b.get("amountMultiplier").and_then(f64_from_json).filter(|m| *m > 0.0).unwrap_or(1.0),
        payout: b.get("payout").and_then(coin_units).unwrap_or(0),
        multiplier: b.get("payoutMultiplier").and_then(f64_from_json).unwrap_or(0.0),
        state: b.get("state").cloned().unwrap_or(Value::Null),
    })
}

fn payouts(v: &Value) -> Option<Vec<f64>> {
    v.as_array().map(|a| a.iter().filter_map(f64_from_json).collect())
}

/// Counts a call in flight for as long as it lives, so a balance poll never overwrites a fresher local balance.
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

/// Currency codes in Stake's order; the ones it doesn't rank keep their order after them.
fn sorted(mut codes: Vec<String>) -> Vec<String> {
    codes.sort_by_key(|c| crate::currency::rank(c));
    codes
}

/// The coin holding the most value, else BTC.
/// The reward cards: rakeback (claimed here), the reload and the weekly / monthly bonuses (`link`:
/// claimed on Stake). The reload comes every `claimInterval` ms after the last claim, until it expires.
fn rewards_of(data: &Value, price: &dyn Fn(&str) -> f64, now: SystemTime, link: &str) -> Vec<Reward> {
    let mut out = Vec::new();
    let user = &data["user"];
    if user.pointer("/rakeback/enabled").and_then(Value::as_bool) != Some(false) {
        let coins: Vec<(String, f64)> = user
            .pointer("/rakeback/balances")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|b| Some((b.get("currency")?.as_str()?.to_string(), b.get("availableAmount").and_then(f64_from_json)?)))
            .filter(|(_, a)| *a > 0.0)
            .collect();
        let usd: f64 = coins.iter().map(|(c, a)| a * price(c)).sum();
        let detail = if coins.is_empty() { "Nothing waiting yet".to_string() } else { coins_text(&coins) };
        out.push(Reward { usd: Some(usd), detail, claimable: usd > 0.0, ..Reward::new("rakeback", RewardKind::Rakeback) });
    }
    if let Some(r) = user.get("reload").filter(|r| r.is_object() && r["active"].as_bool() == Some(true)) {
        let expires = r.get("expireAt").and_then(time_of);
        let interval = r.get("claimInterval").and_then(f64_from_json).unwrap_or(0.0);
        let next = r.get("lastClaim").and_then(time_of).map(|t| t + Duration::from_millis(interval.max(0.0) as u64));
        let ready = next.is_none_or(|n| n <= now) && expires.is_none_or(|e| now < e);
        out.push(Reward {
            title: "Reload".into(),
            link: Some(link.to_string()),
            usd: r.get("value").and_then(f64_from_json).filter(|v| *v > 0.0),
            detail: "Claimed on Stake, which checks it's you".into(),
            claimable: ready,
            next_at: if ready { None } else { next },
            expires,
            ..Reward::new("reload", RewardKind::Reload)
        });
    }
    for b in data.get("claimableSelfServeBonuses").and_then(Value::as_array).into_iter().flatten() {
        let (kind, id) = match b.get("category").and_then(Value::as_str) {
            Some("weekly") => (RewardKind::Weekly, "weekly"),
            Some("monthly") => (RewardKind::Monthly, "monthly"),
            _ => (RewardKind::Bonus, "bonus"),
        };
        let at = b.get("availableAt").and_then(time_of);
        let ready = at.is_none_or(|t| t <= now);
        out.push(Reward {
            link: Some(link.to_string()),
            detail: "Claimed on Stake, which checks it's you".into(),
            claimable: ready,
            next_at: if ready { None } else { at },
            ..Reward::new(&format!("{id}:{}", b.get("bonusCodeId").and_then(Value::as_str).unwrap_or("")), kind)
        });
    }
    out
}

/// `challengeGlobalList` -> the active challenges, biggest prize first. The games keep Stake's slug;
/// the lobby knows which of them it can play.
fn challenges_of(data: &Value, price: &dyn Fn(&str) -> f64) -> Vec<Challenge> {
    let list = data.get("challengeGlobalList").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut out: Vec<Challenge> = list
        .iter()
        .filter(|c| c["active"].as_bool() != Some(false))
        .filter_map(|c| {
            let g = &c["game"];
            let studio = g.pointer("/providerGroup/name").and_then(Value::as_str).map(str::to_string);
            let image = g.get("thumbnailUrl").and_then(Value::as_str).map(|u| format!("{u}{}{THUMB}", if u.contains('?') { '&' } else { '?' })).unwrap_or_default();
            let cur = c.get("currency").and_then(Value::as_str).unwrap_or("usdt");
            Some(Challenge {
                id: c.get("id")?.as_str()?.to_string(),
                koth: false,
                game: crate::slots::SlotGame {
                    id: g.get("slug")?.as_str()?.to_string(),
                    name: g.get("name").and_then(Value::as_str).unwrap_or("?").to_string(),
                    producer: studio.clone().unwrap_or_default().to_ascii_lowercase(),
                    image,
                    demo: true,
                    rtp: 0.0,
                    provider_game_id: None,
                    studio,
                },
                target: c.get("targetMultiplier").and_then(f64_from_json)?,
                min_bet_usd: c.get("minBetUsd").and_then(f64_from_json).unwrap_or(0.0),
                prize_usd: c.get("award").and_then(f64_from_json).unwrap_or(0.0) * price(cur),
                ends: c.get("expireAt").and_then(time_of),
                leader: None,
            })
        })
        .collect();
    out.sort_by(|a, b| b.prize_usd.total_cmp(&a.prize_usd));
    out
}

/// `flagProgress { flag progress }` -> the account's VIP status: the progress is how far from
/// the flag's wager to the next one's, so the XP (dollars wagered) is worked out from it.
fn vip_status(p: &Value) -> Option<VipStatus> {
    let flag = p.get("flag").and_then(Value::as_str)?;
    let progress = p.get("progress").and_then(f64_from_json).unwrap_or(0.0).clamp(0.0, 1.0);
    let i = FLAGS.iter().position(|(f, _, _)| *f == flag)?;
    let (_, name, start) = FLAGS[i];
    let next = FLAGS.get(i + 1).map(|(_, n, at)| (n.to_string(), *at));
    let exp = match &next {
        Some((_, at)) => start + progress * (at - start),
        None => start,
    };
    Some(VipStatus { in_dollars: true, ..VipStatus::ranked(name.to_string(), exp, start, next, Some(1.0)) })
}

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
    fn rewards_and_challenges() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_792_148_400);
        let price = |c: &str| if c == "sol" { 200.0 } else { 0.0 };
        // what Stake answered for the user's account (no reload, no bonuses waiting)
        let data = json!({"claimableSelfServeBonuses": [], "user": {"id": "44c4", "rakeback": {"enabled": true, "balances": [
            {"availableAmount": 4.9e-11, "currency": "btc"}, {"availableAmount": 0.000419482735, "currency": "sol"}, {"availableAmount": 0, "currency": "usdt"}]}, "reload": null}});
        let list = rewards_of(&data, &price, now, "https://stake.com/?tab=rewards&modal=vip");
        assert_eq!(list.len(), 1);
        assert!(list[0].claimable && list[0].link.is_none() && (list[0].usd.unwrap() - 0.000419482735 * 200.0).abs() < 1e-9);
        // a reload claimed an hour ago that comes back every hour, and a weekly bonus: both on the site
        let data = json!({"claimableSelfServeBonuses": [{"bonusCodeId": "b1", "code": "x", "category": "weekly", "availableAt": "2026-10-16T10:00:00.000Z"}],
            "user": {"rakeback": {"enabled": false}, "reload": {"id": "r", "active": true, "claimInterval": 3_600_000, "lastClaim": "2026-10-16T10:30:00.000Z", "expireAt": "2026-10-20T00:00:00.000Z", "value": 2.5}}});
        let list = rewards_of(&data, &price, now, "L");
        assert_eq!(list.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["reload", "weekly:b1"]);
        assert!(!list[0].claimable && list[0].next_at == parse_iso("2026-10-16T11:30:00Z") && list[0].usd == Some(2.5) && list[0].link.as_deref() == Some("L"));
        assert!(list[1].claimable && list[1].kind == RewardKind::Weekly);

        let ch = json!({"challengeGlobalList": [{"id": "c1", "active": true, "targetMultiplier": 5000, "minBetUsd": 0.2, "award": 500, "currency": "usdt", "expireAt": "2026-10-20T00:00:00.000Z",
            "game": {"name": "Gates of Olympus", "slug": "pragmatic-play-gates-of-olympus", "thumbnailUrl": "https://mediumrare.imgix.net/x.png", "providerGroup": {"name": "Pragmatic Play"}}}]});
        let list = challenges_of(&ch, &|_: &str| 1.0);
        assert_eq!((list[0].target, list[0].prize_usd, list[0].game.id.as_str()), (5000.0, 500.0, "pragmatic-play-gates-of-olympus"));
        assert!(list[0].ends.is_some() && list[0].game.image.contains("fit=min"));
    }

    #[test]
    fn vip_from_flag_progress() {
        let v = vip_status(&json!({"flag": "wagered(500k)", "progress": 0.25})).unwrap();
        assert_eq!(v.rank, "Platinum II");
        assert_eq!(v.next_rank, Some(("Platinum III".to_string(), 1e6)));
        assert!((v.exp - 625e3).abs() < 1e-6 && (v.rank_progress() - 0.25).abs() < 1e-9);
        assert!((v.usd_for(v.xp_to_rank().unwrap()).unwrap() - 375e3).abs() < 1e-6);
        let none = vip_status(&json!({"flag": "none", "progress": 0.5})).unwrap();
        assert_eq!((none.rank.as_str(), none.next_rank.as_ref().map(|n| n.1)), ("Unranked", Some(10e3)));
        let top = vip_status(&json!({"flag": "wagered(25000m)", "progress": 0.7})).unwrap();
        assert!(top.next_rank.is_none() && top.rank_progress() == 1.0);
        assert!(vip_status(&json!({"flag": "iron"})).is_none());
    }

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
    fn currency_lists_follow_stakes_order() {
        let got = sorted(["ars", "pkr", "usd", "rub", "eur", "nzd"].map(String::from).to_vec());
        assert_eq!(got, ["usd", "eur", "rub", "ars", "pkr", "nzd"]);
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

        // table games build their own bodies
        let b = baccarat_body(&json!({ "sides": ["banker", "tie"] }), 0.5);
        assert_eq!(b, json!({ "player": 0.0, "banker": 0.5, "tie": 0.5 }));
        let r = roulette_body(&json!({ "chips": { "number17": 2, "colorRed": 1, "parityEven": 1, "range0112": 0 } }), 0.25);
        assert_eq!(r["numbers"], json!([{ "value": "number17", "amount": 0.5 }]));
        assert_eq!(r["colors"], json!([{ "value": "colorRed", "amount": 0.25 }]));
        assert_eq!(r["parities"], json!([{ "value": "parityEven", "amount": 0.25 }]));
        assert!(r.get("ranges").is_none());

        // the originals' REST twins
        let open = r#"{"errors":[{"path":["/dragonTower/bet"],"message":"You already have an active DragonTower game.","errorType":"existingGame"}],"data":null}"#;
        assert!(matches!(parse_rest(200, open), Err(CallError::Casino(CasinoError::RoundOpen))));
        assert!(matches!(parse_rest(404, "<pre>Cannot POST</pre><script>challenge-platform</script>"), Err(CallError::Casino(CasinoError::BadReply(_)))));
        let mines = r#"{"minesNext":{"id":"cb51","active":true,"currency":"btc","amountMultiplier":1,"payoutMultiplier":0,"amount":0.0001,"payout":0,"state":{"rounds":[{"field":12,"payoutMultiplier":1.125}],"minesCount":3,"mines":null}}}"#;
        let r = Stake::round_of(Game::Mines, &parse_rest(200, mines).ok().unwrap()).unwrap();
        assert!(r.active && r.amount == 10_000 && r.state["minesCount"] == 3);

        assert!(matches!(parse_reply(403, "<html><title>Just a moment...</title>"), Err(CallError::Cloudflare)));
        assert!(matches!(parse_reply(403, "<!-- saved from url=(0041)https://stake-errors.pages.dev/ip-blocked -->"), Err(CallError::Blocked)));
    }

    #[test]
    fn engine_games() {
        let g = json!({"id": "01a0", "name": "Tongue ", "slug": "twist-gaming-tongue", "thumbnailUrl": "https://mediumrare.imgix.net/858a",
            "edge": 0.033, "isDemoEnabled": true, "providerGroup": {"name": "Twist Gaming", "translation": "Twist Gaming"}});
        let s = slot_game(&g).unwrap();
        assert_eq!((s.id.as_str(), s.name.as_str(), s.studio.as_deref()), ("twist-gaming-tongue", "Tongue", Some("Twist Gaming")));
        assert!((s.rtp - 96.7).abs() < 1e-9);
        assert_eq!(s.image, format!("https://mediumrare.imgix.net/858a?{THUMB}"));
        assert_eq!(crate::slots::Provider::for_producer(&s.producer), Some(crate::slots::Provider::StakeEngine));

        let key = json!({"name": "", "slug": "x", "providerGroup": {"name": "", "translation": "game_kurator_group.ovryx_gaming"}});
        assert_eq!(slot_game(&key).unwrap().studio, None);
    }

    #[test]
    fn browser_cookies_renew() {
        use reqwest::cookie::CookieStore;
        let c = Clearance { mirror: "stake.bet".into(), user_agent: "Mozilla/5.0".into(), cookie: "__cf_bm=old; session=s1".into() };
        let jar = browser_jar("stake.bet", &c).unwrap();
        let url: reqwest::Url = "https://stake.bet/_api/graphql".parse().unwrap();
        let sent = |jar: &reqwest::cookie::Jar| jar.cookies(&url).map(|h| h.to_str().unwrap().to_string()).unwrap_or_default();
        assert!(sent(&jar).contains("__cf_bm=old") && sent(&jar).contains("session=s1"));
        // Cloudflare's renewal, exactly as it came in a reply
        let renewal = reqwest::header::HeaderValue::from_static("__cf_bm=new; HttpOnly; SameSite=None; Secure; Path=/; Domain=stake.bet; Expires=Sun, 11 Oct 2099 13:56:16 GMT");
        jar.set_cookies(&mut std::iter::once(&renewal), &url);
        let now = sent(&jar);
        assert!(now.contains("__cf_bm=new") && !now.contains("__cf_bm=old"), "{now}");
        assert!(now.contains("session=s1"));
    }

    #[test]
    fn error_pages_show_their_status() {
        let page = "<!DOCTYPE html>
<!--[if lt IE 7]> <html class=\"no-js ie6 oldie\" lang=\"en-US\"> <![endif]-->";
        match parse_json(502, page) {
            Err(CallError::Casino(e)) => assert_eq!(e.to_string(), "Bad reply: HTTP 502 error page"),
            _ => panic!("an error page is a bad reply"),
        }
        assert!(matches!(parse_json(429, page), Err(CallError::Casino(CasinoError::Busy(_)))));
    }

    #[test]
    fn amounts_round_trip() {
        for units in [1, 10_000, 123_456_789, 100_000_000, 2_500_000_000_000] {
            let sent = to_f64(units, DECIMALS);
            assert_eq!(coin_units(&json!(sent)), Some(units), "{units}");
        }
    }
}
