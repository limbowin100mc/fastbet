//! Slot games: the catalog types, the provider driver trait and the drivers.
//! A driver talks to the slot provider's own game server through the
//! browser bridge, skipping the game client (and its animations) entirely.

pub mod avatarux;
pub mod bgaming;
pub mod hacksaw;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::money::Units;

/// One catalog entry from the casino.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SlotGame {
    pub id: String,
    pub name: String,
    pub producer: String,
    pub image: String,
    pub demo: bool,
    pub rtp: f64,
    /// Provider-side game id (e.g. Hacksaw's numeric id).
    pub provider_game_id: Option<String>,
}

/// Which driver handles a producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Hacksaw,
    Avatarux,
    Bgaming,
}

impl Provider {
    /// Producers with a driver, in lobby order.
    pub const SUPPORTED: &'static [&'static str] = &["hacksawgaming", "avatarux", "bgaming"];

    pub fn for_producer(producer: &str) -> Option<Provider> {
        match producer {
            "hacksawgaming" => Some(Provider::Hacksaw),
            "avatarux" => Some(Provider::Avatarux),
            "bgaming" => Some(Provider::Bgaming),
            _ => None,
        }
    }
    pub fn display_producer(producer: &str) -> &str {
        match producer {
            "hacksawgaming" => "Hacksaw Gaming",
            "avatarux" => "AvatarUX",
            "bgaming" => "BGaming",
            "nolimitcity" => "Nolimit City",
            "pragmaticplay" => "Pragmatic Play",
            other => other,
        }
    }
}

/// How a session was launched (real money or demo).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    Real,
    Demo,
}

/// A bet level the game offers, in the session currency's minor units
/// (cents for fiat) with `decimals` fraction digits.
#[derive(Clone, Debug, PartialEq)]
pub struct BetLevel {
    pub raw: String,
    pub minor: i64,
}

/// A buyable feature / feature spin.
#[derive(Clone, Debug, PartialEq)]
pub struct Feature {
    pub id: String,
    pub name: String,
    /// Cost as a multiple of the bet level.
    pub cost_mult: f64,
    pub rtp: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct SessionInfo {
    pub currency: String,
    pub decimals: u32,
    pub bet_levels: Vec<BetLevel>,
    pub default_bet: Option<i64>,
    pub features: Vec<Feature>,
    /// Wallet balance in minor units, when the provider reports it.
    pub balance: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct SpinResult {
    pub round_id: String,
    /// What was charged, in minor units (bet level × feature cost).
    pub cost: i64,
    /// Total round win in minor units.
    pub win: i64,
    pub multiplier: f64,
    pub balance: Option<i64>,
    /// Round still open server-side (bonus in progress etc.).
    pub pending: bool,
    pub free_spins: bool,
    pub events: usize,
    pub ms: f64,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SlotError {
    #[error("{0}")]
    Provider(String),
    #[error("Insufficient balance")]
    Balance,
    #[error("Rate limited by the provider")]
    RateLimited,
    #[error("Session expired")]
    SessionExpired,
    #[error("{0}")]
    Bridge(String),
}

#[async_trait]
pub trait SlotSession: Send + Sync {
    fn info(&self) -> &SessionInfo;
    /// One spin at `bet` minor units, optionally buying `feature`.
    async fn spin(&self, bet: i64, feature: Option<&str>) -> Result<SpinResult, SlotError>;
    fn close(&self);
}

/// Formats minor units of a session currency with its symbol: 200 cents USD -> "$2.00".
pub fn fmt_money(v: i64, currency: &str, decimals: u32) -> String {
    let scale = 10i64.pow(decimals);
    let abs = v.unsigned_abs() as i64;
    let int = crate::money::group_thousands(&(abs / scale).to_string());
    let body = if decimals > 0 { format!("{int}.{:0width$}", abs % scale, width = decimals as usize) } else { int };
    let sign = if v < 0 { "-" } else { "" };
    let _ = Units::MAX;
    match currency.to_ascii_uppercase().as_str() {
        "USD" => format!("{sign}${body}"),
        "EUR" => format!("{sign}€{body}"),
        "GBP" => format!("{sign}£{body}"),
        "FUN" | "XXX" => format!("{sign}FUN {body}"),
        c => format!("{sign}{body} {c}"),
    }
}
