//! The phone's login: no login window yet. The same API as the PC's (`login.rs`), so the casinos
//! that log in through the site ask for a pasted login instead.

use std::sync::Arc;

use crate::casino::CasinoId;

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
pub struct LoginReport {
    #[serde(default)]
    pub token: String,
    #[serde(default, rename = "loggedIn")]
    pub logged_in: bool,
}

pub struct LoginSession {
    pub tried: Option<String>,
    pub last: LoginReport,
}

impl LoginSession {
    pub fn spawn(_id: CasinoId, _url: &str, _profile: &str, _repaint: Arc<dyn Fn() + Send + Sync>) -> Result<Self, String> {
        Err("The phone version can't open the casino's login page yet: paste your login instead".into())
    }

    pub fn poll(&mut self) -> Vec<LoginReport> {
        vec![]
    }

    pub fn finished(&mut self) -> bool {
        true
    }

    pub fn crash(&mut self) -> Option<String> {
        None
    }

    pub fn close(&mut self) {}
}

/// Nothing to clear: the phone keeps no browser profiles yet.
pub fn clear_profile(_profile: &str) -> Result<(), String> {
    Ok(())
}
