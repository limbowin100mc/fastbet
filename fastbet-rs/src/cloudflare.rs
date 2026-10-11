//! Cloudflare clearance copied by hand from the user's own browser, for when
//! a casino's Cloudflare check never passes inside the app's bridge.
//!
//! Cloudflare ties the `cf_clearance` cookie to the browser's User Agent, the
//! IP address that passed the check and the domain it was issued on, so the
//! bridge presents the same User Agent and only sends the cookie to that domain.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

pub const COOKIE: &str = "cf_clearance";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Clearance {
    /// Domain the cookie was issued on (a casino mirror).
    pub mirror: String,
    pub user_agent: String,
    /// Value of the `cf_clearance` cookie.
    pub cookie: String,
}

impl Clearance {
    /// Both values present.
    pub fn is_complete(&self) -> bool {
        !self.mirror.is_empty() && !self.user_agent.is_empty() && !self.cookie.is_empty()
    }

    /// Checks and cleans what the user pasted. `Ok(None)` when both fields are empty.
    pub fn from_input(mirror: &str, user_agent: &str, cookie: &str) -> Result<Option<Self>, String> {
        let (user_agent, cookie) = (clean_user_agent(user_agent), clean_cookie(cookie));
        match (user_agent.is_empty(), cookie.is_empty()) {
            (true, true) => return Ok(None),
            (true, false) => return Err("Paste the User Agent too, the cookie only works with it.".into()),
            (false, true) => return Err(format!("Paste the {COOKIE} cookie too.")),
            _ => {}
        }
        if !user_agent.starts_with("Mozilla/") {
            return Err("That isn't a browser User Agent (it starts with Mozilla/5.0).".into());
        }
        if !cookie.chars().all(|c| c.is_ascii_graphic() && !matches!(c, ';' | ',' | '"' | '\\')) {
            return Err(format!("That isn't a {COOKIE} value."));
        }
        Ok(Some(Self { mirror: mirror.to_string(), user_agent, cookie }))
    }
}

/// A bare value, `cf_clearance=<value>`, or a whole Cookie header -> the value.
fn clean_cookie(s: &str) -> String {
    let s = s.trim();
    let s = strip_prefix_ci(s, "cookie:").unwrap_or(s);
    let prefix = format!("{COOKIE}=");
    let value = s.split(';').map(str::trim).find_map(|kv| kv.strip_prefix(prefix.as_str())).unwrap_or(s);
    value.trim().trim_matches('"').chars().filter(|c| !c.is_whitespace()).collect()
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
        for pasted in [v.to_string(), format!("cf_clearance={v}"), format!("Cookie: __cf_bm=zzz; cf_clearance={v}; _ga=1"), format!(" {v}\n")] {
            assert_eq!(clean_cookie(&pasted), v, "{pasted:?}");
        }
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
        assert_eq!((c.mirror.as_str(), c.user_agent.as_str(), c.cookie.as_str()), ("stake.com", ua, "abc"));
        assert!(c.is_complete());
    }

    #[test]
    fn reads_trace() {
        let body = "fl=12f\nh=stake.com\nip=185.125.177.122\nts=1760.1\nuag=Mozilla/5.0 (X)\ncolo=BEG\n";
        assert_eq!(trace_field(body, "ip").as_deref(), Some("185.125.177.122"));
        assert_eq!(trace_field(body, "uag").as_deref(), Some("Mozilla/5.0 (X)"));
        assert_eq!(trace_field(body, "loc"), None);
    }
}
