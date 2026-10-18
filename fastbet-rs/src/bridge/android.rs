//! The phone's browser bridge: not there yet, so every provider behind a browser check says so.

use std::time::Duration;

use super::*;

const NOT_YET: &str = "This needs the in-app browser, which the phone version doesn't have yet";

#[derive(Clone)]
pub struct Bridge;

impl Bridge {
    pub async fn open(page_url: &str) -> Result<Self, String> {
        Self::open_profile(page_url, None).await
    }

    pub async fn open_profile(_page_url: &str, _profile: Option<&str>) -> Result<Self, String> {
        Err(NOT_YET.into())
    }

    pub async fn open_with(_page_url: &str, _profile: Option<&str>, _setup: &BridgeSetup) -> Result<Self, String> {
        Err(NOT_YET.into())
    }

    pub async fn request(&self, req: BridgeRequest) -> Result<BridgeResponse, String> {
        self.request_within(req, Duration::from_secs(30)).await
    }

    pub async fn request_within(&self, _req: BridgeRequest, _timeout: Duration) -> Result<BridgeResponse, String> {
        Err(NOT_YET.into())
    }

    pub async fn post_json(&self, _url: &str, _headers: &[(&str, &str)], _body: &serde_json::Value) -> Result<(u16, serde_json::Value), String> {
        Err(NOT_YET.into())
    }

    pub async fn cookies(&self, _url: &str) -> Result<Vec<BridgeCookie>, String> {
        Err(NOT_YET.into())
    }

    pub fn show(&self, _on: bool) {}

    pub fn close(&self) {}
}
