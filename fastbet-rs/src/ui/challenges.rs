//! Slot challenges and King of the Hill (casinos that run them): a banner over the slots lobby
//! that opens the list in a window, badges on the challenged games' tiles, and a card on the
//! slot page with what it takes and a button that stops autoplay at the target.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui};

use super::rewards::until;
use super::theme::{self, alpha, mix, BG1, BG2, DIM, GOLD, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::App;
use crate::casino::{Challenge, ConnStatus};
use crate::engine::Command;
use crate::money::{fmt_multi, fmt_usd};
use crate::slots::{PlayMode, Provider, SlotGame};

const WIDTH: f32 = 660.0;
const ROW_H: f32 = 76.0;

const CHALLENGE_HELP: &str = "The first real-money spin on the game that pays the target multiplier or more, at the minimum bet or more, wins the prize. Open to everyone; demo spins don't count.";
const KOTH_HELP: &str = "King of the Hill: while the round runs, the best multiplier on the game (at the minimum bet or more) leads. Whoever leads when it ends wins the pool, which grows with every bet.";

#[derive(Default)]
pub struct State {
    pub list: Vec<Challenge>,
    pub loaded: bool,
    pub error: Option<String>,
    pub asked_at: Option<Instant>,
    pub window_open: bool,
}

impl State {
    pub fn for_game<'a>(&'a self, id: &'a str) -> impl Iterator<Item = &'a Challenge> + 'a {
        self.list.iter().filter(move |c| c.game.id == id)
    }

    pub fn koth(&self) -> Option<&Challenge> {
        self.list.iter().find(|c| c.koth)
    }
}

/// Asks for the challenges once a minute, every 15 seconds while a King of the Hill round runs.
pub fn refresh(app: &mut App) {
    // they are public, but the socket has to be up
    if !app.casino().has_challenges() || matches!(app.status, ConnStatus::Connecting | ConnStatus::Offline) {
        return;
    }
    let every = Duration::from_secs(if app.challenges.koth().is_some() { 15 } else { 60 });
    if app.challenges.asked_at.is_none_or(|t| t.elapsed() >= every) {
        app.challenges.asked_at = Some(Instant::now());
        app.send(Command::Challenges);
    }
}

/// The game to open for a challenge: the catalog's entry where the lobby has it.
fn game_of(app: &App, c: &Challenge) -> SlotGame {
    let cat = &app.catalog;
    cat.featured.iter().chain(&cat.all).chain(&cat.results).find(|g| g.id == c.game.id).cloned().unwrap_or_else(|| c.game.clone())
}

/// The banner over the slots lobby, once there are challenges. Opens the window. Returns
/// whether it is there.
pub fn banner(app: &mut App, ui: &mut Ui) -> bool {
    refresh(app);
    if app.challenges.list.is_empty() {
        return false;
    }
    let list = app.challenges.list.clone();
    let koth = list.iter().find(|c| c.koth);
    let count = list.iter().filter(|c| !c.koth).count();
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 76.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(12), mix(BG1, BG2, t), Stroke::new(1.0, mix(LINE, alpha(GOLD, 110), t)), StrokeKind::Inside);

    let ic = pos2(rect.left() + 38.0, rect.center().y);
    p.circle_filled(ic, 22.0, BG2);
    p.circle_stroke(ic, 22.0, Stroke::new(1.0, LINE2));
    let icon_r = Rect::from_center_size(ic, vec2(20.0, 20.0));
    if koth.is_some() {
        widgets::paint_crown(p, icon_r, GOLD);
    } else {
        widgets::paint_trophy(p, icon_r, GOLD);
    }

    let x = rect.left() + 74.0;
    let title = p.text(pos2(x, rect.center().y - 10.0), Align2::LEFT_CENTER, "Challenges", theme::font_bold(15.0), TEXT);
    let top = list.iter().map(|c| c.prize_usd).fold(0.0, f64::max);
    let sub = match koth {
        Some(k) => {
            live_tag(p, pos2(title.right() + 10.0, title.center().y));
            let ends = k.ends.map(|e| format!(" · ends in {}", until(e))).unwrap_or_default();
            format!("King of the Hill on {}: best {}{ends}   ·   {count} challenges", k.game.name, fmt_multi(k.target))
        }
        None => format!("{count} running · prizes up to {}", dollars(top)),
    };

    // artwork of the challenged games, right to left before the chevron
    let mut ax = rect.right() - 40.0;
    let art = vec2(64.0, 48.0);
    let mut shown = 0;
    for c in &list {
        if shown == 5 || ax - art.x < x + 380.0 {
            break;
        }
        let r = Rect::from_min_size(pos2(ax - art.x, rect.center().y - art.y / 2.0), art);
        match app.images.get(ui.ctx(), &c.game.image) {
            Some(tex) => widgets::image_cover(p, r, &tex, 6),
            None => {
                p.rect_filled(r, CornerRadius::same(6), BG2);
            }
        }
        p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, if c.koth { alpha(GOLD, 200) } else { alpha(Color32::BLACK, 120) }), StrokeKind::Inside);
        ax -= art.x + 8.0;
        shown += 1;
    }
    widgets::fit_text(p, pos2(x, rect.center().y + 11.0), Align2::LEFT_CENTER, &sub, theme::font(12.0), MUTED, (ax - x - 12.0).max(120.0));
    let c = pos2(rect.right() - 20.0, rect.center().y);
    let edge = mix(MUTED, TEXT, t);
    p.line_segment([c + vec2(-2.5, -5.0), c + vec2(2.5, 0.0)], Stroke::new(1.7, edge));
    p.line_segment([c + vec2(2.5, 0.0), c + vec2(-2.5, 5.0)], Stroke::new(1.7, edge));
    if koth.is_some() {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        app.challenges.window_open = true;
    }
    true
}

/// "LIVE" in a small gold tag, its left edge at `at`.
fn live_tag(p: &egui::Painter, at: egui::Pos2) -> Rect {
    let g = p.layout_no_wrap("LIVE".into(), theme::font_bold(9.5), GOLD);
    let r = Rect::from_min_size(pos2(at.x, at.y - g.size().y / 2.0 - 2.0), g.size() + vec2(12.0, 4.0));
    p.rect_filled(r, CornerRadius::same(4), alpha(GOLD, 36));
    p.galley(r.min + vec2(6.0, 2.0), g, GOLD);
    r
}

/// A tile's badge for its challenge, at the bottom left.
pub fn tile_badge(app: &App, p: &egui::Painter, r: Rect, game_id: &str) {
    let mut list = app.challenges.for_game(game_id);
    let Some(c) = list.next() else { return };
    let text = if c.koth { "King of the Hill".to_string() } else { format!("Challenge {}", fmt_multi(c.target)) };
    widgets::tile_badge(p, pos2(r.left() + 6.0, r.bottom() - 6.0), Align2::LEFT_BOTTOM, &text, alpha(Color32::BLACK, 190), GOLD);
}

/// The list, in a window over the lobby.
pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.challenges.window_open {
        return;
    }
    refresh(app);
    let mut close = false;
    let mut play: Option<SlotGame> = None;
    let list = app.challenges.list.clone();
    let frame = Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, LINE2))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(0))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(160) });
    let modal = egui::Modal::new(egui::Id::new("challenges-dialog")).frame(frame).backdrop_color(alpha(theme::BG, 170)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 0.0;
        Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(widgets::bold("Challenges", 14.5, TEXT));
                ui.label(widgets::semi(list.iter().filter(|c| !c.koth).count().to_string(), 12.0, DIM));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, "×", MUTED, true, "").clicked() {
                        close = true;
                    }
                });
            });
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
        // sized from the list itself: a height remembered from an emptier frame would clip it
        let koth = list.iter().any(|c| c.koth);
        let rows = list.iter().filter(|c| !c.koth).count().max(1) as f32;
        let error = if app.challenges.error.is_some() { 32.0 } else { 0.0 };
        let content = 14.0 + error + if koth { 26.0 + ROW_H + 16.0 } else { 0.0 } + 26.0 + rows * ROW_H + 18.0;
        let height = content.min((ctx.content_rect().height() - 200.0).clamp(240.0, 560.0));
        egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
            Frame::new().inner_margin(Margin { left: 18, right: 18, top: 14, bottom: 18 }).show(ui, |ui| {
                let w = WIDTH - 36.0;
                ui.set_width(w);
                if let Some(e) = &app.challenges.error {
                    ui.label(widgets::semi(e, 12.0, RED));
                    ui.add_space(10.0);
                }
                if let Some(k) = list.iter().find(|c| c.koth) {
                    heading(ui, "King of the Hill", KOTH_HELP);
                    if row(app, ui, w, k) {
                        play = Some(game_of(app, k));
                    }
                    ui.add_space(16.0);
                }
                heading(ui, "Challenges", CHALLENGE_HELP);
                let running: Vec<&Challenge> = list.iter().filter(|c| !c.koth).collect();
                if running.is_empty() {
                    ui.label(widgets::semi(if app.challenges.loaded { "No challenges running" } else { "Loading…" }, 12.0, DIM));
                }
                for c in running {
                    if row(app, ui, w, c) {
                        play = Some(game_of(app, c));
                    }
                }
            });
        });
    });
    if let Some(g) = play {
        app.challenges.window_open = false;
        app.open_slot_game(g);
    } else if close || modal.should_close() {
        app.challenges.window_open = false;
    }
    if list.iter().any(|c| c.koth) {
        ctx.request_repaint_after(Duration::from_secs(1));
    }
}

fn heading(ui: &mut Ui, title: &str, help: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(widgets::semi(title, 13.0, TEXT2));
        widgets::help_icon(ui, help);
    });
    ui.add_space(8.0);
}

/// "$1,000" for whole dollars, else "$0.40".
fn dollars(v: f64) -> String {
    if v >= 1.0 && (v - v.round()).abs() < 0.005 {
        format!("${}", crate::money::fmt_int(v.round() as i128))
    } else {
        fmt_usd(v)
    }
}

/// Paints `text` on one line no wider than `max_w`, cut short with "…".
fn text_cut(p: &egui::Painter, pos: egui::Pos2, align: Align2, text: &str, font: egui::FontId, color: Color32, max_w: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    let g = p.layout_job(job);
    p.galley(align.anchor_size(pos, g.size()).min, g, color);
}

/// One challenge: artwork, name and time left, its numbers, Play. Returns true when Play was pressed.
fn row(app: &mut App, ui: &mut Ui, w: f32, c: &Challenge) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::hover());
    let play_w = 76.0;
    let col_w = 92.0;
    let cols_left = rect.right() - play_w - 14.0 - col_w * 3.0;
    if resp.hovered() {
        resp.on_hover_text(&c.game.name);
    }
    let p = ui.painter();
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, LINE));
    let art = Rect::from_min_size(pos2(rect.left(), rect.center().y - 30.0), vec2(80.0, 60.0));
    match app.images.get(ui.ctx(), &c.game.image) {
        Some(tex) => widgets::image_cover(p, art, &tex, 8),
        None => {
            p.rect_filled(art, CornerRadius::same(8), BG2);
        }
    }
    let x = art.right() + 14.0;
    text_cut(p, pos2(x, rect.center().y - 16.0), Align2::LEFT_CENTER, &c.game.name, theme::font_bold(13.0), TEXT, cols_left - x - 14.0);
    p.text(pos2(x, rect.center().y + 1.0), Align2::LEFT_CENTER, c.game.maker(), theme::font(11.0), MUTED);
    let ends = c.ends.map(|e| format!("Ends in {}", until(e))).unwrap_or_default();
    p.text(pos2(x, rect.center().y + 18.0), Align2::LEFT_CENTER, ends, theme::font(10.5), DIM);

    // the numbers, in columns before Play
    let supported = Provider::for_producer(&c.game.producer).is_some();
    let cols: [(&str, String); 3] = if c.koth {
        [("Best", if c.target > 0.0 { fmt_multi(c.target) } else { "–".into() }), ("Min bet", dollars(c.min_bet_usd)), ("Pool", dollars(c.prize_usd))]
    } else {
        [("Target", fmt_multi(c.target)), ("Min bet", dollars(c.min_bet_usd)), ("Prize", dollars(c.prize_usd))]
    };
    let mut cx = cols_left;
    for (i, (label, value)) in cols.iter().enumerate() {
        p.text(pos2(cx, rect.center().y - 9.0), Align2::LEFT_CENTER, *label, theme::font(10.5), MUTED);
        let color = if i == 2 { GOLD } else { TEXT };
        p.text(pos2(cx, rect.center().y + 9.0), Align2::LEFT_CENTER, value, theme::font_semi(13.0), color);
        cx += col_w;
    }
    if c.koth {
        if let Some(l) = &c.leader {
            text_cut(p, pos2(cols_left, rect.center().y + 25.0), Align2::LEFT_CENTER, &format!("by {l}"), theme::font(10.0), DIM, col_w - 6.0);
        }
    }
    let br = Rect::from_min_size(pos2(rect.right() - play_w, rect.center().y - widgets::FIELD_H / 2.0), vec2(play_w, widgets::FIELD_H));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br).id_salt(("challenge-play", &c.id)));
    let resp = button(&mut bui, "Play", if supported { Btn::Primary } else { Btn::Ghost }, supported, Some(play_w));
    if supported {
        resp.clicked()
    } else {
        resp.on_hover_text(format!("{} games aren't supported yet", c.game.maker()));
        false
    }
}

/// The open slot's challenges, as cards at the top of its settings.
pub fn slot_cards(app: &mut App, ui: &mut Ui, w: f32) {
    let Some(game) = app.slot.game.clone() else { return };
    refresh(app);
    let list: Vec<Challenge> = app.challenges.for_game(&game.id).cloned().collect();
    if list.is_empty() {
        return;
    }
    let locked = app.slot.running || app.slot.opening;
    let demo = app.settings.slot_demo || app.slot.mode == Some(PlayMode::Demo);
    // the bet in USD, where the game plays in dollars
    let bet_usd = app.slot.info.as_ref().filter(|i| i.currency.eq_ignore_ascii_case("usd")).and_then(|i| {
        let pref = app.settings.slot_prefs.get(&game.id).and_then(|p| p.bet);
        let bet = pref.filter(|b| i.bet_levels.iter().any(|l| l.minor == *b)).or(i.default_bet).or(i.bet_levels.first().map(|l| l.minor))?;
        Some(bet as f64 / 10f64.powi(i.decimals as i32))
    });
    for c in list {
        let (title, help, right) = if c.koth {
            ("King of the Hill", KOTH_HELP, format!("Pool {}", dollars(c.prize_usd)))
        } else {
            ("Challenge", CHALLENGE_HELP, format!("Prize {}", dollars(c.prize_usd)))
        };
        let mut stop_at: Option<f64> = None;
        widgets::card(ui, w, title, Some(help), Some(widgets::semi(right, 12.0, GOLD)), |ui| {
            let line = if c.koth {
                let by = c.leader.as_ref().map(|l| format!(" by {l}")).unwrap_or_default();
                if c.target > 0.0 {
                    format!("Best so far {}{by}. Beat it at {} or more a spin.", fmt_multi(c.target), fmt_usd(c.min_bet_usd))
                } else {
                    format!("No multiplier yet. Spin at {} or more to lead.", fmt_usd(c.min_bet_usd))
                }
            } else {
                format!("Hit {} on a spin of {} or more.", fmt_multi(c.target), fmt_usd(c.min_bet_usd))
            };
            ui.label(RichText::new(line).size(12.0).color(TEXT2));
            if let Some(e) = c.ends {
                ui.label(RichText::new(format!("Ends in {}", until(e))).size(11.0).color(MUTED));
            }
            if demo {
                ui.label(RichText::new("Demo spins don't count.").size(11.0).color(GOLD));
            } else if bet_usd.is_some_and(|b| b + 1e-9 < c.min_bet_usd) {
                ui.label(RichText::new("Your bet size is below the minimum.").size(11.0).color(RED));
            }
            if c.target > 0.0 {
                let set = app.settings.slot_stop_mult_on && crate::money::num(&app.settings.slot_stop_mult).is_some_and(|m| (m - c.target).abs() < 0.005);
                let label = if set { format!("Stops at {}", fmt_multi(c.target)) } else { format!("Stop at {}", fmt_multi(c.target)) };
                if button(ui, &label, Btn::Ghost, !locked && !set, Some(ui.available_width())).clicked() {
                    stop_at = Some(c.target);
                }
            }
        });
        if let Some(m) = stop_at {
            app.settings.slot_stop_mult_on = true;
            app.settings.slot_stop_mult = super::trim_float(m, 2);
            app.mark_dirty();
        }
        ui.add_space(6.0);
        if c.koth {
            ui.ctx().request_repaint_after(Duration::from_secs(1));
        }
    }
}
