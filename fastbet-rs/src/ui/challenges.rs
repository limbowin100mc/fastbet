//! Slot challenges and King of the Hill (casinos that run them): a card on the lobby banners that
//! opens them in a window of artwork cards, badges on the challenged games' tiles, and a card on
//! the slot page with what it takes and a button that stops autoplay at the target.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, Ui};

use super::rewards::until;
use super::theme::{self, alpha, BG1, BG2, DIM, GOLD, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::{images, slots_lobby, App};
use crate::casino::{Challenge, ConnStatus};
use crate::engine::Command;
use crate::money::{fmt_multi, fmt_usd};
use crate::slots::{PlayMode, Provider, SlotGame};

/// Room around a card's artwork, and between cards.
const PAD: f32 = 8.0;
const GAP: f32 = 14.0;
/// A card's text under its artwork.
const INFO_H: f32 = 98.0;
const MAX_COLS: usize = 7;

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

/// A tile's badge for its challenge, at the bottom left.
pub fn tile_badge(app: &App, p: &egui::Painter, r: Rect, game_id: &str) {
    let mut list = app.challenges.for_game(game_id);
    let Some(c) = list.next() else { return };
    let text = if c.koth { "King of the Hill".to_string() } else { format!("Challenge {}", fmt_multi(c.target)) };
    widgets::tile_badge(p, pos2(r.left() + 7.0, r.bottom() - 7.0), Align2::LEFT_BOTTOM, &text, alpha(Color32::BLACK, 190), GOLD);
}

/// The challenges in a window over the lobby: King of the Hill first, then the rest, as cards
/// with the game's artwork, what it takes, the prize and Play.
pub fn window(app: &mut App, ctx: &egui::Context) {
    if !app.challenges.window_open {
        return;
    }
    refresh(app);
    let mut close = false;
    let mut play: Option<SlotGame> = None;
    let mut list = app.challenges.list.clone();
    list.sort_by_key(|c| !c.koth);
    let art = slots_lobby::tile_size(app);
    let card = vec2(art.x + 2.0 * PAD, PAD + art.y + INFO_H + widgets::FIELD_H + PAD);
    let screen = ctx.content_rect();
    let cols = (((screen.width() - 120.0 - 36.0 + GAP) / (card.x + GAP)).floor() as usize).clamp(1, MAX_COLS).min(list.len().max(3));
    let inner_w = cols as f32 * (card.x + GAP) - GAP;
    let width = inner_w + 36.0;
    let frame = Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, LINE2))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(0))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: Color32::from_black_alpha(160) });
    let modal = egui::Modal::new(egui::Id::new("challenges-dialog")).frame(frame).backdrop_color(alpha(theme::BG, 170)).show(ctx, |ui| {
        ui.set_width(width);
        ui.spacing_mut().item_spacing.y = 0.0;
        Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(widgets::bold("Challenges", 15.5, TEXT));
                ui.label(widgets::semi(list.iter().filter(|c| !c.koth).count().to_string(), 12.0, DIM));
                widgets::help_icon(ui, CHALLENGE_HELP);
                if list.iter().any(|c| c.koth) {
                    ui.add_space(6.0);
                    ui.label(widgets::semi("King of the Hill", 12.0, GOLD));
                    widgets::help_icon(ui, KOTH_HELP);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, "×", MUTED, true, "").clicked() {
                        close = true;
                    }
                });
            });
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
        // sized from the list itself: a height remembered from an emptier frame would clip it
        let rows = list.len().div_ceil(cols).max(1) as f32;
        let error = if app.challenges.error.is_some() { 32.0 } else { 0.0 };
        let content = 16.0 + error + if list.is_empty() { 40.0 } else { rows * (card.y + GAP) - GAP } + 18.0;
        let height = content.min((screen.height() - 170.0).max(260.0));
        egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
            Frame::new().inner_margin(Margin { left: 18, right: 18, top: 16, bottom: 18 }).show(ui, |ui| {
                ui.set_width(inner_w);
                if let Some(e) = &app.challenges.error {
                    ui.label(widgets::semi(e, 12.0, RED));
                    ui.add_space(10.0);
                }
                if list.is_empty() {
                    ui.label(widgets::semi(if app.challenges.loaded { "No challenges running" } else { "Loading…" }, 12.0, DIM));
                    return;
                }
                let (area, _) = ui.allocate_exact_size(vec2(inner_w, rows * (card.y + GAP) - GAP), Sense::hover());
                for (i, c) in list.iter().enumerate() {
                    let at = pos2(area.left() + (i % cols) as f32 * (card.x + GAP), area.top() + (i / cols) as f32 * (card.y + GAP));
                    let rect = Rect::from_min_size(at, card);
                    if ui.is_rect_visible(rect) && challenge_card(app, ui, rect, art, c) {
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

/// "$1,000" for whole dollars, else "$0.40".
pub(super) fn dollars(v: f64) -> String {
    if v >= 1.0 && (v - v.round()).abs() < 0.005 {
        format!("${}", crate::money::fmt_int(v.round() as i128))
    } else {
        fmt_usd(v)
    }
}

/// One challenge: the game's artwork, what it takes, the prize and time left, Play. True when
/// Play was pressed.
fn challenge_card(app: &mut App, ui: &mut Ui, rect: Rect, art: egui::Vec2, c: &Challenge) -> bool {
    let ppp = ui.ctx().pixels_per_point();
    let edge = if c.koth { alpha(GOLD, 150) } else { LINE };
    ui.painter().rect(rect, CornerRadius::same(12), BG2, Stroke::new(1.0, edge), StrokeKind::Inside);
    let ar = images::snap(Rect::from_min_size(rect.min + vec2(PAD, PAD), art), ppp);
    let p = ui.painter();
    if !app.images.paint_fit(p, ar, &c.game.image, 9) {
        p.rect_filled(ar, CornerRadius::same(9), BG1);
        let g = p.layout(c.game.name.clone(), theme::font_bold(14.0), TEXT, ar.width() - 16.0);
        p.galley(ar.center() - g.size() / 2.0, g, TEXT);
    }
    if c.game.rtp > 0.0 {
        widgets::tile_badge(p, pos2(ar.right() - 6.0, ar.top() + 6.0), Align2::RIGHT_TOP, &format!("Edge: {:.2}%", 100.0 - c.game.rtp), alpha(Color32::BLACK, 175), Color32::WHITE);
    }
    if c.koth {
        widgets::tile_badge(p, pos2(ar.left() + 6.0, ar.top() + 6.0), Align2::LEFT_TOP, "King of the Hill", alpha(Color32::BLACK, 190), GOLD);
    }
    p.rect_stroke(ar, CornerRadius::same(9), Stroke::new(1.0, alpha(Color32::BLACK, 120)), StrokeKind::Inside);

    // what it takes, then the prize with the time left beside it
    let x = rect.left() + PAD + 4.0;
    let w = rect.width() - 2.0 * (PAD + 4.0);
    let mut y = ar.bottom() + 10.0;
    let line = if c.koth {
        let best = if c.target > 0.0 { fmt_multi(c.target) } else { "–".into() };
        let by = c.leader.as_ref().map(|l| format!(" by {l}")).unwrap_or_default();
        format!("Best {best}{by}, min {} bet", dollars(c.min_bet_usd))
    } else {
        format!("First to hit {} with min {} bet", fmt_multi(c.target), dollars(c.min_bet_usd))
    };
    let mut job = egui::text::LayoutJob::simple(line, theme::font_semi(12.0), TEXT, w);
    job.wrap.max_rows = 2;
    let g = p.layout_job(job);
    p.galley(pos2(x, y), g, TEXT);
    y += 38.0;
    p.text(pos2(x, y), Align2::LEFT_TOP, if c.koth { "Pool" } else { "Prize" }, theme::font(11.0), MUTED);
    p.text(pos2(x, y + 15.0), Align2::LEFT_TOP, dollars(c.prize_usd), theme::font_bold(14.0), GOLD);
    if let Some(e) = c.ends {
        p.text(pos2(x + w, y), Align2::RIGHT_TOP, "Ends in", theme::font(11.0), MUTED);
        p.text(pos2(x + w, y + 16.0), Align2::RIGHT_TOP, until(e), theme::font_semi(12.0), TEXT2);
    }
    if !c.game.name.is_empty() {
        let resp = ui.interact(ar, ui.id().with(("challenge-art", &c.id)), Sense::hover());
        resp.on_hover_text(format!("{} · {}", c.game.name, c.game.maker()));
    }

    let supported = Provider::for_producer(&c.game.producer).is_some();
    let br = Rect::from_min_size(pos2(rect.left() + PAD, rect.bottom() - PAD - widgets::FIELD_H), vec2(rect.width() - 2.0 * PAD, widgets::FIELD_H));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br).id_salt(("challenge-play", &c.id)));
    let resp = button(&mut bui, "Play", if supported { Btn::Primary } else { Btn::Ghost }, supported, Some(br.width()));
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
