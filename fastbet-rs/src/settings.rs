//! Persisted user settings. Saved as JSON in the OS config directory.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::casino::CasinoId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Manual,
    Auto,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Coin,
    Usd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Adjust {
    Reset,
    Increase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Sections {
    pub general: bool,
    pub strategy: bool,
    pub count: bool,
    pub stops: bool,
    pub seed: bool,
    pub keys: bool,
    pub account: bool,
}

impl Default for Sections {
    fn default() -> Self {
        Self { general: true, strategy: false, count: false, stops: false, seed: false, keys: false, account: false }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub casino: CasinoId,
    /// Login token per casino, keyed by `CasinoId::key()`.
    pub tokens: BTreeMap<String, String>,
    /// Last seen username per casino, for the account list.
    pub usernames: BTreeMap<String, String>,
    /// Chosen domain per casino that has mirrors (Stake).
    pub mirrors: BTreeMap<String, String>,

    pub mode: Mode,
    /// Empty = follow the currency selected on the site.
    pub currency: String,
    pub unit: Unit,
    pub amount: String,
    pub target: String,
    pub count: String,
    pub delay: String,
    pub on_win: Adjust,
    pub on_win_pct: String,
    pub on_loss: Adjust,
    pub on_loss_pct: String,
    pub stop_profit: String,
    pub stop_loss: String,
    pub max_bet: String,
    pub stop_on_hit: bool,
    pub hit_multi: String,
    pub hit_sound: bool,
    pub seed_enabled: bool,
    pub seed_every: String,
    pub hotkeys: bool,
    pub open: Sections,
    /// Wallet dropdown: hide coins with a zero balance.
    pub hide_empty: bool,
    /// Async mode: how many bets may be in flight at once (1 = sequential).
    pub async_bets: u32,

    // slots
    pub slot_demo: bool,
    pub slot_stop_mult_on: bool,
    pub slot_stop_mult: String,
    pub slot_count: String,
    pub slot_stop_profit: String,
    pub slot_stop_loss: String,
    /// Per game: last bet level and feature, keyed by catalog id.
    pub slot_prefs: BTreeMap<String, SlotPrefs>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlotPrefs {
    pub bet: Option<i64>,
    pub feature: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            casino: CasinoId::ChipsGg,
            tokens: Default::default(),
            usernames: Default::default(),
            mirrors: Default::default(),
            mode: Mode::Manual,
            currency: String::new(),
            unit: Unit::Coin,
            amount: String::new(),
            target: "2".into(),
            count: "0".into(),
            delay: "0".into(),
            on_win: Adjust::Reset,
            on_win_pct: "0".into(),
            on_loss: Adjust::Reset,
            on_loss_pct: "100".into(),
            stop_profit: String::new(),
            stop_loss: String::new(),
            max_bet: String::new(),
            stop_on_hit: false,
            hit_multi: String::new(),
            hit_sound: true,
            seed_enabled: false,
            seed_every: "100".into(),
            hotkeys: true,
            open: Sections::default(),
            hide_empty: false,
            async_bets: 1,
            slot_demo: true,
            slot_stop_mult_on: false,
            slot_stop_mult: "100".into(),
            slot_count: "0".into(),
            slot_stop_profit: String::new(),
            slot_stop_loss: String::new(),
            slot_prefs: Default::default(),
        }
    }
}

impl Settings {
    pub fn path() -> Option<PathBuf> {
        // FASTBET_SETTINGS=<file> keeps a UI test run away from the real settings
        if let Ok(p) = std::env::var("FASTBET_SETTINGS") {
            return Some(PathBuf::from(p));
        }
        let dirs = directories::ProjectDirs::from("gg", "fastbet", "FastBet")?;
        Some(dirs.config_dir().join("settings.json"))
    }

    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                tracing::warn!("settings unreadable ({e}), using defaults");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    tracing::warn!("could not save settings: {e}");
                }
            }
            Err(e) => tracing::warn!("could not serialise settings: {e}"),
        }
    }

    pub fn token(&self) -> Option<String> {
        self.tokens.get(self.casino.key()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn token_mut(&mut self) -> &mut String {
        self.tokens.entry(self.casino.key().to_string()).or_default()
    }

    /// Domain to reach the current casino on, for casinos with mirrors.
    pub fn mirror(&self) -> Option<String> {
        let default = self.casino.default_mirror()?;
        let saved = self.mirrors.get(self.casino.key()).map(|m| crate::casino::stake::normalize_mirror(m)).filter(|m| !m.is_empty());
        Some(saved.unwrap_or_else(|| default.to_string()))
    }

    /// Bets between automatic seed changes, 0 = off.
    pub fn seed_every(&self) -> u32 {
        if self.seed_enabled {
            self.seed_every.trim().parse().unwrap_or(0)
        } else {
            0
        }
    }
}
