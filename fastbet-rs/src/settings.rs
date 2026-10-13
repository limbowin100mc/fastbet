//! Persisted user settings. Saved as JSON in the OS config directory.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::casino::CasinoId;
use crate::cloudflare::Clearance;

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
    pub restart: bool,
}

impl Default for Sections {
    fn default() -> Self {
        Self { general: true, strategy: false, count: false, stops: false, seed: false, keys: false, account: false, restart: false }
    }
}

/// One login on one casino; a casino can have several.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Account {
    /// Stable id. It also names the login window's browser profile, so every account keeps
    /// its own site session (accounts from before multi-account use the casino's key).
    pub id: String,
    pub casino: CasinoId,
    pub token: String,
    /// Last seen username, for the account list.
    pub username: Option<String>,
    /// Chosen domain, for casinos with mirrors.
    pub mirror: Option<String>,
    /// Cookies and User Agent the user copied from their browser (casinos behind Cloudflare).
    pub clearance: Option<Clearance>,
}

impl Default for Account {
    fn default() -> Self {
        Self { id: String::new(), casino: CasinoId::ChipsGg, token: String::new(), username: None, mirror: None, clearance: None }
    }
}

impl Account {
    /// A new, empty account on `casino`.
    pub fn new(casino: CasinoId) -> Self {
        let tag: u32 = rand::random();
        Self { id: format!("{}-{:06x}", casino.key(), tag & 0xff_ffff), casino, ..Self::default() }
    }

    pub fn token(&self) -> Option<String> {
        Some(self.token.trim().to_string()).filter(|s| !s.is_empty())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Every saved login, in the order they were added.
    pub accounts: Vec<Account>,
    /// Id of the account in use; empty for none.
    pub account: String,
    /// The account in use's casino.
    pub casino: CasinoId,
    // Before multi-account: one login per casino, keyed by `CasinoId::key()`. Read once to make the
    // first accounts, and still written (from the first account of each casino) for older FastBets.
    pub tokens: BTreeMap<String, String>,
    pub usernames: BTreeMap<String, String>,
    pub mirrors: BTreeMap<String, String>,
    pub clearances: BTreeMap<String, Clearance>,

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
    /// Settings → Seed: every seed change uses `seed_name` as the client seed, not a random one.
    pub own_seed: bool,
    pub seed_name: String,
    /// Settings → Seed: how many characters the random client seeds have (when `own_seed` is off),
    /// and which characters.
    pub seed_length: String,
    pub seed_chars: SeedChars,
    pub hotkeys: bool,
    pub open: Sections,
    /// Wallet dropdown: hide coins with a zero balance.
    pub hide_empty: bool,
    /// Async mode: how many bets may be in flight at once (1 = sequential).
    pub async_bets: u32,
    /// Force restart betting: an auto run that ends by itself (not by Stop) starts again.
    pub force_restart: bool,
    /// Force restart on errors: an error doesn't end an auto run, it bets on.
    pub restart_on_error: bool,

    // slots
    pub slot_demo: bool,
    pub slot_stop_mult_on: bool,
    pub slot_stop_mult: String,
    pub slot_count: String,
    pub slot_stop_profit: String,
    pub slot_stop_loss: String,
    /// Per game: last bet level and feature, keyed by catalog id.
    pub slot_prefs: BTreeMap<String, SlotPrefs>,

    /// The originals' own settings (risk, rows, picked tiles …).
    pub orig: OriginalPrefs,

    /// Where the Session pane sits: docked on the right, or floating, and whether it's minimized.
    pub session: SessionPane,

    /// The file exists but couldn't be read: saving is off so it is never overwritten.
    #[serde(skip)]
    pub unreadable: bool,
    /// Accounts removed by the user this session: the only logins a save may drop.
    #[serde(skip)]
    pub removed: Vec<String>,
}

/// The characters random client seeds are made of: hex, or any mix of the other three.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SeedChars {
    /// 0–9 and a–f; the others don't count while it's on.
    pub hex: bool,
    pub upper: bool,
    pub lower: bool,
    pub digits: bool,
}

impl Default for SeedChars {
    fn default() -> Self {
        Self { hex: true, upper: false, lower: false, digits: false }
    }
}

impl SeedChars {
    /// Every character a seed can have.
    pub fn alphabet(self) -> String {
        if self.hex || !(self.upper || self.lower || self.digits) {
            return "0123456789abcdef".into();
        }
        let mut abc = String::new();
        for (on, set) in [(self.upper, "ABCDEFGHIJKLMNOPQRSTUVWXYZ"), (self.lower, "abcdefghijklmnopqrstuvwxyz"), (self.digits, "0123456789")] {
            if on {
                abc.push_str(set);
            }
        }
        abc
    }
}

/// Where the Session pane sits.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionPane {
    /// On the right side (the default); otherwise floating at `pos`.
    pub docked: bool,
    /// The floating pane's top-left corner, in points from the window's top-left.
    pub pos: [f32; 2],
    /// Shrunk to a small card with just the profit.
    pub minimized: bool,
}

impl Default for SessionPane {
    fn default() -> Self {
        Self { docked: true, pos: [0.0, 0.0], minimized: false }
    }
}

/// What each original's board was last set to.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OriginalPrefs {
    pub dice_target: String,
    pub dice_over: bool,
    pub keno_risk: String,
    /// Picked numbers, 0-based (the casino's numbering; the board shows 1–40).
    pub keno_picks: Vec<u8>,
    /// Auto: new random numbers for every bet.
    pub keno_random: bool,
    pub mines_count: u8,
    /// Auto: tiles revealed by every bet.
    pub mines_picks: Vec<u8>,
    pub plinko_risk: String,
    pub plinko_rows: u8,
    pub wheel_risk: String,
    pub wheel_segments: u8,
    pub dragon_difficulty: String,
    /// Auto: the egg picked on each row, bottom first.
    pub dragon_path: Vec<u8>,
    pub pump_difficulty: String,
    /// Auto: pumps before cashing out.
    pub pump_pumps: u8,
    /// Auto: the calls for each flip ("heads" / "tails").
    pub flip_calls: Vec<String>,
    pub cases_difficulty: String,
    pub darts_difficulty: String,
    pub tarot_difficulty: String,
    pub bars_difficulty: String,
    /// Bars: picked tiles, 0-based.
    pub bars_tiles: Vec<u8>,
    /// Drill: the drill picked (1–3) and the depth it has to reach.
    pub drill_pick: u8,
    pub drill_target: String,
    pub primedice_condition: String,
    /// Primedice: range edges, two or four used.
    pub primedice_targets: Vec<String>,
    pub chicken_difficulty: String,
    /// Auto: lanes to cross before cashing out.
    pub chicken_lanes: u8,
    pub snakes_difficulty: String,
    /// Auto: rolls before cashing out.
    pub snakes_rolls: u8,
    pub moles_count: u8,
    /// Auto: the hole hit each round.
    pub moles_picks: Vec<u8>,
    /// Auto: the hands thrown, in order.
    pub rps_calls: Vec<String>,
    /// Baccarat: the sides the stake goes on.
    pub baccarat_sides: Vec<String>,
    /// Roulette: chips per bet spot, in units of the bet size.
    pub roulette_chips: BTreeMap<String, u32>,
    pub scarab_lines: u8,
    pub tome_lines: u8,
    /// Blitz (Shuffle): cards drawn per bet.
    pub blitz_picks: u8,
    /// HiLo: the start card (rank "A"…"K", suit letter), or a random one for every bet.
    pub hilo_rank: String,
    pub hilo_suit: String,
    pub hilo_random: bool,
    /// HiLo auto: what to do on each card, as Antebot's codes ("2,2,2,5,5").
    pub hilo_pattern: String,
    /// HiLo auto: take the winnings once a round pays this much, and bet on.
    pub hilo_cashout_on: bool,
    pub hilo_cashout: String,
    /// HiLo auto: take the winnings once a round pays this much, and end the run.
    pub hilo_stop_on: bool,
    pub hilo_stop: String,
}

impl Default for OriginalPrefs {
    fn default() -> Self {
        Self {
            dice_target: "50.50".into(),
            dice_over: true,
            keno_risk: "classic".into(),
            keno_picks: vec![],
            keno_random: false,
            mines_count: 3,
            mines_picks: vec![],
            plinko_risk: "medium".into(),
            plinko_rows: 16,
            wheel_risk: "medium".into(),
            wheel_segments: 30,
            dragon_difficulty: "easy".into(),
            dragon_path: vec![],
            pump_difficulty: "easy".into(),
            pump_pumps: 3,
            flip_calls: vec!["heads".into()],
            cases_difficulty: "easy".into(),
            darts_difficulty: "easy".into(),
            tarot_difficulty: "easy".into(),
            bars_difficulty: "easy".into(),
            bars_tiles: vec![],
            drill_pick: 2,
            drill_target: "2".into(),
            primedice_condition: "rollBetween".into(),
            primedice_targets: vec!["25".into(), "75".into(), "".into(), "".into()],
            chicken_difficulty: "easy".into(),
            chicken_lanes: 3,
            snakes_difficulty: "easy".into(),
            snakes_rolls: 3,
            moles_count: 3,
            moles_picks: vec![],
            rps_calls: vec!["rock".into()],
            baccarat_sides: vec!["banker".into()],
            roulette_chips: BTreeMap::new(),
            scarab_lines: 20,
            tome_lines: 20,
            blitz_picks: 10,
            hilo_rank: "A".into(),
            hilo_suit: "S".into(),
            hilo_random: false,
            hilo_pattern: "5".into(),
            hilo_cashout_on: false,
            hilo_cashout: "2".into(),
            hilo_stop_on: true,
            hilo_stop: "100".into(),
        }
    }
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
            accounts: vec![],
            account: String::new(),
            casino: CasinoId::ChipsGg,
            tokens: Default::default(),
            usernames: Default::default(),
            mirrors: Default::default(),
            clearances: Default::default(),
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
            own_seed: false,
            seed_name: String::new(),
            seed_length: RANDOM_SEED_DEFAULT.to_string(),
            seed_chars: SeedChars::default(),
            hotkeys: true,
            open: Sections::default(),
            hide_empty: false,
            async_bets: 1,
            force_restart: false,
            restart_on_error: false,
            slot_demo: true,
            slot_stop_mult_on: false,
            slot_stop_mult: "100".into(),
            slot_count: "0".into(),
            slot_stop_profit: String::new(),
            slot_stop_loss: String::new(),
            slot_prefs: Default::default(),
            orig: OriginalPrefs::default(),
            session: SessionPane::default(),
            unreadable: false,
            removed: vec![],
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
        let mut s = Self::load_file();
        s.migrate();
        if let Some(c) = s.active().map(|a| a.casino) {
            s.casino = c;
        }
        s
    }

    /// Accounts from the one-login-per-casino fields, the first time a multi-account FastBet runs.
    fn migrate(&mut self) {
        if !self.accounts.is_empty() {
            return;
        }
        for casino in CasinoId::ALL {
            let key = casino.key();
            let token = self.tokens.get(key).cloned().unwrap_or_default();
            if token.trim().is_empty() && !self.usernames.contains_key(key) {
                continue;
            }
            self.accounts.push(Account {
                id: key.to_string(),
                casino: *casino,
                token,
                username: self.usernames.get(key).cloned(),
                mirror: self.mirrors.get(key).cloned(),
                clearance: self.clearances.get(key).cloned(),
            });
        }
        if let Some(a) = self.accounts.iter().find(|a| a.casino == self.casino) {
            self.account = a.id.clone();
        }
    }

    /// The old per-casino fields, from the first account of each casino (the one in use first).
    fn sync_legacy(&mut self) {
        self.tokens.clear();
        self.usernames.clear();
        self.mirrors.clear();
        self.clearances.clear();
        let active = self.account.clone();
        let mut order: Vec<&Account> = self.accounts.iter().filter(|a| a.id == active).collect();
        order.extend(self.accounts.iter().filter(|a| a.id != active));
        for a in order {
            let key = a.casino.key().to_string();
            if self.tokens.contains_key(&key) || self.usernames.contains_key(&key) {
                continue;
            }
            self.tokens.insert(key.clone(), a.token.clone());
            if let Some(u) = &a.username {
                self.usernames.insert(key.clone(), u.clone());
            }
            if let Some(m) = &a.mirror {
                self.mirrors.insert(key.clone(), m.clone());
            }
            if let Some(c) = &a.clearance {
                self.clearances.insert(key, c.clone());
            }
        }
    }

    fn load_file() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        let text = match Self::read(&path) {
            Ok(Some(t)) => t,
            Ok(None) => return Self::default(),
            Err(e) => {
                tracing::warn!("settings can't be read ({e}); running on defaults without saving, so they aren't overwritten");
                return Self { unreadable: true, ..Self::default() };
            }
        };
        match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(e) => {
                // a casino from a newer FastBet: start on the first one, keep everything else
                if let Some(s) = Self::without_unknown_casino(&text) {
                    tracing::warn!("settings name a casino this build doesn't know; starting on {}", s.casino.name());
                    return s;
                }
                // the next save would overwrite every saved login with the defaults: keep a copy
                let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
                let backup = path.with_file_name(format!("settings.unreadable-{secs}.json"));
                let _ = std::fs::copy(&path, &backup);
                tracing::warn!("settings unreadable ({e}), using defaults; the old file is kept as {}", backup.display());
                Self::default()
            }
        }
    }

    /// The settings with an unknown `casino` replaced by the default one.
    fn without_unknown_casino(text: &str) -> Option<Self> {
        let mut v: serde_json::Value = serde_json::from_str(text).ok()?;
        let casino = v.get("casino")?.as_str()?;
        if serde_json::from_value::<CasinoId>(serde_json::Value::String(casino.to_string())).is_ok() {
            return None;
        }
        v["casino"] = serde_json::to_value(Self::default().casino).ok()?;
        serde_json::from_value(v).ok()
    }

    /// The file's text; `None` when there is no file. A file another program has open is read again
    /// a few times before giving up.
    fn read(path: &std::path::Path) -> std::io::Result<Option<String>> {
        let mut tries = 0;
        loop {
            match std::fs::read_to_string(path) {
                Ok(t) => return Ok(Some(t)),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) if tries >= 5 => return Err(e),
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(60));
                }
            }
        }
    }

    pub fn save(&mut self) {
        let Some(path) = Self::path() else { return };
        if self.unreadable {
            tracing::warn!("not saving settings: the file couldn't be read at start");
            return;
        }
        // logins in the file that this copy doesn't hold (another FastBet added them, or this one
        // started without them) stay: only an account removed here may go
        if let Ok(Some(text)) = Self::read(&path) {
            if let Ok(mut disk) = serde_json::from_str::<Settings>(&text) {
                disk.migrate();
                for a in disk.accounts {
                    if !self.removed.contains(&a.id) && !self.accounts.iter().any(|x| x.id == a.id) {
                        tracing::info!("keeping the {} account {} found in the settings file", a.casino.name(), a.id);
                        self.accounts.push(a);
                    }
                }
            }
        }
        self.sync_legacy();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = match serde_json::to_string_pretty(self) {
            Ok(t) => t,
            Err(e) => return tracing::warn!("could not serialise settings: {e}"),
        };
        // written aside and swapped in, so the file is never half-written
        let tmp = path.with_extension("json.tmp");
        if let Err(e) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)) {
            tracing::warn!("could not save settings: {e}");
        }
    }

    /// The account in use.
    pub fn active(&self) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == self.account)
    }

    pub fn active_mut(&mut self) -> Option<&mut Account> {
        let id = self.account.clone();
        self.accounts.iter_mut().find(|a| a.id == id)
    }

    pub fn token(&self) -> Option<String> {
        self.active().and_then(Account::token)
    }

    /// The account in use's token, to replace (an account is made for the casino if there is none).
    pub fn token_mut(&mut self) -> &mut String {
        if self.active().is_none() {
            let a = Account::new(self.casino);
            self.account = a.id.clone();
            self.accounts.push(a);
        }
        &mut self.active_mut().expect("just made").token
    }

    /// Domain to reach the account's casino on, for casinos with mirrors.
    pub fn mirror(&self) -> Option<String> {
        let default = self.casino.default_mirror()?;
        let saved = self.active().and_then(|a| a.mirror.as_deref()).map(crate::casino::stake::normalize_mirror).filter(|m| !m.is_empty());
        Some(saved.unwrap_or_else(|| default.to_string()))
    }

    /// Saved Cloudflare cookie for the account in use.
    pub fn clearance(&self) -> Option<Clearance> {
        self.active().and_then(|a| a.clearance.clone()).filter(Clearance::is_complete)
    }

    /// Everything the current casino connects with.
    pub fn setup(&self) -> crate::casino::Setup {
        crate::casino::Setup { token: self.token(), mirror: self.mirror(), clearance: self.clearance() }
    }

    /// Bets between automatic seed changes, 0 = off.
    pub fn seed_every(&self) -> u32 {
        if self.seed_enabled {
            self.seed_every.trim().parse().unwrap_or(0)
        } else {
            0
        }
    }

    /// The client seed every seed change uses; `None` = a random one.
    pub fn client_seed(&self) -> Option<String> {
        Some(clean_seed_name(&self.seed_name)).filter(|s| self.own_seed && !s.is_empty())
    }

    /// Characters in a random client seed: `seed_length` kept to [`RANDOM_SEED_LEN`].
    pub fn random_seed_len(&self) -> usize {
        let (lo, hi) = (*RANDOM_SEED_LEN.start(), *RANDOM_SEED_LEN.end());
        self.seed_length.trim().parse::<usize>().map_or(RANDOM_SEED_DEFAULT, |n| n.clamp(lo, hi))
    }
}

/// Longest client seed name (Antebot's limit too).
pub const SEED_NAME_MAX: usize = 128;

/// Lengths a random client seed can have (as in Antebot), and the one it has unless set
/// (FastBet's seeds were always 24 hex characters before the format could be set).
pub const RANDOM_SEED_LEN: std::ops::RangeInclusive<usize> = 1..=128;
pub const RANDOM_SEED_DEFAULT: usize = 24;

/// A client seed name as the casinos take it: letters, digits, `-` and `_`, at most [`SEED_NAME_MAX`].
pub fn clean_seed_name(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).take(SEED_NAME_MAX).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_casino_keeps_the_logins() {
        let text = r#"{"casino":"SomeNewCasino","tokens":{"shuffle":"t1","stake":"t2"},"target":"10"}"#;
        assert!(serde_json::from_str::<Settings>(text).is_err());
        let s = Settings::without_unknown_casino(text).expect("readable without the casino");
        assert_eq!(s.casino, Settings::default().casino);
        assert_eq!(s.tokens.get("shuffle").map(String::as_str), Some("t1"));
        assert_eq!(s.target, "10");
        // a known casino isn't this fallback's business
        assert!(Settings::without_unknown_casino(r#"{"casino":"Shuffle"}"#).is_none());
    }

    #[test]
    fn own_seed_is_off_until_turned_on_with_a_name() {
        let mut s = Settings::default();
        s.seed_name = "sx seth!".into();
        assert_eq!(s.client_seed(), None);
        s.own_seed = true;
        assert_eq!(s.client_seed().as_deref(), Some("sxseth"));
        s.seed_name = " !".into();
        assert_eq!(s.client_seed(), None);
        assert_eq!(clean_seed_name(&"a".repeat(200)).len(), SEED_NAME_MAX);
    }

    #[test]
    fn random_seed_length_stays_in_range() {
        let mut s = Settings::default();
        assert_eq!(s.random_seed_len(), 24);
        for (typed, len) in [("16", 16), ("0", 1), ("128", 128), ("500", 128), ("", 24), ("x", 24)] {
            s.seed_length = typed.into();
            assert_eq!(s.random_seed_len(), len, "{typed:?}");
        }
    }

    #[test]
    fn seed_characters() {
        let mut c = SeedChars::default();
        assert_eq!(c.alphabet(), "0123456789abcdef");
        c.upper = true;
        c.digits = true;
        // hex wins while it's on
        assert_eq!(c.alphabet(), "0123456789abcdef");
        c.hex = false;
        assert_eq!(c.alphabet(), "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789");
        c = SeedChars { hex: false, upper: false, lower: false, digits: false };
        assert_eq!(c.alphabet(), "0123456789abcdef");
    }

    #[test]
    fn one_login_per_casino_becomes_accounts() {
        let old = r#"{"casino":"Shuffle","tokens":{"chipsgg":"c1","shuffle":"s1","stake":""},"usernames":{"shuffle":"sx","stake":"st"},"mirrors":{"stake":"stake.bet"}}"#;
        let mut s: Settings = serde_json::from_str(old).unwrap();
        s.migrate();
        let ids: Vec<&str> = s.accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["chipsgg", "stake", "shuffle"]);
        assert_eq!(s.account, "shuffle");
        assert_eq!(s.token().as_deref(), Some("s1"));
        assert_eq!(s.accounts[1].mirror.as_deref(), Some("stake.bet"));

        // a second Shuffle login, in use: older FastBets see it as the Shuffle login
        let mut second = Account::new(CasinoId::Shuffle);
        second.token = "s2".into();
        s.account = second.id.clone();
        s.accounts.push(second);
        s.sync_legacy();
        assert_eq!(s.tokens.get("shuffle").map(String::as_str), Some("s2"));
        assert_eq!(s.tokens.get("chipsgg").map(String::as_str), Some("c1"));
        // migrating again changes nothing
        let n = s.accounts.len();
        s.migrate();
        assert_eq!(s.accounts.len(), n);
    }

    #[test]
    fn a_save_never_drops_logins_it_did_not_remove() {
        let dir = std::env::temp_dir().join(format!("fastbet-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("settings.json");
        std::fs::write(&file, r#"{"casino":"Stake","tokens":{"stake":"t1","shuffle":"t2"}}"#).unwrap();
        // SAFETY: the only test touching this variable
        unsafe { std::env::set_var("FASTBET_SETTINGS", &file) };
        // a FastBet that started without the file, then logged in to Goated
        let mut fresh = Settings::default();
        fresh.casino = CasinoId::Goated;
        *fresh.token_mut() = "g1".into();
        fresh.save();
        let saved = Settings::load();
        let mut tokens: Vec<(&str, &str)> = saved.accounts.iter().map(|a| (a.casino.key(), a.token.as_str())).collect();
        tokens.sort();
        assert_eq!(tokens, [("goated", "g1"), ("shuffle", "t2"), ("stake", "t1")]);
        // a removed account does go
        let mut s = saved;
        let id = s.accounts.iter().find(|a| a.casino == CasinoId::Shuffle).unwrap().id.clone();
        s.accounts.retain(|a| a.id != id);
        s.removed.push(id);
        s.save();
        assert!(Settings::load().accounts.iter().all(|a| a.casino != CasinoId::Shuffle));
        unsafe { std::env::remove_var("FASTBET_SETTINGS") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
