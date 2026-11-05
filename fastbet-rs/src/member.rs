//! The FastBet account: the app opens for a FastBet account (made on the website) with a running
//! Weekly or Monthly plan. The website's `/api/app/*` logs in and reports the plan; the session is
//! kept in `member.json` next to the settings, checked when the app starts and every few minutes,
//! and the app locks again the moment the plan runs out.
//!
//! What the account bets for real is reported to `/api/app/wager` every few seconds (in USD, per
//! casino), for the profile and the admin page.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The website. `FASTBET_SITE` when building points the app at another one; at run time only a
/// debug build listens to it (a release build talks to the website it was built for).
const SITE: &str = "https://fastbetsite.vercel.app";

pub fn site() -> String {
    let at_run = if cfg!(debug_assertions) { std::env::var("FASTBET_SITE").ok() } else { None };
    at_run
        .or_else(|| option_env!("FASTBET_SITE").map(str::to_string))
        .unwrap_or_else(|| SITE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// The website's public key for plan tickets (Ed25519; the private half is its PLAN_SIGNING_KEY).
const PLAN_KEY: [u8; 32] = [41, 63, 17, 134, 140, 92, 200, 139, 48, 180, 238, 214, 81, 53, 131, 193, 216, 96, 196, 133, 69, 113, 60, 69, 251, 118, 72, 15, 105, 237, 50, 185];

/// A plan ticket's payload: the plan as the website saw it, for one account on one PC.
#[derive(Debug, Deserialize)]
struct Ticket {
    /// account (user id)
    u: String,
    /// device
    d: String,
    /// unix seconds the plan ends
    e: u64,
    /// unix seconds the ticket lets the app run without reaching the website
    o: u64,
}

/// The ticket's plan when the website signed it, for this account on this PC.
fn ticket(s: &Saved) -> Option<Ticket> {
    ticket_signed_by(&PLAN_KEY, s)
}

fn ticket_signed_by(key: &[u8], s: &Saved) -> Option<Ticket> {
    use base64::Engine;
    let (payload, sig) = s.ticket.split_once('.')?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let (payload, sig) = (b64.decode(payload).ok()?, b64.decode(sig).ok()?);
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, key).verify(&payload, &sig).ok()?;
    let t: Ticket = serde_json::from_slice(&payload).ok()?;
    (t.d == device().id && t.u == s.user_id).then_some(t)
}

/// Until when the app may open on this session: the plan's end, or sooner the end of the time it
/// may run without reaching the website. 0 without a valid ticket.
fn open_until(s: &Saved) -> u64 {
    ticket(s).map_or(0, |t| t.e.min(t.o))
}

/// This PC as one of the account's devices.
pub struct Device {
    /// From the system's own machine id, so a member.json copied to another PC isn't this one.
    pub id: String,
    /// Its name in the account's device list.
    pub label: String,
}

pub fn device() -> &'static Device {
    static DEVICE: OnceLock<Device> = OnceLock::new();
    DEVICE.get_or_init(|| {
        use sha2::{Digest, Sha256};
        // FASTBET_DEVICE=<name> plays another PC (debug builds, for trying the device limit)
        let test = if cfg!(debug_assertions) { std::env::var("FASTBET_DEVICE").ok() } else { None };
        let machine = test.clone().or_else(machine_id).unwrap_or_else(saved_device_id);
        let hash = Sha256::digest(format!("fastbet-device:{machine}").as_bytes());
        let id: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
        let name = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .ok()
            .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "PC".into());
        let os = if cfg!(windows) { "Windows" } else if cfg!(target_os = "macos") { "macOS" } else { "Linux" };
        let label = match test {
            Some(t) => format!("{t} ({os}, test)"),
            None => format!("{name} ({os})"),
        };
        Device { id, label }
    })
}

/// The system's machine id: Windows' MachineGuid, Linux's /etc/machine-id.
fn machine_id() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let out = std::process::Command::new("reg")
            .args(["query", r"HKLM\SOFTWARE\Microsoft\Cryptography", "/v", "MachineGuid"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find(|l| l.contains("MachineGuid"))?;
        return line.split_whitespace().last().map(str::to_string).filter(|g| g.len() >= 16);
    }
    #[cfg(not(windows))]
    {
        ["/etc/machine-id", "/var/lib/dbus/machine-id"].iter().find_map(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim().to_string()).filter(|s| s.len() >= 16)
    }
}

/// Without a machine id: a random one, kept next to the settings.
fn saved_device_id() -> String {
    let path = crate::paths::config_dir().map(|d| d.join("device-id"));
    if let Some(id) = path.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| s.trim().to_string()).filter(|s| s.len() >= 16) {
        return id;
    }
    let id: String = (0..16).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
    if let Some(p) = path {
        let _ = std::fs::write(p, &id);
    }
    id
}

/// A page of the website, e.g. `page("/plans")`.
pub fn page(path: &str) -> String {
    format!("{}{path}", site())
}

/// How often a running plan is checked again (an admin may change it, the session renews).
const RECHECK: Duration = Duration::from_secs(10 * 60);
/// Without a plan: how often the app looks again, so it opens by itself once a payment lands.
const RECHECK_LOCKED: Duration = Duration::from_secs(45);

/// The session as saved on this PC.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    access_token: String,
    refresh_token: String,
    username: String,
    email: String,
    plan: String,
    /// Unix seconds the plan ends (0 = none), as the website said: shown, not trusted (see `ticket`).
    plan_until: u64,
    /// Unix seconds of the last answer from the website.
    checked_at: u64,
    user_id: String,
    /// The plan signed by the website for this account on this PC: what opens the app.
    ticket: String,
}

#[derive(Debug, Deserialize)]
struct Reply {
    access_token: String,
    refresh_token: String,
    user: User,
    plan: Plan,
    #[serde(default)]
    ticket: Option<String>,
}

#[derive(Debug, Deserialize)]
struct User {
    #[serde(default)]
    id: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    email: String,
}

#[derive(Debug, Deserialize)]
struct Plan {
    plan: String,
    active: bool,
    #[serde(default)]
    seconds_left: i64,
}

#[derive(Debug, Deserialize)]
struct ErrorReply {
    #[serde(default)]
    code: String,
    #[serde(default)]
    error: String,
}

/// What the app shows: the gate, or the app itself.
#[derive(Clone, Debug, PartialEq)]
pub enum Gate {
    /// Checking the saved login (app start).
    Checking,
    /// Nobody logged in: the login form, with why if there was a reason.
    LoggedOut { error: Option<String> },
    LoggingIn,
    /// Logged in, but no running plan. `ended`: the plan there was (it ran out).
    NoPlan { username: String, ended: Option<String> },
    /// The website couldn't be reached and there's no recent check to go on.
    Offline { error: String },
    /// Another device took this PC's place on the account: "use it here" takes it back.
    Replaced { username: String },
    /// In. `offline`: running on the last check (the website couldn't be reached).
    Active { username: String, plan: String, offline: bool },
}

struct Inner {
    gate: Gate,
    saved: Option<Saved>,
    /// A request is running.
    busy: bool,
    next_check: Instant,
}

pub struct Member {
    inner: Arc<Mutex<Inner>>,
    repaint: Arc<dyn Fn() + Send + Sync>,
    wager: Arc<Mutex<Wager>>,
}

/// How often the wager goes to the website (only when something was bet), so the profile and the
/// admin page follow it nearly live.
const WAGER_EVERY: Duration = Duration::from_secs(15);

/// Wager not on the website yet.
#[derive(Default)]
struct Wager {
    /// Bet since the last report, per casino key: USD and how many bets.
    pending: BTreeMap<String, (f64, u64)>,
    /// A report sent and not confirmed: it keeps its id, so sending it again counts it once.
    out: Option<Report>,
    sending: bool,
    next: Option<Instant>,
}

#[derive(Clone)]
struct Report {
    id: String,
    entries: Vec<(String, f64, u64)>,
}

/// A random UUID (v4), a report's id.
fn report_id() -> String {
    let mut b: [u8; 16] = rand::random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

/// Sends a report: Ok when the website has it (or will never take it, so it isn't sent again).
fn send_report(token: &str, r: &Report, timeout: Duration) -> Result<(), String> {
    let wagers: Vec<serde_json::Value> = r.entries.iter().map(|(c, usd, bets)| serde_json::json!({ "casino": c, "usd": usd, "bets": bets })).collect();
    let resp = reqwest::blocking::Client::builder()
        .user_agent(format!("FastBet/{}", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?
        .post(page("/api/app/wager"))
        .json(&serde_json::json!({ "access_token": token, "report_id": r.id, "wagers": wagers }))
        .send()
        .map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200..=299 => Ok(()),
        // a report the website can't read never will be
        400 => {
            tracing::warn!("FastBet wager report refused: {}", resp.text().unwrap_or_default());
            Ok(())
        }
        code => Err(format!("FastBet answered {code}")),
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FASTBET_MEMBER") {
        return Some(PathBuf::from(p));
    }
    Some(crate::paths::config_dir()?.join("member.json"))
}

fn load() -> Option<Saved> {
    let text = std::fs::read_to_string(path()?).ok()?;
    serde_json::from_str::<Saved>(&text).ok().filter(|s| !s.refresh_token.is_empty())
}

fn store(saved: Option<&Saved>) {
    let Some(path) = path() else { return };
    let res = match saved {
        Some(s) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = path.with_extension("json.tmp");
            serde_json::to_string_pretty(s)
                .map_err(|e| e.to_string())
                .and_then(|t| std::fs::write(&tmp, t).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| e.to_string()))
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        },
    };
    if let Err(e) = res {
        tracing::warn!("couldn't save the FastBet login: {e}");
    }
}

fn client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(format!("FastBet/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| e.to_string())
}

/// Why a request didn't give a session.
enum Fail {
    /// The website said no to the login itself (wrong password, logged out, account gone).
    Refused(String),
    /// Couldn't get an answer (no connection, the site is down or busy): try again later.
    Unreachable(String),
    /// The account plays on another device in this PC's place.
    Replaced,
}

fn post(path: &str, body: serde_json::Value) -> Result<Reply, Fail> {
    let resp = client()
        .map_err(Fail::Unreachable)?
        .post(page(path))
        .json(&body)
        .send()
        .map_err(|e| Fail::Unreachable(format!("Couldn't reach FastBet: {e}")))?;
    let status = resp.status();
    if status.is_success() {
        return resp.json::<Reply>().map_err(|e| Fail::Unreachable(format!("Unexpected answer from FastBet: {e}")));
    }
    let err = resp.json::<ErrorReply>().unwrap_or(ErrorReply { code: String::new(), error: String::new() });
    let text = if err.error.is_empty() { format!("FastBet answered {status}") } else { err.error };
    if err.code == "device_replaced" {
        Err(Fail::Replaced)
    } else if status.as_u16() == 401 || err.code == "bad_login" || err.code == "signed_out" {
        Err(Fail::Refused(text))
    } else {
        Err(Fail::Unreachable(text))
    }
}

fn saved_from(r: Reply) -> Saved {
    let plan_until = if r.plan.active && r.plan.seconds_left > 0 { now() + r.plan.seconds_left as u64 } else { 0 };
    Saved {
        access_token: r.access_token,
        refresh_token: r.refresh_token,
        username: r.user.username,
        email: r.user.email,
        // kept after it runs out too, for "Your Weekly plan ran out"
        plan: r.plan.plan,
        plan_until,
        checked_at: now(),
        user_id: r.user.id,
        ticket: r.ticket.unwrap_or_default(),
    }
}

/// The gate for a saved session, by the ticket it holds and this PC's clock.
fn gate_for(s: &Saved, offline: bool) -> Gate {
    let name = if s.username.is_empty() { s.email.clone() } else { s.username.clone() };
    if open_until(s) > now() {
        Gate::Active { username: name, plan: s.plan.clone(), offline }
    } else if s.plan_until > now() {
        // the plan runs, but nothing the website signed says so for this PC
        let error = if offline { "FastBet hasn't been reached for a while: connect to keep playing." } else { "FastBet couldn't confirm your plan on this PC. Try again in a minute." };
        Gate::Offline { error: error.into() }
    } else {
        let had = matches!(s.plan.as_str(), "weekly" | "monthly");
        Gate::NoPlan { username: name, ended: had.then(|| s.plan.clone()) }
    }
}

impl Member {
    pub fn new(repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let saved = load();
        let gate = if saved.is_some() { Gate::Checking } else { Gate::LoggedOut { error: None } };
        let m = Self { inner: Arc::new(Mutex::new(Inner { gate, saved, busy: false, next_check: Instant::now() })), repaint, wager: Default::default() };
        if m.inner.lock().saved.is_some() {
            m.check();
        }
        m
    }

    pub fn gate(&self) -> Gate {
        self.inner.lock().gate.clone()
    }

    /// The app may be used.
    pub fn unlocked(&self) -> bool {
        matches!(self.inner.lock().gate, Gate::Active { .. })
    }

    /// Seconds the plan has left (0 when there's none).
    pub fn seconds_left(&self) -> u64 {
        self.inner.lock().saved.as_ref().map(|s| s.plan_until.saturating_sub(now())).unwrap_or(0)
    }

    pub fn busy(&self) -> bool {
        self.inner.lock().busy
    }

    /// Every frame: locks the moment the plan ends, and checks again when it's time.
    pub fn tick(&self) {
        let due = {
            let mut i = self.inner.lock();
            if let (Gate::Active { .. }, Some(s)) = (&i.gate, &i.saved) {
                if open_until(s) <= now() {
                    // ran out (or its time away from the website did): locked at once, then the
                    // website is asked (a renewal may have landed)
                    i.gate = gate_for(s, false);
                    i.next_check = Instant::now();
                }
            }
            let wanted = matches!(i.gate, Gate::Active { .. } | Gate::NoPlan { .. });
            wanted && !i.busy && Instant::now() >= i.next_check
        };
        if due {
            self.check();
        }
        let report = {
            let mut w = self.wager.lock();
            let now = Instant::now();
            if w.sending || w.next.is_some_and(|t| now < t) {
                false
            } else {
                w.next = Some(now + WAGER_EVERY);
                true
            }
        };
        if report {
            self.report_wager();
        }
    }

    /// A real-money bet worth `usd` on `casino` (its key): counted for the account's wager.
    pub fn add_wager(&self, casino: &str, usd: f64) {
        if !(usd.is_finite() && usd > 0.0) {
            return;
        }
        let mut w = self.wager.lock();
        let e = w.pending.entry(casino.to_string()).or_default();
        e.0 += usd;
        e.1 += 1;
    }

    /// The report to send now, and the token to send it with: the one not confirmed yet, else
    /// what's pending. None when there's nothing, or nobody logged in.
    fn take_report(&self) -> Option<(String, Report)> {
        let token = self.inner.lock().saved.as_ref().map(|s| s.access_token.clone())?;
        let mut w = self.wager.lock();
        if w.sending {
            return None;
        }
        if w.out.is_none() {
            if w.pending.is_empty() {
                return None;
            }
            let entries = std::mem::take(&mut w.pending).into_iter().map(|(c, (usd, bets))| (c, usd, bets)).collect();
            w.out = Some(Report { id: report_id(), entries });
        }
        w.sending = true;
        Some((token, w.out.clone()?))
    }

    fn report_done(wager: &Mutex<Wager>, res: Result<(), String>) {
        let mut w = wager.lock();
        w.sending = false;
        match res {
            Ok(()) => w.out = None,
            Err(e) => tracing::warn!("FastBet wager report: {e} (sent again later)"),
        }
    }

    /// Sends the wager since the last report, on a worker thread.
    fn report_wager(&self) {
        let Some((token, report)) = self.take_report() else { return };
        let wager = self.wager.clone();
        std::thread::spawn(move || Self::report_done(&wager, send_report(&token, &report, Duration::from_secs(20))));
    }

    /// The app is closing: the last report goes now (a few seconds at most).
    pub fn flush_wager(&self) {
        // the one a worker is sending: give it a moment
        for _ in 0..20 {
            if !self.wager.lock().sending {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        // the unconfirmed one, then what's pending; offline, the first try is enough
        for _ in 0..2 {
            let Some((token, report)) = self.take_report() else { return };
            let res = send_report(&token, &report, Duration::from_secs(3));
            let failed = res.is_err();
            Self::report_done(&self.wager, res);
            if failed {
                return;
            }
        }
    }

    /// Asks the website about the saved session (renewing it when needed), on a worker thread.
    pub fn check(&self) {
        self.ask(false);
    }

    /// "Use it here": takes this PC's place back from the device that bumped it.
    pub fn claim(&self) {
        self.ask(true);
    }

    fn ask(&self, claim: bool) {
        let body = {
            let mut i = self.inner.lock();
            if i.busy {
                return;
            }
            let Some(s) = &i.saved else { return };
            let d = device();
            let body = serde_json::json!({
                "access_token": s.access_token,
                "refresh_token": s.refresh_token,
                "device_id": d.id,
                "device_label": d.label,
                "claim": claim,
            });
            i.busy = true;
            body
        };
        let (inner, repaint) = (self.inner.clone(), self.repaint.clone());
        std::thread::spawn(move || {
            let res = post("/api/app/session", body);
            let mut i = inner.lock();
            i.busy = false;
            match res {
                Ok(r) => {
                    let s = saved_from(r);
                    store(Some(&s));
                    i.gate = gate_for(&s, false);
                    i.saved = Some(s);
                }
                Err(Fail::Refused(e)) => {
                    store(None);
                    i.saved = None;
                    i.gate = Gate::LoggedOut { error: Some(e) };
                }
                Err(Fail::Replaced) => {
                    let name = i.saved.as_ref().map(|s| if s.username.is_empty() { s.email.clone() } else { s.username.clone() }).unwrap_or_default();
                    i.gate = Gate::Replaced { username: name };
                }
                Err(Fail::Unreachable(e)) => {
                    tracing::warn!("FastBet plan check: {e}");
                    // away from the website, the ticket says how long the app may run
                    i.gate = match &i.saved {
                        Some(s) if open_until(s) > now() => gate_for(s, true),
                        // nothing recent to go on; a plan already known to be over stays over
                        Some(s) if s.plan_until <= now() && s.checked_at > 0 && !matches!(i.gate, Gate::Checking) => gate_for(s, false),
                        _ => Gate::Offline { error: e },
                    };
                }
            }
            let wait = if matches!(i.gate, Gate::Active { offline: false, .. }) {
                RECHECK
            } else if matches!(i.gate, Gate::Active { offline: true, .. } | Gate::Offline { .. }) {
                Duration::from_secs(60)
            } else {
                RECHECK_LOCKED
            };
            i.next_check = Instant::now() + wait;
            drop(i);
            repaint();
        });
    }

    pub fn login(&self, login: String, password: String) {
        {
            let mut i = self.inner.lock();
            if i.busy {
                return;
            }
            i.busy = true;
            i.gate = Gate::LoggingIn;
        }
        let (inner, repaint) = (self.inner.clone(), self.repaint.clone());
        std::thread::spawn(move || {
            let d = device();
            let res = post("/api/app/login", serde_json::json!({ "login": login.trim(), "password": password, "device_id": d.id, "device_label": d.label }));
            let mut i = inner.lock();
            i.busy = false;
            match res {
                Ok(r) => {
                    let s = saved_from(r);
                    store(Some(&s));
                    i.gate = gate_for(&s, false);
                    i.saved = Some(s);
                    i.next_check = Instant::now() + if matches!(i.gate, Gate::Active { .. }) { RECHECK } else { RECHECK_LOCKED };
                }
                Err(Fail::Refused(e) | Fail::Unreachable(e)) => i.gate = Gate::LoggedOut { error: Some(e) },
                // a login takes the PC's place: the website doesn't answer this to one
                Err(Fail::Replaced) => i.gate = Gate::LoggedOut { error: None },
            }
            drop(i);
            repaint();
        });
    }

    /// Forgets the session here and ends it on the website.
    pub fn logout(&self) {
        // what wasn't reported yet belongs to this account: it goes before the session ends
        let report = self.take_report();
        *self.wager.lock() = Wager::default();
        let saved = {
            let mut i = self.inner.lock();
            i.gate = Gate::LoggedOut { error: None };
            i.saved.take()
        };
        store(None);
        if let Some(s) = saved {
            std::thread::spawn(move || {
                if let Some((token, r)) = report {
                    let _ = send_report(&token, &r, Duration::from_secs(10));
                }
                let _ = client().and_then(|c| {
                    c.post(page("/api/app/logout")).json(&serde_json::json!({ "access_token": s.access_token, "device_id": device().id })).send().map_err(|e| e.to_string())
                });
            });
        }
        (self.repaint)();
    }
}

/// "29d 4h", "5h 12m", "8m".
pub fn time_left(secs: u64) -> String {
    let m = secs / 60;
    let (d, h) = (m / 1440, (m % 1440) / 60);
    if d > 0 {
        if h > 0 { format!("{d}d {h}h") } else { format!("{d}d") }
    } else if h > 0 {
        format!("{h}h {}m", m % 60)
    } else {
        format!("{}m", m.max(1))
    }
}

/// "Weekly" / "Monthly".
pub fn plan_label(plan: &str) -> String {
    match plan {
        "weekly" => "Weekly".into(),
        "monthly" => "Monthly".into(),
        "" => "No plan".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn key() -> Ed25519KeyPair {
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new()).unwrap();
        Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap()
    }

    /// A ticket the way the website makes one.
    fn signed(k: &Ed25519KeyPair, payload: &str) -> String {
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        format!("{}.{}", b64.encode(payload), b64.encode(k.sign(payload.as_bytes()).as_ref()))
    }

    #[test]
    fn only_a_ticket_the_website_signed_for_this_pc_opens_the_app() {
        let k = key();
        let public = k.public_key().as_ref().to_vec();
        let d = &device().id;
        let payload = format!(r#"{{"u":"user-1","d":"{d}","p":"weekly","e":2000000000,"o":1900000000,"i":1}}"#);
        let mut s = Saved { user_id: "user-1".into(), ticket: signed(&k, &payload), ..Default::default() };
        let t = ticket_signed_by(&public, &s).expect("a good ticket");
        assert_eq!((t.e, t.o), (2_000_000_000, 1_900_000_000));

        // another account's session, another PC, an edited plan, another key: no
        s.user_id = "user-2".into();
        assert!(ticket_signed_by(&public, &s).is_none());
        s.user_id = "user-1".into();
        let other_pc = payload.replace(d.as_str(), "0123456789abcdef0123456789abcdef");
        assert!(ticket_signed_by(&public, &Saved { ticket: signed(&k, &other_pc), ..s.clone() }).is_none());
        let (_, sig) = s.ticket.split_once('.').unwrap();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let longer = format!("{}.{sig}", b64.encode(payload.replace("2000000000", "2100000000")));
        assert!(ticket_signed_by(&public, &Saved { ticket: longer, ..s.clone() }).is_none());
        assert!(ticket_signed_by(&key().public_key().as_ref().to_vec(), &s).is_none());
        // and the app's own key doesn't take this test key's tickets
        assert!(ticket(&s).is_none());
    }

    #[test]
    fn report_ids_are_uuids() {
        let id = report_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
    }
}
