//! Tabs: every tab plays on its own account with its own connection, page, bet panel and
//! session, and keeps betting while another one is on screen.
//!
//! The tab on screen lives in [`App`]'s own fields (its panel in the tab fields of
//! [`Settings`](crate::settings::Settings)), so every page works on it as before; the others keep
//! theirs in a [`Tab`]. [`App::with_tab`] trades the two for a moment, which is how the tabs off
//! screen take their events every frame.

use std::collections::VecDeque;
use std::time::Instant;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{pos2, vec2, CornerRadius, Id, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, DIM, GREEN, LINE2, MUTED, RED, TEXT, TEXT2};
use super::{challenges, code, hits, originals, rewards, slots_lobby, widgets, App, Screen, Stage, VaultView};
use crate::casino::{CasinoId, ConnStatus, Market, ProvableState, VipStatus};
use crate::cloudflare::IpCheck;
use crate::engine::slots::{SlotState, SpinRecord};
use crate::engine::{self, AutoState, Command, EngineHandle, SeedInfo};
use crate::settings::MAX_TABS;
use crate::ui::stats::{History, Stats};

/// A tab off screen: what the app's tab fields hold for the tab on screen.
pub struct Tab {
    pub(super) engine: EngineHandle,
    pub(super) screen: Screen,
    pub(super) stage: Stage,
    pub(super) onboard_error: Option<String>,
    pub(super) key_input: String,
    pub(super) mirror_input: String,
    pub(super) mirror_search: String,
    pub(super) mirror_open: bool,
    pub(super) cf_open: bool,
    pub(super) cf_agent_input: String,
    pub(super) cf_cookie_input: String,
    pub(super) cookies_cleared: bool,
    pub(super) open_casino: Option<CasinoId>,
    pub(super) account_search: String,
    pub(super) remove_row: Option<String>,
    pub(super) remove_armed: bool,
    pub(super) login_manual: bool,
    pub(super) manual_access: String,
    pub(super) manual_refresh: String,
    pub(super) cf_ip: Option<IpCheck>,
    pub(super) status: ConnStatus,
    pub(super) status_detail: String,
    pub(super) market: Market,
    pub(super) provable: ProvableState,
    pub(super) seed: SeedInfo,
    pub(super) queued: usize,
    pub(super) auto: AutoState,
    pub(super) stats: Stats,
    pub(super) history: History,
    pub(super) hit_badge_n: Option<u64>,
    pub(super) hits: Vec<hits::Hit>,
    pub(super) hits_seen: usize,
    pub(super) hits_tab: bool,
    pub(super) card: Option<hits::CardView>,
    pub(super) flash: Option<(String, Instant)>,
    pub(super) notice: Option<(String, Instant)>,
    pub(super) pop_at: Option<Instant>,
    pub(super) token_visible: bool,
    pub(super) wallet_open: bool,
    pub(super) wallet_search: String,
    pub(super) wallet_search_focused: bool,
    pub(super) wallet_fiats: bool,
    pub(super) seed_sent: Option<engine::ClientSeed>,
    pub(super) seed_applied: Option<(String, String)>,
    pub(super) catalog: slots_lobby::Catalog,
    pub(super) slot: SlotState,
    pub(super) spins: VecDeque<SpinRecord>,
    pub(super) spins_total: u64,
    pub(super) slot_stats: Stats,
    pub(super) slot_pop: Option<Instant>,
    pub(super) orig: originals::State,
    pub(super) code: code::State,
    pub(super) vault_sent: Option<Option<engine::VaultPolicy>>,
    pub(super) vault: VaultView,
    pub(super) vip: Option<VipStatus>,
    pub(super) vip_open: bool,
    pub(super) rewards: rewards::State,
    pub(super) challenges: challenges::State,
}

impl Tab {
    /// A tab that has just opened on `screen`, talking to `engine`.
    pub fn new(engine: EngineHandle, screen: Screen) -> Self {
        Self {
            engine,
            screen,
            stage: Stage::Idle,
            onboard_error: None,
            key_input: String::new(),
            mirror_input: String::new(),
            mirror_search: String::new(),
            mirror_open: false,
            cf_open: false,
            cf_agent_input: String::new(),
            cf_cookie_input: String::new(),
            cookies_cleared: false,
            open_casino: None,
            account_search: String::new(),
            remove_row: None,
            remove_armed: false,
            login_manual: false,
            manual_access: String::new(),
            manual_refresh: String::new(),
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
            hits: Vec::new(),
            hits_seen: 0,
            hits_tab: false,
            card: None,
            flash: None,
            notice: None,
            pop_at: None,
            token_visible: false,
            wallet_open: false,
            wallet_search: String::new(),
            wallet_search_focused: false,
            wallet_fiats: false,
            seed_sent: None,
            seed_applied: None,
            catalog: slots_lobby::Catalog::default(),
            slot: SlotState::default(),
            spins: VecDeque::new(),
            spins_total: 0,
            slot_stats: Stats::fresh("USD", 2),
            slot_pop: None,
            orig: originals::State::default(),
            code: Default::default(),
            vault_sent: None,
            vault: VaultView::default(),
            vip: None,
            vip_open: false,
            rewards: rewards::State::default(),
            challenges: challenges::State::default(),
        }
    }
}

/// Trades the tab fields of an [`App`] with a [`Tab`]'s. Every field of `App` is named, so a
/// new one has to be put with the app (`shell`) or with the tab.
macro_rules! trade {
    ($app:expr, $tab:expr; shell: $($(#[$m:meta])* $s:ident),* ; tab: $($f:ident),* $(,)?) => {{
        let App { $($(#[$m])* $s: _,)* $($f,)* } = $app;
        $(std::mem::swap($f, &mut $tab.$f);)*
    }};
}

/// What the tab strip shows of a tab.
struct Info {
    casino: Option<CasinoId>,
    name: Option<String>,
    /// An auto run or slot autoplay is going.
    running: bool,
}

impl App {
    /// Trades the tab on screen's fields for `t`'s.
    fn trade(&mut self, t: &mut Tab) {
        trade!(self, t;
            shell: settings, repaint, #[cfg(target_os = "android")] paste, #[cfg(target_os = "android")] keyboard,
                sound, account_menu_open, welcome_menu_open, updater, member, gate_login, gate_password, gate_show_pw,
                was_unlocked, update_open, update_on_start, whats_new, prefs_open, prefs_tab, storage, sound_view, alerts_view, seed_name_editing,
                seed_preview, update_close_at, logos, images, dirty, last_save, title, tabs, close_armed;
            tab: engine, screen, stage, onboard_error, key_input, mirror_input, mirror_search, mirror_open, cf_open,
                cf_agent_input, cf_cookie_input, cookies_cleared, open_casino, account_search, remove_row, remove_armed,
                login_manual, manual_access, manual_refresh, cf_ip, status, status_detail, market, provable, seed, queued,
                auto, stats, history, hit_badge_n, hits, hits_seen, hits_tab, card, flash, notice, pop_at, token_visible,
                wallet_open, wallet_search, wallet_search_focused, wallet_fiats, seed_sent, seed_applied, catalog, slot,
                spins, spins_total, slot_stats, slot_pop, orig, code, vault_sent, vault, vip, vip_open, rewards, challenges,
        );
    }

    /// Puts tab `k` in the app's fields (and its panel in the settings' tab fields).
    fn activate(&mut self, k: usize) {
        let cur = self.settings.tab;
        if k == cur {
            return;
        }
        let Some(mut t) = self.tabs.get_mut(k).and_then(Option::take) else { return };
        self.trade(&mut t);
        self.tabs[cur] = Some(t);
        self.settings.select_tab(k);
    }

    /// Runs `f` with tab `k` in the app's fields, then puts back the one that was.
    pub fn with_tab<R>(&mut self, k: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        let cur = self.settings.tab;
        self.activate(k);
        let r = f(self);
        self.activate(cur);
        r
    }

    /// Runs `f` on every tab but the one on screen.
    pub fn each_background(&mut self, mut f: impl FnMut(&mut Self)) {
        for k in 0..self.tabs.len() {
            if self.tabs[k].is_some() {
                self.with_tab(k, &mut f);
            }
        }
    }

    /// Brings tab `k` on screen.
    pub fn show_tab(&mut self, k: usize) {
        if k >= self.tabs.len() {
            return;
        }
        self.account_menu_open = false;
        self.welcome_menu_open = false;
        self.close_armed = None;
        self.activate(k);
        self.settings.shown = k;
        self.mark_dirty();
    }

    /// The tab next to the one on screen (`step` 1 = right, -1 = left), wrapping around.
    pub fn cycle_tab(&mut self, step: isize) {
        let n = self.tabs.len() as isize;
        if n > 1 {
            self.show_tab((self.settings.tab as isize + step).rem_euclid(n) as usize);
        }
    }

    /// A new tab, on the account list, with a copy of this tab's panel.
    pub fn new_tab(&mut self) {
        if self.tabs.len() >= MAX_TABS {
            return self.flash(format!("{MAX_TABS} tabs is the most FastBet keeps open"));
        }
        let casino = self.casino();
        let mut prefs = self.settings.tab_prefs();
        prefs.account.clear();
        let k = self.settings.add_tab(prefs);
        self.tabs.push(Some(Box::new(Tab::new(engine::start(self.repaint.clone()), Screen::Accounts))));
        self.show_tab(k);
        self.open_casino = self.settings.accounts.iter().any(|a| a.casino == casino).then_some(casino);
        self.boot();
        self.settings.save();
    }

    /// Closes tab `k`: its runs stop and its connection closes. The last tab stays.
    pub fn close_tab(&mut self, k: usize) {
        if self.tabs.len() <= 1 || k >= self.tabs.len() {
            return;
        }
        if k == self.settings.tab {
            self.show_tab(if k + 1 < self.tabs.len() { k + 1 } else { k - 1 });
        }
        self.close_armed = None;
        if let Some(t) = self.tabs.remove(k) {
            t.engine.send(Command::Shutdown);
        }
        self.settings.remove_tab(k);
        self.settings.save();
    }

    /// The tab × (or a middle click): closes it, but a tab with a run going asks for a second press.
    fn ask_close(&mut self, k: usize, running: bool) {
        if running && self.close_armed != Some(k) {
            self.close_armed = Some(k);
        } else {
            self.close_tab(k);
        }
    }

    /// Ctrl+T opens a tab, Ctrl+Tab / Ctrl+Shift+Tab go to the next / previous one.
    pub fn tab_hotkeys(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        // with Shift first: Ctrl+Tab would take Ctrl+Shift+Tab too
        let (new, prev, next) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::T),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab),
                i.consume_key(Modifiers::COMMAND, Key::Tab),
            )
        });
        if new {
            self.new_tab();
        } else if prev {
            self.cycle_tab(-1);
        } else if next {
            self.cycle_tab(1);
        }
    }

    /// FASTBET_TAB_STEPS="auto; show:1; new; close:0": tab actions played one every 1.5 s from
    /// the start (UI work; clicks can't be simulated here). `auto` starts an auto run on Limbo.
    pub fn tab_dev_steps(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static DONE: AtomicUsize = AtomicUsize::new(0);
        let Ok(script) = std::env::var("FASTBET_TAB_STEPS") else { return };
        let done = DONE.load(Ordering::Relaxed);
        let Some(step) = script.split(';').map(str::trim).filter(|s| !s.is_empty()).nth(done) else { return };
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        if ctx.input(|i| i.time) < 4.0 + 1.5 * done as f64 {
            return;
        }
        DONE.store(done + 1, Ordering::Relaxed);
        let (what, arg) = step.split_once(':').unwrap_or((step, ""));
        let k = arg.trim().parse().unwrap_or(0);
        match what {
            "show" => self.show_tab(k),
            "new" => self.new_tab(),
            "close" => self.close_tab(k),
            "open" => self.open_account(arg.trim()),
            "auto" => {
                self.screen = Screen::Limbo;
                self.start_auto();
            }
            _ => {}
        }
    }

    /// The other tab playing on `account`, if one does.
    pub fn tab_of(&self, account: &str) -> Option<usize> {
        (0..self.tabs.len()).find(|&k| k != self.settings.tab && !account.is_empty() && self.settings.tab_account(k) == account)
    }

    /// Tells the engine of the tab in the app's fields what its panel says, and connects it to
    /// its account.
    pub fn boot(&mut self) {
        let s = &self.settings;
        if std::env::var("FASTBET_PREVIEW").is_err() && s.active().is_some() {
            self.send(Command::SetCasino { id: s.casino, setup: s.setup() });
        }
        self.send(Command::SeedPolicy { every: s.seed_every() });
        self.send(Command::SetConcurrency(s.async_bets.clamp(1, engine::MAX_CONCURRENCY)));
        self.send(Command::ForceRestart { always: s.force_restart, on_error: s.restart_on_error });
    }

    /// Every tab's engine closes (the window is closing).
    pub fn shutdown_tabs(&self) {
        self.send(Command::Shutdown);
        for t in self.tabs.iter().flatten() {
            t.engine.send(Command::Shutdown);
        }
    }

    fn tab_info(&self, k: usize) -> Info {
        let (live, running) = if k == self.settings.tab {
            (self.market.username.clone(), self.auto.running || self.slot.running)
        } else {
            match self.tabs.get(k).and_then(Option::as_ref) {
                Some(t) => (t.market.username.clone(), t.auto.running || t.slot.running),
                None => (None, false),
            }
        };
        let id = self.settings.tab_account(k);
        let account = self.settings.accounts.iter().find(|a| !id.is_empty() && a.id == id);
        Info { casino: account.map(|a| a.casino), name: live.or_else(|| account.and_then(|a| a.username.clone())), running }
    }
}

const TAB_H: f32 = 26.0;
/// Tabs share the room up to this width each; they get narrower (down to the logo) before
/// any is left out.
const TAB_MAX: f32 = 210.0;
const TAB_MIN: f32 = 40.0;
const GAP: f32 = 4.0;
const PLUS: f32 = 26.0;

/// The tabs, left to right in `area` of the title bar, and the + after them.
pub fn strip(app: &mut App, ui: &mut Ui, area: Rect) {
    let n = app.tabs.len();
    let plus = n < MAX_TABS;
    let room = area.width() - if plus { PLUS + GAP } else { 0.0 };
    let w = (room / n as f32 - GAP).min(TAB_MAX).max(TAB_MIN);
    let top = area.center().y - TAB_H / 2.0;
    let mut pick = None;
    let mut close = None;
    for k in 0..n {
        let rect = Rect::from_min_size(pos2(area.left() + k as f32 * (w + GAP), top), vec2(w, TAB_H));
        if rect.right() > area.right() + 0.5 {
            break;
        }
        let info = app.tab_info(k);
        match tab(app, ui, k, rect, &info, n > 1) {
            Some(Click::Show) => pick = Some(k),
            Some(Click::Close) => close = Some((k, info.running)),
            None => {}
        }
    }
    if plus {
        let x = area.left() + n as f32 * (w + GAP);
        let r = Rect::from_min_size(pos2(x, area.center().y - PLUS / 2.0), vec2(PLUS, PLUS));
        if r.right() <= area.right() + 0.5 && plus_button(ui, r).clicked() {
            app.new_tab();
        }
    }
    // a click anywhere but the armed × takes it back
    if close.is_none() && ui.input(|i| i.pointer.any_pressed() || i.key_pressed(egui::Key::Escape)) {
        app.close_armed = None;
    }
    if let Some((k, running)) = close {
        app.ask_close(k, running);
    } else if let Some(k) = pick {
        app.show_tab(k);
    }
}

enum Click {
    Show,
    Close,
}

/// One tab: the casino's logo (a green dot on it while a run is going), "Casino · username",
/// and its × while hovered or on screen.
fn tab(app: &mut App, ui: &Ui, k: usize, rect: Rect, info: &Info, closable: bool) -> Option<Click> {
    let on = k == app.settings.tab;
    let resp = ui.interact(rect, Id::new(("tab", k)), Sense::click());
    let hot = resp.hovered();
    let armed = app.close_armed == Some(k);
    let x_rect = Rect::from_center_size(pos2(rect.right() - 14.0, rect.center().y), vec2(18.0, 18.0));
    let show_x = closable && (on || hot || armed);
    let x_resp = show_x.then(|| {
        let tip = if armed { "A run is going: press again to stop it and close the tab" } else { "Close tab" };
        ui.interact(x_rect, Id::new(("tab-close", k)), Sense::click()).on_hover_text(tip)
    });

    let p = ui.painter();
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), hot && !on, 0.1);
    if on {
        p.rect(rect, CornerRadius::same(theme::RADIUS), BG2, Stroke::new(1.0, LINE2), StrokeKind::Inside);
    } else if t > 0.0 {
        p.rect_filled(rect, CornerRadius::same(theme::RADIUS), alpha(BG1, (255.0 * t) as u8));
    }

    let logo = Rect::from_center_size(pos2(rect.left() + 16.0, rect.center().y), vec2(14.0, 14.0));
    match info.casino {
        Some(id) => app.logos.paint(p, logo, id, if on { TEXT } else { TEXT2 }),
        None => widgets::icon(p, logo.shrink(2.0), "plus", MUTED),
    }
    if info.running {
        let c = logo.right_bottom() + vec2(0.5, 0.5);
        p.circle_filled(c, 4.0, if on { BG2 } else { theme::BG });
        p.circle_filled(c, 2.6, GREEN);
    }

    let text_left = logo.right() + 8.0;
    let room = if show_x { x_rect.left() - 4.0 } else { rect.right() - 10.0 } - text_left;
    let fmt = |color| TextFormat { font_id: theme::font_semi(12.0), color, ..Default::default() };
    let label = |with_casino: bool| {
        let mut job = LayoutJob::default();
        match info.casino {
            Some(id) => {
                if with_casino {
                    job.append(id.name(), 0.0, fmt(if on { TEXT2 } else { MUTED }));
                    job.append("  ·  ", 0.0, fmt(DIM));
                }
                let name = info.name.clone().unwrap_or_else(|| "New account".into());
                job.append(&name, 0.0, fmt(if on { TEXT } else { TEXT2 }));
            }
            None => job.append("New tab", 0.0, fmt(if on { TEXT } else { MUTED })),
        }
        job
    };
    // a tab squeezed down to its logo shows no text
    if room > 16.0 {
        // short of room, the logo says the casino and the name stays
        let mut job = label(true);
        if p.layout_job(job.clone()).size().x > room {
            job = label(false);
        }
        job.wrap = TextWrapping { max_width: room, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
        let g = p.layout_job(job);
        p.galley(pos2(text_left, rect.center().y - g.size().y / 2.0), g, TEXT);
    }

    if let Some(x) = &x_resp {
        if armed {
            p.rect(x_rect, CornerRadius::same(5), alpha(RED, if x.hovered() { 60 } else { 36 }), Stroke::new(1.0, alpha(RED, 150)), StrokeKind::Inside);
        } else if x.hovered() {
            p.rect_filled(x_rect, CornerRadius::same(5), theme::BG3);
        }
        let color = if armed { RED } else if x.hovered() { TEXT } else { MUTED };
        widgets::icon(p, x_rect.shrink(5.5), "x", color);
    }

    if x_resp.as_ref().is_some_and(|x| x.clicked()) || (closable && resp.middle_clicked()) {
        Some(Click::Close)
    } else if resp.clicked() {
        Some(Click::Show)
    } else {
        None
    }
}

fn plus_button(ui: &Ui, r: Rect) -> egui::Response {
    let resp = ui.interact(r, Id::new("tab-new"), Sense::click()).on_hover_text("New tab  (Ctrl+T)");
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, CornerRadius::same(theme::RADIUS), BG2);
    }
    widgets::icon(p, Rect::from_center_size(r.center(), vec2(11.0, 11.0)), "plus", if resp.hovered() { TEXT } else { MUTED });
    resp
}
