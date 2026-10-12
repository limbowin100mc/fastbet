//! Casino back-ends. Every casino implements [`Casino`]; the engine and UI
//! only ever talk to the trait, so adding a casino means adding one module
//! here and one variant to [`CasinoId`].

pub mod chipsgg;
pub mod creds;
pub mod gambulls;
pub mod goated;
pub mod shuffle;
pub mod stake;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::cloudflare::Clearance;
use crate::money::Units;
use crate::originals::{Game, Round, Step, Tables};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CasinoId {
    ChipsGg,
    Stake,
    Shuffle,
    Goated,
    Gambulls,
}

impl CasinoId {
    pub const ALL: &'static [CasinoId] = &[CasinoId::ChipsGg, CasinoId::Stake, CasinoId::Shuffle, CasinoId::Goated, CasinoId::Gambulls];

    pub fn key(self) -> &'static str {
        match self {
            CasinoId::ChipsGg => "chipsgg",
            CasinoId::Stake => "stake",
            CasinoId::Shuffle => "shuffle",
            CasinoId::Goated => "goated",
            CasinoId::Gambulls => "gambulls",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CasinoId::ChipsGg => "Chips.gg",
            CasinoId::Stake => "Stake.com",
            CasinoId::Shuffle => "Shuffle.com",
            CasinoId::Goated => "Goated.com",
            CasinoId::Gambulls => "Gambulls.com",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|id| id.key() == key)
    }

    /// Domains the casino can be reached on; empty when there is only one.
    pub fn mirrors(self) -> &'static [&'static str] {
        match self {
            CasinoId::ChipsGg | CasinoId::Goated | CasinoId::Gambulls => &[],
            CasinoId::Stake => stake::MIRRORS,
            CasinoId::Shuffle => shuffle::MIRRORS,
        }
    }

    pub fn default_mirror(self) -> Option<&'static str> {
        self.mirrors().first().copied()
    }

    /// Sits behind Cloudflare checks, so the user can hand over a clearance cookie from their browser.
    pub fn behind_cloudflare(self) -> bool {
        matches!(self, CasinoId::Stake)
    }

    /// Takes bets of 0: they play a real round that pays nothing (Stake, Shuffle, Goated, chips.gg).
    pub fn zero_bets(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated)
    }

    /// Whether the account can also be linked through the embedded login window.
    pub fn window_login(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls)
    }

    /// Linked through the login window: the casino has no API keys to paste (Shuffle, Goated, Gambulls).
    pub fn window_only(self) -> bool {
        matches!(self, CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls)
    }

    /// The site's tokens can also be pasted by hand: the access token and the refresh token (Goated).
    pub fn manual_tokens(self) -> bool {
        matches!(self, CasinoId::Goated)
    }

    /// A cookie the login window reads from its browser (it is HttpOnly, out of the page's reach)
    /// and hands over as the refresh token: (a URL it is sent to, its name).
    pub fn login_cookie(self) -> Option<(&'static str, &'static str)> {
        match self {
            CasinoId::Goated => Some(("https://apis.goated.com/user/auth/token/refresh", goated::REFRESH_COOKIE)),
            _ => None,
        }
    }

    /// The login is a cookie the login window reads from its browser (HttpOnly) and hands over
    /// as the token once it exists: (a URL it is sent to, its name).
    pub fn cookie_login(self) -> Option<(&'static str, &'static str)> {
        match self {
            CasinoId::Gambulls => Some(("https://gambulls.com/", gambulls::SESSION_COOKIE)),
            _ => None,
        }
    }

    /// Slot producers the app can autoplay here, in lobby order. Empty: no slots.
    pub fn slot_producers(self) -> &'static [&'static str] {
        match self {
            CasinoId::ChipsGg => &["hacksawgaming", "avatarux", "bgaming"],
            CasinoId::Stake => &[crate::slots::STAKE_ENGINE],
            CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls => &[],
        }
    }

    pub fn has_slots(self) -> bool {
        !self.slot_producers().is_empty()
    }

    /// Originals besides Limbo the app can play here, in lobby order.
    pub fn originals(self) -> Vec<Game> {
        match self {
            CasinoId::Gambulls => vec![],
            CasinoId::ChipsGg => vec![Game::Keno, Game::Mines, Game::Hilo],
            CasinoId::Stake => Game::stake().collect(),
            CasinoId::Goated => vec![Game::Dice, Game::Mines],
            CasinoId::Shuffle => vec![Game::Dice, Game::Mines, Game::Keno, Game::Plinko, Game::Blitz, Game::DragonTower, Game::Chicken],
        }
    }

    /// An original's name here (Shuffle calls its tower "Waifu Tower").
    pub fn original_name(self, game: Game) -> &'static str {
        match (self, game) {
            (CasinoId::Shuffle, Game::DragonTower) => "Waifu Tower",
            _ => game.name(),
        }
    }

    /// HiLo the chips.gg way: red / black and card-kind guesses besides higher and lower, and
    /// the 1% edge taken once per round instead of on every guess.
    pub fn hilo_side_guesses(self) -> bool {
        matches!(self, CasinoId::ChipsGg)
    }

    /// Dice win chances accepted here, in percent (Goated pays at most 990×).
    pub fn dice_chance(self) -> (f64, f64) {
        match self {
            CasinoId::Goated => (0.1, 98.0),
            _ => (0.01, 98.0),
        }
    }

    /// Plinko's risk levels here, as (key, label).
    pub fn plinko_risks(self) -> &'static [(&'static str, &'static str)] {
        match self {
            CasinoId::Shuffle => &[("low", "Low"), ("medium", "Medium"), ("high", "High")],
            _ => &[("low", "Low"), ("medium", "Medium"), ("high", "High"), ("expert", "Expert")],
        }
    }

    /// Slot artwork is landscape (chips.gg's 4:3 thumbnails), so the lobby's tiles are too.
    pub fn slot_art_landscape(self) -> bool {
        matches!(self, CasinoId::ChipsGg)
    }

    /// The slot catalog lists only games the app can play (Stake: Stake Engine games),
    /// so the lobby has no tested / untested split.
    pub fn slots_curated(self) -> bool {
        matches!(self, CasinoId::Stake)
    }

    /// What the casino calls the credential the user pastes.
    pub fn key_label(self) -> &'static str {
        match self {
            CasinoId::ChipsGg => "API key",
            CasinoId::Stake => "API token",
            CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls => "login",
        }
    }

    /// Page the embedded login window opens, on the given mirror.
    pub fn login_url(self, mirror: Option<&str>) -> String {
        match self {
            CasinoId::ChipsGg => "https://chips.gg/".into(),
            CasinoId::Stake => format!("https://{}/", mirror.unwrap_or(stake::DEFAULT_MIRROR)),
            CasinoId::Shuffle => format!("https://{}/", mirror.unwrap_or(shuffle::DEFAULT_MIRROR)),
            CasinoId::Goated => "https://www.goated.com/".into(),
            // straight to the site's login dialog
            CasinoId::Gambulls => "https://gambulls.com/en?login=open".into(),
        }
    }

    /// Where the user creates an API key, on the given mirror.
    pub fn api_key_url(self, mirror: Option<&str>) -> String {
        match self {
            CasinoId::ChipsGg => "https://chips.gg/settings/api-keys".into(),
            CasinoId::Stake => format!("https://{}/settings/security", mirror.unwrap_or(stake::DEFAULT_MIRROR)),
            CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls => self.login_url(mirror),
        }
    }

    /// JavaScript injected into the login window. It must post
    /// `{"token": "...", "loggedIn": bool}` through `window.ipc.postMessage`
    /// whenever either value changes. `loggedIn` has to mean "logged in as a
    /// user", not "has a guest token".
    pub fn token_watch_script(self) -> &'static str {
        match self {
            // chips.gg keeps the same tokenid before and after login (it is
            // upgraded from guest to user server-side), so the token alone says
            // nothing. We watch the site's own websocket instead: private-channel
            // events and a successful auth.login reply only happen for a user.
            CasinoId::ChipsGg => {
                r#"(function () {
  if (window.top !== window || window.__fastbetWatch) return;
  window.__fastbetWatch = true;
  var loggedIn = false, last = '', pendingLogin = {}, pendingMe = {}, sockets = 0, sent = 0, frames = 0, priv = 0, kinds = {};
  function outgoingOne(m) {
    if (!Array.isArray(m)) return;
    if (m[0] === 'auth' && m[2] === 'login') pendingLogin[m[1]] = true;
    if (m[0] === 'private' && m[2] === 'me') pendingMe[m[1]] = true;
    kinds['out:' + m[0] + '/' + m[2]] = 1;
  }
  function outgoing(data) {
    try {
      sent++;
      if (typeof data !== 'string') return;
      var m = JSON.parse(data);
      // frames are either one message or a batch of messages
      if (Array.isArray(m) && Array.isArray(m[0])) { for (var i = 0; i < m.length; i++) outgoingOne(m[i]); }
      else outgoingOne(m);
    } catch (e) {}
  }
  function incoming(data) {
    try {
      frames++;
      if (typeof data !== 'string') { kinds[Object.prototype.toString.call(data)] = 1; return; }
      var msgs = JSON.parse(data);
      if (!Array.isArray(msgs)) { kinds['notarray:' + data.slice(0, 40)] = 1; return; }
      for (var i = 0; i < msgs.length; i++) {
        var m = msgs[i];
        if (!Array.isArray(m)) { kinds['item:' + JSON.stringify(m).slice(0, 60)] = 1; continue; }
        var shape = m[0] + '/' + (m[1] === null ? 'null' : typeof m[1]) + '/' + (Array.isArray(m[2]) ? 'arr' + m[2].length + ':' + JSON.stringify(m[2][0]).slice(0, 40) : typeof m[2]) + '/' + m.length;
        kinds[shape] = (kinds[shape] || 0) + 1;
        if (m[0] === 'private' && m[1] == null) { priv++; loggedIn = true; }
        if (m[0] === 'auth' && pendingLogin[m[1]] && !m[3]) loggedIn = true;
        if (m[0] === 'private' && pendingMe[m[1]] && m[2] && typeof m[2] === 'object' && m[2].id) loggedIn = true;
      }
    } catch (e) {}
  }
  var NativeWS = window.WebSocket;
  function WS(url, protocols) {
    sockets++;
    var ws = protocols === undefined ? new NativeWS(url) : new NativeWS(url, protocols);
    return ws;
  }
  WS.prototype = NativeWS.prototype;
  WS.CONNECTING = 0; WS.OPEN = 1; WS.CLOSING = 2; WS.CLOSED = 3;
  window.WebSocket = WS;
  var P = window.WebSocket.prototype;
  var send = P.send;
  P.send = function (data) { outgoing(data); return send.apply(this, arguments); };
  var add = P.addEventListener;
  P.addEventListener = function (type, fn, opts) {
    if (type === 'message' && typeof fn === 'function') {
      var f = fn;
      fn = function (ev) { incoming(ev.data); return f.call(this, ev); };
    }
    return add.call(this, type, fn, opts);
  };
  var desc = Object.getOwnPropertyDescriptor(P, 'onmessage');
  if (desc && desc.set) {
    Object.defineProperty(P, 'onmessage', {
      configurable: true,
      get: function () { return desc.get.call(this); },
      set: function (fn) {
        var f = fn;
        desc.set.call(this, typeof f === 'function' ? function (ev) { incoming(ev.data); return f.call(this, ev); } : f);
      }
    });
  }
  setInterval(function () {
    try {
      var msg = JSON.stringify({ token: localStorage.getItem('tokenid') || '', loggedIn: loggedIn,
        debug: { sockets: sockets, sent: sent, frames: frames, priv: priv, kinds: kinds } });
      if (msg !== last) { last = msg; window.ipc.postMessage(msg); }
    } catch (e) {}
  }, 400);
})();"#
            }
            // Shuffle keeps its login in localStorage as the redux-persist `auth` slice:
            // an access token (JWT) and the refresh token that renews it. Logged in means
            // both are there and neither has run out, the site's own test.
            CasinoId::Shuffle => {
                r#"(function () {
  if (window.top !== window || window.__fastbetWatch) return;
  window.__fastbetWatch = true;
  var last = '';
  function claims(jwt) {
    var s = String(jwt).split('.')[1].replace(/-/g, '+').replace(/_/g, '/');
    while (s.length % 4) s += '=';
    return JSON.parse(atob(s));
  }
  function live(jwt, strict) {
    try { var exp = claims(jwt).exp; return !exp || exp > Date.now() / 1000 + 5; } catch (e) { return !strict; }
  }
  setInterval(function () {
    try {
      var root = localStorage.getItem('persist:root') || localStorage.getItem('persist:us:root');
      var auth = root ? JSON.parse(JSON.parse(root).auth || '{}') : {};
      var a = auth.accessToken || '', r = auth.refreshToken || '';
      var ok = !!(a && r && live(a, true) && live(r, false));
      var msg = JSON.stringify({ token: ok ? JSON.stringify({ accessToken: a, refreshToken: r }) : '', loggedIn: ok });
      if (msg !== last) { last = msg; window.ipc.postMessage(msg); }
    } catch (e) {}
  }, 400);
})();"#
            }
            // Goated keeps the access token in localStorage `g3_token` (JSON-quoted); the
            // refresh token is an HttpOnly cookie the window adds itself (`login_cookie`).
            // Reported every second while logged in, so the window looks for the cookie again
            // until it is there.
            CasinoId::Goated => {
                r#"(function () {
  if (window.top !== window || window.__fastbetWatch) return;
  window.__fastbetWatch = true;
  var last = '', n = 0;
  function live(jwt) {
    try {
      var s = String(jwt).split('.')[1].replace(/-/g, '+').replace(/_/g, '/');
      while (s.length % 4) s += '=';
      var exp = JSON.parse(atob(s)).exp;
      return !exp || exp > Date.now() / 1000 + 5;
    } catch (e) { return false; }
  }
  setInterval(function () {
    try {
      var raw = localStorage.getItem('g3_token') || '';
      var token = '';
      try { token = raw ? JSON.parse(raw) : ''; } catch (e) { token = raw; }
      var ok = !!(token && typeof token === 'string' && live(token));
      var msg = JSON.stringify({ token: ok ? token : '', loggedIn: ok, tick: ok ? n++ : 0 });
      if (msg !== last) { last = msg; window.ipc.postMessage(msg); }
    } catch (e) {}
  }, 1000);
})();"#
            }
            // Gambulls' login is an HttpOnly cookie the window reads itself (`cookie_login`); the
            // page only has to report now and then so the window looks for it.
            CasinoId::Gambulls => {
                r#"(function () {
  if (window.top !== window || window.__fastbetWatch) return;
  window.__fastbetWatch = true;
  setInterval(function () {
    try { window.ipc.postMessage(JSON.stringify({ token: '', loggedIn: false })); } catch (e) {}
  }, 1000);
})();"#
            }
            // Stake links with an API token only
            CasinoId::Stake => "",
        }
    }

    /// Where the user finds the login token to paste into the app.
    pub fn token_hint(self) -> &'static str {
        match self {
            CasinoId::ChipsGg => "On chips.gg open Settings → API Keys → Create API Key, copy the key (it is shown only once) and paste it here.",
            CasinoId::Stake => "On Stake open Settings → Security → API Tokens, create a token, copy it and paste it here. Or log in with your browser below. It is stored on this PC only.",
            CasinoId::Shuffle => "Shuffle has no API keys: log in to Shuffle in the window that opens with the button below. FastBet takes the login from the site, so your password never reaches the app. It is stored on this PC only.",
            CasinoId::Goated => "Goated has no API keys: log in to Goated in the window that opens with the button below. FastBet takes the login from the site, so your password never reaches the app. It is stored on this PC only.",
            CasinoId::Gambulls => "Gambulls has no API keys: log in to Gambulls in the window that opens with the button below. FastBet takes the login from the site, so your password never reaches the app. It is stored on this PC only.",
        }
    }

    pub fn min_target(self) -> f64 {
        match self {
            CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls => 1.01,
        }
    }

    pub fn max_target(self) -> f64 {
        match self {
            CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls => 1_000_000.0,
        }
    }

    /// Payout multiplier at which the house edge is already deducted (99% RTP).
    pub fn win_chance(self, target: f64) -> f64 {
        match self {
            CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls => 99.0 / target,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrencyInfo {
    pub decimals: u32,
    pub price: f64,
    pub hidden: bool,
}

/// Everything the UI needs from the casino's live state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Market {
    pub currencies: BTreeMap<String, CurrencyInfo>,
    pub wallets: BTreeMap<String, Units>,
    /// Currency the user has selected on the site.
    pub site_currency: Option<String>,
    pub username: Option<String>,
}

impl Market {
    pub fn info(&self, cur: &str) -> Option<&CurrencyInfo> {
        self.currencies.get(cur)
    }
    pub fn balance(&self, cur: &str) -> Option<Units> {
        self.wallets.get(cur).copied()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProvableState {
    pub client_seed: Option<String>,
    pub server_seed_hash: Option<String>,
    pub nonce: Option<u64>,
}

impl ProvableState {
    pub fn from_json(v: &Value) -> Self {
        let get = |keys: &[&str]| keys.iter().find_map(|k| v.get(k)).cloned();
        Self {
            client_seed: get(&["clientSeed", "client_seed"]).and_then(|x| x.as_str().map(str::to_string)),
            server_seed_hash: get(&["serverSeedHash", "hashedServerSeed", "serverSeedHashed", "server_seed_hash"])
                .and_then(|x| x.as_str().map(str::to_string)),
            nonce: get(&["nonce"]).and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnStatus {
    Connecting,
    Authenticating,
    Ready,
    NoToken,
    AuthFailed,
    Offline,
}

impl ConnStatus {
    pub fn is_ready(self) -> bool {
        self == ConnStatus::Ready
    }
    pub fn is_bad(self) -> bool {
        matches!(self, ConnStatus::NoToken | ConnStatus::AuthFailed | ConnStatus::Offline)
    }
    pub fn short(self) -> &'static str {
        match self {
            ConnStatus::Connecting => "Connecting",
            ConnStatus::Authenticating => "Logging in",
            ConnStatus::Ready => "Ready",
            ConnStatus::NoToken => "No token",
            ConnStatus::AuthFailed => "Login failed",
            ConnStatus::Offline => "Offline",
        }
    }
    pub fn long(self, detail: &str) -> String {
        match self {
            ConnStatus::Connecting => "Connecting…".into(),
            ConnStatus::Authenticating => "Logging in…".into(),
            ConnStatus::Ready => "Ready".into(),
            ConnStatus::NoToken => "No login token — paste one in the Account section".into(),
            ConnStatus::AuthFailed => format!("Login failed: {detail}"),
            ConnStatus::Offline => "Disconnected — reconnecting…".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct BetRequest {
    pub currency: String,
    pub amount: Units,
    pub target: f64,
}

/// The server's answer to a bet, normalised. Anything it did not tell us is `None`.
#[derive(Clone, Debug, Default)]
pub struct BetReply {
    pub outcome: f64,
    pub amount: Option<Units>,
    pub winnings: Option<Units>,
    pub win: Option<bool>,
    pub nonce: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum CasinoEvent {
    Status(ConnStatus, String),
    Market(Market),
    Provable(ProvableState),
    /// The casino renewed the login (Shuffle): save this token in place of the old one, which stopped working.
    Token(String),
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CasinoError {
    #[error("{0}")]
    Server(String),
    /// The casino asked us to slow down; the bet is worth retrying.
    #[error("{0}")]
    Busy(String),
    #[error("Not connected")]
    NotConnected,
    #[error("Server timeout")]
    Timeout,
    #[error("Connection lost")]
    ConnectionLost,
    #[error("Bad reply: {0}")]
    BadReply(String),
    /// An original's round is still open on the account; finish it before starting another.
    #[error("You still have a round of this game open")]
    RoundOpen,
}

impl CasinoError {
    /// "Please wait and try again later" and friends: the account still has a bet open.
    pub fn is_busy(&self) -> bool {
        match self {
            CasinoError::Server(m) => {
                let m = m.to_ascii_lowercase();
                ["please wait", "try again later", "too many", "rate limit", "ratelimit", "rate-limit", "slow down"]
                    .iter()
                    .any(|k| m.contains(k))
            }
            CasinoError::Busy(_) => true,
            _ => false,
        }
    }
}

/// Which slots to list.
#[derive(Clone, Debug, Default)]
pub struct SlotFilter {
    pub query: Option<String>,
    pub producer: Option<String>,
    pub skip: usize,
    pub limit: usize,
}

#[async_trait]
pub trait Casino: Send + Sync {
    fn id(&self) -> CasinoId;
    /// Swap the login token; the client re-authenticates on its own.
    fn set_token(&self, token: Option<String>);
    /// Switch to another of the casino's domains (see [`CasinoId::mirrors`]).
    fn set_mirror(&self, _mirror: &str) {}
    /// Cloudflare cookie from the user's browser (see [`CasinoId::behind_cloudflare`]).
    fn set_clearance(&self, _clearance: Option<Clearance>) {}
    /// Close the socket for good.
    fn shutdown(&self);
    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError>;
    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError>;
    async fn provable_state(&self) -> Result<ProvableState, CasinoError>;
    /// Slot catalog.
    async fn list_slots(&self, filter: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError>;
    /// Launch URL for a slot session (real money needs a logged-in token).
    async fn launch_slot(&self, catalog_id: &str, demo: bool, currency: &str) -> Result<String, CasinoError>;

    /// Starts a round of an original ([`CasinoId::originals`]). `params` are the game's own
    /// fields; the stake, currency and an idempotency key are added here. `auto`: a bet of an
    /// auto run, which some games place through their own request.
    async fn original_bet(&self, _game: Game, _currency: &str, _amount: Units, _params: Value, _auto: bool) -> Result<Round, CasinoError> {
        Err(CasinoError::Server("This casino has no such game".into()))
    }
    /// Continues the open round of `game` (reveal, guess …) or cashes it out.
    async fn original_step(&self, _game: Game, _step: Step, _params: Value) -> Result<Round, CasinoError> {
        Err(CasinoError::Server("This casino has no such game".into()))
    }
    /// The round of `game` still open on the account, if any.
    async fn original_active(&self, _game: Game) -> Result<Option<Round>, CasinoError> {
        Ok(None)
    }
    /// Payout tables of the originals that have them.
    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        Err(CasinoError::Server("This casino has no payout tables".into()))
    }
    /// The originals as the casino lists them (name and artwork), keyed by slug.
    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }
}

/// Spawns the casino's connection tasks on the current tokio runtime.
pub fn connect(id: CasinoId, setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
    match id {
        CasinoId::ChipsGg => chipsgg::ChipsGg::spawn(setup.token, events),
        CasinoId::Stake => stake::Stake::spawn(setup, events),
        CasinoId::Shuffle => shuffle::Shuffle::spawn(setup, events),
        CasinoId::Goated => goated::Goated::spawn(setup, events),
        CasinoId::Gambulls => gambulls::Gambulls::spawn(setup, events),
    }
}

/// What a casino connects with: the login token plus, where the casino has them, mirror and Cloudflare cookie.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub token: Option<String>,
    pub mirror: Option<String>,
    pub clearance: Option<Clearance>,
}

/// Reconnect delay: 0.3 s doubling up to 5 s.
fn backoff(retries: &mut u32) -> Duration {
    let ms = (300u64 << (*retries).min(5)).min(5000);
    *retries += 1;
    Duration::from_millis(ms)
}
