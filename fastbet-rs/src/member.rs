//! The FastBet account: the app opens for a FastBet account (made on the website) with a running
//! Weekly or Monthly plan. The website's `/api/app/*` logs in and reports the plan; the session is
//! kept in `member.json` next to the settings, checked when the app starts and every few minutes,
//! and the app locks again the moment the plan runs out.

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
        let m = Self { inner: Arc::new(Mutex::new(Inner { gate, saved, busy: false, next_check: Instant::now() })), repaint };
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
        let saved = {
            let mut i = self.inner.lock();
            i.gate = Gate::LoggedOut { error: None };
            i.saved.take()
        };
        store(None);
        if let Some(s) = saved {
            std::thread::spawn(move || {
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
