//! The egui application: model, event handling, hotkeys, screens and the shell
//! (left rail, top bar, stats panel) around the lobby and the Limbo page.

pub mod accounts;
pub mod controls;
pub mod images;
pub mod limbo;
pub mod lobby;
pub mod logos;
pub mod slot;
pub mod slots_lobby;
pub mod sound;
pub mod stats;
pub mod stats_pane;
pub mod theme;
pub mod update;
pub mod wallet;
pub mod widgets;

use std::collections::VecDeque;

use crate::casino::SlotFilter;
use crate::engine::slots::{SlotConfig, SlotState, SpinRecord};
use crate::engine::{BetRecord, SlotListKind};
use crate::slots::{PlayMode, Provider, SlotGame};

use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{pos2, Align, Frame, Key, Layout, Margin, RichText, Stroke};

use crate::casino::{BetRequest, CasinoId, ConnStatus, CurrencyInfo, Market, ProvableState};
use crate::engine::{self, AutoConfig, AutoState, Command, EngineHandle, Event, SeedInfo};
use crate::login::LoginSession;
use crate::money::{self, format_units, parse_units, to_f64, Units};
use crate::settings::{Adjust, Settings, Unit};
use crate::ui::stats::{History, Stats};
use theme::{BG, BG1, GREEN, LINE, RED, TEXT2};
use widgets::status_dot;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Accounts,
    Account,
    Lobby,
    Limbo,
    Slots,
    Slot,
}

/// Where the account screen is in the connect flow.
pub enum Stage {
    Idle,
    /// The API key card is open.
    Settings,
    /// Connection check modal: trying the saved key.
    Checking(Instant),
    /// The login window is open; waiting for a token that logs in.
    Login(LoginSession),
}

pub struct App {
    pub settings: Settings,
    engine: EngineHandle,
    repaint: Arc<dyn Fn() + Send + Sync>,

    pub screen: Screen,
    pub stage: Stage,
    pub onboard_error: Option<String>,
    /// API key being typed on the account screen.
    pub key_input: String,
    /// Mirror picked on the account screen (casinos with mirrors), and the picker's search.
    pub mirror_input: String,
    pub mirror_search: String,
    pub mirror_open: bool,

    pub status: ConnStatus,
    pub status_detail: String,
    pub market: Market,
    pub provable: ProvableState,
    pub seed: SeedInfo,
    pub queued: usize,
    pub auto: AutoState,
    pub stats: Stats,
    pub history: History,
    /// History number of the bet that ended the last auto run with a hit.
    pub hit_badge_n: Option<u64>,

    flash: Option<(String, Instant)>,
    /// Short neutral message for the status line (e.g. "Session reset").
    notice: Option<(String, Instant)>,
    sound: sound::Sound,
    pub pop_at: Option<Instant>,
    pub token_visible: bool,
    pub wallet_open: bool,
    /// The account menu above the rail's logo button.
    pub account_menu_open: bool,
    /// The gear menu on the welcome screen.
    pub welcome_menu_open: bool,
    pub updater: crate::update::Updater,
    pub update_open: bool,
    /// When the installer started; the app closes shortly after.
    pub update_close_at: Option<Instant>,
    pub wallet_search: String,
    pub wallet_search_focused: bool,

    pub logos: logos::Logos,

    // slots
    pub images: images::ImageCache,
    pub catalog: slots_lobby::Catalog,
    pub slot: SlotState,
    pub spins: VecDeque<SpinRecord>,
    pub slot_stats: Stats,
    pub slot_pop: Option<Instant>,

    dirty: bool,
    last_save: Instant,
    title: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        theme::install_visuals(&cc.egui_ctx);

        let ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let engine = engine::start(repaint.clone());

        let settings = Settings::load();
        let screen = match std::env::var("FASTBET_SCREEN").as_deref() {
            Ok("lobby") => Screen::Lobby,
            Ok("limbo") => Screen::Limbo,
            Ok("slots") => Screen::Slots,
            Ok("account") => Screen::Account,
            _ => Screen::Accounts,
        };
        let preview: Option<usize> = std::env::var("FASTBET_PREVIEW").ok().map(|v| v.parse().unwrap_or(400));
        if preview.is_none() {
            engine.send(Command::SetCasino { id: settings.casino, token: settings.token(), mirror: settings.mirror() });
        }
        engine.send(Command::SeedPolicy { every: settings.seed_every() });
        engine.send(Command::SetConcurrency(settings.async_bets.clamp(1, engine::MAX_CONCURRENCY)));

        // FASTBET_SLOT=<catalog id>:<provider id>:<name>[:<producer>] opens a demo game at startup (UI work).
        let mut screen = screen;
        if let Ok(spec) = std::env::var("FASTBET_SLOT") {
            let mut it = spec.splitn(4, ':');
            let (id, pid, name) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string(), it.next().unwrap_or("Slot").to_string());
            let producer = it.next().unwrap_or("hacksawgaming").to_string();
            let game = SlotGame { id, name, producer, image: String::new(), demo: true, rtp: 96.3, provider_game_id: Some(pid) };
            engine.send(Command::OpenSlot { game, mode: PlayMode::Demo, currency: "usdt".into() });
            screen = Screen::Slot;
        }

        let updater = crate::update::Updater::new(repaint.clone());
        let mut app = Self {
            settings,
            engine,
            repaint,
            screen,
            stage: Stage::Idle,
            onboard_error: None,
            key_input: String::new(),
            mirror_input: String::new(),
            mirror_search: String::new(),
            mirror_open: false,
            status: ConnStatus::Connecting,
            status_detail: String::new(),
            market: Market::default(),
            provable: ProvableState::default(),
            seed: SeedInfo::default(),
            queued: 0,
            auto: AutoState::default(),
            stats: Stats::fresh("", 8),
            history: History::default(),
            hit_badge_n: None,
            flash: None,
            notice: None,
            sound: sound::Sound::new(),
            pop_at: None,
            token_visible: false,
            wallet_open: std::env::var("FASTBET_WALLET").is_ok(),
            account_menu_open: std::env::var("FASTBET_ACCOUNT_MENU").is_ok(),
            welcome_menu_open: std::env::var("FASTBET_WELCOME_MENU").is_ok(),
            updater,
            update_open: false,
            update_close_at: None,
            wallet_search: String::new(),
            wallet_search_focused: false,
            logos: logos::Logos::default(),
            images: images::ImageCache::new(),
            catalog: slots_lobby::Catalog::default(),
            slot: SlotState::default(),
            spins: VecDeque::new(),
            slot_stats: Stats::fresh("USD", 2),
            slot_pop: None,
            dirty: false,
            last_save: Instant::now(),
            title: String::new(),
        };
        if let Some(n) = preview {
            app.load_preview(n);
        }
        // FASTBET_MIRRORS=<search>: account settings open with the mirror picker (UI work)
        if let Ok(q) = std::env::var("FASTBET_MIRRORS") {
            app.open_settings();
            app.mirror_open = true;
            app.mirror_search = if q == "1" { String::new() } else { q };
        }
        // FASTBET_UPDATE=1: run "Check for updates" at start (UI work)
        if std::env::var("FASTBET_UPDATE").is_ok() {
            app.check_updates();
        }
        app
    }

    /// FASTBET_PREVIEW=<bets>: no connection, a made-up USDT wallet and a
    /// synthetic Limbo history, so every screen can be looked at populated (UI work).
    fn load_preview(&mut self, n: usize) {
        use rand::Rng;
        let cur = "usdt".to_string();
        self.market.currencies.insert(cur.clone(), CurrencyInfo { decimals: 6, price: 1.0, hidden: false });
        self.market.currencies.insert("btc".into(), CurrencyInfo { decimals: 8, price: 62_000.0, hidden: false });
        self.market.wallets.insert(cur.clone(), 48_250_000);
        self.market.wallets.insert("btc".into(), 12_000);
        self.market.site_currency = Some(cur.clone());
        self.market.username = Some("preview".into());
        self.status = ConnStatus::Ready;
        self.stats = Stats::fresh(&cur, 6);
        let target = self.target().unwrap_or(2.0);
        let amount: Units = 10_000;
        let mut rng = rand::rng();
        let now = std::time::SystemTime::now();
        for i in 0..n {
            let at = now - Duration::from_millis(((n - i) * 90) as u64);
            if i > 0 && i % 70 == 0 {
                self.history.push_seed(at, format!("{:024x}", rng.random::<u128>() >> 32), "auto");
            }
            let outcome = ((0.99 / rng.random::<f64>().max(1e-9)) * 100.0).floor() / 100.0;
            let outcome = outcome.max(1.0);
            let win = outcome >= target;
            let payout = if win { (amount as f64 * target) as Units } else { 0 };
            let rec = BetRecord {
                at,
                nonce: Some(i as u64 + 1),
                currency: cur.clone(),
                target,
                outcome,
                win,
                hit: false,
                amount,
                payout,
                profit: payout - amount,
                ms: rng.random_range(120.0..220.0),
            };
            self.stats.settle(&rec, 6);
            self.history.push_bet(rec);
        }
        self.provable.client_seed = Some("d0a5a47351e87030faeb9675".into());
        self.provable.nonce = Some(n as u64);
        self.provable.server_seed_hash = Some("9f2c4be1a7d03e56c81f0b2a94d7e3c5a1b68f02d4e9c7a3b5f1e8d2c6a0b4f9".into());
    }

    // ------------------------------------------------------------ model helpers

    pub fn casino(&self) -> CasinoId {
        self.settings.casino
    }

    /// The casino as the user reaches it: its name, or the mirror's domain when that isn't the main site.
    pub fn casino_site(&self) -> String {
        let id = self.casino();
        match self.settings.mirror() {
            Some(m) if Some(m.as_str()) != id.default_mirror() => m,
            _ => id.name().to_string(),
        }
    }

    pub fn username_for(&self, id: CasinoId) -> Option<String> {
        if id == self.settings.casino {
            if let Some(u) = &self.market.username {
                return Some(u.clone());
            }
        }
        self.settings.usernames.get(id.key()).cloned()
    }

    pub fn active_currency(&self) -> String {
        let picked = &self.settings.currency;
        // a coin picked on another casino that this one doesn't have
        let known = self.market.currencies.is_empty() || self.market.currencies.contains_key(picked);
        if !picked.is_empty() && known {
            picked.clone()
        } else {
            self.market.site_currency.clone().unwrap_or_default()
        }
    }

    pub fn info(&self, cur: &str) -> Option<&CurrencyInfo> {
        self.market.info(cur)
    }

    pub fn usd_mode(&self) -> bool {
        self.settings.unit == Unit::Usd
    }

    /// Amount typed in the panel (coin or USD) -> base units.
    pub fn to_base(&self, s: &str, cur: &str) -> Option<Units> {
        let info = self.info(cur)?;
        if s.trim().is_empty() {
            return None;
        }
        if self.usd_mode() {
            let usd = money::num(s)?;
            if usd < 0.0 || info.price <= 0.0 {
                return None;
            }
            let coins = usd / info.price;
            parse_units(&format!("{:.*}", info.decimals.min(20) as usize, coins), info.decimals)
        } else {
            parse_units(s, info.decimals)
        }
    }

    pub fn fmt_coin(&self, v: Units, cur: &str) -> String {
        match self.info(cur) {
            Some(i) => format_units(v, i.decimals, 8),
            None => v.to_string(),
        }
    }

    pub fn usd_value(&self, v: Units, cur: &str) -> Option<f64> {
        let info = self.info(cur)?;
        (info.price > 0.0).then(|| to_f64(v, info.decimals) * info.price)
    }

    pub fn fmt_usd(&self, v: Units, cur: &str) -> String {
        self.usd_value(v, cur).map(money::fmt_usd).unwrap_or_default()
    }

    /// A slot session currency (fiat like USD/EUR) that the casino market does not know.
    fn session_currency(&self, cur: &str) -> Option<u32> {
        if self.info(cur).is_some() {
            return None;
        }
        match &self.slot.info {
            Some(i) if i.currency.eq_ignore_ascii_case(cur) => Some(i.decimals),
            _ => (self.slot_stats.cur.eq_ignore_ascii_case(cur)).then_some(self.slot_stats.decimals),
        }
    }

    /// Primary display string, following the unit picked in the panel.
    pub fn prim(&self, v: Units, cur: &str, sign: bool) -> String {
        let plus = if sign && v > 0 { "+" } else { "" };
        if let Some(dec) = self.session_currency(cur) {
            return format!("{plus}{}", crate::slots::fmt_money(v as i64, cur, dec));
        }
        if self.usd_mode() && self.info(cur).map(|i| i.price > 0.0).unwrap_or(false) {
            format!("{plus}{}", self.fmt_usd(v, cur))
        } else {
            format!("{plus}{} {}", self.fmt_coin(v, cur), cur.to_uppercase()).trim_end().to_string()
        }
    }

    /// Secondary display string: the other unit.
    pub fn sec(&self, v: Units, cur: &str, sign: bool) -> String {
        let plus = if sign && v > 0 { "+" } else { "" };
        if self.session_currency(cur).is_some() {
            return String::new();
        }
        if self.usd_mode() {
            format!("{plus}{} {}", self.fmt_coin(v, cur), cur.to_uppercase()).trim_end().to_string()
        } else {
            let u = self.fmt_usd(v, cur);
            if u.is_empty() {
                u
            } else {
                format!("{plus}{u}")
            }
        }
    }

    pub fn target(&self) -> Option<f64> {
        let t = money::num(&self.settings.target)?;
        let id = self.casino();
        (t >= id.min_target() && t <= id.max_target()).then_some(t)
    }

    /// The multiplier that counts as a hit, if stop-on-hit is on.
    pub fn hit_line(&self, target: f64) -> Option<f64> {
        if !self.settings.stop_on_hit {
            return None;
        }
        match money::num(&self.settings.hit_multi) {
            Some(h) if h >= self.casino().min_target() => Some(h),
            _ => Some(target),
        }
    }

    pub fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), Instant::now()));
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn ready(&self) -> bool {
        self.status.is_ready()
    }

    // ------------------------------------------------------------ actions

    fn bet_setup(&self) -> Result<(String, Units, f64), String> {
        if !self.ready() {
            return Err(self.status.long(&self.status_detail));
        }
        let cur = self.active_currency();
        if cur.is_empty() {
            return Err("No currency selected yet".into());
        }
        if self.info(&cur).is_none() {
            return Err("Currency data not loaded yet".into());
        }
        let amount = self.to_base(&self.settings.amount, &cur).filter(|a| *a > 0).ok_or("Enter a bet amount")?;
        let id = self.casino();
        let target = self.target().ok_or_else(|| {
            format!("Target must be {}× to {}×", id.min_target(), money::group_thousands(&format!("{}", id.max_target() as i64)))
        })?;
        Ok((cur, amount, target))
    }

    pub fn manual_bet(&mut self) {
        if self.auto.running {
            return;
        }
        match self.bet_setup() {
            Ok((cur, amount, target)) => {
                if self.queued >= engine::MAX_QUEUE {
                    self.flash(format!("{} bets already queued", engine::MAX_QUEUE));
                    return;
                }
                let hit_line = self.hit_line(target);
                self.engine.send(Command::Bet { req: BetRequest { currency: cur, amount, target }, hit_line });
            }
            Err(e) => self.flash(e),
        }
    }

    pub fn start_auto(&mut self) {
        let s = &self.settings;
        let (cur, amount, target) = match self.bet_setup() {
            Ok(v) => v,
            Err(e) => return self.flash(e),
        };
        let opt = |v: &str, name: &str| -> Result<Option<Units>, String> {
            if v.trim().is_empty() {
                return Ok(None);
            }
            self.to_base(v, &cur).map(Some).ok_or_else(|| format!("Invalid {name}"))
        };
        let parsed = (|| -> Result<_, String> {
            let stop_profit = opt(&s.stop_profit, "stop profit")?;
            let stop_loss = opt(&s.stop_loss, "stop loss")?;
            let max_bet = opt(&s.max_bet, "max bet")?;
            let hm = s.hit_multi.trim();
            if s.stop_on_hit && !hm.is_empty() && !money::num(hm).map(|h| h >= self.casino().min_target()).unwrap_or(false) {
                return Err(format!("Hit multiplier must be at least {}×", self.casino().min_target()));
            }
            Ok((stop_profit, stop_loss, max_bet))
        })();
        let (stop_profit, stop_loss, max_bet) = match parsed {
            Ok(v) => v,
            Err(e) => return self.flash(e),
        };
        if let Some(m) = max_bet {
            if amount > m {
                return self.flash("Bet size is above max bet");
            }
        }
        let pct = |a: Adjust, v: &str| if a == Adjust::Increase { Some(money::num(v).unwrap_or(0.0)) } else { None };
        let cfg = AutoConfig {
            currency: cur,
            target,
            base: amount,
            limit: s.count.trim().parse().unwrap_or(0),
            delay_ms: money::num(&s.delay).unwrap_or(0.0).max(0.0) as u64,
            on_win: pct(s.on_win, &s.on_win_pct),
            on_loss: pct(s.on_loss, &s.on_loss_pct),
            stop_profit,
            stop_loss,
            max_bet,
            hit_line: self.hit_line(target),
        };
        self.hit_badge_n = None;
        self.flash = None;
        self.engine.send(Command::StartAuto(cfg));
    }

    pub fn stop_auto(&mut self) {
        self.engine.send(Command::StopAuto);
    }

    pub fn stop_on_next_win(&mut self) {
        self.engine.send(Command::StopOnNextWin);
    }

    pub fn toggle_pause(&mut self) {
        if self.auto.running {
            self.engine.send(Command::TogglePause);
        }
    }

    pub fn rotate_now(&mut self) {
        self.engine.send(Command::RotateSeed);
    }

    pub fn reset_stats(&mut self) {
        let cur = self.active_currency();
        let dec = self.info(&cur).map(|i| i.decimals).unwrap_or(8);
        self.stats.reset(&cur, dec);
    }

    /// Ctrl+R: everything back to zero. Stops a run, wipes history and stats,
    /// clears the auto state and changes the client seed.
    pub fn reset_session(&mut self) {
        if self.auto.running {
            self.engine.send(Command::StopAuto);
        }
        self.auto = AutoState::default();
        self.history.clear();
        self.hit_badge_n = None;
        self.pop_at = None;
        self.queued = 0;
        self.reset_stats();
        if self.slot.running {
            self.stop_slot();
        }
        self.spins.clear();
        self.slot_stats = Stats::fresh(&self.slot_stats.cur.clone(), self.slot_stats.decimals);
        self.slot_pop = None;
        self.engine.send(Command::SeedPolicy { every: self.settings.seed_every() });
        if self.ready() {
            self.engine.send(Command::RotateSeed);
        }
        self.flash = None;
        self.notice = Some(("Session reset".into(), Instant::now()));
    }

    pub fn sync_seed_policy(&mut self) {
        self.engine.send(Command::SeedPolicy { every: self.settings.seed_every() });
    }

    pub fn sync_concurrency(&mut self) {
        self.engine.send(Command::SetConcurrency(self.settings.async_bets.clamp(1, engine::MAX_CONCURRENCY)));
    }

    pub fn switch_casino(&mut self, id: CasinoId) {
        if id == self.settings.casino {
            return;
        }
        self.settings.casino = id;
        self.market = Market::default();
        self.provable = ProvableState::default();
        self.status = ConnStatus::Connecting;
        self.status_detail.clear();
        self.engine.send(Command::SetCasino { id, token: self.settings.token(), mirror: self.settings.mirror() });
        self.engine.send(Command::SeedPolicy { every: self.settings.seed_every() });
        self.mark_dirty();
    }

    // ------------------------------------------------------------ account flow

    /// Opens the update dialog and asks GitHub for the latest release.
    pub fn check_updates(&mut self) {
        self.account_menu_open = false;
        self.welcome_menu_open = false;
        self.update_open = true;
        self.updater.check();
    }

    pub fn go_accounts(&mut self) {
        if self.auto.running {
            self.stop_auto();
        }
        self.leave_slot();
        self.account_menu_open = false;
        self.wallet_open = false;
        self.stage = Stage::Idle;
        self.onboard_error = None;
        self.screen = Screen::Accounts;
    }

    pub fn open_account(&mut self, id: CasinoId) {
        self.switch_casino(id);
        self.stage = Stage::Idle;
        self.onboard_error = None;
        self.key_input.clear();
        self.screen = Screen::Account;
    }

    pub fn open_settings(&mut self) {
        self.key_input = self.settings.token().unwrap_or_default();
        self.mirror_input = self.settings.mirror().unwrap_or_default();
        self.mirror_search.clear();
        self.mirror_open = false;
        self.onboard_error = None;
        self.stage = Stage::Settings;
    }

    pub fn close_settings(&mut self) {
        self.stage = Stage::Idle;
        self.onboard_error = None;
    }

    /// Settings card: Save pressed. Stores the key (and mirror) and runs the connection check.
    pub fn save_key(&mut self) {
        let key = self.key_input.trim().to_string();
        if key.is_empty() {
            self.onboard_error = Some(format!("Paste your {} first.", self.casino().key_label()));
            return;
        }
        self.mirror_open = false;
        let mirror = crate::casino::stake::normalize_mirror(&self.mirror_input);
        if self.settings.mirror().is_some() && !mirror.is_empty() && self.settings.mirror().as_deref() != Some(mirror.as_str()) {
            self.settings.mirrors.insert(self.casino().key().to_string(), mirror.clone());
            self.engine.send(Command::SetMirror(mirror));
            self.status = ConnStatus::Connecting;
            self.status_detail.clear();
        }
        *self.settings.token_mut() = key;
        self.settings.save();
        self.onboard_error = None;
        self.begin_login();
    }

    /// Login pressed: (re)authenticate with the saved key and show the check modal.
    pub fn begin_login(&mut self) {
        self.onboard_error = None;
        self.status = if matches!(self.status, ConnStatus::Offline | ConnStatus::Connecting) { self.status } else { ConnStatus::Authenticating };
        self.engine.send(Command::SetToken(self.settings.token()));
        self.stage = Stage::Checking(Instant::now());
    }

    pub fn cancel_check(&mut self) {
        self.stage = Stage::Idle;
    }

    pub fn start_login(&mut self) {
        self.onboard_error = None;
        match LoginSession::spawn(self.settings.casino, self.repaint.clone()) {
            Ok(s) => self.stage = Stage::Login(s),
            Err(e) => {
                self.stage = Stage::Settings;
                self.onboard_error = Some(e);
            }
        }
    }

    pub fn cancel_login(&mut self, msg: &str) {
        self.stage = Stage::Settings;
        self.onboard_error = Some(msg.to_string());
    }

    /// Leaving a slot page closes its provider session.
    fn leave_slot(&mut self) {
        if self.screen == Screen::Slot {
            self.close_slot_session();
        }
    }

    pub fn go_lobby(&mut self) {
        self.leave_slot();
        self.stage = Stage::Idle;
        self.onboard_error = None;
        self.screen = Screen::Lobby;
        self.mark_dirty();
    }

    pub fn go_limbo(&mut self) {
        self.leave_slot();
        self.screen = Screen::Limbo;
    }

    // ------------------------------------------------------------ slots

    pub fn go_slots(&mut self) {
        self.leave_slot();
        self.screen = Screen::Slots;
    }

    pub fn request_slots_initial(&mut self) {
        self.catalog.error = None;
        self.catalog.featured.clear();
        self.catalog.loading = Provider::SUPPORTED.len() as u8 + 1;
        for p in Provider::SUPPORTED {
            self.engine.send(Command::ListSlots { kind: SlotListKind::Featured, filter: SlotFilter { producer: Some((*p).into()), limit: 100, ..Default::default() } });
        }
        self.engine.send(Command::ListSlots { kind: SlotListKind::All, filter: SlotFilter { limit: 60, ..Default::default() } });
    }

    pub fn search_slots(&mut self) {
        let q = self.catalog.query.trim().to_string();
        self.catalog.last_sent = q.clone();
        if q.is_empty() {
            self.catalog.results.clear();
            return;
        }
        self.catalog.loading += 1;
        self.engine.send(Command::ListSlots { kind: SlotListKind::Search, filter: SlotFilter { query: Some(q), limit: 60, ..Default::default() } });
    }

    pub fn load_more_slots(&mut self) {
        if self.catalog.loading > 0 || self.catalog.all_done {
            return;
        }
        self.catalog.loading += 1;
        self.engine.send(Command::ListSlots { kind: SlotListKind::All, filter: SlotFilter { skip: self.catalog.all.len(), limit: 60, ..Default::default() } });
    }

    fn slot_play_mode(&self) -> PlayMode {
        if self.settings.slot_demo {
            PlayMode::Demo
        } else {
            PlayMode::Real
        }
    }

    /// Opens the game page and starts a session in the current mode.
    pub fn open_slot_game(&mut self, game: SlotGame) {
        if Provider::for_producer(&game.producer).is_none() {
            self.flash(format!("{} games are not supported yet. Hacksaw Gaming works.", game.producer));
            return;
        }
        if !self.ready() && !self.settings.slot_demo {
            self.settings.slot_demo = true;
        }
        self.screen = Screen::Slot;
        self.spins.clear();
        self.slot_stats = Stats::fresh("USD", 2);
        self.slot_pop = None;
        let currency = self.active_currency();
        self.engine.send(Command::OpenSlot { game, mode: self.slot_play_mode(), currency });
    }

    pub fn slot_set_mode(&mut self, demo: bool) {
        if self.settings.slot_demo == demo {
            return;
        }
        if !demo && !self.ready() {
            self.flash("Log in first to play for real");
            return;
        }
        self.settings.slot_demo = demo;
        self.mark_dirty();
        if let Some(game) = self.slot.game.clone() {
            self.spins.clear();
            self.slot_stats = Stats::fresh("USD", 2);
            let currency = self.active_currency();
            self.engine.send(Command::OpenSlot { game, mode: self.slot_play_mode(), currency });
        }
    }

    pub fn close_slot_session(&mut self) {
        self.engine.send(Command::CloseSlot);
    }

    /// Opens the current game's session again (after an error).
    pub fn reopen_slot(&mut self) {
        if let Some(game) = self.slot.game.clone() {
            let currency = self.active_currency();
            self.engine.send(Command::OpenSlot { game, mode: self.slot_play_mode(), currency });
        }
    }

    pub fn start_slot(&mut self) {
        let Some(info) = self.slot.info.clone() else { return self.flash("No session yet") };
        let game_id = self.slot.game.as_ref().map(|g| g.id.clone()).unwrap_or_default();
        let prefs = self.settings.slot_prefs.get(&game_id).cloned().unwrap_or_default();
        let bet = prefs.bet.or(info.default_bet).or(info.bet_levels.first().map(|l| l.minor)).unwrap_or(0);
        if bet <= 0 {
            return self.flash("Pick a bet size");
        }
        let feature = prefs.feature.filter(|f| info.features.iter().any(|x| &x.id == f));
        let s = &self.settings;
        let stop_mult = if s.slot_stop_mult_on { money::num(&s.slot_stop_mult).filter(|m| *m > 0.0) } else { None };
        let money_field = |v: &str| -> Option<i64> {
            let f = money::num(v)?;
            (f > 0.0).then(|| (f * 10f64.powi(info.decimals as i32)).round() as i64)
        };
        let cfg = SlotConfig {
            bet,
            feature,
            stop_mult,
            limit: s.slot_count.trim().parse().unwrap_or(0),
            stop_profit: money_field(&s.slot_stop_profit),
            stop_loss: money_field(&s.slot_stop_loss),
            delay_ms: 0,
            currency: self.active_currency(),
        };
        self.flash = None;
        self.engine.send(Command::StartSlot(cfg));
    }

    pub fn stop_slot(&mut self) {
        self.engine.send(Command::StopSlot);
    }

    pub fn pause_slot(&mut self) {
        self.engine.send(Command::PauseSlot);
    }

    pub fn slot_stop_on_next_win(&mut self) {
        self.engine.send(Command::SlotStopOnNextWin);
    }

    fn on_spin(&mut self, rec: SpinRecord) {
        let bet = BetRecord {
            at: rec.at,
            nonce: None,
            currency: rec.currency.clone(),
            target: 1.0,
            outcome: rec.multiplier,
            // a round only counts as a win when it pays back at least the stake
            win: rec.multiplier >= 1.0,
            hit: rec.hit,
            amount: rec.cost as Units,
            payout: rec.win as Units,
            profit: (rec.win - rec.cost) as Units,
            ms: rec.ms,
        };
        if self.slot_stats.cur != rec.currency {
            self.slot_stats = Stats::fresh(&rec.currency, rec.decimals);
        }
        self.slot_stats.settle(&bet, rec.decimals);
        if rec.hit && self.settings.hit_sound {
            self.sound.ding();
        }
        self.spins.push_front(rec);
        if self.spins.len() > 500 {
            self.spins.pop_back();
        }
        self.slot_pop = Some(Instant::now());
    }

    /// Advances the connect / login flow from status events and login-window tokens.
    fn onboarding_logic(&mut self) {
        match &mut self.stage {
            Stage::Idle | Stage::Settings => {}
            Stage::Checking(since) => {
                if self.status.is_ready() {
                    // let the second tick show for a moment, then go
                    if since.elapsed() > Duration::from_millis(600) {
                        self.go_lobby();
                    }
                }
            }
            Stage::Login(session) => {
                session.poll();
                let finished = session.finished();
                let report = session.last.clone();
                if report.logged_in && !report.token.is_empty() && session.tried.as_deref() != Some(report.token.as_str()) {
                    session.tried = Some(report.token.clone());
                    *self.settings.token_mut() = report.token.clone();
                    self.engine.send(Command::SetToken(Some(report.token)));
                    return;
                }
                let tried = session.tried.is_some();
                if tried && self.status.is_ready() {
                    session.close();
                    self.settings.save();
                    self.key_input = self.settings.token().unwrap_or_default();
                    self.go_lobby();
                } else if finished {
                    self.cancel_login("The login window was closed before you logged in.");
                }
            }
        }
    }

    /// Switch COIN <-> USD, converting the typed amount so the stake stays the same.
    pub fn set_unit(&mut self, unit: Unit) {
        if self.settings.unit == unit || self.auto.running {
            return;
        }
        let cur = self.active_currency();
        let base = self.to_base(&self.settings.amount, &cur);
        self.settings.unit = unit;
        if let (Some(base), Some(info)) = (base, self.info(&cur).cloned()) {
            self.settings.amount = match unit {
                Unit::Usd => self.usd_value(base, &cur).map(|u| trim_float(u, 4)).unwrap_or_default(),
                Unit::Coin => format_units(base, info.decimals, info.decimals),
            };
        }
        self.mark_dirty();
    }

    pub fn adjust_amount(&mut self, double: bool) {
        let cur = self.active_currency();
        if self.usd_mode() {
            let v = money::num(&self.settings.amount).unwrap_or(0.0);
            self.settings.amount = trim_float(if double { v * 2.0 } else { v / 2.0 }, 4);
        } else if let Some(info) = self.info(&cur).cloned() {
            let base = parse_units(&self.settings.amount, info.decimals).unwrap_or(0);
            let next = if double { base * 2 } else { base / 2 };
            self.settings.amount = format_units(next, info.decimals, info.decimals);
        }
        self.mark_dirty();
    }

    // ------------------------------------------------------------ events

    fn drain_events(&mut self) {
        while let Ok(ev) = self.engine.rx.try_recv() {
            match ev {
                Event::Status(s, d) => {
                    self.status = s;
                    self.status_detail = d;
                }
                Event::Market(m) => {
                    if let Some(u) = &m.username {
                        let key = self.settings.casino.key().to_string();
                        if self.settings.usernames.get(&key) != Some(u) {
                            self.settings.usernames.insert(key, u.clone());
                            self.mark_dirty();
                        }
                    }
                    self.market = m;
                }
                Event::Provable(p) => self.provable = p,
                Event::Bet(rec) => {
                    let dec = self.info(&rec.currency).map(|i| i.decimals).unwrap_or(8);
                    self.stats.settle(&rec, dec);
                    if let Some(n) = rec.nonce {
                        self.provable.nonce = Some(n);
                    }
                    let hit = rec.hit;
                    let n = self.history.push_bet(rec);
                    if hit {
                        self.hit_badge_n = Some(n);
                        if self.settings.hit_sound {
                            self.sound.ding();
                        }
                    }
                    self.pop_at = Some(Instant::now());
                }
                Event::SeedRotated { at, client_seed, reason } => self.history.push_seed(at, client_seed, reason),
                Event::Retry => self.stats.retries += 1,
                Event::Queue(q) => self.queued = q,
                Event::Seed(s) => self.seed = s,
                Event::Auto(a) => {
                    if a.running && !self.auto.running {
                        self.hit_badge_n = None;
                    }
                    self.auto = a;
                }
                Event::Flash(m) => self.flash(m),
                Event::Slots { kind, skip, games, error } => {
                    self.catalog.loading = self.catalog.loading.saturating_sub(1);
                    self.catalog.error = error.filter(|e| !e.contains("Not connected"));
                    match kind {
                        SlotListKind::Featured => {
                            for g in games {
                                if !self.catalog.featured.iter().any(|x| x.id == g.id) {
                                    self.catalog.featured.push(g);
                                }
                            }
                        }
                        SlotListKind::Search => self.catalog.results = games,
                        SlotListKind::All => {
                            if games.len() < 60 {
                                self.catalog.all_done = true;
                            }
                            if skip == 0 {
                                self.catalog.all = games;
                            } else {
                                self.catalog.all.extend(games);
                            }
                        }
                    }
                }
                Event::Slot(st) => {
                    // FASTBET_SLOT_RUN=1: start spinning as soon as the session is up (UI work)
                    let auto = std::env::var("FASTBET_SLOT_RUN").is_ok() && st.info.is_some() && !st.running && self.slot.info.is_none() && st.done == 0;
                    self.slot = st;
                    if auto {
                        self.settings.slot_demo = true;
                        self.start_slot();
                    }
                }
                Event::Spin(rec) => self.on_spin(rec),
            }
        }
    }

    fn hotkeys(&mut self, ctx: &egui::Context) {
        // Ctrl+R resets the session, on every screen that bets
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::R)) {
            self.reset_session();
            return;
        }
        if !self.settings.hotkeys {
            return;
        }
        let typing = ctx.memory(|m| m.focused().is_some());
        if typing {
            return;
        }
        let (space, esc, p, mods) = ctx.input(|i| {
            (i.key_pressed(Key::Space), i.key_pressed(Key::Escape), i.key_pressed(Key::P), i.modifiers)
        });
        if mods.ctrl || mods.alt || mods.command {
            return;
        }
        if self.screen == Screen::Slot {
            if esc && self.slot.running {
                self.stop_slot();
            }
            if p && self.slot.running {
                self.pause_slot();
            }
            return;
        }
        if space {
            self.manual_bet();
        }
        if esc && self.auto.running {
            self.engine.send(Command::StopAuto);
        }
        if p {
            self.toggle_pause();
        }
    }

    fn maybe_save(&mut self, force: bool) {
        if self.dirty && (force || self.last_save.elapsed() > Duration::from_millis(400)) {
            self.settings.save();
            self.dirty = false;
            self.last_save = Instant::now();
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.screen {
            Screen::Accounts | Screen::Account => "FastBet".to_string(),
            _ => {
                let who = self.username_for(self.casino()).unwrap_or_default();
                let page = match self.screen {
                    Screen::Limbo => " – Limbo".to_string(),
                    Screen::Slots => " – Slots".to_string(),
                    Screen::Slot => {
                        let g = self.slot.game.as_ref().map(|g| g.name.clone()).unwrap_or_else(|| "Slot".into());
                        let state = if self.slot.running { " [Running]" } else { "" };
                        format!(" – {g}{state}")
                    }
                    _ => String::new(),
                };
                if who.is_empty() {
                    format!("FastBet{page}")
                } else {
                    format!("{who} ({}){page} – FastBet", self.casino().name())
                }
            }
        };
        if title != self.title {
            self.title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }

    // ------------------------------------------------------------ shell

    fn rail(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.add_space(4.0);
            widgets::logo_mark(ui, 32.0);
            ui.add_space(18.0);
            if widgets::rail_button(ui, self.screen == Screen::Lobby, "Lobby", |p, r, c| widgets::icon(p, r, "home", c)).clicked() {
                self.go_lobby();
            }
            if widgets::rail_button(ui, self.screen == Screen::Limbo, "Limbo", |p, r, c| widgets::icon(p, r, "trend", c)).clicked() {
                self.go_limbo();
            }
            if self.casino().has_slots() {
                let on_slots = matches!(self.screen, Screen::Slots | Screen::Slot);
                if widgets::rail_button(ui, on_slots, "Slots", |p, r, c| widgets::icon(p, r, "slots", c)).clicked() {
                    self.go_slots();
                }
            }
            ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).size(9.5).color(theme::DIM));
                ui.add_space(6.0);
                self.account_button(ui);
            });
        });
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // left: status
            let (color, pulse) = if self.ready() {
                (GREEN, false)
            } else if self.status.is_bad() {
                (RED, false)
            } else {
                (theme::GOLD, true)
            };
            let detail = self.status.long(&self.status_detail);
            status_dot(ui, color, self.status.short(), pulse).on_hover_text(detail);
            // in-flight bets of an auto run are expected; only manual spam is worth a pill
            if self.queued > 1 && !self.auto.running {
                widgets::pill(ui, format!("{} queued", self.queued), theme::BLUE);
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // centre: the wallet pill
                let hr = ui.max_rect();
                let pill_rect = egui::Rect::from_center_size(pos2(hr.center().x, hr.center().y), egui::vec2(260.0, 34.0));
                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(pill_rect).layout(Layout::centered_and_justified(egui::Direction::LeftToRight)));
                child.horizontal_centered(|ui| wallet::pill(self, ui));
            });
        });
        if !self.wallet_open {
            self.wallet_search_focused = false;
        }
    }

    /// The casino's logo in a circle at the bottom of the rail; opens the account menu.
    fn account_button(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::click());
        let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() || self.account_menu_open, 0.1);
        let p = ui.painter();
        let c = rect.center();
        p.circle_filled(c, 17.0, theme::mix(theme::BG2, theme::BG3, t));
        let edge = if self.account_menu_open { theme::alpha(GREEN, 170) } else { theme::mix(theme::LINE2, theme::MUTED, t) };
        p.circle_stroke(c, 17.0, Stroke::new(1.5, edge));
        self.logos.paint(p, egui::Rect::from_center_size(c, egui::vec2(18.0, 18.0)), self.casino(), theme::TEXT);
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        let resp = if self.account_menu_open { resp } else { resp.on_hover_text("Account") };
        if resp.clicked() {
            self.account_menu_open = !self.account_menu_open;
        }
        if self.account_menu_open {
            self.account_menu(ui, rect);
        }
    }

    /// Pops up beside the rail button: who and where you are, and Switch account.
    fn account_menu(&mut self, ui: &egui::Ui, anchor: egui::Rect) {
        let id = self.casino();
        let name = self.username_for(id).unwrap_or_else(|| "Account".into());
        let site = self.casino_site();
        let width = 230.0;
        let mut switch = false;
        let mut updates = false;
        let logos = &mut self.logos;
        let area = egui::Area::new(egui::Id::new("account-menu"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(pos2(anchor.right() + 14.0, anchor.bottom()))
            .show(ui.ctx(), |ui| {
                Frame::new()
                    .fill(BG1)
                    .stroke(Stroke::new(1.0, theme::LINE2))
                    .corner_radius(egui::CornerRadius::same(12))
                    .inner_margin(Margin::same(6))
                    .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(170) })
                    .show(ui, |ui| {
                        let w = width - 12.0;
                        ui.set_width(w);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        let (hr, _) = ui.allocate_exact_size(egui::vec2(w, 50.0), egui::Sense::hover());
                        let p = ui.painter();
                        let lc = pos2(hr.left() + 24.0, hr.center().y);
                        p.circle_filled(lc, 16.0, theme::BG2);
                        p.circle_stroke(lc, 16.0, Stroke::new(1.0, theme::LINE2));
                        logos.paint(p, egui::Rect::from_center_size(lc, egui::vec2(16.0, 16.0)), id, theme::TEXT);
                        let x = hr.left() + 50.0;
                        widgets::fit_text(p, pos2(x, hr.center().y - 8.0), egui::Align2::LEFT_CENTER, &name, theme::font_semi(13.0), theme::TEXT, hr.right() - x - 8.0);
                        p.text(pos2(x, hr.center().y + 9.0), egui::Align2::LEFT_CENTER, &site, theme::font(11.0), theme::MUTED);
                        widgets::hsep(ui);
                        if widgets::menu_item(ui, "swap", "Switch account").clicked() {
                            switch = true;
                        }
                        if widgets::menu_item(ui, "download", "Check for updates").clicked() {
                            updates = true;
                        }
                    });
            });
        // a click anywhere else, or Esc, closes it
        let rect = area.response.rect;
        let outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !anchor.contains(p)));
        if outside || ui.input(|i| i.key_pressed(Key::Escape)) {
            self.account_menu_open = false;
        }
        if switch {
            self.go_accounts();
        }
        if updates {
            self.check_updates();
        }
    }

    /// A short message (a bet error, "Session reset") floating above the bottom of `area`, fading out.
    fn toast(&self, ui: &egui::Ui, area: egui::Rect) {
        let flash = self.flash.as_ref().map(|(m, t)| (m, *t, RED, 6.0));
        let notice = self.notice.as_ref().map(|(m, t)| (m, *t, GREEN, 3.0));
        let live = |x: &(&String, Instant, egui::Color32, f32)| x.1.elapsed().as_secs_f32() < x.3;
        let Some((msg, at, color, life)) = flash.filter(live).or(notice.filter(live)) else { return };
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        let age = at.elapsed().as_secs_f32();
        let fade = ((life - age) / 0.4).clamp(0.0, 1.0) * (age / 0.15).clamp(0.0, 1.0);

        let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("toast")));
        let g = p.layout_no_wrap(msg.clone(), theme::font_semi(12.0), theme::mix(color, TEXT2, 0.25).gamma_multiply(fade));
        let w = (g.size().x + 40.0).min(area.width() - 48.0);
        let r = egui::Rect::from_center_size(pos2(area.center().x, area.bottom() - 36.0), egui::vec2(w, 32.0));
        p.rect(
            r,
            egui::CornerRadius::same(16),
            BG1.gamma_multiply(fade),
            Stroke::new(1.0, theme::alpha(color, 90).gamma_multiply(fade)),
            egui::StrokeKind::Inside,
        );
        p.circle_filled(pos2(r.left() + 16.0, r.center().y), 3.5, color.gamma_multiply(fade));
        p.with_clip_rect(r.shrink(2.0)).galley(pos2(r.left() + 28.0, r.center().y - g.size().y / 2.0), g, TEXT2);
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        if !self.casino().has_slots() && matches!(self.screen, Screen::Slots | Screen::Slot) {
            self.go_lobby();
        }
        match self.screen {
            Screen::Accounts | Screen::Account => self.onboarding_logic(),
            _ => self.hotkeys(ctx),
        }
        self.update_title(ctx);

        // the speed and session counters keep moving between bets
        if self.auto.running || !self.stats.times.is_empty() || self.slot.running || !self.slot_stats.times.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }

        let closing = ctx.input(|i| i.viewport().close_requested());
        self.maybe_save(closing);
        if closing {
            self.engine.send(Command::Shutdown);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        match self.screen {
            Screen::Accounts => {
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(0))).show(ui, |ui| accounts::accounts(self, ui));
            }
            Screen::Account => {
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(0))).show(ui, |ui| accounts::account(self, ui));
            }
            Screen::Lobby | Screen::Limbo | Screen::Slots | Screen::Slot => {
                egui::Panel::left("rail")
                    .exact_size(60.0)
                    .resizable(false)
                    .frame(Frame::new().fill(BG1).inner_margin(Margin::symmetric(0, 10)))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        self.rail(ui);
                        let r = ui.max_rect();
                        ui.painter().vline(r.right() + 0.5, r.y_range().expand(10.0), Stroke::new(1.0, LINE));
                    });

                egui::Panel::top("topbar")
                    .exact_size(56.0)
                    .frame(Frame::new().fill(BG1).inner_margin(Margin::symmetric(16, 10)))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        let r = ui.max_rect();
                        ui.painter().hline(r.x_range().expand(16.0), r.bottom() + 10.0, Stroke::new(1.0, LINE));
                        self.top_bar(ui);
                    });

                egui::Panel::right("stats")
                    .resizable(true)
                    .default_size(280.0)
                    .size_range(250.0..=380.0)
                    .frame(Frame::new().fill(BG1).inner_margin(Margin::same(0)))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        let r = ui.max_rect();
                        ui.painter().vline(r.left() - 0.5, r.y_range(), Stroke::new(1.0, LINE));
                        stats_pane::show(self, ui)
                    });

                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(0))).show(ui, |ui| {
                    let area = ui.max_rect();
                    match self.screen {
                        Screen::Limbo => limbo::show(self, ui),
                        Screen::Slots => slots_lobby::show(self, ui),
                        Screen::Slot => slot::show(self, ui),
                        _ => lobby::show(self, ui),
                    }
                    self.toast(ui, area);
                });
            }
        }
        update::show(self, ui.ctx());
    }
}

fn trim_float(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".into()
    } else {
        s.to_string()
    }
}

pub fn clear_history(app: &mut App) {
    app.history.clear();
    app.hit_badge_n = None;
}
