//! The bell beside the VIP pill: every hit (what the Hits list gets: the stop-on-hit line, or a
//! win of 10× and up), runs stopped by an empty balance or an error, and a new FastBet version. Every tab's land in the one list, newest first;
//! clicking one goes to it (the hit's card, its tab, the update dialog).

use std::time::{Duration, Instant, SystemTime};

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Id, Margin, Rect, Response, Sense, Stroke, StrokeKind, Ui};

use super::hits::{self, Hit};
use super::theme::{self, alpha, BG1, BG2, DIM, G1, GOLD, GREEN, INK, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets;
use super::App;
use crate::update::State;

/// Notices kept, newest first.
const KEEP: usize = 100;
/// The same notice again within this (a restarted run running into the same error) moves the
/// old one to the top instead of adding another. Hits always get their own.
const SAME: Duration = Duration::from_secs(60);
/// How often the app looks for a new version while it runs.
const UPDATE_EVERY: Duration = Duration::from_secs(60 * 60);
const WIDTH: f32 = 340.0;
/// How a run that stopped on an error reads (a stop of the user's, a stop line, Done … don't count).
const ERRORS: [&str; 7] = ["Error", "Script error", "Seed change failed", "Session expired", "Session keeps expiring", "Could not reopen", "The round stayed open"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Hit,
    Balance,
    Error,
    Update,
}

impl Kind {
    fn icon(self) -> (&'static str, Color32) {
        match self {
            Kind::Hit => ("trophy", GOLD),
            Kind::Balance => ("coins", RED),
            Kind::Error => ("cross", RED),
            Kind::Update => ("download", GREEN),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Notice {
    kind: Kind,
    at: SystemTime,
    title: String,
    detail: String,
    /// "Limbo on Stake · Hound971"; empty for the app's own (an update).
    place: String,
    /// The account it happened on, for its tab.
    account: String,
    hit: Option<Hit>,
    read: bool,
}

#[derive(Default)]
pub struct Bell {
    list: Vec<Notice>,
    pub open: bool,
    /// The version the last update notice was for.
    told: Option<String>,
    /// The hourly update check is running: its result is the bell's to clear.
    checking: bool,
    last_check: Option<Instant>,
}

impl Bell {
    /// FASTBET_BELL=1: one notice of each kind, the list open (`closed`: shut, with its badge) (UI work).
    pub fn preview(open: bool) -> Self {
        let ago = |s: u64| SystemTime::now() - Duration::from_secs(s);
        let n = |kind, at, title: &str, detail: &str, place: &str, read| Notice {
            kind,
            at,
            title: title.into(),
            detail: detail.into(),
            place: place.into(),
            account: String::new(),
            hit: None,
            read,
        };
        Self {
            list: vec![
                n(Kind::Hit, ago(20), "Hit 1,000×", "Won $10.00 on a $0.01 bet", "Limbo on Stake · Hound971", false),
                n(Kind::Balance, ago(240), "Out of balance", "Insufficient balance · the run stopped", "Dice on Shuffle · Hound971", false),
                n(Kind::Error, ago(3_900), "Run stopped", "Connection lost", "Mines on chips.gg · Hound971", true),
                n(Kind::Update, ago(90_000), "FastBet 3.24.0 is out", "Click to update", "", true),
            ],
            open,
            ..Default::default()
        }
    }

    fn unread(&self) -> usize {
        self.list.iter().filter(|n| !n.read).count()
    }

    fn add(&mut self, n: Notice) {
        let fresh = |old: &Notice| old.at.elapsed().is_ok_and(|d| d < SAME);
        let same = |o: &Notice| n.kind != Kind::Hit && o.kind == n.kind && o.title == n.title && o.detail == n.detail && o.account == n.account && fresh(o);
        if let Some(i) = self.list.iter().position(same) {
            self.list.remove(i);
        }
        self.list.insert(0, n);
        self.list.truncate(KEEP);
    }

    /// Everything seen: the list closed.
    fn close(&mut self) {
        self.open = false;
        for n in &mut self.list {
            n.read = true;
        }
    }
}

/// "Limbo on Stake · Hound971", for the tab in the app's fields.
fn place(app: &App, game: &str) -> String {
    let who = app.active_name().map(|u| format!(" · {u}")).unwrap_or_default();
    format!("{game} on {}{who}", app.casino().name())
}

/// A notice from the tab in the app's fields.
fn push(app: &mut App, kind: Kind, title: String, detail: String, place: String, hit: Option<Hit>) {
    let account = app.settings.account.clone();
    app.bell.add(Notice { kind, at: SystemTime::now(), title, detail, place, account, hit, read: false });
}

/// A bet or spin went into the Hits list.
pub fn hit(app: &mut App, hit: Hit) {
    let title = format!("Hit {}", crate::money::fmt_multi(hit.multiplier));
    let demo = if hit.demo { "Demo · " } else { "" };
    let detail = format!("{demo}Won {} on a {} bet", hit.win, hit.bet);
    let place = place(app, &hit.game);
    push(app, Kind::Hit, title, detail, place, Some(hit));
}

/// An auto run or slot autoplay on `game` stopped with `msg`: a notice when the balance ran
/// out or an error stopped it.
pub fn run_ended(app: &mut App, game: &str, msg: &str) {
    if let Some((kind, title, detail)) = ended_badly(msg) {
        let place = place(app, game);
        push(app, kind, title, detail, place, None);
    }
}

/// What a run's end message says went wrong (its notice's kind, title and detail), if anything did.
fn ended_badly(msg: &str) -> Option<(Kind, String, String)> {
    let why = msg.strip_prefix("Error: ").unwrap_or(msg).to_string();
    if crate::casino::low_balance(msg) {
        Some((Kind::Balance, "Out of balance".into(), format!("{why} · the run stopped")))
    } else if ERRORS.iter().any(|e| msg.starts_with(e)) {
        Some((Kind::Error, "Run stopped".into(), why))
    } else {
        None
    }
}

/// Every frame: a notice for a new version the updater found (the check at start, Check for
/// updates, or the hourly one), and that hourly check, made quietly.
pub fn watch_updates(app: &mut App) {
    let state = app.updater.state();
    if let State::Available { version, .. } = &state {
        if app.bell.told.as_ref() != Some(version) {
            app.bell.told = Some(version.clone());
            let notice = Notice {
                kind: Kind::Update,
                at: SystemTime::now(),
                title: format!("FastBet {version} is out"),
                detail: "Click to update".into(),
                place: String::new(),
                account: String::new(),
                hit: None,
                read: false,
            };
            app.bell.add(notice);
        }
    }
    let last = *app.bell.last_check.get_or_insert_with(Instant::now);
    if app.bell.checking {
        if !matches!(state, State::Idle | State::Checking) {
            app.bell.checking = false;
            // the dialog opened meanwhile: the result is its to show
            if !app.update_open {
                app.updater.dismiss();
            }
        }
    } else if last.elapsed() >= UPDATE_EVERY && matches!(state, State::Idle) && !app.update_open && !app.update_on_start {
        app.bell.last_check = Some(Instant::now());
        app.bell.checking = true;
        app.updater.check();
    }
}

/// The bell, with the unread count on it. Toggles the list.
pub fn button(app: &mut App, ui: &mut Ui) {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let open = app.bell.open;
    let hot = resp.hovered() || open;
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(16), if hot { BG2 } else { BG1 }, Stroke::new(1.0, if open { alpha(G1, 140) } else { LINE }), StrokeKind::Inside);
    widgets::icon(p, Rect::from_center_size(rect.center(), vec2(15.0, 15.0)), "bell", if hot { TEXT } else { TEXT2 });
    let unread = app.bell.unread();
    if unread > 0 && !open {
        let text = if unread > 9 { "9+".to_string() } else { unread.to_string() };
        let galley = p.layout_no_wrap(text, theme::font_bold(9.5), INK);
        let badge = Rect::from_center_size(pos2(rect.right() - 5.0, rect.top() + 6.0), vec2((galley.size().x + 8.0).max(15.0), 15.0));
        p.rect(badge, CornerRadius::same(8), GREEN, Stroke::new(2.0, BG1), StrokeKind::Outside);
        p.galley(badge.center() - galley.size() / 2.0, galley, INK);
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = if open { resp } else { resp.on_hover_text("Notifications") };
    if resp.clicked() {
        if open {
            app.bell.close();
        } else {
            app.bell.open = true;
        }
    }
    if app.bell.open {
        dropdown(app, ui, rect);
    }
}

fn dropdown(app: &mut App, ui: &mut Ui, anchor: Rect) {
    let screen = ui.ctx().content_rect();
    let x = (anchor.center().x - WIDTH / 2.0).min(screen.right() - WIDTH - 8.0).max(screen.left() + 8.0);
    let mut go = None;
    let mut clear = false;
    let area = egui::Area::new(Id::new("bell-dropdown")).fixed_pos(pos2(x, anchor.bottom() + 8.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        Frame::new()
            .fill(BG1)
            .stroke(Stroke::new(1.0, LINE2))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(Margin::same(8))
            .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(170) })
            .show(ui, |ui| {
                let w = WIDTH - 16.0;
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 2.0;
                let (hr, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
                ui.painter().text(pos2(hr.left() + 6.0, hr.center().y), Align2::LEFT_CENTER, "Notifications", theme::font_bold(13.5), TEXT);
                if !app.bell.list.is_empty() {
                    let galley = ui.painter().layout_no_wrap("Clear".into(), theme::font_semi(11.5), MUTED);
                    let r = Rect::from_center_size(pos2(hr.right() - 6.0 - galley.size().x / 2.0, hr.center().y), galley.size() + vec2(8.0, 6.0));
                    let resp = ui.interact(r, Id::new("bell-clear"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
                    let color = if resp.hovered() { TEXT } else { MUTED };
                    ui.painter().galley(r.center() - galley.size() / 2.0, galley, color);
                    clear = resp.clicked();
                }
                widgets::hsep(ui);
                ui.add_space(4.0);
                if app.bell.list.is_empty() {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 74.0), Sense::hover());
                    let p = ui.painter();
                    p.text(pos2(r.center().x, r.center().y - 9.0), Align2::CENTER_CENTER, "Nothing yet", theme::font_semi(12.5), MUTED);
                    p.text(pos2(r.center().x, r.center().y + 10.0), Align2::CENTER_CENTER, "Hits, balance and errors, updates", theme::font(11.5), DIM);
                    return;
                }
                egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, n) in app.bell.list.iter().enumerate() {
                        if row(ui, n, w).clicked() {
                            go = Some(i);
                        }
                    }
                });
            });
    });
    let rect = area.response.rect;
    let outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !anchor.contains(p)));
    if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.bell.close();
    }
    if clear {
        app.bell.list.clear();
    }
    if let Some(i) = go {
        app.bell.close();
        follow(app, i);
    }
}

/// A notice: its icon, what happened, where and when.
fn row(ui: &mut Ui, n: &Notice, w: f32) -> Response {
    const PAD: f32 = 9.0;
    let text_x = 44.0;
    let text_w = w - text_x - 44.0;
    let p = ui.painter();
    let title = p.layout_no_wrap(n.title.clone(), theme::font_semi(12.5), TEXT);
    let detail = p.layout(n.detail.clone(), theme::font(11.5), MUTED, text_w);
    let place = (!n.place.is_empty()).then(|| p.layout(n.place.clone(), theme::font(11.0), DIM, text_w));
    let h = PAD + title.size().y + 2.0 + detail.size().y + place.as_ref().map_or(0.0, |g| g.size().y + 1.0) + PAD;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), alpha(Color32::WHITE, 8));
    } else if !n.read {
        p.rect_filled(rect, CornerRadius::same(8), alpha(Color32::WHITE, 4));
    }
    let (icon, color) = n.kind.icon();
    let c = pos2(rect.left() + 21.0, rect.top() + PAD + 13.0);
    p.circle_filled(c, 14.0, alpha(color, 26));
    widgets::icon(p, Rect::from_center_size(c, vec2(13.0, 13.0)), icon, color);
    let mut y = rect.top() + PAD;
    p.text(pos2(rect.right() - 10.0, y + title.size().y / 2.0), Align2::RIGHT_CENTER, ago(n.at), theme::font(11.0), DIM);
    if !n.read {
        p.circle_filled(pos2(rect.right() - 13.0, y + title.size().y + 9.0), 3.0, GREEN);
    }
    let x = rect.left() + text_x;
    y += title.size().y + 2.0;
    p.galley(pos2(x, rect.top() + PAD), title, TEXT);
    let dh = detail.size().y;
    p.galley(pos2(x, y), detail, MUTED);
    if let Some(g) = place {
        p.galley(pos2(x, y + dh + 1.0), g, DIM);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// "now", "5m", "2h", "3d".
fn ago(at: SystemTime) -> String {
    let s = at.elapsed().map(|d| d.as_secs()).unwrap_or(0);
    match s {
        0..60 => "now".into(),
        60..3_600 => format!("{}m", s / 60),
        3_600..86_400 => format!("{}h", s / 3_600),
        _ => format!("{}d", s / 86_400),
    }
}

/// Goes to notice `i`: the update dialog, or the tab it happened on (and the hit's card).
fn follow(app: &mut App, i: usize) {
    let Some(n) = app.bell.list.get(i).cloned() else { return };
    if n.kind == Kind::Update {
        app.check_updates();
        return;
    }
    if n.account != app.settings.account {
        if let Some(k) = app.tab_of(&n.account) {
            app.show_tab(k);
        }
    }
    if let Some(hit) = n.hit {
        hits::open(app, hit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_run_end_tells() {
        let kind = |m: &str| ended_badly(m).map(|(k, ..)| k);
        assert_eq!(kind("Error: Insufficient balance"), Some(Kind::Balance));
        assert_eq!(kind("Error: BALANCE_NOT_ENOUGH"), Some(Kind::Balance));
        assert_eq!(kind("Insufficient balance"), Some(Kind::Balance));
        assert_eq!(kind("Error: Connection lost"), Some(Kind::Error));
        assert_eq!(kind("Script error: x is not defined"), Some(Kind::Error));
        assert_eq!(kind("Session keeps expiring"), Some(Kind::Error));
        for fine in ["Stopped", "Done", "Stop profit reached", "Stop loss reached", "HIT 10.00×", "Stopped on win", "Condition 2 stopped the run", "Session closed"] {
            assert_eq!(kind(fine), None, "{fine}");
        }
        assert_eq!(ended_badly("Error: Connection lost").unwrap().2, "Connection lost");
    }

    #[test]
    fn every_hit_counts_but_an_error_once() {
        let n = |kind, title: &str| Notice { kind, at: SystemTime::now(), title: title.into(), detail: "Won $0.00 on a $0.00 bet".into(), place: String::new(), account: "a".into(), hit: None, read: false };
        let mut bell = Bell::default();
        bell.add(n(Kind::Hit, "Hit 10.00×"));
        bell.add(n(Kind::Hit, "Hit 10.00×"));
        bell.add(n(Kind::Error, "Run stopped"));
        bell.add(n(Kind::Error, "Run stopped"));
        assert_eq!(bell.list.iter().filter(|x| x.kind == Kind::Hit).count(), 2);
        assert_eq!(bell.list.iter().filter(|x| x.kind == Kind::Error).count(), 1);
    }
}
