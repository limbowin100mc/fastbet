//! The FastBet account: the app opens for a FastBet account (made on the website) with a running
//! Weekly or Monthly plan. The website's `/api/app/*` logs in and reports the plan; the session is
//! kept in `member.json` next to the settings, checked when the app starts and every few minutes,
//! and the app locks again the moment the plan runs out.
//!
//! What the account bets for real is reported to `/api/app/wager` every minute (in USD, per casino),
//! for the admin page.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The website. `FASTBET_SITE` (when running, or when building) points the app at another one.
const SITE: &str = "https://fastbetsite.vercel.app";

pub fn site() -> String {
    std::env::var("FASTBET_SITE")
        .ok()
        .or_else(|| option_env!("FASTBET_SITE").map(str::to_string))
        .unwrap_or_else(|| SITE.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// A page of the website, e.g. `page("/plans")`.
pub fn page(path: &str) -> String {
    format!("{}{path}", site())
}

/// How often a running plan is checked again (an admin may change it, the session renews).
const RECHECK: Duration = Duration::from_secs(10 * 60);
/// Without a plan: how often the app looks again, so it opens by itself once a payment lands.
const RECHECK_LOCKED: Duration = Duration::from_secs(45);
/// When the website can't be reached, a plan checked this recently and still running is trusted.
const OFFLINE_GRACE_SECS: u64 = 3 * 24 * 3600;

/// The session as saved on this PC.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    access_token: String,
    refresh_token: String,
    username: String,
    email: String,
    plan: String,
    /// Unix seconds the plan ends (0 = none).
    plan_until: u64,
    /// Unix seconds of the last answer from the website.
    checked_at: u64,
}

#[derive(Debug, Deserialize)]
struct Reply {
    access_token: String,
    refresh_token: String,
    user: User,
    plan: Plan,
}

#[derive(Debug, Deserialize)]
struct User {
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

/// How often the wager goes to the website.
const WAGER_EVERY: Duration = Duration::from_secs(60);

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
    if status.as_u16() == 401 || err.code == "bad_login" || err.code == "signed_out" {
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
    }
}

/// The gate for a saved session, by the plan it holds and this PC's clock.
fn gate_for(s: &Saved, offline: bool) -> Gate {
    let name = if s.username.is_empty() { s.email.clone() } else { s.username.clone() };
    if s.plan_until > now() {
        Gate::Active { username: name, plan: s.plan.clone(), offline }
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
                if s.plan_until <= now() {
                    // ran out: locked at once, then the website is asked (a renewal may have landed)
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
        let body = {
            let mut i = self.inner.lock();
            if i.busy {
                return;
            }
            let Some(s) = &i.saved else { return };
            let body = serde_json::json!({ "access_token": s.access_token, "refresh_token": s.refresh_token });
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
                Err(Fail::Unreachable(e)) => {
                    tracing::warn!("FastBet plan check: {e}");
                    let recent = i.saved.as_ref().is_some_and(|s| now().saturating_sub(s.checked_at) < OFFLINE_GRACE_SECS);
                    i.gate = match &i.saved {
                        Some(s) if recent && s.plan_until > now() => gate_for(s, true),
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
            let res = post("/api/app/login", serde_json::json!({ "login": login.trim(), "password": password }));
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
                    c.post(page("/api/app/logout")).json(&serde_json::json!({ "access_token": s.access_token })).send().map_err(|e| e.to_string())
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
