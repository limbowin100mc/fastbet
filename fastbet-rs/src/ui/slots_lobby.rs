//! Slots lobby: hero, search, and the tile grid from the casino catalog.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, BG3, DIM, G1, GOLD, GREEN, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::App;
use crate::slots::{Provider, SlotGame};

pub const TILE_W: f32 = 132.0;
pub const TILE_H: f32 = 164.0;
const GAP: f32 = 12.0;
/// Games per catalog request (Stake allows at most 50).
pub const PAGE: usize = 50;

#[derive(Default)]
pub struct Catalog {
    pub query: String,
    pub query_at: Option<Instant>,
    pub last_sent: String,
    pub featured: Vec<SlotGame>,
    pub all: Vec<SlotGame>,
    pub all_done: bool,
    pub results: Vec<SlotGame>,
    pub loading: u8,
    pub error: Option<String>,
    pub requested: bool,
    pub retry_at: Option<Instant>,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    if !app.catalog.requested {
        app.catalog.requested = true;
        app.catalog.retry_at = Some(Instant::now());
        app.request_slots_initial();
    }
    // the first request can race the socket; retry until the lists are in (a curated catalog has no featured list)
    let featured_missing = !app.casino().slots_curated() && app.catalog.featured.is_empty();
    if app.catalog.loading == 0 && (featured_missing || app.catalog.all.is_empty()) {
        if app.catalog.retry_at.map(|t| t.elapsed() > Duration::from_secs(2)).unwrap_or(true) {
            app.catalog.retry_at = Some(Instant::now());
            app.request_slots_initial();
        }
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
    // debounce the search box
    if let Some(t) = app.catalog.query_at {
        if t.elapsed() > Duration::from_millis(350) && app.catalog.query.trim() != app.catalog.last_sent {
            app.catalog.query_at = None;
            app.search_slots();
        } else {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
    }

    // header, same shape as the lobby's
    let p = ui.painter_at(full);
    widgets::glow(&p, pos2(full.left() + 160.0, full.top() - 60.0), 560.0, alpha(Color32::from_rgb(0x8b, 0x5c, 0xf6), 22));
    let x0 = full.left() + 40.0;
    p.text(pos2(x0, full.top() + 40.0), Align2::LEFT_TOP, "Slots", theme::font_bold(28.0), TEXT);
    let casino = app.casino();
    let names: Vec<&str> = casino.slot_producers().iter().map(|p| Provider::display_producer(p)).collect();
    let sub = if casino.slots_curated() {
        format!("Autoplay any {} game on {}. No animations, no delays.", names.join(", "), casino.name())
    } else {
        format!("Autoplay {}. Others show as Untested.", names.join(", "))
    };
    p.text(pos2(x0, full.top() + 82.0), Align2::LEFT_TOP, sub, theme::font(13.5), MUTED);
    let hero = Rect::from_min_size(full.min, vec2(full.width(), 118.0));

    // search, right-aligned on the title row
    let search_w = 360.0_f32.min(full.width() - 80.0);
    let sr = Rect::from_min_size(pos2(full.right() - 40.0 - search_w, full.top() + 44.0), vec2(search_w, 38.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr).layout(egui::Layout::left_to_right(egui::Align::Center)));
    Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, LINE2))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 8))
        .show(&mut child, |ui| {
            ui.set_width(search_w - 28.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                let p = ui.painter();
                p.circle_stroke(r.center() - vec2(1.5, 1.5), 4.5, Stroke::new(1.5, MUTED));
                p.line_segment([r.center() + vec2(2.0, 2.0), r.center() + vec2(5.5, 5.5)], Stroke::new(1.5, MUTED));
                let edit = egui::TextEdit::singleline(&mut app.catalog.query)
                    .frame(Frame::NONE)
                    .hint_text(egui::RichText::new(if casino.slots_curated() { format!("Search {} games", names.join(", ")) } else { "Search slot games".into() }).color(DIM))
                    .font(theme::font(13.0))
                    .margin(Margin::same(0))
                    .desired_width(ui.available_width());
                if ui.add(edit).changed() {
                    app.catalog.query_at = Some(Instant::now());
                }
            });
        });

    // grid
    let top = hero.bottom() + 12.0;
    let grid = Rect::from_min_max(pos2(full.left(), top), full.max);
    let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(grid).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut gui, |ui| {
        Frame::new().inner_margin(Margin::symmetric(40, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let searching = !app.catalog.query.trim().is_empty();
            if let Some(e) = app.catalog.error.clone() {
                ui.label(widgets::semi(e, 12.0, RED));
            }
            if searching {
                let games = app.catalog.results.clone();
                section(app, ui, &format!("Results for \"{}\"", app.catalog.query.trim()), &games, app.catalog.loading > 0);
            } else {
                if !casino.slots_curated() {
                    let featured = app.catalog.featured.clone();
                    section(app, ui, &format!("Tested · {}", names.join(", ")), &featured, app.catalog.loading > 0 && featured.is_empty());
                    ui.add_space(10.0);
                }
                let all = app.catalog.all.clone();
                let title = if casino.slots_curated() { format!("{} · most played this week", names.join(", ")) } else { "All video slots".into() };
                section(app, ui, &title, &all, app.catalog.loading > 0 && all.is_empty());
                if !app.catalog.all_done && !all.is_empty() {
                    ui.add_space(8.0);
                    if button(ui, if app.catalog.loading > 0 { "Loading…" } else { "Load more" }, Btn::Ghost, app.catalog.loading == 0, Some(140.0)).clicked() {
                        app.load_more_slots();
                    }
                }
            }
            ui.add_space(24.0);
        });
    });
}

fn section(app: &mut App, ui: &mut Ui, title: &str, games: &[SlotGame], loading: bool) {
    ui.label(widgets::semi(title, 14.0, TEXT2));
    ui.add_space(8.0);
    if games.is_empty() {
        ui.label(widgets::semi(if loading { "Loading…" } else { "Nothing here" }, 11.5, DIM));
        return;
    }
    let cols = ((ui.available_width() + GAP) / (TILE_W + GAP)).floor().max(1.0) as usize;
    let rows = games.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * (TILE_H + GAP) - GAP), Sense::hover());
    let mut clicked: Option<SlotGame> = None;
    for (i, g) in games.iter().enumerate() {
        let col = (i % cols) as f32;
        let row = (i / cols) as f32;
        let rect = Rect::from_min_size(pos2(area.left() + col * (TILE_W + GAP), area.top() + row * (TILE_H + GAP)), vec2(TILE_W, TILE_H));
        if !ui.is_rect_visible(rect) {
            continue;
        }
        if tile(app, ui, rect, g).clicked() {
            clicked = Some(g.clone());
        }
    }
    if let Some(g) = clicked {
        app.open_slot_game(g);
    }
}

fn tile(app: &mut App, ui: &mut Ui, rect: Rect, g: &SlotGame) -> egui::Response {
    let supported = Provider::for_producer(&g.producer).is_some();
    let curated = app.casino().slots_curated();
    let resp = ui.interact(rect, egui::Id::new(("slot-tile", &g.id, rect.min.y as i32)), Sense::click());
    let hovered = resp.hovered();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect.expand(5.0), CornerRadius::same(14), alpha(if supported { G1 } else { MUTED }, 30));
    }
    let tex = app.images.get(ui.ctx(), &g.image);
    match tex {
        Some(t) => {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            p.add(egui::epaint::RectShape::filled(rect, CornerRadius::same(10), Color32::WHITE).with_texture(t.id(), uv));
        }
        None => {
            p.rect(rect, CornerRadius::same(10), BG2, Stroke::new(1.0, LINE), StrokeKind::Inside);
            let name = g.name.clone();
            let galley = p.layout(name, theme::font_bold(13.0), TEXT, rect.width() - 16.0);
            let size = galley.size();
            p.galley(pos2(rect.center().x - size.x / 2.0, rect.center().y - size.y / 2.0), galley, TEXT);
            if app.images.failed(&g.image) {
                p.text(pos2(rect.center().x, rect.bottom() - 14.0), Align2::CENTER_CENTER, g.maker(), theme::font(9.5), DIM);
            }
        }
    }
    p.rect_stroke(rect, CornerRadius::same(10), Stroke::new(1.0, if hovered { if supported { G1 } else { LINE2 } } else { alpha(Color32::BLACK, 90) }), StrokeKind::Inside);
    // badges; in a curated catalog every game is playable, so only the edge shows
    if !curated {
        let (bt, bc) = if supported { ("Tested", GREEN) } else { ("Untested", RED) };
        let b1 = Rect::from_min_size(rect.left_top() + vec2(5.0, 5.0), vec2(50.0, 15.0));
        p.rect_filled(b1, CornerRadius::same(4), alpha(bc, 200));
        p.text(b1.center(), Align2::CENTER_CENTER, bt, theme::font_semi(9.0), Color32::WHITE);
    }
    if g.rtp > 0.0 {
        let txt = format!("Edge: {:.2}%", 100.0 - g.rtp);
        let b2 = Rect::from_min_size(pos2(rect.right() - 63.0, rect.top() + 5.0), vec2(58.0, 15.0));
        p.rect_filled(b2, CornerRadius::same(4), alpha(Color32::BLACK, 180));
        p.text(b2.center(), Align2::CENTER_CENTER, txt, theme::font_semi(9.0), Color32::WHITE);
    }
    let _ = (BG3, GOLD, DIM);
    let tip = match (supported, &g.studio) {
        (true, Some(studio)) => format!("{} · {studio}", g.name),
        (true, None) => g.name.clone(),
        (false, _) => format!("{} — {} is not supported yet", g.name, g.producer),
    };
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}
