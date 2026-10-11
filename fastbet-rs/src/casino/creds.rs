//! Logins made of an access token plus a refresh token (Shuffle, Goated): the
//! site's own session, renewed by the app the way the site renews it.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A site's login: a short-lived access token and the refresh token that renews it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Creds {
    #[serde(rename = "accessToken")]
    pub access: String,
    #[serde(rename = "refreshToken", default)]
    pub refresh: String,
}

impl Creds {
    /// A saved login: the JSON pair the login window reports, or a bare access token
    /// (which works until it runs out).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        if s.starts_with('{') {
            let c: Creds = serde_json::from_str(s).ok()?;
            return (!c.access.trim().is_empty()).then_some(c);
        }
        let s = s.strip_prefix("Bearer ").unwrap_or(s).trim();
        (!s.is_empty()).then(|| Creds { access: s.to_string(), refresh: String::new() })
    }

    /// The form the settings keep.
    pub fn to_token(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Seconds until the access token runs out; `None` when it carries no expiry.
    pub fn expires_in(&self) -> Option<i64> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        jwt_exp(&self.access).map(|exp| exp as i64 - now)
    }
}

/// The `exp` claim of a JWT.
pub fn jwt_exp(token: &str) -> Option<u64> {
    let payload = base64url(token.split('.').nth(1)?)?;
    serde_json::from_slice::<Value>(&payload).ok()?.get("exp")?.as_u64()
}

fn base64url(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut buf, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => break,
            _ => return None,
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logins_parse() {
        let pair = r#"{"accessToken":"a.b.c","refreshToken":"r.s.t"}"#;
        let c = Creds::parse(pair).unwrap();
        assert_eq!((c.access.as_str(), c.refresh.as_str()), ("a.b.c", "r.s.t"));
        assert_eq!(Creds::parse(&c.to_token()), Some(c));
        assert_eq!(Creds::parse("Bearer x.y.z").unwrap().access, "x.y.z");
        assert!(Creds::parse("  ").is_none());
        assert!(Creds::parse(r#"{"accessToken":""}"#).is_none());
    }

    #[test]
    fn jwt_expiry() {
        // {"alg":"HS256"} . {"sub":"1","exp":1791740000} . sig
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIiwiZXhwIjoxNzkxNzQwMDAwfQ.sig";
        assert_eq!(jwt_exp(jwt), Some(1791740000));
        assert_eq!(jwt_exp("not-a-jwt"), None);
    }
}
