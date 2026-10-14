//! The egui application: model, event handling, hotkeys, screens and the shell
//! (left rail, top bar, stats panel) around the lobby and the Limbo page.

pub mod accounts;
pub mod coins;
pub mod controls;
pub mod images;
pub mod limbo;
pub mod lobby;
pub mod logos;
pub mod original;
pub mod originals;
pub mod prefs;
pub mod slot;
pub mod slots_lobby;
pub mod sound;
pub mod stats;
pub mod stats_pane;
pub mod theme;
pub mod titlebar;
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
use crate::cloudflare::{Clearance, IpCheck};
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
    /// An original besides Limbo.
    Original(crate::originals::Game),
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
    /// Cloudflare cookie section of the account settings (casinos behind Cloudflare).
    pub cf_open: bool,
    pub cf_agent_input: String,
    pub cf_cookie_input: String,
    /// "Clear cookies" just emptied the login window's profile (settings card note).
    pub cookies_cleared: bool,
    /// Account list: the casino opened on its accounts, the search over them, and the account
    /// whose remove button was pressed once (a second press removes it).
    pub open_casino: Option<CasinoId>,
    pub account_search: String,
    pub remove_row: Option<String>,
    /// Settings card: "Remove account" was pressed once and waits for the second press.
    pub remove_armed: bool,
    /// Manual login (Goated): the tokens pasted from the browser.
    pub login_manual: bool,
    pub manual_access: String,
    pub manual_refresh: String,
    /// The IP address Cloudflare sees, for the picked mirror.
    cf_ip: Option<IpCheck>,

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
    /// The Settings window, and the category it shows.
    pub prefs_open: bool,
    pub prefs_tab: prefs::Tab,
    /// The seed name field has the focus: the name is only used once typing is done.
    pub seed_name_editing: bool,
    /// The client seed the engine was last given.
    seed_sent: Option<engine::ClientSeed>,
    /// Account and seed name the seed was last changed for, so it happens once each.
    seed_applied: Option<(String, String)>,
    /// Settings → Seed: a sample random seed, for the format (length, characters) it was made for.
    pub seed_preview: Option<((usize, String), String)>,
    /// When the installer started; the app closes shortly after.
    pub update_close_at: Option<Instant>,
    pub wallet_search: String,
    pub wallet_search_focused: bool,
    /// The wallet dropdown shows the fiat picker instead of the coins.
    pub wallet_fiats: bool,

    pub logos: logos::Logos,

    // slots
    pub images: images::ImageCache,
    pub catalog: slots_lobby::Catalog,
    pub slot: SlotState,
    pub spins: VecDeque<SpinRecord>,
    pub slot_stats: Stats,
    pub slot_pop: Option<Instant>,

    // originals
    pub orig: originals::State,

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
            // FASTBET_SCREEN=orig:<slug> opens an original's page (UI work)
            Ok(v) if v.starts_with("orig:") => crate::originals::Game::from_slug(&v[5..]).map(Screen::Original).unwrap_or(Screen::Lobby),
            _ => Screen::Accounts,
        };
        let preview: Option<usize> = std::env::var("FASTBET_PREVIEW").ok().map(|v| v.parse().unwrap_or(400));
        if preview.is_none() {
            engine.send(Command::SetCasino { id: settings.casino, setup: settings.setup() });
        }
        engine.send(Command::SeedPolicy { every: settings.seed_every() });
        engine.send(Command::SetConcurrency(settings.async_bets.clamp(1, engine::MAX_CONCURRENCY)));
        engine.send(Command::ForceRestart { always: settings.force_restart, on_error: settings.restart_on_error });

        // FASTBET_SLOT=<catalog id>:<provider id>:<name>[:<producer>] opens a demo game at startup (UI work).
        let mut screen = screen;
        if let Ok(spec) = std::env::var("FASTBET_SLOT") {
            let mut it = spec.splitn(4, ':');
            let (id, pid, name) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string(), it.next().unwrap_or("Slot").to_string());
            let producer = it.next().unwrap_or("hacksawgaming").to_string();
            let game = SlotGame { id, name, producer, image: String::new(), demo: true, rtp: 96.3, provider_game_id: Some(pid), studio: None };
            engine.send(Command::OpenSlot { game, mode: PlayMode::Demo, currency: "usdt".into(), fiat: "usd".into() });
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
            cf_open: false,
            cookies_cleared: false,
            open_casino: None,
            account_search: String::new(),
            remove_row: None,
            remove_armed: false,
            login_manual: false,
            manual_access: String::new(),
            manual_refresh: String::new(),
            cf_agent_input: String::new(),
            cf_cookie_input: String::new(),
            cf_ip: None,
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
            // FASTBET_PREFS=1: the Settings window opens at start (UI work)
            prefs_open: std::env::var("FASTBET_PREFS").is_ok(),
            prefs_tab: prefs::Tab::default(),
            seed_name_editing: false,
            seed_sent: None,
            seed_applied: None,
            seed_preview: None,
            update_close_at: None,
            wallet_search: String::new(),
            wallet_search_focused: false,
            wallet_fiats: std::env::var("FASTBET_WALLET").is_ok_and(|v| v == "fiat"),
            logos: logos::Logos::default(),
            images: images::ImageCache::new(),
            catalog: slots_lobby::Catalog::default(),
            slot: SlotState::default(),
            spins: VecDeque::new(),
            slot_stats: Stats::fresh("USD", 2),
            slot_pop: None,
            orig: originals::State::default(),
            dirty: false,
            last_save: Instant::now(),
            title: String::new(),
        };
        // the account list opens on the casino last used
        let last = app.settings.casino;
        app.open_casino = app.settings.accounts.iter().any(|a| a.casino == last).then_some(last);
        if let Some(n) = preview {
            app.load_preview(n);
        }
        // FASTBET_MIRRORS=<search>: account settings open with the mirror picker (UI work)
        if let Ok(q) = std::env::var("FASTBET_MIRRORS") {
            app.open_settings();
            app.mirror_open = true;
            app.mirror_search = if q == "1" { String::new() } else { q };
        }
        // FASTBET_CLOUDFLARE=1: account settings open on the Cloudflare cookie section (UI work)
        if std::env::var("FASTBET_CLOUDFLARE").is_ok() {
            app.open_settings();
            app.cf_open = true;
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
        self.market.currencies.insert(cur.clone(), CurrencyInfo { decimals: 6, price: 1.0, hidden: false, fiat: false });
        self.market.currencies.insert("btc".into(), CurrencyInfo { decimals: 8, price: 62_000.0, hidden: false, fiat: false });
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
                game: None,
                round: None,
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

    /// An account's username: live for the account in use, else the last one seen.
    pub fn account_name(&self, a: &crate::settings::Account) -> Option<String> {
        if a.id == self.settings.account {
            if let Some(u) = &self.market.username {
                return Some(u.clone());
            }
        }
        a.username.clone()
    }

    /// The username of the account in use.
    pub fn active_name(&self) -> Option<String> {
        self.market.username.clone().or_else(|| self.settings.active().and_then(|a| a.username.clone()))
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

    /// Amounts are shown and typed in fiat (see [`App::fiat`]) rather than in the coin.
    pub fn fiat_mode(&self) -> bool {
        self.settings.unit == Unit::Fiat
    }

    /// The fiat amounts are in: the one picked in the wallet when this casino offers it, else USD.
    pub fn fiat(&self) -> String {
        let f = self.settings.fiat.trim().to_ascii_lowercase();
        if self.market.fiats.contains(&f) && self.fiat_rate(&f).is_some() {
            f
        } else {
            "usd".into()
        }
    }

    /// USD value of one unit of a fiat.
    fn fiat_rate(&self, fiat: &str) -> Option<f64> {
        if fiat == "usd" {
            return Some(1.0);
        }
        self.info(fiat).map(|i| i.price).filter(|p| *p > 0.0)
    }

    /// Amount typed in the panel (coin or fiat) -> base units.
    pub fn to_base(&self, s: &str, cur: &str) -> Option<Units> {
        let info = self.info(cur)?;
        if s.trim().is_empty() {
            return None;
        }
        if self.fiat_mode() {
            let fiat = money::num(s)?;
            if fiat == 0.0 {
                return Some(0);
            }
            if fiat < 0.0 || info.price <= 0.0 {
                return None;
            }
            let coins = fiat * self.fiat_rate(&self.fiat())? / info.price;
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

    /// Worth of `v` in the fiat amounts are shown in.
    pub fn fiat_value(&self, v: Units, cur: &str) -> Option<f64> {
        Some(self.usd_value(v, cur)? / self.fiat_rate(&self.fiat())?)
    }

    pub fn fmt_fiat(&self, v: Units, cur: &str) -> String {
        self.fiat_value(v, cur).map(|f| money::fmt_fiat(f, &self.fiat())).unwrap_or_default()
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
        if self.fiat_mode() && self.info(cur).map(|i| i.price > 0.0).unwrap_or(false) {
            format!("{plus}{}", self.fmt_fiat(v, cur))
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
        if self.fiat_mode() {
            format!("{plus}{} {}", self.fmt_coin(v, cur), cur.to_uppercase()).trim_end().to_string()
        } else {
            let u = self.fmt_fiat(v, cur);
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
        let amount = self.to_base(&self.settings.amount, &cur).ok_or("Enter a bet amount")?;
        let id = self.casino();
        // a 0 bet is still placed where the casino takes it (Stake, Shuffle, Goated, chips.gg)
        if amount == 0 && !id.zero_bets() {
            return Err(format!("{} doesn't take bets of 0", id.name()));
        }
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
        let (cur, amount, target) = match self.bet_setup() {
            Ok(v) => v,
            Err(e) => return self.flash(e),
        };
        let cfg = match self.auto_config(cur, amount, target) {
            Ok(c) => c,
            Err(e) => return self.flash(e),
        };
        self.hit_badge_n = None;
        self.flash = None;
        self.engine.send(Command::StartAuto(cfg));
    }

    /// An auto run from the panel's strategy, stops and limits, betting `amount` on Limbo's `target`.
    fn auto_config(&self, cur: String, amount: Units, target: f64) -> Result<AutoConfig, String> {
        let s = &self.settings;
        let opt = |v: &str, name: &str| -> Result<Option<Units>, String> {
            if v.trim().is_empty() {
                return Ok(None);
            }
            self.to_base(v, &cur).map(Some).ok_or_else(|| format!("Invalid {name}"))
        };
        let stop_profit = opt(&s.stop_profit, "stop profit")?;
        let stop_loss = opt(&s.stop_loss, "stop loss")?;
        let max_bet = opt(&s.max_bet, "max bet")?;
        let hm = s.hit_multi.trim();
        if s.stop_on_hit && !hm.is_empty() && !money::num(hm).map(|h| h >= self.casino().min_target()).unwrap_or(false) {
            return Err(format!("Hit multiplier must be at least {}×", self.casino().min_target()));
        }
        if let Some(m) = max_bet {
            if amount > m {
                return Err("Bet size is above max bet".into());
            }
        }
        let pct = |a: Adjust, v: &str| if a == Adjust::Increase { Some(money::num(v).unwrap_or(0.0)) } else { None };
        Ok(AutoConfig {
            currency: cur,
            target,
            original: None,
            base: amount,
            limit: s.count.trim().parse().unwrap_or(0),
            delay_ms: money::num(&s.delay).unwrap_or(0.0).max(0.0) as u64,
            on_win: pct(s.on_win, &s.on_win_pct),
            on_loss: pct(s.on_loss, &s.on_loss_pct),
            stop_profit,
            stop_loss,
            max_bet,
            hit_line: self.hit_line(target),
        })
    }

    pub fn stop_auto(&mut self) {
        self.engine.send(Command::StopAuto);
    }

    // ------------------------------------------------------------ originals

    pub fn go_original(&mut self, game: crate::originals::Game) {
        self.leave_slot();
        self.screen = Screen::Original(game);
    }

    /// Currency and stake for an original's bet. A zero stake is allowed: Stake plays it for free.
    fn orig_setup(&self) -> Result<(String, Units), String> {
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
        let amount = self.to_base(&self.settings.amount, &cur).ok_or("Enter a bet amount")?;
        Ok((cur, amount))
    }

    /// The stop-on-hit line for originals: only when a multiplier is set.
    fn orig_hit_line(&self) -> Option<f64> {
        if !self.settings.stop_on_hit {
            return None;
        }
        money::num(&self.settings.hit_multi).filter(|h| *h > 0.0)
    }

    /// FASTBET_ORIG_PLAY="next {json}; next {json}; cashout": steps played into the open round
    /// of the page's original, one after the other (UI work; clicks can't be simulated here).
    fn orig_dev_play(&mut self) {
        let Screen::Original(g) = self.screen else { return };
        if !self.orig.open.contains_key(&g) || self.orig.busy.contains(&g) {
            return;
        }
        let Ok(script) = std::env::var("FASTBET_ORIG_PLAY") else { return };
        let done = self.orig.dev_steps;
        let Some(cmd) = script.split(';').map(str::trim).filter(|c| !c.is_empty()).nth(done) else { return };
        self.orig.dev_steps += 1;
        let (step, json) = cmd.split_once(' ').unwrap_or((cmd, "{}"));
        let params = serde_json::from_str(json).unwrap_or(serde_json::json!({}));
        let step = if step == "cashout" { crate::originals::Step::Cashout } else { crate::originals::Step::Next };
        self.orig_step(g, step, params);
    }

    /// One manual round: settles at once, or opens a round the board carries on.
    pub fn orig_bet(&mut self, game: crate::originals::Game) {
        if self.auto.running || self.orig.busy.contains(&game) || self.orig.open.contains_key(&game) {
            return;
        }
        let res = self.orig_setup().and_then(|(cur, amount)| originals::bet_params(self, game, false).map(|params| (cur, amount, params)));
        let (currency, amount, params) = match res {
            Ok(v) => v,
            Err(e) => return self.flash(e),
        };
        if game.has_steps() {
            self.orig.busy.insert(game);
        } else if self.queued >= engine::MAX_QUEUE {
            return self.flash(format!("{} bets already queued", engine::MAX_QUEUE));
        }
        let hit_line = self.orig_hit_line();
        self.engine.send(Command::Original { bet: engine::OriginalBet { game, currency, amount, params, auto: false }, hit_line });
    }

    /// Reveal, guess or cash out in the open round.
    pub fn orig_step(&mut self, game: crate::originals::Game, step: crate::originals::Step, params: serde_json::Value) {
        if self.orig.busy.contains(&game) || !self.orig.open.contains_key(&game) {
            return;
        }
        self.orig.busy.insert(game);
        self.engine.send(Command::OriginalStep { game, step, params });
    }

    pub fn orig_start_auto(&mut self, game: crate::originals::Game) {
        let res = self.orig_setup().and_then(|(cur, amount)| originals::bet_params(self, game, true).map(|params| (cur, amount, params)));
        let (cur, amount, params) = match res {
            Ok(v) => v,
            Err(e) => return self.flash(e),
        };
        let mut cfg = match self.auto_config(cur, amount, 0.0) {
            Ok(c) => c,
            Err(e) => return self.flash(e),
        };
        cfg.original = Some((game, params));
        cfg.hit_line = self.orig_hit_line();
        // HiLo's Stop takes the winnings and ends the run: a hit
        if let Some(stop) = originals::stop_line(self, game) {
            cfg.hit_line = Some(cfg.hit_line.map_or(stop, |h| h.min(stop)));
        }
        self.hit_badge_n = None;
        self.flash = None;
        self.engine.send(Command::StartAuto(cfg));
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

    /// Settings → Seed: tells the engine which client seed to send (the user's name, or random ones
    /// of the length set), and when the casino's current one isn't the name, changes the seed once
    /// (per account and name) to put the name on it.
    fn sync_own_seed(&mut self) {
        if self.seed_name_editing {
            return;
        }
        let want = match self.settings.client_seed() {
            Some(name) => engine::ClientSeed::Own(name),
            None => engine::ClientSeed::Random { len: self.settings.random_seed_len(), alphabet: self.settings.seed_chars.alphabet() },
        };
        if self.seed_sent.as_ref() != Some(&want) {
            self.seed_sent = Some(want.clone());
            self.seed_applied = None;
            self.engine.send(Command::ClientSeed(want.clone()));
        }
        let engine::ClientSeed::Own(name) = want else { return };
        let key = (self.settings.account.clone(), name);
        if !self.ready() || self.seed.busy || self.seed_applied.as_ref() == Some(&key) {
            return;
        }
        // wait until the casino has told us its current seed
        let Some(current) = self.provable.client_seed.as_deref() else { return };
        if current != key.1 {
            self.rotate_now();
        }
        self.seed_applied = Some(key);
    }

    /// Opens the Settings window.
    pub fn open_prefs(&mut self) {
        self.account_menu_open = false;
        self.welcome_menu_open = false;
        self.prefs_open = true;
    }

    /// Force restart betting (always / on errors) on or off; it applies to a run already going too.
    pub fn set_force_restart(&mut self, always: bool, on_error: bool) {
        self.settings.force_restart = always;
        self.settings.restart_on_error = on_error;
        self.mark_dirty();
        self.engine.send(Command::ForceRestart { always, on_error });
    }

    pub fn sync_concurrency(&mut self) {
        self.engine.send(Command::SetConcurrency(self.settings.async_bets.clamp(1, engine::MAX_CONCURRENCY)));
    }

    /// Makes `account` the one in use and connects to its casino with its login.
    pub fn switch_account(&mut self, account: &str) {
        let Some(id) = self.settings.accounts.iter().find(|a| a.id == account).map(|a| a.casino) else { return };
        if account == self.settings.account {
            return;
        }
        self.settings.account = account.to_string();
        self.settings.casino = id;
        self.market = Market::default();
        // every casino has its own slot catalog and originals
        self.catalog = slots_lobby::Catalog::default();
        self.orig = originals::State::default();
        self.provable = ProvableState::default();
        self.status = ConnStatus::Connecting;
        self.status_detail.clear();
        self.engine.send(Command::SetCasino { id, setup: self.settings.setup() });
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
        self.open_casino = Some(self.casino());
        self.account_search.clear();
        self.remove_row = None;
        self.screen = Screen::Accounts;
    }

    /// Account list: opens `casino` on its accounts, or closes it when it's the open one.
    pub fn toggle_casino(&mut self, casino: CasinoId) {
        self.open_casino = if self.open_casino == Some(casino) { None } else { Some(casino) };
        self.account_search.clear();
        self.remove_row = None;
    }

    pub fn open_account(&mut self, account: &str) {
        self.switch_account(account);
        self.stage = Stage::Idle;
        self.onboard_error = None;
        self.key_input.clear();
        self.remove_armed = false;
        self.screen = Screen::Account;
    }

    /// "Add account" on `casino`: a new, empty account, opened on its settings.
    pub fn add_account(&mut self, casino: CasinoId) {
        let a = crate::settings::Account::new(casino);
        let id = a.id.clone();
        self.settings.accounts.push(a);
        self.settings.save();
        self.open_account(&id);
        self.open_settings();
    }

    /// Settings card: "Remove account" pressed twice.
    pub fn remove_account(&mut self) {
        let id = self.settings.account.clone();
        self.forget_account(&id);
        self.go_accounts();
    }

    /// Forgets an account's login and its login window profile (the casino account itself stays).
    pub fn forget_account(&mut self, id: &str) {
        let active = id == self.settings.account;
        self.settings.accounts.retain(|a| a.id != id);
        self.settings.removed.push(id.to_string());
        if active {
            self.settings.account.clear();
        }
        self.settings.save();
        let _ = crate::login::clear_profile(id);
        if active {
            // nothing is connected until another account is opened
            self.engine.send(Command::SetToken(None));
            self.market = Market::default();
        }
        self.remove_armed = false;
        self.remove_row = None;
    }

    pub fn open_settings(&mut self) {
        self.key_input = self.settings.token().unwrap_or_default();
        self.mirror_input = self.settings.mirror().unwrap_or_default();
        self.mirror_search.clear();
        self.mirror_open = false;
        let saved = self.settings.clearance();
        self.cf_open = saved.is_some();
        self.cf_agent_input = saved.as_ref().map(|c| c.user_agent.clone()).unwrap_or_default();
        self.cf_cookie_input = saved.map(|c| c.cookie).unwrap_or_default();
        self.cf_ip = None;
        self.cookies_cleared = false;
        self.onboard_error = None;
        self.stage = Stage::Settings;
    }

    /// Settings with the Cloudflare cookie section open (the connection check is stuck on Cloudflare).
    pub fn open_cookie_settings(&mut self) {
        self.cancel_check();
        self.open_settings();
        self.cf_open = true;
    }

    /// The IP address Cloudflare sees on `domain`, looked up once per domain. `None` while asking.
    pub fn public_ip(&mut self, domain: &str) -> Option<Result<String, String>> {
        if self.cf_ip.as_ref().map(|c| c.domain.as_str()) != Some(domain) {
            self.cf_ip = Some(IpCheck::start(domain, self.repaint.clone()));
        }
        self.cf_ip.as_ref().and_then(IpCheck::result)
    }

    /// Stores (or drops) the Cloudflare cookie and hands it to the casino.
    fn store_clearance(&mut self, clearance: Option<Clearance>) {
        if self.settings.clearance() == clearance {
            return;
        }
        if let Some(a) = self.settings.active_mut() {
            a.clearance = clearance.clone();
        }
        self.engine.send(Command::SetClearance(clearance));
    }

    /// Cloudflare section: Remove pressed.
    pub fn remove_clearance(&mut self) {
        self.cf_agent_input.clear();
        self.cf_cookie_input.clear();
        self.store_clearance(None);
        self.settings.save();
        self.onboard_error = None;
    }

    pub fn close_settings(&mut self) {
        self.stage = Stage::Idle;
        self.onboard_error = None;
    }

    /// The browser values typed in the settings card, checked (casinos behind Cloudflare).
    pub fn typed_clearance(&self) -> Result<Option<Clearance>, String> {
        if !self.casino().behind_cloudflare() {
            return Ok(None);
        }
        let mirror = crate::casino::stake::normalize_mirror(&self.mirror_input);
        let on = if mirror.is_empty() { self.settings.mirror().unwrap_or_default() } else { mirror };
        Clearance::from_input(&on, &self.cf_agent_input, &self.cf_cookie_input)
    }

    /// The login the settings card would save: the API token typed, or else the session of a
    /// logged-in browser's cookies. A token that is an older pasted session gives way to a newer one.
    pub fn typed_login(&self, clearance: Option<&Clearance>) -> Option<String> {
        crate::cloudflare::pick_login(self.key_input.trim(), clearance.and_then(Clearance::session), self.settings.clearance().and_then(|c| c.session()))
    }

    /// Settings card: Save pressed. Stores the login, mirror and browser values and runs the connection check.
    pub fn save_key(&mut self) {
        self.mirror_open = false;
        // the browser values belong to the mirror picked above them
        let clearance = match self.typed_clearance() {
            Ok(c) => c,
            Err(e) => {
                self.cf_open = true;
                self.onboard_error = Some(e);
                return;
            }
        };
        let Some(key) = self.typed_login(clearance.as_ref()) else {
            self.onboard_error = Some(if self.casino().behind_cloudflare() {
                format!("Paste your {}, or the cookies of a browser where you're logged in.", self.casino().key_label())
            } else {
                format!("Paste your {} first.", self.casino().key_label())
            });
            return;
        };
        self.apply_picked_mirror();
        if self.casino().behind_cloudflare() {
            if let Some(c) = &clearance {
                self.cf_agent_input = c.user_agent.clone();
                self.cf_cookie_input = c.cookie.clone();
            }
            self.store_clearance(clearance);
        }
        self.key_input = key.clone();
        *self.settings.token_mut() = key;
        self.settings.save();
        self.onboard_error = None;
        self.begin_login();
    }

    /// Stores the mirror picked in the settings card and moves the connection there.
    fn apply_picked_mirror(&mut self) {
        let mirror = crate::casino::stake::normalize_mirror(&self.mirror_input);
        if self.settings.mirror().is_some() && !mirror.is_empty() && self.settings.mirror().as_deref() != Some(mirror.as_str()) {
            if let Some(a) = self.settings.active_mut() {
                a.mirror = Some(mirror.clone());
            }
            self.settings.save();
            self.engine.send(Command::SetMirror(mirror));
            self.status = ConnStatus::Connecting;
            self.status_detail.clear();
        }
    }

    /// Settings card of a casino linked through the login window: "Open … to log in" pressed.
    pub fn open_login_window(&mut self) {
        self.mirror_open = false;
        self.cookies_cleared = false;
        self.apply_picked_mirror();
        self.start_login();
    }

    /// Manual login: "Authenticate with tokens" pressed. Saves the pair and runs the connection check.
    pub fn save_manual_tokens(&mut self) {
        // pasted from the console, the access token may still carry its quotes
        let clean = |s: &str| s.trim().trim_matches('"').trim().to_string();
        let (access, refresh) = (clean(&self.manual_access), clean(&self.manual_refresh));
        if access.split('.').count() != 3 {
            self.onboard_error = Some("That access token doesn't look right: it has three parts separated by dots (eyJ…).".into());
            return;
        }
        *self.settings.token_mut() = crate::casino::creds::Creds { access, refresh }.to_token();
        self.settings.save();
        self.manual_access.clear();
        self.manual_refresh.clear();
        self.onboard_error = None;
        self.begin_login();
    }

    /// "Clear cookies": logs the login window out of the casino, to log in to another account.
    pub fn clear_login_cookies(&mut self) {
        self.onboard_error = None;
        match crate::login::clear_profile(&self.settings.account) {
            Ok(()) => self.cookies_cleared = true,
            Err(e) => self.onboard_error = Some(e),
        }
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
        let url = self.casino().login_url(self.settings.mirror().as_deref());
        // a login window before any account exists makes one (`token_mut` does)
        let _ = self.settings.token_mut();
        let profile = self.settings.account.clone();
        match LoginSession::spawn(self.settings.casino, &url, &profile, self.repaint.clone()) {
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
        self.catalog.loading = 1;
        // a curated catalog is all playable: no separate list of the supported producers
        if !self.casino().slots_curated() {
            for p in self.casino().slot_producers() {
                self.catalog.loading += 1;
                self.engine.send(Command::ListSlots { kind: SlotListKind::Featured, filter: SlotFilter { producer: Some((*p).into()), limit: 100, ..Default::default() } });
            }
        }
        self.engine.send(Command::ListSlots { kind: SlotListKind::All, filter: SlotFilter { limit: slots_lobby::PAGE, ..Default::default() } });
    }

    pub fn search_slots(&mut self) {
        let q = self.catalog.query.trim().to_string();
        self.catalog.last_sent = q.clone();
        if q.is_empty() {
            self.catalog.results.clear();
            return;
        }
        self.catalog.loading += 1;
        self.engine.send(Command::ListSlots { kind: SlotListKind::Search, filter: SlotFilter { query: Some(q), limit: slots_lobby::PAGE, ..Default::default() } });
    }

    pub fn load_more_slots(&mut self) {
        if self.catalog.loading > 0 || self.catalog.all_done {
            return;
        }
        self.catalog.loading += 1;
        self.engine.send(Command::ListSlots { kind: SlotListKind::All, filter: SlotFilter { skip: self.catalog.all.len(), limit: slots_lobby::PAGE, ..Default::default() } });
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
        let (currency, fiat) = (self.active_currency(), self.fiat());
        self.engine.send(Command::OpenSlot { game, mode: self.slot_play_mode(), currency, fiat });
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
            let (currency, fiat) = (self.active_currency(), self.fiat());
            self.engine.send(Command::OpenSlot { game, mode: self.slot_play_mode(), currency, fiat });
        }
    }

    pub fn close_slot_session(&mut self) {
        self.engine.send(Command::CloseSlot);
    }

    /// Opens the current game's session again (after an error).
    pub fn reopen_slot(&mut self) {
        if let Some(game) = self.slot.game.clone() {
            let (currency, fiat) = (self.active_currency(), self.fiat());
            self.engine.send(Command::OpenSlot { game, mode: self.slot_play_mode(), currency, fiat });
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
            fiat: self.fiat(),
            stop_before_fs: s.slot_stop_fs,
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
            game: None,
            round: None,
        };
        if self.slot_stats.cur != rec.currency || self.slot_stats.decimals != rec.decimals {
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
                    match session.crash() {
                        Some(why) => self.cancel_login(&format!("The login window failed: {why}")),
                        None => self.cancel_login("The login window was closed before you logged in."),
                    }
                }
            }
        }
    }

    /// Switch COIN <-> fiat, converting the typed amount so the stake stays the same.
    pub fn set_unit(&mut self, unit: Unit) {
        if self.settings.unit == unit || self.auto.running {
            return;
        }
        let cur = self.active_currency();
        let base = self.to_base(&self.settings.amount, &cur);
        self.settings.unit = unit;
        if let (Some(base), Some(info)) = (base, self.info(&cur).cloned()) {
            self.settings.amount = match unit {
                Unit::Fiat => self.fiat_value(base, &cur).map(|u| trim_float(u, 4)).unwrap_or_default(),
                Unit::Coin => format_units(base, info.decimals, info.decimals),
            };
        }
        self.mark_dirty();
    }

    /// Picks the fiat amounts are shown in (wallet). Amounts typed in fiat keep their worth, and an
    /// open slot session starts again in the new fiat.
    pub fn set_fiat(&mut self, code: &str) {
        let code = code.to_ascii_lowercase();
        if self.settings.fiat == code || self.auto.running {
            return;
        }
        let cur = self.active_currency();
        let typed = |s: &Settings| [s.amount.clone(), s.stop_profit.clone(), s.stop_loss.clone(), s.max_bet.clone()];
        let worth: Vec<Option<Units>> = if self.fiat_mode() { typed(&self.settings).iter().map(|v| self.to_base(v, &cur)).collect() } else { vec![] };
        self.settings.fiat = code;
        let fresh: Vec<Option<String>> = worth.into_iter().map(|b| b.and_then(|b| self.fiat_value(b, &cur)).map(|f| trim_float(f, 4))).collect();
        let s = &mut self.settings;
        for (field, v) in [&mut s.amount, &mut s.stop_profit, &mut s.stop_loss, &mut s.max_bet].into_iter().zip(fresh) {
            if let Some(v) = v {
                *field = v;
            }
        }
        self.mark_dirty();
        self.slot_money_changed();
    }

    /// The coin or fiat a real slot session runs in changed: open it again with the new one.
    pub fn slot_money_changed(&mut self) {
        if self.screen == Screen::Slot && !self.slot.running && self.slot_play_mode() == PlayMode::Real {
            self.reopen_slot();
        }
    }

    pub fn adjust_amount(&mut self, double: bool) {
        let cur = self.active_currency();
        if self.fiat_mode() {
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
                        let casino = self.settings.casino;
                        if let Some(a) = self.settings.active_mut().filter(|a| a.casino == casino && a.username.as_ref() != Some(u)) {
                            a.username = Some(u.clone());
                            self.mark_dirty();
                        }
                    }
                    self.market = m;
                }
                Event::Provable(p) => self.provable = p,
                // the old login stopped working the moment the casino renewed it: save right away
                Event::Token(id, token) => {
                    if let Some(a) = self.settings.active_mut().filter(|a| a.casino == id) {
                        a.token = token;
                        self.settings.save();
                    }
                }
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
                            if games.len() < slots_lobby::PAGE {
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
                Event::Round(r) => {
                    let g = r.game;
                    self.orig.busy.remove(&g);
                    if r.active {
                        self.orig.open.insert(g, r.clone());
                    } else {
                        self.orig.open.remove(&g);
                    }
                    self.orig.last.insert(g, r);
                    self.orig.landed = Some((g, Instant::now()));
                }
                Event::RoundFailed { game, .. } => {
                    self.orig.busy.remove(&game);
                }
                Event::OriginalTables(t) => self.orig.tables = t,
                Event::Originals(list) => self.orig.catalog = list,
            }
        }
    }

    fn hotkeys(&mut self, ctx: &egui::Context) {
        // Ctrl+R resets the session, on every screen that bets
        if ctx.input(|i| i.modifiers.ctrl && i.key_pressed(Key::R)) {
            self.reset_session();
            return;
        }
        if !self.settings.hotkeys || self.prefs_open {
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
            match self.screen {
                Screen::Original(g) => {
                    if self.settings.mode == crate::settings::Mode::Manual || !g.auto() {
                        self.orig_bet(g);
                    }
                }
                _ => self.manual_bet(),
            }
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
                let who = self.active_name().unwrap_or_default();
                let page = match self.screen {
                    Screen::Limbo => " – Limbo".to_string(),
                    Screen::Original(g) => format!(" – {}", self.casino().original_name(g)),
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
            // Limbo and every other original open from the Originals lobby
            let in_originals = matches!(self.screen, Screen::Lobby | Screen::Limbo | Screen::Original(_));
            if widgets::rail_button(ui, in_originals, "Originals", widgets::paint_die).clicked() {
                self.go_lobby();
            }
            if self.casino().has_slots() {
                let on_slots = matches!(self.screen, Screen::Slots | Screen::Slot);
                if widgets::rail_button(ui, on_slots, "Slots", widgets::paint_slot_machine).clicked() {
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
            self.wallet_fiats = false;
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
        let name = self.active_name().unwrap_or_else(|| "Account".into());
        let site = self.casino_site();
        let width = 230.0;
        let mut switch = false;
        let mut prefs = false;
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
                        if widgets::menu_item(ui, "gear", "Settings").clicked() {
                            prefs = true;
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
        if prefs {
            self.open_prefs();
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
        self.sync_own_seed();
        if !self.casino().has_slots() && matches!(self.screen, Screen::Slots | Screen::Slot) {
            self.go_lobby();
        }
        if let Screen::Original(g) = self.screen {
            if !self.casino().originals().contains(&g) {
                self.go_lobby();
            } else if self.ready() && g.has_steps() && self.orig.resumed.insert(g) {
                self.engine.send(Command::OriginalResume(g));
            }
        }
        self.orig_dev_play();
        if self.ready() && !self.orig.info_requested {
            self.orig.info_requested = true;
            if !self.casino().originals().is_empty() {
                self.engine.send(Command::OriginalInfo);
            }
            // FASTBET_ORIG_BET=1: one manual bet on the open original (or Limbo) once connected
            // (UI work; FASTBET_ORIG_BET=auto starts an auto run instead)
            match (self.screen, std::env::var("FASTBET_ORIG_BET").as_deref()) {
                (Screen::Original(g), Ok("auto")) => self.orig_start_auto(g),
                (Screen::Original(g), Ok(_)) => self.orig_bet(g),
                (Screen::Limbo, Ok("auto")) => self.start_auto(),
                (Screen::Limbo, Ok(_)) => self.manual_bet(),
                _ => {}
            }
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
        egui::Panel::top("titlebar")
            .exact_size(titlebar::HEIGHT)
            .frame(Frame::new().fill(BG))
            .show_separator_line(false)
            .show(ui, |ui| titlebar::show(self, ui));
        match self.screen {
            Screen::Accounts => {
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(0))).show(ui, |ui| accounts::accounts(self, ui));
            }
            Screen::Account => {
                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(0))).show(ui, |ui| accounts::account(self, ui));
            }
            Screen::Lobby | Screen::Limbo | Screen::Slots | Screen::Slot | Screen::Original(_) => {
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

                // the Session pane: on the right, or floating over the page (see stats_pane)
                let docked = self.settings.session.docked && !self.settings.session.minimized;
                if docked {
                    egui::Panel::right("stats")
                        .resizable(true)
                        .default_size(280.0)
                        .size_range(250.0..=380.0)
                        .frame(Frame::new().fill(BG1).inner_margin(Margin::same(0)))
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            let r = ui.max_rect();
                            ui.painter().vline(r.left() - 0.5, r.y_range(), Stroke::new(1.0, LINE));
                            stats_pane::docked(self, ui)
                        });
                }

                egui::CentralPanel::default().frame(Frame::new().fill(BG).inner_margin(Margin::same(0))).show(ui, |ui| {
                    let area = ui.max_rect();
                    match self.screen {
                        Screen::Limbo => limbo::show(self, ui),
                        Screen::Slots => slots_lobby::show(self, ui),
                        Screen::Slot => slot::show(self, ui),
                        Screen::Original(g) => original::show(self, ui, g),
                        _ => lobby::show(self, ui),
                    }
                    self.toast(ui, area);
                });
                if !docked {
                    stats_pane::floating(self, ui.ctx());
                }
            }
        }
        prefs::show(self, ui.ctx());
        update::show(self, ui.ctx());
        titlebar::resize_grips(ui);
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
