//! Shuffle over its GraphQL API (`https://<mirror>/main-api/graphql/api/graphql`).
//!
//! Shuffle has no API keys. The user logs in on the site itself, in the login
//! window, the way Antebot does it: the site keeps its session in localStorage
//! (`persist:root` → `auth`) as a short-lived access token (a JWT, sent as
//! `Authorization: Bearer …`) plus a refresh token. The window hands both to the
//! app ([`Creds`]), which then calls the API itself and trades the refresh token
//! for a new pair when the access token runs out (the `refreshToken` mutation, as
//! the site does). Every new pair goes back to the settings: the old refresh
//! token is spent.
//!
//! The API answers plain HTTPS. Should Cloudflare turn the app away, calls go
//! through the browser bridge parked on the mirror instead.
//!
//! Originals (Dice, Mines, Keno, Plinko, Blitz, Waifu Tower, Chicken) are
//! GraphQL mutations too. Every reply is a `Bet` with one action per step; it is
//! turned into the state shape of Stake's game, so the same boards draw it
//! (Waifu Tower plays exactly like Stake's Dragon Tower). Blitz is Shuffle's own.
//!
//! Slots: the catalog is `cachedGames` (public), and `gameCreateSession` opens a game, real or
//! demo, through Shuffle's SoftSwiss integration. Its `gameUrl` is the provider's own launch
//! link (Hacksaw's launcher, BGaming's interlayer, the fh8labs launcher that redirects to the
//! Peter & Sons / Shady Lady / Penguin King / OneTouch game …), which the slot drivers take
//! from there as they do chips.gg's.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::bridge::{Bridge, BridgeRequest};
use crate::currency::is_fiat;
use crate::money::{f64_from_json, format_units, parse_units, to_f64};
use crate::originals::{mines_state, Game, Round, Step, Tables};
use super::creds::Creds;

/// Amounts travel as decimal strings; 8 decimals cover every coin's bet sizes.
const DECIMALS: u32 = 8;
const RPC_TIMEOUT: Duration = Duration::from_secs(20);
/// Balances are re-read this often while idle; bets update them locally.
const REFRESH: Duration = Duration::from_secs(10);
const RATES_EVERY: Duration = Duration::from_secs(120);
/// A hidden bridge gets this long to clear Cloudflare before we show it.
const SHOW_CHECK_AFTER: Duration = Duration::from_secs(20);
const CHECK_TIMEOUT: Duration = Duration::from_secs(150);
/// The access token is renewed this many seconds before it runs out, so a bet never carries an expired one.
const RENEW_BEFORE: i64 = 60;
const API_PATH: &str = "/main-api/graphql/api/graphql";
/// Browser profile of the bridge, apart from the login window's.
const PROFILE: &str = "shuffle-bridge";
/// Site build named in the correlation id, as the site's own requests carry it.
const SITE_VERSION: &str = "v6.14.5";
/// Plain HTTPS presents itself as desktop Chrome.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";

pub const DEFAULT_MIRROR: &str = "shuffle.com";

/// Shuffle's domains as its own site lists them, named ones first. Every one reaches the same account.
pub const MIRRORS: &[&str] = &[
    "shuffle.com", "shuffle.game", "shuffle.gg", "shuffle.bet", "shuffle.casino", "shuffle.net", "shuffle.vip",
    "shuffle.site", "shuffle.money", "shuffle.global", "shuffle.online", "shuffle.gold", "shuffle.digital",
    "shuffle.club", "shuffle.ag", "shuffle.vegas", "shuffle.red", "shuffle.blue", "shuffle.black", "shuffle71.com",
    "shuffle72.com", "shuffle73.com", "shuffle888.com", "shuffle1060.com", "shuffle1061.com", "shuffle1062.com",
    "shuffle1063.com", "shuffle1064.com", "shuffle1065.com", "shuffle1066.com", "shuffle1067.com", "shuffle1068.com",
    "shuffle1069.com", "shuffle1070.com", "disorder1.com", "disorder2.com", "disorder4.com", "disorder5.com",
    "disorder6.com", "disorder7.com", "disorder8.com", "disorder9.com", "disorder10.com", "disorder11.com",
];

// Like the site's GetPrices: coin prices in USD, and `fiatPrices`, how many of each fiat Shuffle
// shows amounts in buy one USD (EUR 0.89, JPY 158 …).
const PRICES: &str = "query FastbetPrices { cryptoPrices(currency: USD) { name price } fiatPrices { name price } }";
const ME: &str = "query FastbetMe { me { id username vipLevel xp usdWagered account { balances { currency amount: availableAmount } } } }";
/// Everything the VIP rewards page shows, in one request.
const REWARDS: &str = "query FastbetRewards { vipRewardsAggregate { dailyRakeback { nextClaimDate eligible currencyAmounts { amount currency } } \
    weeklyBonus { usdValue nextClaimDate eligible } monthlyBonus { usdValue nextClaimDate eligible } instantRakebackBonus { currency amount } \
    vipUpgradeBonuses { upgradeBonusUsdAmount vipLevel createdAt id } challengeRewards { claimedAt expiredAt challenge { id amount currency game { name } } } } }";
const CLAIM_RAKEBACK: &str = "mutation FastbetClaimRakeback { instantRakebackClaim { currency amount } }";
const CLAIM_DAILY_RAKEBACK: &str = "mutation FastbetClaimDailyRakeback { vipRewardsClaimDailyRakeback { nextClaimDate eligible currencyAmounts { amount currency } } }";
const CLAIM_WEEKLY: &str = "mutation FastbetClaimWeekly($currency: Currency!) { vipRewardsClaimWeeklyBonus(currency: $currency) { claimedAmount { currency amount } nextClaimDate } }";
const CLAIM_MONTHLY: &str = "mutation FastbetClaimMonthly($currency: Currency!) { vipRewardsClaimMonthlyBonus(currency: $currency) { claimedAmount { currency amount } nextClaimDate } }";
const CLAIM_UPGRADE: &str = "mutation FastbetClaimUpgrade($currency: Currency!, $vipLevel: VipLevel!) { claimVipUpgradeBonus(currency: $currency, vipLevel: $vipLevel) { vipUserUpgradeId currency amount } }";
const CLAIM_CHALLENGE: &str = "mutation FastbetClaimChallenge($challengeId: String!) { claimChallengeReward(challengeId: $challengeId) { challenge { currency amount } claimedAt } }";
/// The open slot challenges (public).
const CHALLENGES: &str = "query FastbetChallenges { challenges(first: 50, completed: false) { nodes { id usdMinBet minMultiplier currency amount completed referredOnly \
    game { id name slug image { key } provider { id name } edge demoMode } } } }";
/// The XP each VIP level starts at, lowest first (public).
const VIP_LEVELS: &str = "query FastbetVipLevels { vipLevels { level amount } }";
const SEEDS: &str = "query FastbetSeeds { currentGameSeeds { id clientSeed hashedSeed status currentNonce } }";
const ROTATE: &str = "mutation FastbetSeed($clientSeed: String!) { gameSeedChangeAndReveal(newClientSeed: $clientSeed) { \
    id clientSeed hashedSeed status currentNonce } }";
const RENEW: &str = "mutation FastbetRenew($refreshToken: String!) { refreshToken(refreshToken: $refreshToken) { accessToken refreshToken } }";
const LIMBO: &str = "mutation FastbetLimbo($data: LimboPlayInput!) { limboPlay(data: $data) { id currency amount payout afterBalance \
    gameSeedNonce shuffleOriginalActions { action { limbo { resultValue userValue } } } } }";

/// What every original's reply carries besides the game's own action fields.
const BET_FIELDS: &str = "id currency amount payout multiplier afterBalance gameSeedNonce";
/// Names and artwork of the originals: (our slug, Shuffle's slug).
const ORIGINAL_SLUGS: &[(&str, &str)] = &[
    ("limbo", "limbo"),
    ("dice", "dice"),
    ("mines", "mines"),
    ("keno", "keno"),
    ("plinko", "plinko"),
    ("blitz", "blitz"),
    ("dragon-tower", "tower"),
    ("chicken", "chicken"),
];
/// Lobby tile size (2x the tile), as JPEG for the image decoder.
const THUMB: &str = "w=264&h=346&fit=crop&fm=jpg&q=85";
const SLOTS: &str = "query FastbetSlots($skip: Int, $first: Int, $providers: [String!], $search: String) { \
    cachedGames(skip: $skip, first: $first, providers: $providers, search: $search, categories: [SLOTS], sortBy: POPULAR) { \
    nodes { slug name demoMode edge image { key } provider { id name } } } }";
const LAUNCH: &str = "mutation FastbetLaunch($clientType: ClientType!, $cryptoCurrency: Currency!, $demoMode: Boolean!, \
    $fiatCurrency: FiatCurrency!, $gameSlug: String!, $locale: String!) { gameCreateSession(clientType: $clientType, \
    cryptoCurrency: $cryptoCurrency, demoMode: $demoMode, fiatCurrency: $fiatCurrency, gameSlug: $gameSlug, locale: $locale) \
    { gameUrl launchOptions } }";
/// Most games `cachedGames` returns at once.
const SLOTS_PAGE: usize = 40;
/// Shuffle's provider ids for the slot drivers' producer keys (chips.gg's names).
const PROVIDERS: &[(&str, &str)] = &[
    ("hacksawgaming", "hacksaw"),
    ("bgaming", "bgmng"),
    ("peter&sons", "peter-sons"),
    ("shadylady", "shady-lady"),
    ("penguinking", "penguin-king"),
    ("onetouch", "one-touch"),
    ("relaxgaming", "relax"),
    ("pragmaticplay", "pragmaticplay"),
    ("avatarux", "avatarux"),
    ("nolimitcity", "nolimit"),
    ("wickedgames", "wickedgames"),
    // studios on Hacksaw's RGS: the same ids here and in the drivers
    ("bullsharkgames", "bullsharkgames"),
    ("nownow", "nownow"),
    ("trustygaming", "trustygaming"),
    ("kitsunegaming", "kitsunegaming"),
    ("aceroll", "aceroll"),
    ("goodtimesstudios", "goodtimesstudios"),
    ("pineappleplay", "pineappleplay"),
    ("alohagaming", "alohagaming"),
    // studios on Relax's platform
    ("printstudios", "printstudios"),
    ("fantasma", "fantasma"),
    ("fourleafgaming", "fourleafgaming"),
];

const EXPIRED: &str = "Your Shuffle login has expired. Log in again in Settings.";
/// Sweepstakes coins of Shuffle's US site, not money.
const HIDDEN: &[&str] = &["gc", "sc"];

enum CallError {
    /// Cloudflare answered instead of Shuffle: plain HTTPS was turned away, or the bridge has to pass the check (again).
    Cloudflare,
    /// The access token was refused.
    Unauthenticated,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::Cloudflare => CasinoError::Server("Cloudflare check, reconnecting".into()),
            CallError::Unauthenticated => CasinoError::Server(EXPIRED.into()),
            CallError::Casino(e) => e,
        }
    }
}

/// How calls reach Shuffle.
#[derive(Clone)]
enum Transport {
    /// Plain HTTPS.
    Direct(reqwest::Client),
    /// The hidden browser window, parked on the mirror.
    Bridge(Bridge),
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

pub struct Shuffle {
    creds: Mutex<Option<Creds>>,
    /// Refresh tokens already traded in. A stale copy handed back (a Login click racing a
    /// renewal) would log the account out, so it is ignored.
    spent: Mutex<HashSet<String>>,
    /// One renewal at a time; calls that saw the same token refused wait for it.
    renewing: tokio::sync::Mutex<()>,
    mirror: Mutex<String>,
    /// Bumped when the mirror changes: the link is rebuilt.
    setup: AtomicU64,
    /// Login or mirror changed.
    wake: Notify,
    /// A call found the link dead or challenged; the run loop rebuilds it.
    broken: Notify,
    broken_flag: AtomicBool,
    link: Mutex<Option<Link>>,
    link_gen: AtomicU64,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// The VIP ladder: (level, XP it starts at), lowest first; and the status last sent.
    vip_levels: Mutex<Vec<(String, f64)>>,
    last_vip: Mutex<Option<VipStatus>>,
    /// Bets in flight and bets settled, so a balance poll never overwrites a fresher local balance.
    inflight: AtomicUsize,
    settled: AtomicU64,
    /// Parts of the `X-Correlation-Id` every request carries (bets are refused without it):
    /// `<browser>-<window>-<site version>-<time>-<count>::<user id>`. The window id also goes with each bet.
    browser_id: String,
    window_id: String,
    requests: AtomicU64,
    user_id: Mutex<Option<String>>,
    /// Open rounds of the originals, as last answered (their id and progress feed the next step).
    open_rounds: Mutex<HashMap<Game, Round>>,
}

impl Shuffle {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        let mirror = setup.mirror.map(|m| stake::normalize_mirror(&m)).filter(|m| !m.is_empty()).unwrap_or_else(|| DEFAULT_MIRROR.into());
        let me = Arc::new(Self {
            creds: Mutex::new(setup.token.as_deref().and_then(Creds::parse)),
            spent: Mutex::new(HashSet::new()),
            renewing: tokio::sync::Mutex::new(()),
            mirror: Mutex::new(mirror),
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
            vip_levels: Mutex::new(Vec::new()),
            last_vip: Mutex::new(None),
            inflight: AtomicUsize::new(0),
            settled: AtomicU64::new(0),
            browser_id: random_id(),
            window_id: random_id(),
            requests: AtomicU64::new(0),
            user_id: Mutex::new(None),
            open_rounds: Mutex::new(HashMap::new()),
        });
        tokio::spawn(me.clone().run());
        me
    }

    fn status(&self, s: ConnStatus, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!("shuffle: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn setup_is(&self, setup: u64) -> bool {
        self.setup.load(Ordering::SeqCst) == setup
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

    fn install(&self, transport: Transport, origin: String) -> Link {
        let link = Link { gen: self.link_gen.fetch_add(1, Ordering::SeqCst) + 1, transport, origin };
        self.broken_flag.store(false, Ordering::SeqCst);
        if let Some(old) = self.link.lock().replace(link.clone()) {
            old.transport.close();
        }
        link
    }

    /// Reaches the API on `mirror`: plain HTTPS, or the bridge when Cloudflare turns that away.
    async fn connect(&self, mirror: &str, setup: u64) -> Result<(), String> {
        self.status(ConnStatus::Connecting, format!("Opening {mirror}"));
        let origin = format!("https://{mirror}");
        let link = self.install(Transport::Direct(direct_client(mirror)?), origin.clone());
        match self.post(&link, PRICES, json!({}), None, Duration::from_secs(15)).await {
            Ok(data) => {
                self.apply_rates(&data);
                return Ok(());
            }
            Err(CallError::Cloudflare) => tracing::info!("shuffle: Cloudflare turned plain HTTPS away, using the browser bridge"),
            Err(e) => return Err(CasinoError::from(e).to_string()),
        }

        self.status(ConnStatus::Connecting, "Passing the Cloudflare check…");
        let bridge = Bridge::open_profile(&format!("{origin}/robots.txt"), Some(PROFILE)).await?;
        let link = self.install(Transport::Bridge(bridge), origin);
        let t0 = Instant::now();
        let mut shown = false;
        loop {
            if !self.alive() || !self.setup_is(setup) {
                return Err(String::new());
            }
            match self.post(&link, PRICES, json!({}), None, Duration::from_secs(8)).await {
                Ok(data) => {
                    if shown {
                        link.transport.show(false);
                    }
                    self.apply_rates(&data);
                    return Ok(());
                }
                Err(CallError::Cloudflare) => {
                    if t0.elapsed() > CHECK_TIMEOUT {
                        return Err(format!("Cloudflare didn't let us through on {mirror}. Try another mirror."));
                    }
                    if !shown && t0.elapsed() > SHOW_CHECK_AFTER {
                        link.transport.show(true);
                        shown = true;
                    }
                    self.status(ConnStatus::Connecting, if shown { "Finish the Cloudflare check in the window that opened" } else { "Passing the Cloudflare check…" });
                }
                Err(CallError::Casino(CasinoError::ConnectionLost)) => return Err(format!("The connection to {mirror} was closed")),
                // a challenge page navigating away drops the request; just ask again
                Err(e) => {
                    if t0.elapsed() > Duration::from_secs(60) {
                        return Err(CasinoError::from(e).to_string());
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(1500)).await;
        }
    }

    /// Signs in and keeps balances fresh until the link breaks or the mirror changes.
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
                    // a permit left over from an earlier link is not a reason to rebuild this one
                    if self.broken_flag.swap(false, Ordering::SeqCst) {
                        return;
                    }
                }
                _ = tokio::time::sleep(REFRESH) => {
                    if rates_at.elapsed() > RATES_EVERY {
                        rates_at = Instant::now();
                        if let Ok(data) = self.gql(PRICES, json!({}), false).await {
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
            link.transport.close();
        }
    }

    async fn authenticate(&self) {
        if self.creds.lock().is_none() {
            self.status(ConnStatus::NoToken, "");
            return;
        }
        self.status(ConnStatus::Authenticating, "");
        match self.gql(ME, json!({}), true).await {
            Ok(data) => match data.get("me").filter(|u| u.is_object()) {
                Some(me) => {
                    if self.vip_levels.lock().is_empty() {
                        match self.gql(VIP_LEVELS, json!({}), false).await {
                            Ok(data) => *self.vip_levels.lock() = vip_ladder(&data),
                            Err(e) => tracing::info!("shuffle: vip levels: {}", CasinoError::from(e)),
                        }
                    }
                    self.apply_me(me, true);
                    self.status(ConnStatus::Ready, "");
                    if let Ok(st) = self.seeds().await {
                        let _ = self.events.send(CasinoEvent::Provable(st));
                    }
                }
                None => self.status(ConnStatus::AuthFailed, "Shuffle returned no account for this login"),
            },
            Err(CallError::Unauthenticated) => self.status(ConnStatus::AuthFailed, EXPIRED),
            Err(CallError::Casino(CasinoError::Server(m))) => self.status(ConnStatus::AuthFailed, m),
            // the link broke; serve() is about to rebuild it
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
                if let Some(me) = data.get("me").filter(|u| u.is_object()) {
                    let fresh = idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled;
                    self.apply_me(me, fresh);
                }
            }
            Err(CallError::Unauthenticated) => self.status(ConnStatus::AuthFailed, EXPIRED),
            Err(CallError::Casino(CasinoError::Server(m))) if m == EXPIRED => self.status(ConnStatus::AuthFailed, m),
            Err(_) => {}
        }
    }

    async fn seeds(&self) -> Result<ProvableState, CasinoError> {
        let data = self.gql(SEEDS, json!({}), true).await?;
        Ok(provable_from(&data["currentGameSeeds"]))
    }

    fn apply_me(&self, me: &Value, balances: bool) {
        {
            if let Some(id) = me.get("id").and_then(Value::as_str) {
                *self.user_id.lock() = Some(id.to_string());
            }
            if let Some(vip) = vip_status(me, &self.vip_levels.lock()) {
                send_vip(&self.events, &self.last_vip, vip);
            }
            let mut m = self.market.lock();
            m.username = me.get("username").and_then(Value::as_str).map(str::to_string);
            if balances {
                if let Some(list) = me.pointer("/account/balances").and_then(Value::as_array) {
                    m.wallets.clear();
                    for b in list {
                        if let (Some(cur), Some(amount)) = (b.get("currency").and_then(Value::as_str), b.get("amount").and_then(coin_units)) {
                            m.wallets.insert(cur.to_ascii_lowercase(), amount);
                        }
                    }
                }
                // the site keeps the picked coin in the browser, not the account; start on the biggest balance
                if m.site_currency.is_none() {
                    m.site_currency = richest(&m);
                }
            }
        }
        self.publish();
    }

    fn apply_rates(&self, data: &Value) {
        let Some(list) = data.get("cryptoPrices").and_then(Value::as_array) else { return };
        let per_usd: Vec<(String, f64)> = data
            .get("fiatPrices")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(|f| Some((f.get("name")?.as_str()?.to_ascii_lowercase(), f.get("price").and_then(f64_from_json)?)))
            .filter(|(_, r)| *r > 0.0)
            .collect();
        {
            let mut m = self.market.lock();
            // the coins, and the fiat wallets (CAD, BRL …) Shuffle prices next to them
            for c in list {
                let Some(name) = c.get("name").and_then(Value::as_str) else { continue };
                let name = name.to_ascii_lowercase();
                let price = c.get("price").and_then(f64_from_json).unwrap_or(0.0);
                let hidden = HIDDEN.contains(&name.as_str());
                let fiat = is_fiat(&name) || per_usd.iter().any(|(f, _)| *f == name);
                m.currencies.insert(name, CurrencyInfo { decimals: DECIMALS, price, hidden, fiat });
            }
            // the fiats amounts can be shown in, at the site's own rates
            for (name, rate) in &per_usd {
                let info = CurrencyInfo { decimals: DECIMALS, price: 1.0 / rate, hidden: false, fiat: true };
                m.currencies.insert(name.clone(), info);
            }
            if !per_usd.is_empty() {
                // the site's picker lists them alphabetically
                let mut fiats: Vec<String> = per_usd.into_iter().map(|(f, _)| f).collect();
                fiats.sort();
                m.fiats = fiats;
            }
        }
        self.publish();
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

    /// One GraphQL request over `link`, carrying `access` when given.
    async fn post(&self, link: &Link, query: &str, variables: Value, access: Option<&str>, timeout: Duration) -> Result<Value, CallError> {
        let body = json!({ "query": query, "variables": variables }).to_string();
        let mut headers = vec![("Content-Type".to_string(), "application/json".to_string()), ("X-Correlation-Id".to_string(), self.correlation_id())];
        if let Some(t) = access {
            headers.push(("Authorization".into(), format!("Bearer {t}")));
        }
        let url = format!("{}{API_PATH}", link.origin);
        let (status, text) = match &link.transport {
            Transport::Bridge(bridge) => {
                let req = BridgeRequest { method: "POST".into(), url, headers, body: Some(body) };
                let res = bridge.request_within(req, timeout).await.map_err(|e| {
                    CallError::Casino(if e.contains("timeout") { CasinoError::Timeout } else { CasinoError::ConnectionLost })
                })?;
                (res.status, res.body)
            }
            Transport::Direct(client) => {
                let mut req = client.post(url).timeout(timeout).body(body);
                for (k, v) in headers {
                    req = req.header(k, v);
                }
                let lost = |e: reqwest::Error| CallError::Casino(if e.is_timeout() { CasinoError::Timeout } else { CasinoError::ConnectionLost });
                let res = req.send().await.map_err(lost)?;
                let status = res.status().as_u16();
                (status, res.text().await.map_err(lost)?)
            }
        };
        parse_reply(status, &text)
    }

    /// Sends one of an original's mutations (or the open-round query) and reads the `Bet` it answers.
    async fn play(&self, game: Game, op: Op, data: Option<Value>) -> Result<Option<Round>, CasinoError> {
        let (key, field, input) = op_names(game, op).ok_or_else(|| CasinoError::Server(format!("{} can't do that on Shuffle", game.name())))?;
        let fields = format!("{BET_FIELDS} shuffleOriginalActions {{ action {{ {} {{ {} }} }} }}", action_key(game), action_fields(game));
        let query = match (op, input) {
            (Op::Active, _) => format!("query Fastbet{key} {{ {field} {{ {fields} }} }}"),
            (_, Some(input)) => format!("mutation Fastbet{key}($data: {input}!) {{ {field}(data: $data) {{ {fields} }} }}"),
            (_, None) => format!("mutation Fastbet{key} {{ {field} {{ {fields} }} }}"),
        };
        let vars = data.map(|d| json!({ "data": d })).unwrap_or_else(|| json!({}));
        let reply = self.gql(&query, vars, true).await?;
        let Some(bet) = reply.get(field).filter(|b| b.is_object()) else {
            return if op == Op::Active { Ok(None) } else { Err(CasinoError::BadReply(reply.to_string().chars().take(200).collect())) };
        };
        let round = round_of(game, bet).ok_or_else(|| CasinoError::BadReply(bet.to_string().chars().take(200).collect()))?;
        tracing::debug!("shuffle: {} {op:?} -> open {} ×{} {}", game.name(), round.active, round.multiplier, round.state);
        // the balance after this step, straight from Shuffle
        if op != Op::Active {
            if let Some(after) = bet.get("afterBalance").and_then(coin_units) {
                self.market.lock().wallets.insert(round.currency.clone(), after);
                self.settled.fetch_add(1, Ordering::SeqCst);
                self.publish();
            }
        }
        let mut open = self.open_rounds.lock();
        if round.active {
            open.insert(game, round.clone());
        } else {
            open.remove(&game);
        }
        Ok(Some(round))
    }

    /// The open round of `game`, as last seen (to continue it).
    fn open_round(&self, game: Game) -> Result<Round, CasinoError> {
        self.open_rounds.lock().get(&game).cloned().ok_or_else(|| CasinoError::Server("No open round".into()))
    }

    /// A fresh `X-Correlation-Id`, built the way the site builds it.
    fn correlation_id(&self) -> String {
        let n = self.requests.fetch_add(1, Ordering::SeqCst) + 1;
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
        let user = self.user_id.lock().clone().unwrap_or_else(|| "anon".into());
        format!("{}-{}-{SITE_VERSION}-{}-{}::{user}", self.browser_id, self.window_id, base32(ms), base32(n))
    }

    /// One API call over the current link. A dead or challenged link is flagged for the run loop.
    async fn gql(&self, query: &str, variables: Value, auth: bool) -> Result<Value, CallError> {
        let link = self.link.lock().clone().ok_or(CallError::Casino(CasinoError::NotConnected))?;
        let res = if auth { self.signed(&link, query, variables).await } else { self.post(&link, query, variables, None, RPC_TIMEOUT).await };
        let broken = matches!(res, Err(CallError::Cloudflare) | Err(CallError::Casino(CasinoError::ConnectionLost)));
        if broken && link.gen == self.link_gen.load(Ordering::SeqCst) {
            self.ready.store(false, Ordering::SeqCst);
            self.broken_flag.store(true, Ordering::SeqCst);
            self.broken.notify_one();
        }
        res
    }

    /// A signed-in call. The access token is renewed when it is about to run out, or when
    /// the API refuses it, and then the call goes out once more.
    async fn signed(&self, link: &Link, query: &str, variables: Value) -> Result<Value, CallError> {
        let creds = self.creds.lock().clone().ok_or(CallError::Casino(CasinoError::Server("Not logged in".into())))?;
        let mut access = creds.access.clone();
        if let Some(left) = creds.expires_in().filter(|l| *l < RENEW_BEFORE && !creds.refresh.is_empty()) {
            match self.renew(link, &creds.access).await {
                Ok(fresh) => access = fresh,
                // still valid for a moment: use it, the next call tries again
                Err(CallError::Casino(e)) if left > 5 && !matches!(&e, CasinoError::Server(m) if m == EXPIRED) => {}
                Err(e) => return Err(e),
            }
        }
        match self.post(link, query, variables.clone(), Some(&access), RPC_TIMEOUT).await {
            Err(CallError::Unauthenticated) => {
                let fresh = self.renew(link, &access).await?;
                self.post(link, query, variables, Some(&fresh), RPC_TIMEOUT).await
            }
            res => res,
        }
    }

    /// Trades the refresh token for a new pair, once for every call that saw `stale` refused.
    /// The new pair is handed to the app to save.
    async fn renew(&self, link: &Link, stale: &str) -> Result<String, CallError> {
        let _one = self.renewing.lock().await;
        let creds = self.creds.lock().clone().ok_or(CallError::Unauthenticated)?;
        if creds.access != stale {
            // renewed (or logged in again) while we waited
            return Ok(creds.access);
        }
        if creds.refresh.is_empty() {
            return Err(CallError::Unauthenticated);
        }
        let data = self.post(link, RENEW, json!({ "refreshToken": creds.refresh }), None, RPC_TIMEOUT).await?;
        let pair = data
            .get("refreshToken")
            .and_then(|p| serde_json::from_value::<Creds>(p.clone()).ok())
            .filter(|c| !c.access.is_empty() && !c.refresh.is_empty())
            .ok_or_else(|| CallError::Casino(CasinoError::BadReply(data.to_string())))?;
        self.spent.lock().insert(creds.refresh.clone());
        {
            let mut cur = self.creds.lock();
            if cur.as_ref() != Some(&creds) {
                // a new login arrived meanwhile; it wins
                return cur.as_ref().map(|c| c.access.clone()).ok_or(CallError::Unauthenticated);
            }
            *cur = Some(pair.clone());
        }
        tracing::info!("shuffle: login renewed");
        let _ = self.events.send(CasinoEvent::Token(pair.to_token()));
        Ok(pair.access)
    }
}

#[async_trait]
impl Casino for Shuffle {
    fn id(&self) -> CasinoId {
        CasinoId::Shuffle
    }

    async fn rewards(&self) -> Result<Vec<Reward>, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let data = self.gql(REWARDS, json!({}), true).await?;
        let r = data.get("vipRewardsAggregate").filter(|r| r.is_object()).ok_or_else(|| CasinoError::BadReply(data.to_string().chars().take(200).collect()))?;
        Ok(rewards_of(r, &|c: &str| self.usd_price(c), SystemTime::now()))
    }

    async fn claim_reward(&self, id: &str, currency: &str) -> Result<String, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let cur = currency.to_ascii_uppercase();
        let (query, vars, key) = match id {
            "rakeback" => (CLAIM_RAKEBACK, json!({}), "instantRakebackClaim"),
            "daily-rakeback" => (CLAIM_DAILY_RAKEBACK, json!({}), "vipRewardsClaimDailyRakeback"),
            "weekly" => (CLAIM_WEEKLY, json!({ "currency": cur }), "vipRewardsClaimWeeklyBonus"),
            "monthly" => (CLAIM_MONTHLY, json!({ "currency": cur }), "vipRewardsClaimMonthlyBonus"),
            _ if id.starts_with("upgrade:") => (CLAIM_UPGRADE, json!({ "currency": cur, "vipLevel": &id["upgrade:".len()..] }), "claimVipUpgradeBonus"),
            _ if id.starts_with("challenge:") => (CLAIM_CHALLENGE, json!({ "challengeId": &id["challenge:".len()..] }), "claimChallengeReward"),
            _ => return Err(CasinoError::Server("Nothing to claim".into())),
        };
        let data = self.gql(query, vars, true).await?;
        // the balances take the payout
        self.refresh_me().await;
        Ok(claimed_text(data.get(key)))
    }

    async fn challenges(&self) -> Result<Vec<Challenge>, CasinoError> {
        let data = self.gql(CHALLENGES, json!({}), self.ready.load(Ordering::SeqCst)).await?;
        Ok(challenges_of(&data, &|c: &str| self.usd_price(c)))
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

    fn set_mirror(&self, mirror: &str) {
        let m = stake::normalize_mirror(mirror);
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
        let cur = req.currency.to_ascii_lowercase();
        let vars = json!({ "data": {
            "amount": format_units(req.amount, DECIMALS, DECIMALS),
            "bet": format!("{:.2}", req.target),
            "currency": cur.to_ascii_uppercase(),
            "windowId": self.window_id,
        } });
        let data = self.gql(LIMBO, vars, true).await?;
        let bad = || CasinoError::BadReply(data.to_string().chars().take(200).collect());
        let bet = data.get("limboPlay").filter(|b| b.is_object()).ok_or_else(bad)?;
        let limbo = bet.pointer("/shuffleOriginalActions/0/action/limbo").ok_or_else(bad)?;
        let outcome = limbo.get("resultValue").and_then(f64_from_json).ok_or_else(bad)?;
        let amount = bet.get("amount").and_then(coin_units);
        let payout = bet.get("payout").and_then(coin_units);
        if let (Some(a), Some(p)) = (amount, payout) {
            let cur = bet.get("currency").and_then(Value::as_str).map(str::to_ascii_lowercase).unwrap_or(cur);
            self.credit(&cur, p - a);
        }
        self.settled.fetch_add(1, Ordering::SeqCst);
        // a bet of 0 pays 0 either way: the result against the target decides it
        let target = limbo.get("userValue").and_then(f64_from_json).unwrap_or(req.target);
        let win = if req.amount == 0 { Some(outcome >= target) } else { payout.map(|p| p > 0) };
        Ok(BetReply { outcome, amount, winnings: payout, win, nonce: bet.get("gameSeedNonce").and_then(Value::as_u64) })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        self.gql(ROTATE, json!({ "clientSeed": client_seed }), true).await?;
        // the reply reveals the seed that was retired; the new pair is read afresh
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

    async fn list_slots(&self, f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let search = f.query.as_deref().map(str::trim).filter(|q| !q.is_empty());
        let providers = f.producer.as_deref().map(|p| vec![PROVIDERS.iter().find(|(ours, _)| *ours == p).map_or(p, |(_, theirs)| *theirs)]);
        // a provider's own list (the lobby's Tested section) is its 40 most played: one request each
        let want = if providers.is_some() { f.limit.clamp(1, SLOTS_PAGE) } else { f.limit.max(1) };
        let mut games = Vec::new();
        // Shuffle hands out at most 40 at a time
        while games.len() < want {
            let first = (want - games.len()).min(SLOTS_PAGE);
            let vars = json!({ "skip": f.skip + games.len(), "first": first, "providers": providers, "search": search });
            let data = self.gql(SLOTS, vars, false).await?;
            let nodes = data.pointer("/cachedGames/nodes").and_then(Value::as_array).cloned().unwrap_or_default();
            let got = nodes.len();
            games.extend(nodes.iter().filter_map(slot_game));
            if got < first {
                break;
            }
        }
        Ok(games)
    }

    async fn launch_slot(&self, slug: &str, demo: bool, currency: &str, fiat: &str) -> Result<String, CasinoError> {
        let logged_in = self.creds.lock().is_some();
        if !demo && !logged_in {
            return Err(CasinoError::Server("Log in to play for real".into()));
        }
        let fiat = if fiat.is_empty() { "USD".to_string() } else { fiat.to_ascii_uppercase() };
        let vars = json!({
            "clientType": "DESKTOP",
            "cryptoCurrency": currency.to_ascii_uppercase(),
            "demoMode": demo,
            "fiatCurrency": fiat,
            "gameSlug": slug,
            "locale": "en",
        });
        let data = self.gql(LAUNCH, vars, logged_in).await?;
        let session = &data["gameCreateSession"];
        session["gameUrl"]
            .as_str()
            .or_else(|| session.pointer("/launchOptions/game_url").and_then(Value::as_str))
            .filter(|u| !u.is_empty())
            .map(str::to_string)
            .ok_or_else(|| CasinoError::BadReply(data.to_string().chars().take(200).collect()))
    }

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let _inflight = InFlight::new(&self.inflight);
        let mut data = bet_input(game, &params, auto).map_err(CasinoError::Server)?;
        data["amount"] = json!(format_units(amount, DECIMALS, DECIMALS));
        data["currency"] = json!(currency.to_ascii_uppercase());
        if !game.has_steps() {
            data["windowId"] = json!(self.window_id);
        }
        let op = if auto && game.has_steps() { Op::Auto } else { Op::Start };
        self.play(game, op, Some(data)).await?.ok_or_else(|| CasinoError::BadReply("no bet in the reply".into()))
    }

    async fn original_step(&self, game: Game, step: Step, params: Value) -> Result<Round, CasinoError> {
        let _inflight = InFlight::new(&self.inflight);
        let (op, data) = match step {
            Step::Cashout => (Op::Cashout, None),
            Step::Next => (Op::Next, Some(next_input(game, &self.open_round(game)?, &params).map_err(CasinoError::Server)?)),
        };
        self.play(game, op, data).await?.ok_or_else(|| CasinoError::BadReply("no bet in the reply".into()))
    }

    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        if !game.has_steps() {
            return Ok(None);
        }
        self.play(game, Op::Active, None).await
    }

    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        // Shuffle's Keno and Plinko pay exactly like Stake's (Plinko without the expert risk)
        Ok(Tables::builtin())
    }

    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let aliases: Vec<String> = ORIGINAL_SLUGS.iter().map(|(_, s)| format!(r#"{s}: game(slug: "originals/{s}") {{ name edge image {{ key }} }}"#)).collect();
        let data = self.gql(&format!("query FastbetOriginals {{ {} }}", aliases.join(" ")), json!({}), false).await?;
        Ok(ORIGINAL_SLUGS
            .iter()
            .filter_map(|(ours, theirs)| {
                let g = data.get(*theirs)?;
                let key = g.pointer("/image/key")?.as_str()?;
                let edge = g.get("edge").and_then(f64_from_json).unwrap_or(1.0);
                Some(crate::slots::SlotGame {
                    id: ours.to_string(),
                    name: g.get("name")?.as_str()?.to_string(),
                    producer: "shuffle".into(),
                    image: format!("https://shuffle-com.imgix.net/{key}?{THUMB}"),
                    demo: false,
                    rtp: 100.0 - edge,
                    provider_game_id: None,
                    studio: None,
                })
            })
            .collect())
    }
}

/// The kinds of request an original takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    /// Starts a round (and settles it, for the one-shot games).
    Start,
    /// Plays a whole round from picks made up front (the stepped games' auto bet).
    Auto,
    Next,
    Cashout,
    /// The round left open, if any.
    Active,
}

/// (operation name, field, input type) of an original's request.
fn op_names(game: Game, op: Op) -> Option<(&'static str, &'static str, Option<&'static str>)> {
    Some(match (game, op) {
        (Game::Dice, Op::Start) => ("Dice", "dicePlay", Some("DicePlayInput")),
        (Game::Keno, Op::Start) => ("Keno", "kenoPlay", Some("KenoPlayInput")),
        (Game::Plinko, Op::Start) => ("Plinko", "plinkoPlay", Some("PlinkoPlayInput")),
        (Game::Blitz, Op::Start) => ("Blitz", "blitzPlay", Some("BlitzPlayInput")),
        (Game::Mines, Op::Start) => ("MinesStart", "minesStart", Some("MinesStartInput")),
        (Game::Mines, Op::Auto) => ("MinesAuto", "minesAuto", Some("MinesBetInput")),
        (Game::Mines, Op::Next) => ("MinesNext", "minesNext", Some("MinesNextInput")),
        (Game::Mines, Op::Cashout) => ("MinesCashout", "minesCashout", None),
        (Game::Mines, Op::Active) => ("MinesOpen", "minesActiveBet", None),
        (Game::DragonTower, Op::Start) => ("TowerStart", "towerStart", Some("TowerStartInput")),
        (Game::DragonTower, Op::Auto) => ("TowerAuto", "towerAuto", Some("TowerBetInput")),
        (Game::DragonTower, Op::Next) => ("TowerNext", "towerNext", Some("TowerNextInput")),
        (Game::DragonTower, Op::Cashout) => ("TowerCashout", "towerCashout", None),
        (Game::DragonTower, Op::Active) => ("TowerOpen", "towerActiveBet", None),
        (Game::Chicken, Op::Start) => ("ChickenStart", "chickenStart", Some("ChickenStartInput")),
        (Game::Chicken, Op::Auto) => ("ChickenAuto", "chickenAuto", Some("ChickenBetInput")),
        (Game::Chicken, Op::Next) => ("ChickenNext", "chickenNext", Some("ChickenNextInput")),
        (Game::Chicken, Op::Cashout) => ("ChickenCashout", "chickenCashout", None),
        (Game::Chicken, Op::Active) => ("ChickenOpen", "chickenActiveBet", None),
        _ => return None,
    })
}

/// `{currency, amount}` entries with something in them.
fn coin_amounts(list: Option<&Value>) -> Vec<(String, f64)> {
    list.and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|c| Some((c.get("currency")?.as_str()?.to_string(), c.get("amount").and_then(f64_from_json)?)))
        .filter(|(_, a)| *a > 0.0)
        .collect()
}

/// `vipRewardsAggregate` -> the reward cards, the way the site decides what is claimable: an amount
/// that is there can be claimed until `nextClaimDate`; without one it is cooling down until then.
fn rewards_of(r: &Value, price: &dyn Fn(&str) -> f64, now: SystemTime) -> Vec<Reward> {
    let worth = |list: &[(String, f64)]| list.iter().map(|(c, a)| a * price(c)).sum::<f64>();
    let mut out = Vec::new();

    let instant = coin_amounts(r.get("instantRakebackBonus"));
    let usd = worth(&instant);
    let detail = if instant.is_empty() { "Nothing waiting yet".to_string() } else { coins_text(&instant) };
    out.push(Reward { usd: Some(usd), detail, claimable: usd > 0.0, ..Reward::new("rakeback", RewardKind::Rakeback) });

    if let Some(d) = r.get("dailyRakeback").filter(|d| d.is_object()) {
        let list = coin_amounts(d.get("currencyAmounts"));
        let next = d.get("nextClaimDate").and_then(time_of);
        let ready = d["eligible"].as_bool() == Some(true) && !list.is_empty() && next.is_none_or(|n| now < n);
        let detail = if list.is_empty() { "Builds up as you play".to_string() } else { coins_text(&list) };
        out.push(Reward {
            title: "Daily rakeback".into(),
            usd: (!list.is_empty()).then(|| worth(&list)),
            detail,
            claimable: ready,
            next_at: if ready { None } else { next },
            expires: if ready { next } else { None },
            ..Reward::new("daily-rakeback", RewardKind::Daily)
        });
    }

    for (key, id, kind, unlock) in [("weeklyBonus", "weekly", RewardKind::Weekly, "Bronze 1"), ("monthlyBonus", "monthly", RewardKind::Monthly, "Silver 1")] {
        let Some(b) = r.get(key).filter(|b| b.is_object()) else { continue };
        let usd = b.get("usdValue").and_then(f64_from_json).unwrap_or(0.0);
        let next = b.get("nextClaimDate").and_then(time_of);
        let eligible = b["eligible"].as_bool() == Some(true);
        let ready = eligible && usd > 0.0 && next.is_none_or(|n| now < n);
        let detail = if !eligible { format!("Unlocks at {unlock}") } else { "Grows with what you wager".to_string() };
        out.push(Reward {
            usd: (usd > 0.0).then_some(usd),
            detail,
            claimable: ready,
            next_at: if ready || !eligible { None } else { next },
            expires: if ready { next } else { None },
            ..Reward::new(id, kind)
        });
    }

    for u in r.get("vipUpgradeBonuses").and_then(Value::as_array).into_iter().flatten() {
        let Some(level) = u.get("vipLevel").and_then(Value::as_str) else { continue };
        // the site gives them a week
        let expires = u.get("createdAt").and_then(time_of).map(|c| c + Duration::from_secs(7 * 86_400 - 60));
        out.push(Reward {
            title: format!("{} bonus", level_name(level)),
            usd: u.get("upgradeBonusUsdAmount").and_then(f64_from_json),
            detail: format!("For reaching {}", level_name(level)),
            claimable: expires.is_none_or(|e| now < e),
            expires,
            ..Reward::new(&format!("upgrade:{level}"), RewardKind::LevelUp)
        });
    }

    for c in r.get("challengeRewards").and_then(Value::as_array).into_iter().flatten() {
        let expires = c.get("expiredAt").and_then(time_of);
        if c.get("claimedAt").is_some_and(|v| !v.is_null()) || expires.is_some_and(|e| e <= now) {
            continue;
        }
        let ch = &c["challenge"];
        let Some(id) = ch.get("id").and_then(Value::as_str) else { continue };
        let cur = ch.get("currency").and_then(Value::as_str).unwrap_or("USDC");
        let amount = ch.get("amount").and_then(f64_from_json).unwrap_or(0.0);
        let game = ch.pointer("/game/name").and_then(Value::as_str).unwrap_or("a slot");
        out.push(Reward { usd: Some(amount * price(cur)), detail: format!("Won on {game}"), claimable: true, expires, ..Reward::new(&format!("challenge:{id}"), RewardKind::Challenge) });
    }
    out
}

/// `challenges { nodes }` -> the open challenges anyone can take, biggest prize first.
fn challenges_of(data: &Value, price: &dyn Fn(&str) -> f64) -> Vec<Challenge> {
    let nodes = data.pointer("/challenges/nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut out: Vec<Challenge> = nodes
        .iter()
        .filter(|c| c["completed"].as_bool() != Some(true) && c["referredOnly"].as_bool() != Some(true))
        .filter_map(|c| {
            let cur = c.get("currency").and_then(Value::as_str).unwrap_or("USDC");
            Some(Challenge {
                id: c.get("id")?.as_str()?.to_string(),
                koth: false,
                game: slot_game(&c["game"])?,
                target: c.get("minMultiplier").and_then(f64_from_json)?,
                min_bet_usd: c.get("usdMinBet").and_then(f64_from_json).unwrap_or(0.0),
                prize_usd: c.get("amount").and_then(f64_from_json).unwrap_or(0.0) * price(cur),
                ends: None,
                leader: None,
            })
        })
        .collect();
    out.sort_by(|a, b| b.prize_usd.total_cmp(&a.prize_usd));
    out
}

/// `vipLevels` -> (level, XP it starts at), lowest first.
fn vip_ladder(data: &Value) -> Vec<(String, f64)> {
    let mut ladder: Vec<(String, f64)> = data
        .get("vipLevels")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(|l| Some((l.get("level")?.as_str()?.to_string(), l.get("amount").and_then(f64_from_json)?)))
        .collect();
    ladder.sort_by(|a, b| a.1.total_cmp(&b.1));
    ladder
}

/// "GOLD_3" -> "Gold 3", "UNRANKED" -> "Unranked".
fn level_name(code: &str) -> String {
    code.split('_')
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The account's VIP level from `me { vipLevel xp usdWagered }` on the ladder, the way the site
/// works it out: the next level is the first one above the XP.
fn vip_status(me: &Value, ladder: &[(String, f64)]) -> Option<VipStatus> {
    let xp = me.get("xp").and_then(f64_from_json)?;
    let wagered = me.get("usdWagered").and_then(f64_from_json).unwrap_or(0.0);
    let next = ladder.iter().position(|(_, at)| xp < *at);
    let (start, next) = match next {
        Some(i) => (ladder.get(i.wrapping_sub(1)).map(|(_, at)| *at).unwrap_or(0.0), ladder.get(i).map(|(l, at)| (level_name(l), *at))),
        None => (ladder.last().map(|(_, at)| *at).unwrap_or(0.0), None),
    };
    let rank = me.get("vipLevel").and_then(Value::as_str).map(level_name).unwrap_or_else(|| "Unranked".into());
    Some(VipStatus::ranked(rank, xp, start, next, Some(xp_rate(xp, wagered))))
}

/// The game's key inside `shuffleOriginalActions { action { … } }`, and the fields asked for.
fn action_key(game: Game) -> &'static str {
    match game {
        Game::DragonTower => "tower",
        Game::Mines => "mines",
        Game::Chicken => "chicken",
        Game::Keno => "keno",
        Game::Plinko => "plinko",
        Game::Blitz => "blitz",
        _ => "dice",
    }
}

fn action_fields(game: Game) -> &'static str {
    match game {
        Game::Keno => "results risk multiplier selected",
        Game::Plinko => "risk results rows multiplier",
        Game::Blitz => "cards picks maxRepeatedCount maxUniqueCardsBeforeDuplicate",
        Game::Mines => "minesResult minesCount winMultiplier selected",
        Game::DragonTower => "towerResult towerDifficulty winMultiplier selected",
        Game::Chicken => "chickenResult chickenDifficulty winMultiplier selectedLane",
        _ => "userDiceDirection userValue resultValue",
    }
}

/// Keno and Plinko risks: the boards' names <-> Shuffle's (`classic` / `LOW_RISK` …).
fn risk_up(risk: &str) -> String {
    match risk {
        "classic" => "CLASSIC".into(),
        r => format!("{}_RISK", r.to_ascii_uppercase()),
    }
}

fn risk_down(risk: &str) -> String {
    risk.trim_end_matches("_RISK").to_ascii_lowercase()
}

/// A board's params (Stake's field names) -> Shuffle's input for a new round, without the stake.
fn bet_input(game: Game, p: &Value, auto: bool) -> Result<Value, String> {
    let difficulty = || p["difficulty"].as_str().unwrap_or("easy").to_ascii_uppercase();
    Ok(match game {
        Game::Dice => {
            let target = p["target"].as_f64().ok_or("Pick a target")?;
            let direction = if p["condition"].as_str() == Some("below") { "BELOW" } else { "ABOVE" };
            json!({ "bet": format!("{target:.2}"), "direction": direction })
        }
        // the boards number keno 0–39, Shuffle 1–40
        Game::Keno => {
            let picks: Vec<i64> = p["numbers"].as_array().map(|a| a.iter().filter_map(Value::as_i64).map(|n| n + 1).collect()).unwrap_or_default();
            if picks.is_empty() {
                return Err("Pick some numbers".into());
            }
            json!({ "risk": risk_up(p["risk"].as_str().unwrap_or("classic")), "selected": picks })
        }
        Game::Plinko => json!({ "risk": risk_up(p["risk"].as_str().unwrap_or("medium")), "rows": p["rows"].as_u64().unwrap_or(16) }),
        Game::Blitz => json!({ "picks": p["picks"].as_u64().unwrap_or(10) }),
        Game::Mines if auto => json!({ "minesCount": p["minesCount"].as_u64().unwrap_or(3), "selected": p["fields"].clone() }),
        Game::Mines => json!({ "minesCount": p["minesCount"].as_u64().unwrap_or(3) }),
        Game::DragonTower if auto => json!({ "difficulty": difficulty(), "selected": p["eggs"].clone() }),
        Game::DragonTower => json!({ "difficulty": difficulty() }),
        // the board counts lanes to cross, Shuffle wants the last lane (0-based)
        Game::Chicken if auto => {
            let lanes = p["round"].as_u64().unwrap_or(1).max(1);
            json!({ "difficulty": difficulty(), "selectedLane": lanes - 1 })
        }
        Game::Chicken => json!({ "difficulty": difficulty() }),
        _ => return Err(format!("{} isn't on Shuffle", game.name())),
    })
}

/// A board's next step -> Shuffle's input, continuing `open`.
fn next_input(game: Game, open: &Round, p: &Value) -> Result<Value, String> {
    Ok(match game {
        Game::Mines => json!({ "selected": p["fields"].clone() }),
        Game::DragonTower => {
            let row = open.state["tilesSelected"].as_array().map(Vec::len).unwrap_or(0);
            json!({ "selected": p["egg"].as_u64().ok_or("Pick a tile")?, "row": row, "betId": open.id })
        }
        // the next lane over
        Game::Chicken => json!({ "selectedLane": open.state["round"].as_u64().unwrap_or(0), "betId": open.id }),
        _ => return Err(format!("{} has no steps", game.name())),
    })
}

/// Whole numbers from a number, a list of them, or numeric strings.
fn ints(v: &Value) -> Vec<i64> {
    match v {
        Value::Array(a) => a.iter().filter_map(|x| x.as_i64().or_else(|| x.as_str()?.parse().ok())).collect(),
        Value::Number(n) => n.as_i64().into_iter().collect(),
        _ => vec![],
    }
}

/// A Shuffle `Bet` -> a round in the state shape of Stake's board for the game.
fn round_of(game: Game, bet: &Value) -> Option<Round> {
    let key = action_key(game);
    let actions: Vec<&Value> = bet["shuffleOriginalActions"].as_array()?.iter().map(|a| &a["action"][key]).filter(|a| a.is_object()).collect();
    let first = *actions.first()?;
    let multiplier = bet.get("multiplier").and_then(f64_from_json).unwrap_or(0.0);
    // the field only filled in once the round is over
    let outcome = |field: &str| actions.iter().rev().map(|a| &a[field]).find(|r| r.as_array().is_some_and(|x| !x.is_empty())).cloned();
    let difficulty = |field: &str| first[field].as_str().unwrap_or("EASY").to_ascii_lowercase();
    let (active, state) = match game {
        Game::Dice => {
            let condition = first["userDiceDirection"].as_str().unwrap_or("ABOVE").to_ascii_lowercase();
            (false, json!({ "result": f64_from_json(&first["resultValue"])?, "target": f64_from_json(&first["userValue"])?, "condition": condition }))
        }
        Game::Keno => {
            let down = |v: &Value| ints(v).into_iter().map(|n| n - 1).collect::<Vec<_>>();
            let risk = risk_down(first["risk"].as_str().unwrap_or("CLASSIC"));
            (false, json!({ "risk": risk, "drawnNumbers": down(&first["results"]), "selectedNumbers": down(&first["selected"]) }))
        }
        // one 0/1 per row, 1 bouncing right
        Game::Plinko => {
            let path: Vec<&str> = ints(&first["results"]).into_iter().map(|b| if b == 1 { "R" } else { "L" }).collect();
            (false, json!({ "risk": risk_down(first["risk"].as_str().unwrap_or("MEDIUM_RISK")), "rows": first["rows"], "path": path }))
        }
        Game::Blitz => (false, json!({ "cards": first["cards"], "picks": first["picks"], "maxRepeatedCount": first["maxRepeatedCount"] })),
        // one action per revealed tile (all of them at once for an auto bet)
        Game::Mines => {
            let count = first["minesCount"].as_u64().unwrap_or(0) as u32;
            let mines = outcome("minesResult").map(|r| ints(&r)).unwrap_or_default();
            let picks: Vec<i64> = actions.iter().flat_map(|a| ints(&a["selected"])).collect();
            (mines.is_empty(), mines_state(count, &picks, &mines))
        }
        // the layout (safe columns per row) comes with the end of the round
        Game::DragonTower => {
            let picks: Vec<i64> = actions.iter().flat_map(|a| ints(&a["selected"])).collect();
            let layout = outcome("towerResult");
            let state = json!({
                "difficulty": difficulty("towerDifficulty"),
                "tilesSelected": picks,
                "currentRound": picks.len(),
                "rounds": layout.clone().unwrap_or(json!([])),
                "playedRounds": [],
            });
            (layout.is_none(), state)
        }
        // the lanes with cars come with the end of the round
        Game::Chicken => {
            let lane = actions.iter().filter_map(|a| a["selectedLane"].as_i64()).max();
            let cars = outcome("chickenResult").map(|r| ints(&r));
            // lanes crossed: up to the first car on a loss, else every lane picked
            let crossed = match (&cars, lane) {
                (Some(c), _) if multiplier <= 0.0 => c.iter().min().copied().unwrap_or(0),
                (_, Some(l)) => l + 1,
                _ => 0,
            };
            let now = actions.last().and_then(|a| f64_from_json(&a["winMultiplier"])).unwrap_or(multiplier);
            (cars.is_none(), json!({ "difficulty": difficulty("chickenDifficulty"), "round": crossed, "payoutMultiplier": now }))
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
        multiplier,
        state,
    })
}

/// Plain HTTPS to the mirror, looking like the site's own requests.
fn direct_client(mirror: &str) -> Result<reqwest::Client, String> {
    use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, ACCEPT_LANGUAGE, ORIGIN, REFERER};
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/json"));
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(ORIGIN, HeaderValue::from_str(&format!("https://{mirror}")).map_err(|e| e.to_string())?);
    headers.insert(REFERER, HeaderValue::from_str(&format!("https://{mirror}/")).map_err(|e| e.to_string())?);
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        // header names as browsers write them: Cloudflare turns away lowercase ones over HTTP/1.1
        .http1_title_case_headers()
        // keeps the cookies Cloudflare hands out
        .cookie_store(true)
        .default_headers(headers)
        .gzip(true)
        .build()
        .map_err(|e| format!("Could not set up the connection: {e}"))
}

/// Shuffle answers `{data, errors}`; anything that isn't JSON came from Cloudflare.
fn parse_reply(status: u16, body: &str) -> Result<Value, CallError> {
    let Ok(v) = serde_json::from_str::<Value>(body) else {
        if status == 429 {
            return Err(CallError::Casino(CasinoError::Busy("Too many requests".into())));
        }
        let challenge = ["challenge-platform", "Just a moment", "Attention Required", "cf-chl"].iter().any(|k| body.contains(k));
        if challenge || status == 403 || status == 503 {
            return Err(CallError::Cloudflare);
        }
        if body.trim_start().starts_with('<') {
            return Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status} error page"))));
        }
        let snippet: String = body.chars().take(160).collect();
        return Err(CallError::Casino(CasinoError::BadReply(format!("HTTP {status}: {snippet}"))));
    };
    if let Some(e) = v.get("errors").and_then(Value::as_array).and_then(|a| a.first()) {
        return Err(error_of(e.get("message").and_then(Value::as_str).unwrap_or("Unknown error")));
    }
    if status == 429 {
        return Err(CallError::Casino(CasinoError::Busy("Too many requests".into())));
    }
    Ok(v.get("data").cloned().unwrap_or(Value::Null))
}

/// Shuffle's errors carry a code as the message (`BALANCE_NOT_ENOUGH`).
fn error_of(code: &str) -> CallError {
    match code {
        "UNAUTHENTICATED" => CallError::Unauthenticated,
        "INVALID_REFRESH_TOKEN" | "REFRESH_TOKEN_EXPIRED" | "USER_SESSION_NOT_FOUND" | "INVALID_ACCESS_TOKEN" => {
            CallError::Casino(CasinoError::Server(EXPIRED.into()))
        }
        "TOO_MANY_CONCURRENT_REQUESTS" | "TOO_MANY_REQUEST" | "TOO_MANY_REQUESTS" => CallError::Casino(CasinoError::Busy("Please slow down".into())),
        "ACTIVE_BET_EXISTS" | "ACTIVE_MINES_EXISTS" | "INCOMPLETE_BET" | "GAME_IN_PROGRESS" => CallError::Casino(CasinoError::RoundOpen),
        "INVALID_ACTIVE_BET" => CallError::Casino(CasinoError::Server("No open round".into())),
        c => CallError::Casino(CasinoError::Server(readable(c))),
    }
}

/// A `cachedGames` node as a slot tile. Producers the drivers know take their keys, so the
/// right driver opens them; the rest keep Shuffle's id and show Shuffle's name.
fn slot_game(n: &Value) -> Option<crate::slots::SlotGame> {
    let theirs = n.pointer("/provider/id")?.as_str()?;
    let known = PROVIDERS.iter().find(|(_, t)| *t == theirs).map(|(ours, _)| *ours);
    let key = n.pointer("/image/key").and_then(Value::as_str);
    Some(crate::slots::SlotGame {
        id: n.get("slug")?.as_str()?.to_string(),
        name: n.get("name")?.as_str()?.to_string(),
        producer: known.unwrap_or(theirs).to_string(),
        image: key.map(|k| format!("https://shuffle-com.imgix.net/{k}?{THUMB}")).unwrap_or_default(),
        demo: n.get("demoMode").and_then(Value::as_bool).unwrap_or(false),
        rtp: n.get("edge").and_then(f64_from_json).map_or(0.0, |e| 100.0 - e),
        provider_game_id: None,
        studio: if known.is_some() { None } else { n.pointer("/provider/name").and_then(Value::as_str).map(str::to_string) },
    })
}

/// The site's own English for the codes a bet can meet; any other code reads as words.
fn readable(code: &str) -> String {
    let text = match code {
        "GAME_NOT_AVAILABLE_IN_COUNTRY" => "This game isn't available from your region",
        "BALANCE_NOT_ENOUGH" => "Insufficient balance",
        "INVALID_AMOUNT" => "Enter a valid amount",
        "AMOUNT_TOO_LARGE" => "Amount too large",
        "LIMBO_GAME_BOUNDS" => "Multiplier out of bounds",
        "EXCEED_MAX_PAYOUT_LIMIT" | "ESTIMATED_PAYOUT_EXCEEDED_MAX_PAYOUT" => "Max payout exceeded. Reduce stake.",
        "DISABLED_CURRENCY" => "The currency provided is disabled.",
        "NOT_ALLOW_FIAT_CURRENCY" => "Fiat currency is not allowed",
        "GEO_REGION_RESTRICTED" => "Shuffle isn't available from your region",
        "WITHDRAWAL_MODE_ONLY" => "Your account is in withdrawal-only mode",
        "LOSS_LIMIT_EXCEED" => "You have exceeded your loss limit",
        "ROLLOVER_MAX_BET_EXCEED" => "Bet amount must not exceed maximum during rollover.",
        "ROLLOVER_RESTRICTED_GAME" => "You are not allowed to play this game.",
        "USER_SELF_EXCLUDED" => "You have self-excluded",
        "USER_SUSPENDED" => "Your account is suspended. Contact support for help.",
        "EMAIL_UNVERIFIED" => "Email has not been verified",
        "GAME_GENERIC_ERROR" | "ERROR_GENERIC" | "ERROR_OCCURRED" => "Shuffle couldn't place the bet. Try again.",
        _ => "",
    };
    if !text.is_empty() {
        return text.into();
    }
    if !code.is_empty() && code.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_') {
        let words = code.replace('_', " ").to_ascii_lowercase();
        let mut chars = words.chars();
        return chars.next().map(|f| f.to_ascii_uppercase().to_string() + chars.as_str()).unwrap_or_default();
    }
    code.to_string()
}

/// The seed pair in use, from one seed or a list of them (the active one and the next).
fn provable_from(v: &Value) -> ProvableState {
    let seed = match v {
        Value::Array(list) => list.iter().find(|s| s["status"].as_str().is_some_and(|st| st.eq_ignore_ascii_case("active"))).or_else(|| list.first()),
        Value::Object(_) => Some(v),
        _ => None,
    };
    let Some(s) = seed else { return ProvableState::default() };
    ProvableState {
        client_seed: s.get("clientSeed").and_then(Value::as_str).map(str::to_string),
        server_seed_hash: s.get("hashedSeed").and_then(Value::as_str).map(str::to_string),
        nonce: s.get("currentNonce").and_then(|n| n.as_u64().or_else(|| n.as_str().and_then(|s| s.parse().ok()))),
    }
}

/// "0.00012345" (or a number) -> 12345 base units. Digits past the 8th decimal are dropped.
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

/// A random id the way the site makes its browser and window ids: a number below 10^15 in base 32.
fn random_id() -> String {
    use rand::Rng;
    base32(rand::rng().random_range(0..=1_000_000_000_000_000))
}

/// JavaScript's `n.toString(32)`.
fn base32(mut n: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuv";
    let mut out = Vec::new();
    loop {
        out.push(DIGITS[(n % 32) as usize]);
        n /= 32;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
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
    fn rewards_and_challenges() {
        // 2026-10-16 11:00 UTC; replies as Shuffle sent them that day
        let now = std::time::UNIX_EPOCH + Duration::from_secs(1_792_148_400);
        let agg = json!({"dailyRakeback":{"nextClaimDate":"2026-10-17T00:00:00.000Z","eligible":true,"currencyAmounts":[{"amount":"0.04759564","currency":"TRX"}]},
            "weeklyBonus":{"usdValue":"0","nextClaimDate":"2026-10-22T11:00:00.000Z","eligible":true},"monthlyBonus":{"usdValue":"0","nextClaimDate":"2026-11-06T00:00:00.000Z","eligible":true},
            "instantRakebackBonus":[{"currency":"BTC","amount":"0"},{"currency":"TRX","amount":"0.11898911"},{"currency":"USDT","amount":"0.002055"}],
            "vipUpgradeBonuses":[],"challengeRewards":[]});
        let price = |c: &str| match c.to_ascii_lowercase().as_str() { "trx" => 0.3, "usdt" => 1.0, _ => 0.0 };
        let list = rewards_of(&agg, &price, now);
        assert_eq!(list.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["rakeback", "daily-rakeback", "weekly", "monthly"]);
        assert!(list[0].claimable && (list[0].usd.unwrap() - (0.11898911 * 0.3 + 0.002055)).abs() < 1e-9);
        assert_eq!(list[0].detail, "0.11898911 TRX + 0.002055 USDT");
        // what's there can be claimed until the next one comes
        assert!(list[1].claimable && list[1].expires == parse_iso("2026-10-17T00:00:00Z") && list[1].title == "Daily rakeback");
        // nothing yet: waiting for the date
        assert!(!list[2].claimable && list[2].next_at == parse_iso("2026-10-22T11:00:00Z"));
        assert!(list.iter().all(|r| r.link.is_none()));

        let ch = json!({"challenges":{"nodes":[
            {"id":"8dca","game":{"id":"e6c4","name":"Max The Mathmagician","slug":"hacksaw-max-the-mathmagician","image":{"key":"b2c6"},"provider":{"id":"kitsunegaming","name":"Kitsune Gaming"},"edge":"3.66","demoMode":true},"usdMinBet":"1","minMultiplier":3000,"currency":"USDC","amount":"850","referredOnly":false,"completed":false},
            {"id":"29da","game":{"id":"267b","name":"F* Louvre","slug":"7-rings-f-louvre","image":{"key":"20e6"},"provider":{"id":"7rings","name":"7rings"},"edge":"4"},"usdMinBet":"0.4","minMultiplier":10000,"currency":"USDC","amount":"1250","referredOnly":false,"completed":false},
            {"id":"done","game":{"id":"x","name":"X","slug":"x","provider":{"id":"p","name":"P"}},"usdMinBet":"1","minMultiplier":10,"currency":"USDC","amount":"5","completed":true}]}});
        let list = challenges_of(&ch, &|_: &str| 1.0);
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].game.id.as_str(), list[0].target, list[0].prize_usd, list[0].min_bet_usd), ("7-rings-f-louvre", 10_000.0, 1250.0, 0.4));
        assert!(list[1].game.image.starts_with("https://shuffle-com.imgix.net/b2c6"));
    }

    #[test]
    fn vip_from_xp() {
        // the account's reply and a slice of the ladder, as Shuffle sends them
        let ladder = vip_ladder(&json!({"vipLevels": [{"level": "GOLD_4", "amount": "250000"}, {"level": "GOLD_3", "amount": "200000"}, {"level": "UNRANKED", "amount": "0"}]}));
        assert_eq!(ladder[0], ("UNRANKED".to_string(), 0.0));
        let me = json!({"vipLevel": "GOLD_3", "xp": 224678, "usdWagered": "224668.14819016"});
        let v = vip_status(&me, &ladder).unwrap();
        assert_eq!(v.rank, "Gold 3");
        assert_eq!(v.next_rank, Some(("Gold 4".to_string(), 250_000.0)));
        assert!((v.rank_progress() - 24_678.0 / 50_000.0).abs() < 1e-9);
        assert!((v.xp_per_usd.unwrap() - 1.0).abs() < 1e-3);
        assert!(v.level.is_none());
        // past the top of the ladder
        let top = vip_status(&json!({"vipLevel": "GOLD_4", "xp": 300000, "usdWagered": "300000"}), &ladder).unwrap();
        assert!(top.next_rank.is_none() && top.rank_progress() == 1.0);
        assert_eq!(level_name("UNRANKED"), "Unranked");
    }

    /// Every originals request as it goes out, one JSON line each, to check against the live
    /// schema: `cargo test shuffle_queries -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn shuffle_queries() {
        let games = [Game::Dice, Game::Keno, Game::Plinko, Game::Blitz, Game::Mines, Game::DragonTower, Game::Chicken];
        for game in games {
            for op in [Op::Start, Op::Auto, Op::Next, Op::Cashout, Op::Active] {
                let Some((key, field, input)) = op_names(game, op) else { continue };
                let fields = format!("{BET_FIELDS} shuffleOriginalActions {{ action {{ {} {{ {} }} }} }}", action_key(game), action_fields(game));
                let q = match (op, input) {
                    (Op::Active, _) => format!("query Fastbet{key} {{ {field} {{ {fields} }} }}"),
                    (_, Some(input)) => format!("mutation Fastbet{key}($data: {input}!) {{ {field}(data: $data) {{ {fields} }} }}"),
                    (_, None) => format!("mutation Fastbet{key} {{ {field} {{ {fields} }} }}"),
                };
                println!("{}", json!({ "name": key, "query": q }));
            }
        }
        println!("{}", json!({ "name": "Limbo", "query": LIMBO }));
    }

    #[test]
    fn mirrors_are_unique_bare_hosts() {
        let mut seen = HashSet::new();
        for m in MIRRORS {
            assert_eq!(stake::normalize_mirror(m), *m, "{m} is not a bare lowercase host");
            assert!(seen.insert(*m), "{m} listed twice");
        }
        assert_eq!(MIRRORS[0], DEFAULT_MIRROR);
    }



    #[test]
    fn replies() {
        let ok = parse_reply(200, r#"{"data":{"me":{"username":"x"}}}"#).ok().unwrap();
        assert_eq!(ok["me"]["username"], "x");
        let unauth = r#"{"errors":[{"message":"UNAUTHENTICATED","extensions":{"code":"UNAUTHENTICATED"}}],"data":null}"#;
        assert!(matches!(parse_reply(200, unauth), Err(CallError::Unauthenticated)));
        let spent = r#"{"errors":[{"message":"INVALID_REFRESH_TOKEN"}],"data":null}"#;
        assert!(matches!(parse_reply(200, spent), Err(CallError::Casino(CasinoError::Server(m))) if m == EXPIRED));
        let busy = r#"{"errors":[{"message":"TOO_MANY_CONCURRENT_REQUESTS"}]}"#;
        assert!(matches!(parse_reply(200, busy), Err(CallError::Casino(e)) if e.is_busy()));
        let poor = r#"{"errors":[{"message":"BALANCE_NOT_ENOUGH"}]}"#;
        assert!(matches!(parse_reply(200, poor), Err(CallError::Casino(CasinoError::Server(m))) if m == "Insufficient balance"));
        assert!(matches!(parse_reply(403, "<html>Just a moment...</html>"), Err(CallError::Cloudflare)));
        assert_eq!(readable("SOME_NEW_CODE"), "Some new code");
        assert_eq!(readable("Invalid request!"), "Invalid request!");
    }

    #[test]
    fn seeds_and_amounts() {
        let list = json!([{ "clientSeed": "next", "hashedSeed": "h2", "status": "NEXT", "currentNonce": 0 },
                          { "clientSeed": "cs", "hashedSeed": "h1", "status": "ACTIVE", "currentNonce": "42" }]);
        let st = provable_from(&list);
        assert_eq!((st.client_seed.as_deref(), st.server_seed_hash.as_deref(), st.nonce), (Some("cs"), Some("h1"), Some(42)));
        assert_eq!(coin_units(&json!("1.234567891")), Some(123_456_789));
        assert_eq!(coin_units(&json!("0")), Some(0));
        assert_eq!(format_units(100, DECIMALS, DECIMALS), "0.000001");
        assert_eq!(base32(0), "0");
        assert_eq!(base32(1_791_740_000_000), "1k4m0ddo0");
        assert!(random_id().chars().all(|c| c.is_ascii_digit() || ('a'..='v').contains(&c)));
    }

    #[test]
    fn slot_tiles_take_the_drivers_names() {
        let n = json!({ "slug": "hacksaw-le-vampire", "name": "Le Vampire", "demoMode": true, "edge": "3.64", "image": { "key": "k" }, "provider": { "id": "hacksaw", "name": "Hacksaw Gaming" } });
        let g = slot_game(&n).unwrap();
        assert_eq!((g.id.as_str(), g.producer.as_str(), g.studio, g.demo), ("hacksaw-le-vampire", "hacksawgaming", None, true));
        assert!((g.rtp - 96.36).abs() < 1e-9);
        assert!(g.image.starts_with("https://shuffle-com.imgix.net/k?"));
        // a studio no driver plays keeps Shuffle's id and shows Shuffle's name
        let n = json!({ "slug": "x", "name": "X", "demoMode": false, "edge": "4", "provider": { "id": "redtiger", "name": "Red Tiger" } });
        let g = slot_game(&n).unwrap();
        assert_eq!((g.producer.as_str(), g.studio.as_deref()), ("redtiger", Some("Red Tiger")));
        assert!(crate::slots::Provider::for_producer(&g.producer).is_none());
        let n = json!({ "slug": "y", "name": "Y", "provider": { "id": "bullsharkgames", "name": "Bullshark Games" } });
        assert_eq!(crate::slots::Provider::for_producer(&slot_game(&n).unwrap().producer), Some(crate::slots::Provider::Hacksaw));
    }
}
