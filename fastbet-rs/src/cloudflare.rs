//! The user's own browser, copied by hand: its User Agent and its cookies for a
//! casino mirror, for when the casino's Cloudflare check doesn't let the app's
//! bridge through.
//!
//! Cloudflare ties its cookies (`cf_clearance` after a solved check, `__cf_bm`
//! from its bot detection) to the browser's User Agent, the IP address and the
//! domain they were issued on. With the same User Agent, from the same IP, on
//! the same mirror, a plain HTTPS client carrying them is let through the way
//! that browser is. When the browser was logged in, its cookie line also holds
//! the casino's own login (Stake: `session`), which then works as the token.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub const COOKIE: &str = "cf_clearance";
/// Cloudflare's bot-detection cookie; with the matching User Agent it lets requests through too.
pub const BOT_COOKIE: &str = "__cf_bm";
/// Stake keeps the website login in this cookie; the site sends it as `x-access-token`.
pub const SESSION_COOKIE: &str = "session";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clearance {
    /// Domain the cookie was issued on (a casino mirror).
    pub mirror: String,
    pub user_agent: String,
    /// The browser's cookies for the mirror as a cookie line (`a=1; b=2`). Older
    /// versions saved the bare `cf_clearance` value here.
    pub cookie: String,
}

impl Clearance {
    /// Both values present.
    pub fn is_complete(&self) -> bool {
        !self.mirror.is_empty() && !self.user_agent.is_empty() && !self.cookie.is_empty()
    }

    /// Checks and cleans what the user pasted. `Ok(None)` when both fields are empty.
    pub fn from_input(mirror: &str, user_agent: &str, cookie: &str) -> Result<Option<Self>, String> {
        let user_agent = clean_user_agent(user_agent);
        let pairs = parse_cookies(cookie);
        match (user_agent.is_empty(), cookie.trim().is_empty()) {
            (true, true) => return Ok(None),
            (true, false) => return Err("Paste the User Agent too, the cookies only work with it.".into()),
            (false, true) => return Err("Paste the cookies too.".into()),
            _ => {}
        }
        if !user_agent.starts_with("Mozilla/") {
            return Err("That isn't a browser User Agent (it starts with Mozilla/5.0).".into());
        }
        if pairs.is_empty() {
            return Err(format!("That isn't a cookie line or a {COOKIE} value."));
        }
        if !pairs.iter().any(|(k, _)| k == COOKIE || k == BOT_COOKIE || k == SESSION_COOKIE) {
            return Err(format!("Those cookies have no {COOKIE}, {BOT_COOKIE} or {SESSION_COOKIE} in them. Copy the cookie line of a request to the casino."));
        }
        let cookie = pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
        Ok(Some(Self { mirror: mirror.to_string(), user_agent, cookie }))
    }

    /// The cookies to send, as name and value.
    pub fn cookies(&self) -> Vec<(String, String)> {
        parse_cookies(&self.cookie)
    }

    fn get(&self, name: &str) -> Option<String> {
        self.cookies().into_iter().find(|(k, _)| k == name).map(|(_, v)| v).filter(|v| !v.is_empty())
    }

    /// The casino login the browser was in, when its cookie line came with it.
    pub fn session(&self) -> Option<String> {
        self.get(SESSION_COOKIE)
    }

    /// A Cloudflare cookie is among them (without one, Cloudflare won't let a plain client through).
    pub fn has_cloudflare(&self) -> bool {
        self.get(COOKIE).is_some() || self.get(BOT_COOKIE).is_some()
    }
}

/// What logs the account in: the token typed, or else the session of a logged-in browser's
/// cookies. A typed token that is just the session pasted last time gives way to the new one.
pub fn pick_login(typed: &str, session: Option<String>, old_session: Option<String>) -> Option<String> {
    match session {
        Some(s) if typed.is_empty() || old_session.as_deref() == Some(typed) => Some(s),
        _ => (!typed.is_empty()).then(|| typed.to_string()),
    }
}

/// What the user pasted as cookies -> name/value pairs. Takes a cookie line
/// (`a=1; b=2`, with or without `Cookie:` in front), rows of DevTools' cookie
/// table (name, tab, value …), or a bare value, which is taken as `cf_clearance`.
pub fn parse_cookies(s: &str) -> Vec<(String, String)> {
    let s = s.trim();
    let s = strip_prefix_ci(s, "cookie:").unwrap_or(s);
    let valid_name = |n: &str| !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$'));
    let valid_value = |v: &str| v.chars().all(|c| c.is_ascii_graphic() && !matches!(c, ';' | ',' | '"' | '\\'));
    let mut out: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: &str| {
        let (k, v) = (k.trim(), v.trim().trim_matches('"'));
        if valid_name(k) && valid_value(v) && !v.is_empty() {
            out.retain(|(name, _)| name != k);
            out.push((k.to_string(), v.to_string()));
        }
    };
    if s.contains('=') {
        for part in s.split([';', '\n']) {
            if let Some((k, v)) = part.split_once('=') {
                push(k, v);
            }
        }
    } else if s.contains('\t') {
        // rows copied from DevTools' cookie table: name, value, domain …
        for row in s.lines() {
            let cells: Vec<&str> = row.split('\t').collect();
            if cells.len() >= 2 {
                push(cells[0], cells[1]);
            }
        }
    } else {
        let v: String = s.trim_matches('"').chars().filter(|c| !c.is_whitespace()).collect();
        push(COOKIE, &v);
    }
    out
}

/// Accepts the value as a header (`User-Agent: ...`) or Cloudflare's trace line (`uag=...`) too.
fn clean_user_agent(s: &str) -> String {
    let s = s.trim();
    let s = strip_prefix_ci(s, "user-agent:").or_else(|| strip_prefix_ci(s, "uag=")).unwrap_or(s);
    let s: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    s.trim().trim_matches('"').trim().to_string()
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| s[prefix.len()..].trim_start())
}

/// Cloudflare's info page on a domain: the visitor's IP (`ip=`) and User Agent (`uag=`).
pub fn trace_url(domain: &str) -> String {
    format!("https://{domain}/cdn-cgi/trace")
}

/// The IP address Cloudflare sees this PC on, read from a domain's trace page on a worker thread.
pub struct IpCheck {
    pub domain: String,
    result: Arc<Mutex<Option<Result<String, String>>>>,
}

impl IpCheck {
    pub fn start(domain: &str, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let result = Arc::new(Mutex::new(None));
        let (url, out) = (trace_url(domain), result.clone());
        std::thread::spawn(move || {
            let ip = reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()
                .and_then(|c| c.get(&url).send())
                .and_then(|r| r.text())
                .map_err(|e| e.to_string())
                .and_then(|body| trace_field(&body, "ip").ok_or_else(|| "no ip in the reply".to_string()));
            if let Err(e) = &ip {
                tracing::warn!("ip check via {url}: {e}");
            }
            *out.lock() = Some(ip);
            repaint();
        });
        Self { domain: domain.to_string(), result }
    }

    /// `None` while still asking.
    pub fn result(&self) -> Option<Result<String, String>> {
        self.result.lock().clone()
    }
}

fn trace_field(body: &str, key: &str) -> Option<String> {
    body.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix('=')).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_pasted_values() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
        for pasted in [ua.to_string(), format!("  {ua}\n"), format!("User-Agent: {ua}"), format!("uag={ua}"), format!("\"{ua}\"")] {
            assert_eq!(clean_user_agent(&pasted), ua, "{pasted:?}");
        }
        let v = "Ab3_x.Yz-1760000000-1.2.1.1-QWE.rty_123";
        let clearance = |c: &[(String, String)]| c.iter().find(|(k, _)| k == COOKIE).map(|(_, v)| v.clone());
        for pasted in [v.to_string(), format!("cf_clearance={v}"), format!("Cookie: __cf_bm=zzz; cf_clearance={v}; _ga=1"), format!(" {v}\n")] {
            assert_eq!(clearance(&parse_cookies(&pasted)).as_deref(), Some(v), "{pasted:?}");
        }
        let line = parse_cookies("cookie: session=abc123; __cf_bm=b.m-1; currency_currency=usdt; bad cookie; x=");
        assert_eq!(line, vec![("session".into(), "abc123".into()), ("__cf_bm".into(), "b.m-1".into()), ("currency_currency".into(), "usdt".into())]);
        let table = parse_cookies("__cf_bm\tb.m-1\t.stake.bet\t/\nsession\tabc\tstake.bet\t/");
        assert_eq!(table, vec![("__cf_bm".into(), "b.m-1".into()), ("session".into(), "abc".into())]);
    }

    #[test]
    fn input_rules() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64)";
        assert_eq!(Clearance::from_input("stake.com", " ", ""), Ok(None));
        assert!(Clearance::from_input("stake.com", ua, "").is_err());
        assert!(Clearance::from_input("stake.com", "", "abc").is_err());
        assert!(Clearance::from_input("stake.com", "curl/8", "abc").is_err());
        assert!(Clearance::from_input("stake.com", ua, "a,b").is_err());
        let c = Clearance::from_input("stake.com", ua, "cf_clearance=abc").unwrap().unwrap();
        assert_eq!((c.mirror.as_str(), c.user_agent.as_str(), c.cookie.as_str()), ("stake.com", ua, "cf_clearance=abc"));
        assert!(c.is_complete() && c.has_cloudflare() && c.session().is_none());
        // an older save holds the bare value
        let old = Clearance { mirror: "stake.com".into(), user_agent: ua.into(), cookie: "abc".into() };
        assert_eq!(old.cookies(), vec![(COOKIE.to_string(), "abc".to_string())]);
        // a logged-in browser's cookie line carries the login
        let logged = Clearance::from_input("stake.bet", ua, "session=s3ss; __cf_bm=bm; _ga=x").unwrap().unwrap();
        assert_eq!(logged.session().as_deref(), Some("s3ss"));
        assert!(logged.has_cloudflare());
        assert!(Clearance::from_input("stake.bet", ua, "_ga=1; _gid=2").is_err(), "nothing that helps in it");
    }

    #[test]
    fn login_choice() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(pick_login("api", None, None), s("api"));
        assert_eq!(pick_login("", s("sess"), None), s("sess"), "the browser login stands in for a token");
        assert_eq!(pick_login("api", s("sess"), None), s("api"), "a token typed wins");
        assert_eq!(pick_login("old", s("new"), s("old")), s("new"), "a newer browser login replaces the last one");
        assert_eq!(pick_login("", None, None), None);
    }

    #[test]
    fn reads_trace() {
        let body = "fl=12f\nh=stake.com\nip=185.125.177.122\nts=1760.1\nuag=Mozilla/5.0 (X)\ncolo=BEG\n";
        assert_eq!(trace_field(body, "ip").as_deref(), Some("185.125.177.122"));
        assert_eq!(trace_field(body, "uag").as_deref(), Some("Mozilla/5.0 (X)"));
        assert_eq!(trace_field(body, "loc"), None);
    }
}
