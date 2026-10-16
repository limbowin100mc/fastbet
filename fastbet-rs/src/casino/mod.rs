//! Casino back-ends. Every casino implements [`Casino`]; the engine and UI
//! only ever talk to the trait, so adding a casino means adding one module
//! here and one variant to [`CasinoId`].

pub mod chipsgg;
pub mod creds;
pub mod gambulls;
pub mod goated;
pub mod metaspins;
pub mod shuffle;
pub mod stake;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

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
    Metaspins,
}

impl CasinoId {
    pub const ALL: &'static [CasinoId] = &[CasinoId::ChipsGg, CasinoId::Stake, CasinoId::Shuffle, CasinoId::Goated, CasinoId::Gambulls, CasinoId::Metaspins];

    pub fn key(self) -> &'static str {
        match self {
            CasinoId::ChipsGg => "chipsgg",
            CasinoId::Stake => "stake",
            CasinoId::Shuffle => "shuffle",
            CasinoId::Goated => "goated",
            CasinoId::Gambulls => "gambulls",
            CasinoId::Metaspins => "metaspins",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            CasinoId::ChipsGg => "Chips.gg",
            CasinoId::Stake => "Stake.com",
            CasinoId::Shuffle => "Shuffle.com",
            CasinoId::Goated => "Goated.com",
            CasinoId::Gambulls => "Gambulls.com",
            CasinoId::Metaspins => "Metaspins.com",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|id| id.key() == key)
    }

    /// Domains the casino can be reached on; empty when there is only one.
    pub fn mirrors(self) -> &'static [&'static str] {
        match self {
            CasinoId::ChipsGg | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins => &[],
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

    /// An auto bet on `game` settles its round in one request (the tiles go up front), so an auto
    /// run can have several in flight, like Limbo's: Mines on Stake (`mines/bet` with the tiles)
    /// and Shuffle (`minesAuto`).
    pub fn auto_settles(self, game: Game) -> bool {
        matches!(self, CasinoId::Stake | CasinoId::Shuffle) && game == Game::Mines
    }

    /// Takes bets of 0: they play a real round that pays nothing (Stake, Shuffle, Goated, chips.gg).
    pub fn zero_bets(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated)
    }

    /// Whether the account can also be linked through the embedded login window.
    pub fn window_login(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins)
    }

    /// Linked through the login window: the casino has no API keys to paste (Shuffle, Goated, Gambulls, Metaspins).
    pub fn window_only(self) -> bool {
        matches!(self, CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins)
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
            CasinoId::Metaspins => Some(("https://services.metaspins.com/api/session", metaspins::SESSION_COOKIE)),
            _ => None,
        }
    }

    /// The `cookie_login` cookie exists before the login too (Metaspins hands guests one): it only
    /// counts once the page reports it is logged in.
    pub fn cookie_needs_page_login(self) -> bool {
        matches!(self, CasinoId::Metaspins)
    }

    /// Slot producers the app can autoplay here, in lobby order. Empty: no slots.
    pub fn slot_producers(self) -> &'static [&'static str] {
        match self {
            CasinoId::ChipsGg => &[
                "hacksawgaming",
                "pragmaticplay",
                "relaxgaming",
                "playson",
                "bgaming",
                "avatarux",
                "onetouch",
                "onlyplay",
                "penguinking",
                "peter&sons",
                "popiplay",
                "shadylady",
                "slotmillgames",
                "spinomenal",
                "wickedgames",
                "play'ngo",
                "retrogaming",
            ],
            CasinoId::Stake => &[crate::slots::STAKE_ENGINE],
            CasinoId::Shuffle => &[
                "hacksawgaming",
                "shadylady",
                "peter&sons",
                "bgaming",
                "pragmaticplay",
                "avatarux",
                "penguinking",
                "onetouch",
                "relaxgaming",
                "bullsharkgames",
                "nownow",
                "trustygaming",
                "kitsunegaming",
                "aceroll",
                "goodtimesstudios",
                "pineappleplay",
                "alohagaming",
                "printstudios",
                "fantasma",
                "fourleafgaming",
            ],
            // the providers of Goated's (Hub88) catalog that have drivers
            CasinoId::Goated => &[
                "hacksawgaming",
                "pragmaticplay",
                "relaxgaming",
                "bgaming",
                "spinomenal",
                "avatarux",
                "penguinking",
                "peter&sons",
                "popiplay",
                "shadylady",
                "slotmillgames",
                "play'ngo",
                "retrogaming",
            ],
            CasinoId::Gambulls | CasinoId::Metaspins => &[],
        }
    }

    pub fn has_slots(self) -> bool {
        !self.slot_producers().is_empty()
    }

    /// Originals besides Limbo the app can play here, in lobby order.
    pub fn originals(self) -> Vec<Game> {
        match self {
            CasinoId::Gambulls | CasinoId::Metaspins => vec![],
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

    /// The account has a VIP level the casino reports live ([`CasinoEvent::Vip`]).
    pub fn has_vip(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated)
    }

    /// Rakeback and VIP bonuses the app can claim ([`Casino::rewards`]).
    pub fn has_rewards(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated)
    }

    /// Public slot challenges and King of the Hill rounds ([`Casino::challenges`]).
    pub fn has_challenges(self) -> bool {
        matches!(self, CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle)
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
            CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins => "login",
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
            CasinoId::Metaspins => "https://metaspins.com/#sign-in".into(),
        }
    }

    /// Where the user creates an API key, on the given mirror.
    pub fn api_key_url(self, mirror: Option<&str>) -> String {
        match self {
            CasinoId::ChipsGg => "https://chips.gg/settings/api-keys".into(),
            CasinoId::Stake => format!("https://{}/settings/security", mirror.unwrap_or(stake::DEFAULT_MIRROR)),
            CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins => self.login_url(mirror),
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
            // Metaspins' login is the HttpOnly cookie `usersession` the window reads itself
            // (`cookie_login`), but guests have one too: logged in means the site keeps a user in
            // localStorage `isUser` (or set its `loggedIn` cookie after a login).
            CasinoId::Metaspins => {
                r#"(function () {
  if (window.top !== window || window.__fastbetWatch) return;
  window.__fastbetWatch = true;
  setInterval(function () {
    try {
      var u = null;
      try { u = JSON.parse(localStorage.getItem('isUser') || 'null'); } catch (e) {}
      var ok = !!(u && typeof u === 'object' && u.userid) || /(?:^|;\s*)loggedIn=true/.test(document.cookie);
      window.ipc.postMessage(JSON.stringify({ token: '', loggedIn: ok }));
    } catch (e) {}
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
            CasinoId::Metaspins => "Metaspins has no API keys: log in to Metaspins in the window that opens with the button below. FastBet takes the login from the site, so your password never reaches the app. It is stored on this PC only.",
        }
    }

    pub fn min_target(self) -> f64 {
        match self {
            CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins => 1.01,
        }
    }

    pub fn max_target(self) -> f64 {
        match self {
            CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins => 1_000_000.0,
        }
    }

    /// Payout multiplier at which the house edge is already deducted (99% RTP).
    pub fn win_chance(self, target: f64) -> f64 {
        match self {
            CasinoId::ChipsGg | CasinoId::Stake | CasinoId::Shuffle | CasinoId::Goated | CasinoId::Gambulls | CasinoId::Metaspins => 99.0 / target,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CurrencyInfo {
    pub decimals: u32,
    /// USD value of one unit.
    pub price: f64,
    /// Not money (points, sweepstakes coins): never listed in the wallet.
    pub hidden: bool,
    /// A fiat currency: the wallet lists it only while it holds money.
    pub fiat: bool,
}

/// Everything the UI needs from the casino's live state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Market {
    pub currencies: BTreeMap<String, CurrencyInfo>,
    pub wallets: BTreeMap<String, Units>,
    /// Currency the user has selected on the site.
    pub site_currency: Option<String>,
    /// Fiat currencies the casino can show amounts in, in its own order (none: USD only).
    /// Their USD rates are in `currencies`, like the coins'.
    pub fiats: Vec<String>,
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

/// The account's VIP level and how far it is to the next one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VipStatus {
    pub rank: String,
    /// A finer level inside the rank, where the casino has one (chips.gg's "Level 101").
    pub level: Option<u32>,
    pub exp: f64,
    /// XP where this level started and where the next one starts.
    pub level_start: f64,
    pub level_end: f64,
    /// XP where the current rank started.
    pub rank_start: f64,
    /// The next rank up and the XP it starts at.
    pub next_rank: Option<(String, f64)>,
    /// XP a dollar wagered earns this account, measured from its own history (None: not known).
    pub xp_per_usd: Option<f64>,
    /// The casino counts dollars wagered, not XP (Stake): the amounts are exact and shown only in dollars.
    pub in_dollars: bool,
}

impl VipStatus {
    /// A ladder of named ranks and nothing finer: `exp` on the way from where `rank` starts to
    /// where the next one does.
    pub fn ranked(rank: String, exp: f64, rank_start: f64, next_rank: Option<(String, f64)>, xp_per_usd: Option<f64>) -> Self {
        let level_end = next_rank.as_ref().map(|(_, at)| *at).unwrap_or(exp);
        Self { rank, level: None, exp, level_start: rank_start, level_end, rank_start, next_rank, xp_per_usd, in_dollars: false }
    }

    /// How far through the level, 0–1.
    pub fn progress(&self) -> f64 {
        let span = self.level_end - self.level_start;
        if span > 0.0 {
            ((self.exp - self.level_start) / span).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    pub fn xp_to_level(&self) -> f64 {
        (self.level_end - self.exp).max(0.0)
    }

    /// How far from the current rank to the next one, 0–1 (1 at the top rank).
    pub fn rank_progress(&self) -> f64 {
        match &self.next_rank {
            Some((_, at)) if *at > self.rank_start => ((self.exp - self.rank_start) / (at - self.rank_start)).clamp(0.0, 1.0),
            Some(_) => 0.0,
            None => 1.0,
        }
    }

    pub fn xp_to_rank(&self) -> Option<f64> {
        self.next_rank.as_ref().map(|(_, at)| (at - self.exp).max(0.0))
    }

    /// Dollars to wager for `xp`, at the account's own rate.
    pub fn usd_for(&self, xp: f64) -> Option<f64> {
        self.xp_per_usd.filter(|r| *r > 0.0).map(|r| xp / r)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RewardKind {
    Rakeback,
    Daily,
    Weekly,
    Monthly,
    LevelUp,
    Reload,
    /// Anything else the casino pays out (its own name is the title).
    Bonus,
    /// The prize of a slot challenge the account won.
    Challenge,
}

impl RewardKind {
    pub fn name(self) -> &'static str {
        match self {
            RewardKind::Rakeback => "Rakeback",
            RewardKind::Daily => "Daily bonus",
            RewardKind::Weekly => "Weekly bonus",
            RewardKind::Monthly => "Monthly bonus",
            RewardKind::LevelUp => "Level-up bonus",
            RewardKind::Reload => "Cash reload",
            RewardKind::Bonus => "Bonus",
            RewardKind::Challenge => "Challenge prize",
        }
    }
}

/// Something the casino pays out on request.
#[derive(Clone, Debug, PartialEq)]
pub struct Reward {
    /// What [`Casino::claim_reward`] takes.
    pub id: String,
    pub kind: RewardKind,
    /// What the card is called (the kind's name, or the casino's own).
    pub title: String,
    /// The casino only takes this claim on its site (it asks for a captcha there): where.
    pub link: Option<String>,
    /// Its worth in USD, when known.
    pub usd: Option<f64>,
    /// One line under it ("0.0248 USDT + 0.00004 SOL", "10% rakeback for an hour").
    pub detail: String,
    pub claimable: bool,
    /// Not claimable yet: when it will be (a guess where the casino keeps a schedule).
    pub next_at: Option<SystemTime>,
    /// Claimable until.
    pub expires: Option<SystemTime>,
}

impl Reward {
    /// A reward of `kind` with nothing known about it yet.
    pub fn new(id: &str, kind: RewardKind) -> Self {
        Self { id: id.to_string(), kind, title: kind.name().to_string(), link: None, usd: None, detail: String::new(), claimable: false, next_at: None, expires: None }
    }
}

/// A public slot challenge: the first bet on `game` paying `target`× or more, at `min_bet_usd` or
/// more, wins `prize_usd`. A King of the Hill round (`koth`) instead pays the pool to the best
/// multiplier on `game` when it ends; `target` is the best so far.
#[derive(Clone, Debug, PartialEq)]
pub struct Challenge {
    pub id: String,
    pub koth: bool,
    pub game: crate::slots::SlotGame,
    pub target: f64,
    pub min_bet_usd: f64,
    pub prize_usd: f64,
    pub ends: Option<SystemTime>,
    /// Who leads a King of the Hill round.
    pub leader: Option<String>,
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
    Vip(VipStatus),
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

    /// "Insufficient balance" and friends (`BALANCE_NOT_ENOUGH`, "Not enough funds" …): the
    /// account can't pay for the bet, and no retry helps until the user deposits.
    pub fn is_balance(&self) -> bool {
        let CasinoError::Server(m) = self else { return false };
        let m = m.to_ascii_lowercase().replace('_', " ");
        let money = ["balance", "funds", "money"].iter().any(|k| m.contains(k));
        money && ["insufficient", "not enough", "too low", "low balance"].iter().any(|k| m.contains(k))
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
    /// Launch URL for a slot session (real money needs a logged-in token). `currency` pays for the
    /// spins; `fiat` is the money the game shows, where the casino lets the player pick it.
    async fn launch_slot(&self, catalog_id: &str, demo: bool, currency: &str, fiat: &str) -> Result<String, CasinoError>;

    /// See [`CasinoId::auto_settles`].
    fn auto_settles(&self, game: Game) -> bool {
        self.id().auto_settles(game)
    }

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

    /// Rakeback and bonuses, claimable or coming ([`CasinoId::has_rewards`]).
    async fn rewards(&self) -> Result<Vec<Reward>, CasinoError> {
        Ok(vec![])
    }
    /// Pays out the reward with this [`Reward::id`] in `currency`. Answers what was paid.
    async fn claim_reward(&self, _id: &str, _currency: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("This casino has no rewards to claim".into()))
    }
    /// Running slot challenges and King of the Hill rounds ([`CasinoId::has_challenges`]).
    async fn challenges(&self) -> Result<Vec<Challenge>, CasinoError> {
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
        CasinoId::Metaspins => metaspins::Metaspins::spawn(setup, events),
    }
}

/// What a casino connects with: the login token plus, where the casino has them, mirror and Cloudflare cookie.
#[derive(Clone, Debug, Default)]
pub struct Setup {
    pub token: Option<String>,
    pub mirror: Option<String>,
    pub clearance: Option<Clearance>,
}

/// "2026-10-17T00:00:00.000Z" (an offset like "+02:00" too) -> the moment it names.
fn parse_iso(s: &str) -> Option<SystemTime> {
    let s = s.trim();
    let (date, time) = s.split_once('T').or_else(|| s.split_once(' '))?;
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    // the zone: Z, +hh:mm or -hh:mm after the clock
    let (clock, offset) = match time.find(['Z', 'z', '+', '-']) {
        Some(i) => {
            let zone = &time[i..];
            let secs = if zone.eq_ignore_ascii_case("z") {
                0
            } else {
                let sign = if zone.starts_with('-') { -1 } else { 1 };
                let mut hm = zone[1..].split(':').map(|x| x.parse::<i64>().unwrap_or(0));
                sign * (hm.next().unwrap_or(0) * 3600 + hm.next().unwrap_or(0) * 60)
            };
            (&time[..i], secs)
        }
        None => (time, 0),
    };
    let mut c = clock.split(':');
    let (h, mi) = (c.next()?.parse::<i64>().ok()?, c.next().unwrap_or("0").parse::<i64>().ok()?);
    let sec: f64 = c.next().unwrap_or("0").parse().ok()?;
    let days = days_from_civil(y, u32::try_from(m).ok()?, u32::try_from(day).ok()?);
    let ms = ((days * 86_400 + h * 3600 + mi * 60 - offset) as f64 + sec) * 1000.0;
    (ms >= 0.0).then(|| std::time::UNIX_EPOCH + Duration::from_millis(ms as u64))
}

/// A time a casino sends as text (ISO) or as milliseconds.
fn time_of(v: &Value) -> Option<SystemTime> {
    match v {
        Value::String(s) => parse_iso(s),
        Value::Number(n) => n.as_f64().filter(|ms| *ms > 0.0).map(|ms| std::time::UNIX_EPOCH + Duration::from_millis(ms as u64)),
        _ => None,
    }
}

/// "0.0476 TRX + 0.00024 SOL": coins and amounts, the first three.
fn coins_text(list: &[(String, f64)]) -> String {
    list.iter()
        .filter_map(|(c, a)| {
            let s = format!("{a:.8}");
            let s = s.trim_end_matches('0').trim_end_matches('.');
            // dust below the eighth decimal says nothing
            (s != "0").then(|| format!("{s} {}", c.to_uppercase()))
        })
        .take(3)
        .collect::<Vec<_>>()
        .join(" + ")
}

/// "Claimed 0.12 TRX" from a claim's answer: a list of `{currency, amount}`, one of them, or one
/// under `claimedAmount` / `currencyAmounts` / `challenge`.
fn claimed_text(v: Option<&Value>) -> String {
    fn coins(v: &Value) -> Vec<(String, f64)> {
        match v {
            Value::Array(a) => a.iter().flat_map(coins).collect(),
            Value::Object(o) => {
                for k in ["claimedAmount", "currencyAmounts", "challenge", "amounts"] {
                    if let Some(inner) = o.get(k).filter(|x| !x.is_null()) {
                        return coins(inner);
                    }
                }
                match (o.get("currency").and_then(Value::as_str), o.get("amount").and_then(crate::money::f64_from_json)) {
                    (Some(c), Some(a)) if a > 0.0 => vec![(c.to_string(), a)],
                    _ => vec![],
                }
            }
            _ => vec![],
        }
    }
    let list = v.map(coins).unwrap_or_default();
    if list.is_empty() {
        "Claimed".into()
    } else {
        format!("Claimed {}", coins_text(&list))
    }
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Hands `status` to the app unless it is what was sent last.
fn send_vip(events: &mpsc::UnboundedSender<CasinoEvent>, last: &parking_lot::Mutex<Option<VipStatus>>, status: VipStatus) {
    let mut last = last.lock();
    if last.as_ref() != Some(&status) {
        *last = Some(status.clone());
        let _ = events.send(CasinoEvent::Vip(status));
    }
}

/// XP per dollar from an account's XP and its wager in USD (1 where it can't tell).
fn xp_rate(xp: f64, usd_wagered: f64) -> f64 {
    if xp > 0.0 && usd_wagered > 0.0 {
        xp / usd_wagered
    } else {
        1.0
    }
}

/// Reconnect delay: 0.3 s doubling up to 5 s.
fn backoff(retries: &mut u32) -> Duration {
    let ms = (300u64 << (*retries).min(5)).min(5000);
    *retries += 1;
    Duration::from_millis(ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn iso_times_and_claim_texts() {
        let at = |s: u64| Some(std::time::UNIX_EPOCH + Duration::from_secs(s));
        // 2026-10-16 08:00 UTC
        assert_eq!(parse_iso("2026-10-16T08:00:00.000Z"), at(1_792_137_600));
        assert_eq!(parse_iso("2026-10-16T08:00:00Z"), at(1_792_137_600));
        assert_eq!(parse_iso("2026-10-16T10:00:00+02:00"), at(1_792_137_600));
        assert_eq!(parse_iso("2026-10-16T03:30:00-04:30"), at(1_792_137_600));
        assert_eq!(parse_iso("not a date"), None);
        assert_eq!(time_of(&json!(1_792_137_600_000u64)), at(1_792_137_600));
        assert_eq!(civil_from_days(days_from_civil(2028, 2, 29)), (2028, 2, 29));

        assert_eq!(claimed_text(Some(&json!([{"currency": "TRX", "amount": "0.119"}, {"currency": "USDT", "amount": "0"}]))), "Claimed 0.119 TRX");
        assert_eq!(claimed_text(Some(&json!({"claimedAmount": {"currency": "USDT", "amount": "2.5"}, "nextClaimDate": "x"}))), "Claimed 2.5 USDT");
        assert_eq!(claimed_text(Some(&json!({"nextClaimDate": "x"}))), "Claimed");
        assert_eq!(claimed_text(None), "Claimed");
        assert_eq!(coins_text(&[("btc".into(), 4.9e-11), ("sol".into(), 0.000419482735)]), "0.00041948 SOL");
    }
}
