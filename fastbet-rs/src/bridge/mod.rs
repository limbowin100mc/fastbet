//! Browser bridge: a hidden WebView2 window parked on a provider's origin
//! that performs HTTP requests on our behalf. Slot providers sit behind
//! Cloudflare bot checks that reject plain HTTP clients but accept a real
//! browser engine, so every provider call goes through here.
//!
//! Child: `fastbet --bridge <page-url> [profile]`. Reads `REQ <id> <json>`
//! lines on stdin, runs `fetch` in the page, prints `RES <id> <json>` on
//! stdout. `SHOW` / `HIDE` toggle the window, for a check that wants a click.
//! A [`BridgeSetup`] (User Agent, cookies) reaches the child in `FASTBET_BRIDGE_SETUP`.
//! That is the PC's bridge (`desktop.rs`); the phone has none yet (`android.rs`).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BridgeResponse {
    pub status: u16,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub ms: f64,
}

/// How the bridge's browser presents itself, applied before the first page loads.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BridgeSetup {
    pub user_agent: Option<String>,
    pub cookies: Vec<BridgeCookie>,
}

/// A secure, HttpOnly cookie for `domain` (".example.com" covers its subdomains too).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BridgeCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
}

impl BridgeSetup {
    #[cfg_attr(target_os = "android", allow(dead_code))]
    fn is_empty(&self) -> bool {
        self.user_agent.is_none() && self.cookies.is_empty()
    }
}

#[cfg_attr(target_os = "android", allow(dead_code))]
const BRIDGE_SCRIPT: &str = r#"(function () {
  if (window.top !== window || window.__fbreq) return;
  function out(line) { try { window.ipc.postMessage(line); } catch (e) {} }
  window.__fbreq = function (id, req) {
    var t0 = Date.now();
    var init = { method: req.method || 'GET', headers: {} };
    (req.headers || []).forEach(function (h) { init.headers[h[0]] = h[1]; });
    if (req.body != null) init.body = req.body;
    fetch(req.url, init).then(function (r) {
      return r.text().then(function (t) {
        out('RES ' + id + ' ' + JSON.stringify({ status: r.status, body: t, ms: Date.now() - t0 }));
      });
    }).catch(function (e) {
      out('RES ' + id + ' ' + JSON.stringify({ status: 0, body: '', error: String(e), ms: Date.now() - t0 }));
    });
  };
  document.addEventListener('DOMContentLoaded', function () { out('READY'); });
  if (document.readyState !== 'loading') out('READY');
})();"#;

#[cfg(not(target_os = "android"))]
mod desktop;
#[cfg(not(target_os = "android"))]
pub use desktop::{run_child, Bridge};

#[cfg(target_os = "android")]
mod android;
#[cfg(target_os = "android")]
pub use android::Bridge;
