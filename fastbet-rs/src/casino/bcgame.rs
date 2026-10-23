//! BC.Game: the account over the site's REST API and the originals (Limbo, Classic Dice, Keno,
//! Mines) over its socket, both from a hidden browser window parked on bc.game ([`Bridge`]).
//!
//! The site signs its socket address with its own obfuscated code, so the app opens no socket of
//! its own: the bridge loads bc.game with the login's cookies, the site connects its socket as
//! usual and [`SOCKET_SCRIPT`] sends the app's requests over that same connection (namespaces
//! joined when first used, ack ids from 2^30 up so they never meet the site's).
//!
//! The socket is socket.io v2 with BC.Game's binary parser. A frame is `[4, type (+128 with an
//! ack id), ack id (u32 BE), namespace (u8 length + text), event (u8 length + text), data]`; the
//! data is protobuf. Every request is answered with `ResultObj {code, msg, data}`. Every original
//! takes `Bet {currencyName, betAmount, betValue}` and answers `BetResult`, with the game's own
//! message in `betValue` / `gameValue`. Odds travel ×10000, Limbo targets and Dice chances ×100.
//!
//! The login is the site's HttpOnly cookies `SESSION` and `TK` (a year long) with Cloudflare's
//! `cf_clearance` and the login window's User Agent. Bets of 0 are allowed. The originals pay 98%.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};

use super::*;
use crate::bridge::{Bridge, BridgeCookie, BridgeRequest, BridgeSetup};
use crate::money::{f64_from_json, parse_units};
use crate::originals::{mines_state_with, Game, Round, Step, Tables};

/// Where the bridge waits: a page of the site that plays nothing itself (the site's own game
/// pages join their game's namespace, which the app does on its own).
const PARK: &str = "https://bc.game/help";
/// The bridge's browser profile; the login's cookies are put in it at every start.
const PROFILE: &str = "bcgame-bridge";
/// The cookies that make the login, as the login window hands them over (the first one must be
/// there), and where they are sent.
pub const LOGIN_COOKIES: &[&str] = &["SESSION", "TK", "cf_clearance"];
pub const LOGIN_URLS: &[&str] = &["https://bc.game/"];
const COOKIE_DOMAIN: &str = ".bc.game";
/// What the originals pay back, in percent.
pub const RTP: f64 = 98.0;
const RPC_TIMEOUT: Duration = Duration::from_secs(25);
const REFRESH: Duration = Duration::from_secs(10);
const PRICES_EVERY: u32 = 12;
/// How often the bridge's cookies are read back, in refresh ticks: the site renews its login.
const COOKIES_EVERY: u32 = 30;
/// A Cloudflare check that hasn't passed by itself after this long is shown to the user.
const SHOW_CHECK_AFTER: Duration = Duration::from_secs(10);
const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
const EXPIRED: &str = "Your BC.Game login has expired. Log in again in Settings.";

/// The originals the app plays here: (game, the site's path, its game name and namespace as last
/// seen; the site's config replaces them when it lists others).
const GAMES: &[(Option<Game>, &str, &str, &str)] = &[
    (None, "limbo", "Limbo98", "/g/l/98"),
    (Some(Game::Dice), "classic-dice", "ClassicDice98", "/g/cd/98"),
    (Some(Game::Keno), "keno", "KenoSingle98", "/g/kenos/98"),
    (Some(Game::Mines), "mines", "Mines98", "/g/mines/98"),
];

/// Answers `__fbsock(id, {url: namespace, body: '{"event", "data": base64}'})` over the site's
/// own socket; see the module docs for the frames.
const SOCKET_SCRIPT: &str = r#"(function () {
  if (window.top !== window || window.__fbsock || !window.WebSocket) return;
  var Native = window.WebSocket, P = Native.prototype, send = P.send, listen = P.addEventListener;
  var enc = new TextEncoder(), dec = new TextDecoder();
  var cur = null, joined = {}, joining = {}, acks = {}, next = 0x40000000;
  function out(line) { try { window.ipc.postMessage(line); } catch (e) {} }
  function reply(id, t0, status, body, error) {
    out('RES ' + id + ' ' + JSON.stringify({ status: status, body: body || '', error: error || null, ms: Date.now() - t0 }));
  }
  function b64(u) { var s = ''; for (var i = 0; i < u.length; i += 0x8000) s += String.fromCharCode.apply(null, u.subarray(i, i + 0x8000)); return btoa(s); }
  function unb64(s) { var b = atob(s || ''), u = new Uint8Array(b.length); for (var i = 0; i < b.length; i++) u[i] = b.charCodeAt(i); return u; }
  function frame(type, ack, nsp, ev, data) {
    var n = enc.encode(nsp), e = enc.encode(ev), d = data || new Uint8Array(0);
    var u = new Uint8Array(4 + (ack === null ? 0 : 4) + n.length + e.length + d.length), i = 0;
    u[i++] = 4;
    u[i++] = ack === null ? type : type + 128;
    if (ack !== null) { new DataView(u.buffer).setUint32(i, ack); i += 4; }
    u[i++] = n.length; u.set(n, i); i += n.length;
    u[i++] = e.length; u.set(e, i); i += e.length;
    u.set(d, i);
    return u.buffer;
  }
  function parse(buf) {
    var u = new Uint8Array(buf);
    if (u.length < 4 || u[0] !== 4) return null;
    var i = 1, t = u[i++], ack = null;
    if (t >= 128) { t -= 128; ack = new DataView(buf).getUint32(i); i += 4; }
    var n = u[i++], nsp = dec.decode(u.subarray(i, i + n)); i += n;
    var m = u[i++], ev = dec.decode(u.subarray(i, i + m)); i += m;
    return { type: t, ack: ack, nsp: nsp, ev: ev, data: u.subarray(i) };
  }
  function settle(nsp, err) {
    var w = joining[nsp];
    if (!w) return;
    delete joining[nsp];
    if (!err) joined[nsp] = true;
    w.forEach(function (f) { f(err); });
  }
  function failAll(err) {
    Object.keys(joining).forEach(function (k) { settle(k, err); });
    Object.keys(acks).forEach(function (k) { var a = acks[k]; delete acks[k]; a(err); });
  }
  function onFrame(ws, buf) {
    if (ws !== cur) return;
    var p = parse(buf);
    if (!p) return;
    if (p.type === 0) settle(p.nsp, null);
    else if (p.type === 4) settle(p.nsp, 'refused ' + p.nsp);
    else if (p.type === 1) delete joined[p.nsp];
    else if (p.type === 3 && p.ack !== null && acks[p.ack]) { var a = acks[p.ack]; delete acks[p.ack]; a(null, p.data); }
  }
  function track(ws) {
    listen.call(ws, 'open', function () { if (cur && cur !== ws) failAll('socket closed'); cur = ws; joined = {}; });
    listen.call(ws, 'message', function (ev) {
      if (ev.data instanceof ArrayBuffer) onFrame(ws, ev.data);
      else if (typeof Blob !== 'undefined' && ev.data instanceof Blob) ev.data.arrayBuffer().then(function (b) { onFrame(ws, b); });
    });
    listen.call(ws, 'close', function () { if (ws === cur) { cur = null; joined = {}; failAll('socket closed'); } });
  }
  window.WebSocket = function (url, protocols) {
    var ws = protocols === undefined ? new Native(url) : new Native(url, protocols);
    try { if (/\/\/socketv4\./.test(String(url))) track(ws); } catch (e) {}
    return ws;
  };
  window.WebSocket.prototype = P;
  ['CONNECTING', 'OPEN', 'CLOSING', 'CLOSED'].forEach(function (k, i) { window.WebSocket[k] = i; });
  function join(nsp, cb) {
    if (joined[nsp]) return cb(null);
    if (joining[nsp]) return joining[nsp].push(cb);
    joining[nsp] = [cb];
    send.call(cur, frame(0, null, nsp, '', null));
    setTimeout(function () { settle(nsp, 'no answer from ' + nsp); }, 15000);
  }
  function whenOpen(cb, until) {
    if (cur && cur.readyState === 1) return cb(null);
    if (Date.now() > until) return cb('not connected');
    setTimeout(function () { whenOpen(cb, until); }, 100);
  }
  window.__fbsock = function (id, req) {
    var t0 = Date.now(), msg;
    try { msg = JSON.parse(req.body || '{}'); } catch (e) { return reply(id, t0, 0, '', 'bad request'); }
    whenOpen(function (err) {
      if (err) return reply(id, t0, 0, '', err);
      var ws = cur;
      join(req.url, function (err) {
        if (err) return reply(id, t0, 0, '', err);
        if (ws !== cur) return reply(id, t0, 0, '', 'socket closed');
        var ack = next++;
        if (next > 0x7ffffff0) next = 0x40000000;
        var timer = setTimeout(function () { if (acks[ack]) { delete acks[ack]; reply(id, t0, 0, '', 'timeout'); } }, 20000);
        acks[ack] = function (err, data) { clearTimeout(timer); if (err) reply(id, t0, 0, '', err); else reply(id, t0, 200, b64(data)); };
        try { send.call(ws, frame(2, ack, req.url, msg.event || '', unb64(msg.data))); }
        catch (e) { delete acks[ack]; clearTimeout(timer); reply(id, t0, 0, '', 'socket closed'); }
      });
    }, t0 + (msg.wait || 20000));
  };
})();"#;

/// Protobuf, the little the games need: a writer and a field reader.
mod pb {
    #[derive(Default)]
    pub struct W(Vec<u8>);

    impl W {
        fn varint(&mut self, mut v: u64) {
            while v >= 0x80 {
                self.0.push((v as u8) | 0x80);
                v >>= 7;
            }
            self.0.push(v as u8);
        }
        fn key(&mut self, field: u32, wire: u8) {
            self.varint((u64::from(field) << 3) | u64::from(wire));
        }
        pub fn bytes(mut self, field: u32, b: &[u8]) -> Self {
            self.key(field, 2);
            self.varint(b.len() as u64);
            self.0.extend_from_slice(b);
            self
        }
        pub fn string(self, field: u32, s: &str) -> Self {
            self.bytes(field, s.as_bytes())
        }
        pub fn sint32(mut self, field: u32, v: i32) -> Self {
            self.key(field, 0);
            self.varint(u64::from(((v << 1) ^ (v >> 31)) as u32));
            self
        }
        pub fn bool(mut self, field: u32, v: bool) -> Self {
            self.key(field, 0);
            self.varint(u64::from(v));
            self
        }
        pub fn packed_sint32(self, field: u32, vs: &[i32]) -> Self {
            let mut inner = W::default();
            for v in vs {
                inner.varint(u64::from(((v << 1) ^ (v >> 31)) as u32));
            }
            self.bytes(field, &inner.0)
        }
        pub fn done(self) -> Vec<u8> {
            self.0
        }
    }

    #[derive(Clone, Copy)]
    enum V<'a> {
        Int(u64),
        Bytes(&'a [u8]),
        Fixed64(u64),
        Fixed32,
    }

    /// A message's fields, in order.
    pub struct Msg<'a>(Vec<(u32, V<'a>)>);

    fn varint(b: &[u8], i: &mut usize) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let c = *b.get(*i)?;
            *i += 1;
            v |= u64::from(c & 0x7f) << shift;
            if c < 0x80 {
                return Some(v);
            }
        }
        None
    }

    fn zigzag(v: u64) -> i64 {
        ((v >> 1) as i64) ^ -((v & 1) as i64)
    }

    impl<'a> Msg<'a> {
        pub fn parse(b: &'a [u8]) -> Option<Msg<'a>> {
            let mut out = vec![];
            let mut i = 0;
            while i < b.len() {
                let key = varint(b, &mut i)?;
                let field = (key >> 3) as u32;
                let v = match key & 7 {
                    0 => V::Int(varint(b, &mut i)?),
                    1 => {
                        let raw = b.get(i..i + 8)?;
                        i += 8;
                        V::Fixed64(u64::from_le_bytes(raw.try_into().ok()?))
                    }
                    2 => {
                        let n = varint(b, &mut i)? as usize;
                        let raw = b.get(i..i.checked_add(n)?)?;
                        i += n;
                        V::Bytes(raw)
                    }
                    5 => {
                        b.get(i..i + 4)?;
                        i += 4;
                        V::Fixed32
                    }
                    _ => return None,
                };
                out.push((field, v));
            }
            Some(Msg(out))
        }

        fn last(&self, field: u32) -> Option<V<'a>> {
            self.0.iter().rev().find(|(f, _)| *f == field).map(|(_, v)| *v)
        }

        pub fn bytes(&self, field: u32) -> Option<&'a [u8]> {
            match self.last(field)? {
                V::Bytes(b) => Some(b),
                _ => None,
            }
        }

        pub fn str(&self, field: u32) -> Option<String> {
            self.bytes(field).map(|b| String::from_utf8_lossy(b).into_owned())
        }

        /// A sint32 / sint64 (zigzag); 0 when it isn't there, like protobuf's default.
        pub fn sint(&self, field: u32) -> i64 {
            match self.last(field) {
                Some(V::Int(v)) => zigzag(v),
                _ => 0,
            }
        }

        pub fn has(&self, field: u32) -> bool {
            self.last(field).is_some()
        }

        #[cfg_attr(not(test), allow(dead_code))]
        pub fn double(&self, field: u32) -> Option<f64> {
            match self.last(field)? {
                V::Fixed64(v) => Some(f64::from_bits(v)),
                _ => None,
            }
        }

        pub fn msg(&self, field: u32) -> Option<Msg<'a>> {
            Msg::parse(self.bytes(field)?)
        }

        /// Every value of a repeated message field.
        pub fn all(&self, field: u32) -> Vec<&'a [u8]> {
            self.0
                .iter()
                .filter_map(|(f, v)| match v {
                    V::Bytes(b) if *f == field => Some(*b),
                    _ => None,
                })
                .collect()
        }

        /// A packed repeated sint32.
        pub fn packed_sint(&self, field: u32) -> Vec<i64> {
            let Some(b) = self.bytes(field) else { return vec![] };
            let mut out = vec![];
            let mut i = 0;
            while i < b.len() {
                match varint(b, &mut i) {
                    Some(v) => out.push(zigzag(v)),
                    None => break,
                }
            }
            out
        }
    }
}

use pb::{Msg, W};

#[derive(Clone, Debug)]
enum CallError {
    LoggedOut,
    Cloudflare,
    Casino(CasinoError),
}

impl From<CallError> for CasinoError {
    fn from(e: CallError) -> Self {
        match e {
            CallError::LoggedOut => CasinoError::Server(EXPIRED.into()),
            CallError::Cloudflare => CasinoError::Server("Cloudflare stopped the request; try again in a moment".into()),
            CallError::Casino(e) => e,
        }
    }
}

impl From<CasinoError> for CallError {
    fn from(e: CasinoError) -> Self {
        CallError::Casino(e)
    }
}

/// The login as saved: `{"cookies": "SESSION=…; TK=…; cf_clearance=…", "ua": "<the login
/// window's User Agent>"}` (a bare cookie line works too).
#[derive(Clone, Debug, PartialEq)]
struct Login {
    cookies: Vec<(String, String)>,
    ua: Option<String>,
}

impl Login {
    fn parse(token: &str) -> Option<Login> {
        let t = token.trim();
        let (line, ua) = match serde_json::from_str::<Value>(t) {
            Ok(v) if v.is_object() => (v["cookies"].as_str().unwrap_or("").to_string(), v["ua"].as_str().map(str::to_string).filter(|u| !u.is_empty())),
            _ => (t.to_string(), None),
        };
        let mut cookies: Vec<(String, String)> = vec![];
        for kv in line.split(';').map(str::trim) {
            if let Some((k, v)) = kv.split_once('=') {
                if LOGIN_COOKIES.contains(&k) && !v.is_empty() && !cookies.iter().any(|(n, _)| n == k) {
                    cookies.push((k.to_string(), v.to_string()));
                }
            }
        }
        cookies.iter().any(|(k, _)| k == LOGIN_COOKIES[0]).then_some(Login { cookies, ua })
    }

    fn line(&self) -> String {
        self.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }

    fn to_token(&self) -> String {
        json!({ "cookies": self.line(), "ua": self.ua }).to_string()
    }
}

/// An original as the site runs it: its game name (for seeds) and socket namespace.
#[derive(Clone, Debug)]
struct Spot {
    name: String,
    nsp: String,
}

/// A coin as the site names it, and its smallest unit.
#[derive(Clone, Debug)]
struct Coin {
    name: String,
    decimals: u32,
}

/// What a bet came back with (`BetResult`).
#[derive(Debug, Default)]
struct Settled {
    id: String,
    /// 1 won, 2 lost, 0 still in play (Mines).
    result: i64,
    odds: f64,
    nonce: Option<u64>,
    game: Vec<u8>,
    bet_value: Vec<u8>,
    currency: String,
    win_amount: String,
}

impl Settled {
    fn parse(data: &[u8]) -> Result<Settled, CasinoError> {
        let m = Msg::parse(data).ok_or_else(|| CasinoError::BadReply("unreadable bet result".into()))?;
        let id = m.str(1).or_else(|| m.str(10)).unwrap_or_default();
        if id.is_empty() {
            return Err(CasinoError::BadReply("bet result without an id".into()));
        }
        Ok(Settled {
            id,
            result: m.sint(2),
            odds: m.sint(3) as f64 / 10_000.0,
            nonce: m.has(4).then(|| m.sint(4).max(0) as u64).or(Some(0)),
            game: m.bytes(5).unwrap_or_default().to_vec(),
            bet_value: m.bytes(11).unwrap_or_default().to_vec(),
            currency: m.str(7).unwrap_or_default(),
            win_amount: m.str(9).unwrap_or_default(),
        })
    }
}

/// A Mines round in play.
#[derive(Clone, Debug)]
struct MinesPlay {
    id: String,
    count: u32,
    coin: String,
    amount: Units,
    /// Revealed so far, 0–24.
    picks: Vec<i64>,
}

pub struct BcGame {
    login: Mutex<Option<Login>>,
    /// A browser profile that already holds a login (`FASTBET_BCGAME_PROFILE`, for testing).
    dev_profile: Option<String>,
    bridge: Mutex<Option<Bridge>>,
    wake: Notify,
    alive: AtomicBool,
    ready: AtomicBool,
    events: mpsc::UnboundedSender<CasinoEvent>,
    market: Mutex<Market>,
    published: Mutex<Market>,
    /// Lowercase code -> the site's coin.
    coins: Mutex<HashMap<String, Coin>>,
    /// The site's path -> its game name and namespace.
    spots: Mutex<HashMap<&'static str, Spot>>,
    /// The game played last: seeds are per game, the seed shown and changed is its.
    last: Mutex<&'static str>,
    keno: Mutex<HashMap<String, Vec<Vec<f64>>>>,
    mines: Mutex<Option<MinesPlay>>,
    mines_joined: AtomicBool,
    /// One request at a time: the site answers anything sent while a request is in flight
    /// (a bet, even a game's `init`) with 5999 "Please bet again".
    one: tokio::sync::Mutex<()>,
    /// An auto bet's Mines round runs bet, reveal and cash out in a row: one round at a time.
    mines_auto: tokio::sync::Mutex<()>,
    inflight: AtomicUsize,
    settled: AtomicU64,
}

impl BcGame {
    pub fn spawn(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<dyn Casino> {
        Self::start(setup, events)
    }

    fn start(setup: Setup, events: mpsc::UnboundedSender<CasinoEvent>) -> Arc<Self> {
        let spots = GAMES.iter().map(|(_, path, name, nsp)| (*path, Spot { name: name.to_string(), nsp: nsp.to_string() })).collect();
        let me = Arc::new(Self {
            login: Mutex::new(setup.token.as_deref().and_then(Login::parse)),
            dev_profile: std::env::var("FASTBET_BCGAME_PROFILE").ok().filter(|p| !p.is_empty()),
            bridge: Mutex::new(None),
            wake: Notify::new(),
            alive: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            events,
            market: Mutex::new(Market::default()),
            published: Mutex::new(Market::default()),
            coins: Mutex::new(HashMap::new()),
            spots: Mutex::new(spots),
            last: Mutex::new("limbo"),
            keno: Mutex::new(HashMap::new()),
            mines: Mutex::new(None),
            mines_joined: AtomicBool::new(false),
            one: tokio::sync::Mutex::new(()),
            mines_auto: tokio::sync::Mutex::new(()),
            inflight: AtomicUsize::new(0),
            settled: AtomicU64::new(0),
        });
        tokio::spawn(me.clone().run());
        me
    }

    fn status(&self, s: ConnStatus, detail: impl Into<String>) {
        let detail = detail.into();
        tracing::info!("bcgame: {} {detail}", s.short());
        self.ready.store(s.is_ready(), Ordering::SeqCst);
        let _ = self.events.send(CasinoEvent::Status(s, detail));
    }

    fn alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn bridge(&self) -> Result<Bridge, CasinoError> {
        self.bridge.lock().clone().ok_or(CasinoError::NotConnected)
    }

    fn close_bridge(&self) {
        if let Some(b) = self.bridge.lock().take() {
            b.close();
        }
    }

    async fn run(self: Arc<Self>) {
        let mut retries: u32 = 0;
        while self.alive() {
            if self.login.lock().is_none() && self.dev_profile.is_none() {
                self.status(ConnStatus::NoToken, "");
                self.wake.notified().await;
                continue;
            }
            let lost = match self.connect().await {
                Ok(()) => {
                    retries = 0;
                    self.serve().await
                }
                Err(CallError::LoggedOut) => {
                    self.ready.store(false, Ordering::SeqCst);
                    self.close_bridge();
                    self.status(ConnStatus::AuthFailed, EXPIRED);
                    self.wake.notified().await;
                    continue;
                }
                Err(e) => {
                    if self.alive() {
                        self.status(ConnStatus::Offline, CasinoError::from(e).to_string());
                    }
                    true
                }
            };
            self.ready.store(false, Ordering::SeqCst);
            self.close_bridge();
            if lost && self.alive() {
                tokio::select! {
                    _ = tokio::time::sleep(backoff(&mut retries)) => {}
                    _ = self.wake.notified() => {}
                }
            }
        }
        self.close_bridge();
    }

    /// Opens the bridge on bc.game with the login, checks who is logged in, reads the coins,
    /// the games and the balances, and makes sure the site's socket answers.
    async fn connect(&self) -> Result<(), CallError> {
        self.status(ConnStatus::Connecting, "Opening bc.game");
        self.mines_joined.store(false, Ordering::SeqCst);
        let login = self.login.lock().clone();
        let mut setup = BridgeSetup { user_agent: None, cookies: vec![], script: Some(SOCKET_SCRIPT.into()) };
        let profile = match (&self.dev_profile, &login) {
            (Some(p), _) => p.clone(),
            (None, Some(l)) => {
                setup.user_agent = l.ua.clone();
                setup.cookies = l.cookies.iter().map(|(k, v)| BridgeCookie { name: k.clone(), value: v.clone(), domain: COOKIE_DOMAIN.into() }).collect();
                PROFILE.to_string()
            }
            (None, None) => return Err(CallError::LoggedOut),
        };
        let page = std::env::var("FASTBET_BCGAME_PARK").unwrap_or_else(|_| PARK.to_string());
        let bridge = Bridge::open_with(&page, Some(&profile), &setup).await.map_err(|e| CallError::Casino(CasinoError::Server(e)))?;
        if let Some(old) = self.bridge.lock().replace(bridge.clone()) {
            old.close();
        }

        // the page may have to get through a Cloudflare check first
        self.status(ConnStatus::Authenticating, "");
        let t0 = Instant::now();
        let mut shown = false;
        let account = loop {
            if !self.alive() {
                return Err(CallError::Casino(CasinoError::NotConnected));
            }
            match self.rest("/api/account/get/").await {
                Ok(a) => break a,
                Err(CallError::Cloudflare) => {
                    if t0.elapsed() > CHECK_TIMEOUT {
                        return Err(CallError::Casino(CasinoError::Server("Cloudflare didn't let the app through to bc.game".into())));
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
        if account.get("login").and_then(Value::as_bool) != Some(true) {
            return Err(CallError::LoggedOut);
        }
        self.market.lock().username = account.get("name").and_then(Value::as_str).map(str::to_string);

        self.read_config().await?;
        self.refresh_prices().await;
        self.refresh_balances().await?;

        // the socket: the site opens it once the page is up; Limbo's `init` proves it answers
        let limbo = self.spot("limbo");
        self.call_within(&limbo.nsp, "init", vec![], Duration::from_secs(45)).await.map_err(|e| match e {
            CasinoError::NotConnected => CallError::Casino(CasinoError::Server("BC.Game's game connection didn't open".into())),
            e => CallError::Casino(e),
        })?;
        self.status(ConnStatus::Ready, "");
        match self.seeds("limbo").await {
            Ok(st) => {
                let _ = self.events.send(CasinoEvent::Provable(st));
            }
            Err(e) => tracing::info!("bcgame: seeds: {e}"),
        }
        Ok(())
    }

    /// Runs until the login changes (false) or the connection is lost (true).
    async fn serve(&self) -> bool {
        let mut failures = 0;
        let mut ticks = 0u32;
        loop {
            tokio::select! {
                _ = self.wake.notified() => return false,
                _ = tokio::time::sleep(REFRESH) => {}
            }
            if !self.alive() {
                return false;
            }
            ticks += 1;
            if ticks % PRICES_EVERY == 0 {
                self.refresh_prices().await;
            }
            if ticks % COOKIES_EVERY == 0 {
                self.save_login().await;
            }
            match self.refresh_balances().await {
                Ok(()) => failures = 0,
                Err(CallError::LoggedOut) => {
                    self.status(ConnStatus::AuthFailed, EXPIRED);
                    self.close_bridge();
                    self.wake.notified().await;
                    return false;
                }
                Err(_) => failures += 1,
            }
            if failures >= 3 {
                return true;
            }
        }
    }

    /// The coins (`currencyV2`) and where each original runs (`gameChannels`).
    async fn read_config(&self) -> Result<(), CallError> {
        let conf = self.rest("/cache/game/support/system/conf/index/minimal").await?;
        let mut coins = HashMap::new();
        {
            let mut m = self.market.lock();
            for c in conf.get("currencyV2").and_then(Value::as_array).into_iter().flatten() {
                let Some(name) = c.get("currencyName").and_then(Value::as_str) else { continue };
                let unit = c.get("unitAmount").and_then(Value::as_f64).unwrap_or(1e8).max(1.0);
                let decimals = unit.log10().round().clamp(0.0, 18.0) as u32;
                let kind = c.get("currencyType").and_then(Value::as_str).unwrap_or("");
                let code = name.to_ascii_lowercase();
                let info = m.currencies.entry(code.clone()).or_default();
                info.decimals = decimals;
                // points, NFT shares and third-party coins aren't money to bet
                info.hidden = !matches!(kind, "CHAIN" | "FIAT");
                info.fiat = kind == "FIAT";
                coins.insert(code, Coin { name: name.to_string(), decimals });
            }
        }
        if coins.is_empty() {
            return Err(CallError::Casino(CasinoError::BadReply("BC.Game listed no coins".into())));
        }
        *self.coins.lock() = coins;
        let mut spots = self.spots.lock();
        for ch in conf.get("gameChannels").and_then(Value::as_array).into_iter().flatten() {
            let (Some(path), Some(name), Some(nsp)) = (ch.get("gamePath").and_then(Value::as_str), ch.get("gameName").and_then(Value::as_str), ch.get("gameSocketNameSpace").and_then(Value::as_str)) else { continue };
            if let Some((_, key, _, _)) = GAMES.iter().find(|(_, p, _, _)| *p == path) {
                spots.insert(key, Spot { name: name.to_string(), nsp: nsp.to_string() });
            }
        }
        Ok(())
    }

    /// Dollars per coin.
    async fn refresh_prices(&self) {
        let Ok(prices) = self.rest("/api/game/support/system/conf/usdPrice").await else { return };
        let Some(prices) = prices.as_object() else { return };
        {
            let mut m = self.market.lock();
            for (name, p) in prices {
                if let (Some(info), Some(p)) = (m.currencies.get_mut(&name.to_ascii_lowercase()), f64_from_json(p)) {
                    info.price = p;
                }
            }
        }
        self.publish();
    }

    /// Re-reads the balances, unless a bet ran meanwhile (its local update is newer).
    async fn refresh_balances(&self) -> Result<(), CallError> {
        let settled = self.settled.load(Ordering::SeqCst);
        let idle = self.inflight.load(Ordering::SeqCst) == 0;
        let list = self.rest("/api/user/amount/").await?;
        if !(idle && self.inflight.load(Ordering::SeqCst) == 0 && self.settled.load(Ordering::SeqCst) == settled) {
            return Ok(());
        }
        let coins = self.coins.lock().clone();
        {
            let mut m = self.market.lock();
            for w in list.as_array().into_iter().flatten() {
                let Some(code) = w.get("currencyName").and_then(Value::as_str).map(str::to_ascii_lowercase) else { continue };
                let Some(coin) = coins.get(&code) else { continue };
                let bal = w.get("amount").and_then(|a| units_of(a, coin.decimals)).unwrap_or(0);
                m.wallets.insert(code, bal);
            }
            // the site keeps no coin "in use" the app can read: the one worth the most, else USDT
            if m.site_currency.is_none() {
                let usd = |(c, v): (&String, &Units)| m.currencies.get(c).filter(|i| !i.hidden).map(|i| crate::money::to_f64(*v, i.decimals) * i.price).unwrap_or(0.0);
                let top = m.wallets.iter().map(|w| (w.0.clone(), usd(w))).filter(|(_, u)| *u > 0.0).max_by(|a, b| a.1.total_cmp(&b.1)).map(|(c, _)| c);
                m.site_currency = Some(top.unwrap_or_else(|| "usdt".into()));
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

    fn credit(&self, coin: &str, delta: Units) {
        self.settled.fetch_add(1, Ordering::SeqCst);
        if delta == 0 {
            return;
        }
        *self.market.lock().wallets.entry(coin.to_string()).or_insert(0) += delta;
        self.publish();
    }

    /// The site renews its login cookies now and then: the bridge's go to the app to save.
    async fn save_login(&self) {
        if self.dev_profile.is_some() {
            return;
        }
        let Ok(bridge) = self.bridge() else { return };
        let Ok(list) = bridge.cookies(LOGIN_URLS[0]).await else { return };
        let line = LOGIN_COOKIES
            .iter()
            .filter_map(|n| list.iter().find(|c| c.name == *n && !c.value.is_empty()).map(|c| format!("{}={}", c.name, c.value)))
            .collect::<Vec<_>>()
            .join("; ");
        let mut cur = self.login.lock();
        let Some(next) = Login::parse(&line).map(|l| Login { ua: cur.as_ref().and_then(|c| c.ua.clone()), ..l }) else { return };
        if cur.as_ref() != Some(&next) {
            tracing::info!("bcgame: the site renewed the login");
            *cur = Some(next.clone());
            let _ = self.events.send(CasinoEvent::Token(next.to_token()));
        }
    }

    /// A GET of the site's REST API through the bridge: `{code, msg, data}` -> data.
    async fn rest(&self, path: &str) -> Result<Value, CallError> {
        let bridge = self.bridge()?;
        let headers = vec![("Accept".to_string(), "application/json, text/plain, */*".to_string())];
        let res = bridge.request_within(BridgeRequest { method: "GET".into(), url: path.into(), headers, body: None }, RPC_TIMEOUT).await.map_err(|e| {
            CallError::Casino(if e.contains("timeout") { CasinoError::Timeout } else if e.contains("closed") { CasinoError::ConnectionLost } else { CasinoError::Server(e) })
        })?;
        parse_rest(res.status, &res.body)
    }

    fn spot(&self, key: &str) -> Spot {
        self.spots.lock().get(key).cloned().unwrap_or_else(|| Spot { name: String::new(), nsp: String::new() })
    }

    fn coin(&self, code: &str) -> Result<Coin, CasinoError> {
        self.coins.lock().get(code).cloned().ok_or_else(|| CasinoError::Server(format!("BC.Game has no {} wallet", code.to_ascii_uppercase())))
    }

    async fn call(&self, nsp: &str, event: &str, data: Vec<u8>) -> Result<Vec<u8>, CasinoError> {
        self.call_within(nsp, event, data, RPC_TIMEOUT).await
    }

    /// One request over the site's socket: the answer's data, or its error.
    async fn call_within(&self, nsp: &str, event: &str, data: Vec<u8>, wait: Duration) -> Result<Vec<u8>, CasinoError> {
        let b64 = base64::engine::general_purpose::STANDARD;
        let bridge = self.bridge()?;
        let body = json!({ "event": event, "data": b64.encode(&data), "wait": wait.as_millis() as u64 }).to_string();
        let _one = self.one.lock().await;
        let res = bridge
            .request_within(BridgeRequest { method: "SOCK".into(), url: nsp.into(), headers: vec![], body: Some(body) }, wait + Duration::from_secs(5))
            .await
            .map_err(|e| match e.as_str() {
                "not connected" => CasinoError::NotConnected,
                "timeout" | "bridge timeout" => CasinoError::Timeout,
                e if e.contains("closed") => CasinoError::ConnectionLost,
                e if e.starts_with("no answer") || e.starts_with("refused") => CasinoError::Server(format!("BC.Game's game server didn't take the app in ({e})")),
                e => CasinoError::Server(e.to_string()),
            })?;
        let raw = b64.decode(res.body.trim()).map_err(|_| CasinoError::BadReply("unreadable socket answer".into()))?;
        let reply = Msg::parse(&raw).ok_or_else(|| CasinoError::BadReply("unreadable socket answer".into()))?;
        let code = reply.sint(1);
        if code != 0 {
            let msg = reply.str(2).filter(|m| !m.trim().is_empty()).unwrap_or_else(|| format!("BC.Game refused it (code {code})"));
            tracing::warn!("bcgame: {nsp} {event} -> {code} {msg}");
            return Err(socket_error(code, msg));
        }
        Ok(reply.bytes(3).unwrap_or_default().to_vec())
    }

    /// A bet on one of the originals: `value` is the game's own `betValue`.
    async fn place(&self, key: &'static str, coin: &str, amount: Units, value: Vec<u8>) -> Result<Settled, CasinoError> {
        if !self.ready.load(Ordering::SeqCst) {
            return Err(CasinoError::NotConnected);
        }
        let c = self.coin(coin)?;
        // the fields the site sends with every bet: no script, the page in front (-1), a real-money bet (2)
        let bet = W::default().string(1, &c.name).string(2, &amount_str(amount, c.decimals)).bytes(3, &value).sint32(4, 0).sint32(15, -1).sint32(16, 2).done();
        let spot = self.spot(key);
        let data = self.call(&spot.nsp, "bet", bet).await?;
        *self.last.lock() = key;
        Settled::parse(&data)
    }

    /// The winnings of a settled bet in base units.
    fn paid(&self, s: &Settled, coin: &str) -> Units {
        let dec = self.coins.lock().get(coin).map(|c| c.decimals).unwrap_or(8);
        units_of(&Value::String(s.win_amount.clone()), dec).unwrap_or(0)
    }

    /// The seeds of the game at `key` (`seed-next-hash`): the pair in use and its nonce.
    async fn seeds(&self, key: &str) -> Result<ProvableState, CasinoError> {
        let spot = self.spot(key);
        let data = self.call(&spot.nsp, "seed-next-hash", W::default().string(1, &spot.name).done()).await?;
        let m = Msg::parse(&data).ok_or_else(|| CasinoError::BadReply("unreadable seeds".into()))?;
        let cur = m.msg(1).ok_or_else(|| CasinoError::BadReply("no seeds in use".into()))?;
        Ok(ProvableState { client_seed: cur.str(3), server_seed_hash: cur.str(2), nonce: Some(cur.sint(4).max(0) as u64) })
    }

    /// Mines answers `join` with the round still open, if any; the site joins before its first bet.
    async fn mines_join(&self) -> Result<Option<Round>, CasinoError> {
        let spot = self.spot("mines");
        let data = self.call(&spot.nsp, "join", vec![]).await?;
        self.mines_joined.store(true, Ordering::SeqCst);
        if data.is_empty() {
            return Ok(None);
        }
        tracing::info!("bcgame: mines join -> {}", hex(&data));
        let s = Settled::parse(&data)?;
        let count = Msg::parse(&s.bet_value).map(|v| v.sint(1)).unwrap_or(0).clamp(1, 24) as u32;
        let game = Msg::parse(&s.game);
        let picks: Vec<i64> = game.as_ref().map(|g| g.packed_sint(2)).unwrap_or_default().into_iter().map(|t| t - 1).collect();
        let coin = s.currency.to_ascii_lowercase();
        let m = Msg::parse(&data).ok_or_else(|| CasinoError::BadReply("unreadable Mines round".into()))?;
        let dec = self.coins.lock().get(&coin).map(|c| c.decimals).unwrap_or(8);
        let amount = m.str(8).and_then(|a| units_of(&Value::String(a), dec)).unwrap_or(0);
        let play = MinesPlay { id: s.id.clone(), count, coin: coin.clone(), amount, picks: picks.clone() };
        *self.mines.lock() = Some(play);
        Ok(Some(mines_round(&s.id, true, &coin, amount, 0, 0.0, count, &picks, &[])))
    }

    /// A Mines answer that ends the round: `GameValue` with the board (`3`: the 25 tiles
    /// shuffled, the mines first) after a mine, or `BetResult` after a cash out.
    fn mines_over(&self, play: &MinesPlay, game: &Msg, settled: Option<&Settled>) -> Round {
        let picks: Vec<i64> = {
            let p = game.packed_sint(2);
            if p.is_empty() { play.picks.clone() } else { p.into_iter().map(|t| t - 1).collect() }
        };
        let board = game.packed_sint(3);
        let mines: Vec<i64> = board.iter().take(play.count as usize).map(|t| t - 1).collect();
        let payout = settled.map(|s| self.paid(s, &play.coin)).unwrap_or(0);
        let multiplier = settled.map(|s| s.odds).unwrap_or(0.0);
        *self.mines.lock() = None;
        self.credit(&play.coin, payout - play.amount);
        mines_round(&play.id, false, &play.coin, play.amount, payout, multiplier, play.count, &picks, &mines)
    }
}

#[async_trait]
impl Casino for BcGame {
    fn id(&self) -> CasinoId {
        CasinoId::BcGame
    }

    fn set_token(&self, token: Option<String>) {
        *self.login.lock() = token.as_deref().and_then(Login::parse);
        self.wake.notify_one();
    }

    fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.ready.store(false, Ordering::SeqCst);
        self.close_bridge();
        self.wake.notify_one();
    }

    async fn bet_limbo(&self, req: &BetRequest) -> Result<BetReply, CasinoError> {
        let _inflight = InFlight::new(&self.inflight);
        let coin = req.currency.to_ascii_lowercase();
        let target = (req.target * 100.0).round() as i32;
        let s = self.place("limbo", &coin, req.amount, W::default().sint32(1, target).done()).await?;
        let outcome = Msg::parse(&s.game).map(|g| g.sint(1) as f64 / 100.0).ok_or_else(|| CasinoError::BadReply("Limbo result without a multiplier".into()))?;
        let payout = self.paid(&s, &coin);
        self.credit(&coin, payout - req.amount);
        Ok(BetReply { outcome, amount: Some(req.amount), winnings: Some(payout), win: Some(s.result == 1), nonce: s.nonce })
    }

    async fn rotate_seed(&self, client_seed: &str) -> Result<ProvableState, CasinoError> {
        let key = *self.last.lock();
        let spot = self.spot(key);
        let seed = client_seed_for(client_seed);
        self.call(&spot.nsp, "seed-change", W::default().string(1, &spot.name).string(2, &seed).done()).await?;
        self.seeds(key).await
    }

    async fn provable_state(&self) -> Result<ProvableState, CasinoError> {
        let key = *self.last.lock();
        self.seeds(key).await
    }

    async fn list_slots(&self, _f: SlotFilter) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        Ok(vec![])
    }

    async fn launch_slot(&self, _id: &str, _demo: bool, _cur: &str, _fiat: &str) -> Result<String, CasinoError> {
        Err(CasinoError::Server("Slots on BC.Game aren't in FastBet yet".into()))
    }

    async fn original_bet(&self, game: Game, currency: &str, amount: Units, params: Value, auto: bool) -> Result<Round, CasinoError> {
        let _inflight = InFlight::new(&self.inflight);
        let coin = currency.to_ascii_lowercase();
        match game {
            Game::Dice => {
                let target = params["target"].as_f64().ok_or_else(|| CasinoError::Server("Pick a target".into()))?;
                let target = (target * 100.0).round() / 100.0;
                let over = params["condition"].as_str() != Some("below");
                // the site bets a win chance and a side: over wins above 100 - chance
                let chance = if over { 100.0 - target } else { target };
                let value = W::default().sint32(1, (chance * 100.0).round() as i32).bool(2, over).done();
                let s = self.place("classic-dice", &coin, amount, value).await?;
                let roll = Msg::parse(&s.game).map(|g| g.sint(4) as f64 / 100.0).ok_or_else(|| CasinoError::BadReply("Dice result without a roll".into()))?;
                let payout = self.paid(&s, &coin);
                self.credit(&coin, payout - amount);
                let won = s.result == 1;
                Ok(Round {
                    game,
                    id: s.id,
                    active: false,
                    currency: coin,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier: if won { s.odds } else { 0.0 },
                    state: json!({ "result": roll, "target": target, "condition": if over { "above" } else { "below" } }),
                })
            }
            Game::Keno => {
                let picks: Vec<i64> = params["numbers"].as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
                if picks.is_empty() || picks.len() > 10 {
                    return Err(CasinoError::Server("Pick 1 to 10 numbers".into()));
                }
                let risk = params["risk"].as_str().unwrap_or("classic").to_ascii_lowercase();
                // the board counts 0–39, the site 1–40
                let nums = picks.iter().map(|n| (n + 1).to_string()).collect::<Vec<_>>().join(",");
                let s = self.place("keno", &coin, amount, W::default().string(1, &format!("{risk}:{nums}")).done()).await?;
                let drawn: Vec<i64> = Msg::parse(&s.game).and_then(|g| g.str(1)).unwrap_or_default().split(',').filter_map(|n| n.trim().parse::<i64>().ok()).map(|n| n - 1).collect();
                let payout = self.paid(&s, &coin);
                self.credit(&coin, payout - amount);
                Ok(Round {
                    game,
                    id: s.id,
                    active: false,
                    currency: coin,
                    amount,
                    amount_mult: 1.0,
                    payout,
                    multiplier: s.odds,
                    state: json!({ "risk": risk, "drawnNumbers": drawn, "selectedNumbers": picks }),
                })
            }
            Game::Mines => {
                if !self.mines_joined.load(Ordering::SeqCst) {
                    if let Some(open) = self.mines_join().await? {
                        if open.active {
                            return Err(CasinoError::RoundOpen);
                        }
                    }
                }
                let count = params["minesCount"].as_u64().unwrap_or(3).clamp(1, 24) as u32;
                // an auto bet plays its round out like the site's auto mode: the bet, every tile in
                // one reveal, then the cash out
                let tiles: Vec<i64> = if auto { params["fields"].as_array().map(|a| a.iter().filter_map(Value::as_i64).filter(|t| (0..25).contains(t)).collect()).unwrap_or_default() } else { vec![] };
                if auto && tiles.is_empty() {
                    return Err(CasinoError::Server("Pick the tiles every bet reveals".into()));
                }
                let _round = if auto { Some(self.mines_auto.lock().await) } else { None };
                let s = self.place("mines", &coin, amount, W::default().sint32(1, count as i32).done()).await?;
                let play = MinesPlay { id: s.id.clone(), count, coin: coin.clone(), amount, picks: vec![] };
                *self.mines.lock() = Some(play.clone());
                if !auto {
                    return Ok(mines_round(&s.id, true, &coin, amount, 0, 0.0, count, &[], &[]));
                }
                let spot = self.spot("mines");
                let picks: Vec<i32> = tiles.iter().map(|t| (t + 1) as i32).collect();
                let data = self.call(&spot.nsp, "next", W::default().packed_sint32(1, &picks).done()).await?;
                let g = Msg::parse(&data).ok_or_else(|| CasinoError::BadReply("unreadable Mines step".into()))?;
                if g.has(3) {
                    return Ok(self.mines_over(&MinesPlay { picks: tiles, ..play }, &g, None));
                }
                let data = self.call(&spot.nsp, "cashout", W::default().sint32(15, -1).done()).await?;
                let settled = Settled::parse(&data)?;
                let board = Msg::parse(&settled.game).ok_or_else(|| CasinoError::BadReply("Mines cash out without the board".into()))?;
                Ok(self.mines_over(&MinesPlay { picks: tiles, ..play }, &board, Some(&settled)))
            }
            _ => Err(CasinoError::Server(format!("{} isn't on BC.Game in FastBet", game.name()))),
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
                self.mines_join().await?;
                let resumed = self.mines.lock().clone();
                resumed.ok_or(CasinoError::Server("No Mines round is open".into()))?
            }
        };
        let spot = self.spot("mines");
        match step {
            Step::Next => {
                let tile = params["fields"].get(0).and_then(Value::as_i64).ok_or_else(|| CasinoError::Server("Pick a tile".into()))?;
                let data = self.call(&spot.nsp, "next", W::default().packed_sint32(1, &[(tile + 1) as i32]).done()).await?;
                let g = Msg::parse(&data).ok_or_else(|| CasinoError::BadReply("unreadable Mines step".into()))?;
                if g.has(3) {
                    let mut play = play;
                    if !play.picks.contains(&tile) {
                        play.picks.push(tile);
                    }
                    return Ok(self.mines_over(&play, &g, None));
                }
                let picks: Vec<i64> = g.packed_sint(2).into_iter().map(|t| t - 1).collect();
                let mut next = play.clone();
                next.picks = if picks.is_empty() { [play.picks.clone(), vec![tile]].concat() } else { picks };
                *self.mines.lock() = Some(next.clone());
                Ok(mines_round(&next.id, true, &next.coin, next.amount, 0, 0.0, next.count, &next.picks, &[]))
            }
            Step::Cashout => {
                let data = self.call(&spot.nsp, "cashout", W::default().sint32(15, -1).done()).await?;
                let s = Settled::parse(&data)?;
                let game = Msg::parse(&s.game).ok_or_else(|| CasinoError::BadReply("Mines cash out without the board".into()))?;
                Ok(self.mines_over(&play, &game, Some(&s)))
            }
        }
    }

    async fn original_active(&self, game: Game) -> Result<Option<Round>, CasinoError> {
        if game != Game::Mines || !self.ready.load(Ordering::SeqCst) {
            return Ok(None);
        }
        let open = self.mines.lock().clone();
        if let Some(p) = open {
            return Ok(Some(mines_round(&p.id, true, &p.coin, p.amount, 0, 0.0, p.count, &p.picks, &[])));
        }
        self.mines_join().await
    }

    /// Keno's tables from the game's `init`; Mines' worked out at 98%.
    async fn original_tables(&self) -> Result<Tables, CasinoError> {
        let mut t = Tables::builtin();
        t.mines = mines_tables();
        let spot = self.spot("keno");
        for _ in 0..3 {
            if !self.keno.lock().is_empty() {
                break;
            }
            match self.call(&spot.nsp, "init", vec![]).await {
                Ok(data) => {
                    let tables = Msg::parse(&data).map(|m| keno_tables(&m)).unwrap_or_default();
                    *self.keno.lock() = tables;
                }
                // a bet on the site itself was in flight
                Err(e) if e.is_busy() => tokio::time::sleep(Duration::from_millis(500)).await,
                Err(e) => {
                    tracing::info!("bcgame: keno tables: {e}");
                    break;
                }
            }
        }
        let keno = self.keno.lock().clone();
        if !keno.is_empty() {
            t.keno = keno;
        }
        Ok(t)
    }

    /// The site's covers for its originals.
    async fn list_originals(&self) -> Result<Vec<crate::slots::SlotGame>, CasinoError> {
        let mut out = vec![];
        for (game, path, _, _) in GAMES {
            let detail = self.rest(&format!("/api/game/home/detail/?gameUrl={path}")).await.ok();
            let image = detail.as_ref().and_then(|d| d.get("iconThreeFour").or(d.get("thumbnail"))).and_then(Value::as_str).unwrap_or_default().to_string();
            let rtp = detail.as_ref().and_then(|d| d.pointer("/slotsInfo/rtpDes")).and_then(f64_from_json).filter(|r| *r > 0.0).unwrap_or(RTP);
            out.push(crate::slots::SlotGame {
                id: game.map(|g| g.slug()).unwrap_or("limbo").into(),
                name: game.map(|g| CasinoId::BcGame.original_name(g)).unwrap_or("Limbo").into(),
                producer: "bcgame".into(),
                image,
                demo: false,
                rtp,
                provider_game_id: None,
                studio: None,
            });
        }
        Ok(out)
    }
}

/// A Mines round in the app's shape, its tiles paying at 98%.
#[allow(clippy::too_many_arguments)]
fn mines_round(id: &str, active: bool, coin: &str, amount: Units, payout: Units, multiplier: f64, count: u32, picks: &[i64], mines: &[i64]) -> Round {
    Round {
        game: Game::Mines,
        id: id.to_string(),
        active,
        currency: coin.to_string(),
        amount,
        amount_mult: 1.0,
        payout,
        multiplier,
        state: mines_state_with(count, picks, mines, mines_multiplier),
    }
}

/// Mines: what `gems` safe tiles pay with `mines` mines, as the site pays a cash out: the
/// inverse odds to 4 decimals, times 98%, to 4 decimals (3 mines, 1 gem: 1.1364 × 0.98 = 1.1137).
fn mines_multiplier(mines: u32, gems: u32) -> f64 {
    if mines == 0 || mines >= 25 || gems + mines > 25 {
        return 0.0;
    }
    let round4 = |x: f64| (x * 10_000.0).round() / 10_000.0;
    let odds: f64 = (0..gems).map(|i| (25 - i) as f64 / (25 - mines - i) as f64).product();
    round4(RTP / 100.0 * round4(odds))
}

/// Mines -> payout per number of gems, for every mine count.
fn mines_tables() -> HashMap<String, Vec<f64>> {
    (1..=24u32).map(|m| (m.to_string(), (0..=25 - m).map(|g| mines_multiplier(m, g)).collect())).collect()
}

/// Keno's `init` -> risk -> [picks - 1] -> payout per number of hits, from the game config's
/// `payTable` (`[{payouts, risk}]`, picks 1 to 10 in order per risk).
fn keno_tables(init: &Msg) -> HashMap<String, Vec<Vec<f64>>> {
    let mut out: HashMap<String, Vec<Vec<f64>>> = HashMap::new();
    for cfg in init.all(2) {
        let Some(table) = Msg::parse(cfg).and_then(|c| c.str(5)) else { continue };
        let Ok(rows) = serde_json::from_str::<Vec<Value>>(&table) else { continue };
        for row in rows {
            let (Some(risk), Some(pays)) = (row.get("risk").and_then(Value::as_str), row.get("payouts").and_then(Value::as_array)) else { continue };
            out.entry(risk.to_ascii_lowercase()).or_default().push(pays.iter().filter_map(f64_from_json).collect());
        }
    }
    out.retain(|_, rows| rows.len() == 10);
    out
}

/// A client seed the site takes: 10 to 32 letters and digits.
fn client_seed_for(seed: &str) -> String {
    let mut s: String = seed.chars().filter(char::is_ascii_alphanumeric).take(32).collect();
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    while s.len() < 10 {
        s.push(CHARS[rand::random_range(0..CHARS.len())] as char);
    }
    s
}

/// The REST API's answer: `{code, msg, data}` -> data.
fn parse_rest(status: u16, body: &str) -> Result<Value, CallError> {
    let v = serde_json::from_str::<Value>(body).ok();
    let Some(v) = v.filter(|v| v.is_object()) else {
        if status == 403 || body.contains("challenge-platform") || body.contains("Just a moment") {
            return Err(CallError::Cloudflare);
        }
        if status == 401 {
            return Err(CallError::LoggedOut);
        }
        return Err(CallError::Casino(CasinoError::Server(format!("HTTP {status}: {}", body.chars().take(160).collect::<String>()))));
    };
    let code = v.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == 0 && (200..300).contains(&status) {
        return Ok(v.get("data").cloned().unwrap_or(Value::Null));
    }
    let msg = v.get("msg").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_string).unwrap_or_else(|| format!("HTTP {status}, code {code}"));
    let lower = msg.to_ascii_lowercase();
    if status == 401 || lower.contains("login") || lower.contains("log in") {
        return Err(CallError::LoggedOut);
    }
    Err(CallError::Casino(CasinoError::Server(msg)))
}

/// A socket error -> what the engine makes of it.
fn socket_error(code: i64, msg: String) -> CasinoError {
    let lower = msg.to_ascii_lowercase();
    // "End this game first, please."
    if lower.contains("end this game") || lower.contains("unfinished") || lower.contains("not finished") || lower.contains("in progress") || lower.contains("game exist") {
        return CasinoError::RoundOpen;
    }
    // 5999 "Please bet again": another bet was still in flight
    if code == 5999 || lower.contains("bet again") || lower.contains("frequen") || lower.contains("too fast") || lower.contains("too many") || lower.contains("busy") || lower.contains("please wait") {
        return CasinoError::Busy(msg);
    }
    if lower.contains("login") || lower.contains("log in") {
        return CasinoError::Server(EXPIRED.into());
    }
    CasinoError::Server(msg)
}

/// Base units -> the amount as the site writes it: "0.0001", "0".
fn amount_str(v: Units, decimals: u32) -> String {
    crate::money::format_units(v.max(0), decimals, decimals)
}

/// "0.0029" (or a number) -> base units, rounded down.
fn units_of(v: &Value, decimals: u32) -> Option<Units> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            let neg = t.starts_with('-');
            parse_units(t.trim_start_matches('-'), decimals).map(|u| if neg { -u } else { u })
        }
        _ => f64_from_json(v).filter(|f| f.is_finite()).map(|f| (f * 10f64.powi(decimals as i32) + 1e-6).floor() as Units),
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().take(96).map(|x| format!("{x:02x}")).collect()
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

/// `fastbet --bcgame-test [bets]`: connects to BC.Game the way the app does (the login in
/// `FASTBET_BCGAME_TOKEN`, or a browser profile that holds one in `FASTBET_BCGAME_PROFILE`), then
/// plays bets of 0 in USDT: Limbo, Dice, Keno and a Mines round, and reads and changes the seeds.
pub async fn self_test(bets: usize) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let token = std::env::var("FASTBET_BCGAME_TOKEN").ok().filter(|t| !t.is_empty());
    let coin = std::env::var("FASTBET_BCGAME_COIN").unwrap_or_else(|_| "usdt".into());
    let casino = BcGame::start(Setup { token, ..Default::default() }, tx);
    let t0 = Instant::now();
    let show = |e: CasinoEvent| match e {
        CasinoEvent::Status(s, d) => println!("status: {} {d}", s.short()),
        CasinoEvent::Market(m) => {
            let held: Vec<String> = m.wallets.iter().filter(|(_, v)| **v != 0).map(|(c, v)| format!("{c} {v}")).collect();
            println!("account: {:?}, {} coins, held: {}", m.username, m.currencies.len(), held.join(", "));
        }
        CasinoEvent::Provable(p) => println!("seeds: client {:?}, server hash {:?}, nonce {:?}", p.client_seed, p.server_seed_hash, p.nonce),
        CasinoEvent::Token(t) => println!("login renewed ({} characters)", t.len()),
        CasinoEvent::Vip(v) => println!("vip: {} ({} xp)", v.rank, v.exp),
    };
    loop {
        match tokio::time::timeout(Duration::from_secs(120), rx.recv()).await {
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
    while let Ok(e) = rx.try_recv() {
        show(e);
    }
    // Limbo: one after the other, then several at once
    let req = BetRequest { currency: coin.clone(), amount: 0, target: 2.0 };
    let t = Instant::now();
    for _ in 0..bets {
        match casino.bet_limbo(&req).await {
            Ok(r) => println!("limbo {:.2}x win {:?} nonce {:?}", r.outcome, r.win, r.nonce),
            Err(e) => println!("limbo: {e}"),
        }
    }
    println!("{bets} limbo bets one by one: {} ms each", t.elapsed().as_millis() / bets.max(1) as u128);
    let t = Instant::now();
    let all = futures_util::future::join_all((0..bets).map(|_| casino.bet_limbo(&req))).await;
    let ok = all.iter().filter(|r| r.is_ok()).count();
    println!("{bets} limbo bets at once: {ok} ok in {} ms {:?}", t.elapsed().as_millis(), all.iter().filter_map(|r| r.as_ref().err().map(|e| e.to_string())).next());

    let dice = casino.original_bet(Game::Dice, &coin, 0, json!({ "target": 50.5, "condition": "above" }), false).await;
    println!("dice: {:?}", dice.map(|r| (r.state, r.multiplier)));
    let keno = casino.original_bet(Game::Keno, &coin, 0, json!({ "numbers": [0, 5, 9, 17, 39], "risk": "medium" }), false).await;
    println!("keno: {:?}", keno.map(|r| (r.state, r.multiplier)));
    match casino.original_tables().await {
        Ok(t) => println!("keno tables: {:?}, medium 5 picks {:?}", t.keno.keys().collect::<Vec<_>>(), t.keno("medium", 5)),
        Err(e) => println!("tables: {e}"),
    }
    match casino.original_active(Game::Mines).await {
        Ok(Some(r)) => {
            println!("mines open before: {} active {} state {}", r.id, r.active, r.state);
            // a round without a gem can't be cashed out: reveal one first
            if r.state["rounds"].as_array().is_none_or(|a| a.is_empty()) {
                let _ = casino.original_step(Game::Mines, Step::Next, json!({ "fields": [7] })).await.map(|r| println!("mines resumed tile 7: active {}", r.active));
            }
            match casino.original_step(Game::Mines, Step::Cashout, json!({})).await {
                Ok(r) => println!("mines resumed and cashed out: x{} state {}", r.multiplier, r.state),
                Err(e) => println!("mines resumed cash out: {e}"),
            }
        }
        Ok(None) => println!("mines open before: none"),
        Err(e) => println!("mines join: {e}"),
    }
    match casino.original_bet(Game::Mines, &coin, 0, json!({ "minesCount": 3 }), false).await {
        Ok(r) if std::env::var_os("FASTBET_BCGAME_LEAVE_MINES").is_some() => {
            let _ = casino.original_step(Game::Mines, Step::Next, json!({ "fields": [12] })).await.map(|r| println!("mines tile 12: {}", r.state));
            println!("mines {} left open", r.id);
        }
        Ok(r) => {
            println!("mines started {} active {}", r.id, r.active);
            for tile in [0, 24, 12] {
                match casino.original_step(Game::Mines, Step::Next, json!({ "fields": [tile] })).await {
                    Ok(r) => {
                        println!("mines tile {tile}: active {} state {}", r.active, r.state);
                        if !r.active {
                            break;
                        }
                    }
                    Err(e) => {
                        println!("mines tile {tile}: {e}");
                        break;
                    }
                }
            }
            if casino.mines.lock().is_some() {
                match casino.original_step(Game::Mines, Step::Cashout, json!({})).await {
                    Ok(r) => println!("mines cash out: x{} payout {} state {}", r.multiplier, r.payout, r.state),
                    Err(e) => println!("mines cash out: {e}"),
                }
            }
        }
        Err(e) => println!("mines: {e}"),
    }
    // auto rounds (bet, reveal, cash out in the driver), two sent at once
    let auto = || casino.original_bet(Game::Mines, &coin, 0, json!({ "minesCount": 3, "fields": [0, 24] }), true);
    let (a, b) = tokio::join!(auto(), auto());
    for r in [a, b] {
        match r {
            Ok(r) => println!("mines auto: active {} x{} state {}", r.active, r.multiplier, r.state),
            Err(e) => println!("mines auto: {e}"),
        }
    }
    // the hidden window over a while: does the site's socket stay up?
    if let Some(secs) = std::env::var("FASTBET_BCGAME_IDLE").ok().and_then(|s| s.parse::<u64>().ok()) {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            tokio::time::sleep(Duration::from_secs(60)).await;
            let t = Instant::now();
            match casino.bet_limbo(&req).await {
                Ok(r) => println!("after {} s idle: limbo {:.2}x in {} ms", t0.elapsed().as_secs(), r.outcome, t.elapsed().as_millis()),
                Err(e) => println!("after {} s idle: limbo: {e}", t0.elapsed().as_secs()),
            }
        }
    }
    match casino.provable_state().await {
        Ok(p) => println!("seeds before: {p:?}"),
        Err(e) => println!("seeds: {e}"),
    }
    match casino.rotate_seed("FastBetTest").await {
        Ok(p) => println!("seeds after change: {p:?}"),
        Err(e) => println!("seed change: {e}"),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    while let Ok(e) = rx.try_recv() {
        show(e);
    }
    casino.shutdown();
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins() {
        let l = Login::parse("SESSION=s; _ga=x; TK=t; cf_clearance=c").unwrap();
        assert_eq!(l.line(), "SESSION=s; TK=t; cf_clearance=c");
        assert!(Login::parse("TK=t; cf_clearance=c").is_none());
        let j = Login::parse(r#"{"cookies":"SESSION=s; TK=t","ua":"Mozilla/5.0 Edg/141"}"#).unwrap();
        assert_eq!(j.ua.as_deref(), Some("Mozilla/5.0 Edg/141"));
        assert_eq!(Login::parse(&j.to_token()), Some(j));
    }

    #[test]
    fn frames_from_the_site() {
        // a Limbo bet the site sent: 0 ETH at 2.00×
        let bet = W::default().string(1, "ETH").string(2, "0").bytes(3, &W::default().sint32(1, 200).done()).sint32(4, 0).sint32(15, -1).sint32(16, 2).done();
        assert_eq!(hex(&bet), "0a034554481201301a0308900320007801800104");
        // Mines: reveal tile 12 (the site counts 1–25)
        assert_eq!(hex(&W::default().packed_sint32(1, &[12]).done()), "0a0118");
        assert_eq!(hex(&W::default().sint32(15, -1).done()), "7801");
        // Dice: 50% under
        assert_eq!(hex(&W::default().sint32(1, 5000).bool(2, false).done()), "08904e1000");
    }

    #[test]
    fn results_from_the_site() {
        let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        // a winning Limbo bet: 4.27× against a 2× target
        let data = unhex("0a1331383739383533363237323233383931323931100218c0b80220022a0308d606308c80f594ad683a034554484201304a0130");
        let s = Settled::parse(&data).unwrap();
        assert_eq!((s.id.as_str(), s.result, s.odds, s.nonce), ("1879853627223891291", 1, 2.0, Some(1)));
        assert_eq!(Msg::parse(&s.game).unwrap().sint(1), 427);
        assert_eq!(s.currency, "ETH");
        // a Mines step: 1.1667× after tile 12
        let step = unhex("0926e4839ecdaaf23f120118");
        let g = Msg::parse(&step).unwrap();
        assert_eq!(g.double(1), Some(1.1667));
        assert_eq!(g.packed_sint(2), vec![12]);
        assert!(!g.has(3));
        // the answer around it: code 0, data
        let wrapped = W::default().bytes(3, &step).done();
        assert_eq!(Msg::parse(&wrapped).unwrap().sint(1), 0);
        assert!(socket_error(5999, "Please bet again".into()).is_busy());
        let refused = W::default().sint32(1, 1003).string(2, "Insufficient balance").done();
        let m = Msg::parse(&refused).unwrap();
        assert_eq!((m.sint(1), m.str(2).as_deref()), (1003, Some("Insufficient balance")));
        assert!(socket_error(1003, "Insufficient balance".into()).is_balance());
    }

    #[test]
    fn mines_pays_like_the_site() {
        // the site's cash outs: 4 mines -> 1.1667, 1.4, 1.6947; 3 mines, 1 gem -> 1.1137
        assert_eq!(mines_multiplier(3, 1), 1.1137);
        assert_eq!(mines_multiplier(4, 1), 1.1667);
        assert_eq!(mines_multiplier(4, 2), 1.4);
        assert_eq!(mines_multiplier(4, 3), 1.6947);
        assert_eq!(mines_tables()["4"][0], 0.98);
    }

    #[test]
    fn amounts_and_seeds() {
        assert_eq!(amount_str(0, 6), "0");
        assert_eq!(amount_str(100, 6), "0.0001");
        assert_eq!(amount_str(12_500_000, 6), "12.5");
        assert_eq!(units_of(&json!("0.0029"), 6), Some(2900));
        let s = client_seed_for("Fast-Bet!");
        assert!(s.starts_with("FastBet") && s.len() == 10, "{s}");
        assert!(client_seed_for("ab").len() == 10 && client_seed_for("ab").starts_with("ab"));
        assert_eq!(client_seed_for(&"x".repeat(40)).len(), 32);
    }

    #[test]
    fn keno_from_init() {
        let table = r#"[{"payouts":[0,3.96],"risk":"classic"},{"payouts":[0,1.9,4.5],"risk":"classic"},{"payouts":[0,1,3.1,10.4],"risk":"classic"},{"payouts":[0,0.8,1.8,5,22.5],"risk":"classic"},{"payouts":[0,0.25,1.4,4.1,16.5,36],"risk":"classic"},{"payouts":[0,0,1,3.68,7,16.5,40],"risk":"classic"},{"payouts":[0,0,0.47,3,4.5,14,31,60],"risk":"classic"},{"payouts":[0,0,0,2.2,4,13,22,55,70],"risk":"classic"},{"payouts":[0,0,0,1.55,3,8,15,44,60,85],"risk":"classic"},{"payouts":[0,0,0,1.4,2.25,4.5,8,17,50,80,100],"risk":"classic"},{"payouts":[0.7,1.85],"risk":"low"}]"#;
        let cfg = W::default().string(1, "KenoSingle98").string(2, "/g/kenos/98").string(5, table).done();
        let init = W::default().bytes(2, &cfg).done();
        let t = keno_tables(&Msg::parse(&init).unwrap());
        assert_eq!(t["classic"][0], vec![0.0, 3.96]);
        assert_eq!(t["classic"][9][10], 100.0);
        // a risk without all ten rows is left out
        assert!(!t.contains_key("low"));
    }
}
