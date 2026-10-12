//! Slots lobby, laid out like Antebot's: a header, the search in the middle, then the casino's
//! catalog in sections of artwork tiles with their status and edge.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui, Vec2};

use super::theme::{self, alpha, mix, BG1, BG2, DIM, G1, GREEN, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::App;
use crate::slots::{Provider, SlotGame};

const GAP: f32 = 14.0;
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

/// Tile size: the shape of the casino's artwork (chips.gg's is 4:3 landscape, Stake's portrait).
fn tile_size(app: &App) -> Vec2 {
    if app.casino().slot_art_landscape() {
        vec2(172.0, 129.0)
    } else {
        vec2(142.0, 186.0)
    }
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

    // header, same shape as the originals'
    let p = ui.painter_at(full);
    widgets::glow(&p, pos2(full.left() + 160.0, full.top() - 60.0), 560.0, alpha(Color32::from_rgb(0x8b, 0x5c, 0xf6), 22));
    let x0 = full.left() + 40.0;
    p.text(pos2(x0, full.top() + 40.0), Align2::LEFT_TOP, "Slots", theme::font_bold(28.0), TEXT);
    let casino = app.casino();
    let names: Vec<&str> = casino.slot_producers().iter().map(|p| Provider::display_producer(p)).collect();
    // a long provider list reads as a count
    let who = if names.len() > 4 { format!("{} providers", names.len()) } else { names.join(", ") };
    let sub = if casino.slots_curated() {
        format!("Autoplay any {who} game on {}. No animations, no delays.", casino.name())
    } else {
        format!("Autoplay slots from {who} on {}. Other providers show as Untested.", casino.name())
    };
    p.text(pos2(x0, full.top() + 82.0), Align2::LEFT_TOP, sub, theme::font(13.5), MUTED);

    // the search, centred under the header
    let search_w = 560.0_f32.min(full.width() - 80.0);
    let sr = Rect::from_min_size(pos2(full.center().x - search_w / 2.0, full.top() + 122.0), vec2(search_w, 42.0));
    search(app, ui, sr, &names);

    // the catalog
    let grid = Rect::from_min_max(pos2(full.left(), sr.bottom() + 14.0), full.max);
    let mut gui = ui.new_child(egui::UiBuilder::new().max_rect(grid).layout(egui::Layout::top_down(egui::Align::Min)));
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut gui, |ui| {
        Frame::new().inner_margin(Margin { left: 40, right: 40, top: 10, bottom: 8 }).show(ui, |ui| {
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
                    section(app, ui, &format!("Tested · {who}"), &featured, app.catalog.loading > 0 && featured.is_empty());
                    ui.add_space(18.0);
                }
                let all = app.catalog.all.clone();
                let title = if casino.slots_curated() { format!("{who} · most played this week") } else { "All video slots".into() };
                section(app, ui, &title, &all, app.catalog.loading > 0 && all.is_empty());
                if !app.catalog.all_done && !all.is_empty() {
                    ui.add_space(12.0);
                    if button(ui, if app.catalog.loading > 0 { "Loading…" } else { "Load more" }, Btn::Ghost, app.catalog.loading == 0, Some(140.0)).clicked() {
                        app.load_more_slots();
                    }
                }
            }
            ui.add_space(24.0);
        });
    });
}

/// The search box: a wide pill with a magnifier.
fn search(app: &mut App, ui: &mut Ui, sr: Rect, names: &[&str]) {
    let curated = app.casino().slots_curated();
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sr).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let focused = child.memory(|m| m.focused()).is_some_and(|id| id == child.id().with("slot-search"));
    Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, if focused { alpha(G1, 170) } else { LINE2 }))
        .corner_radius(CornerRadius::same(21))
        .inner_margin(Margin::symmetric(18, 10))
        .show(&mut child, |ui| {
            ui.set_width(sr.width() - 36.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let (r, _) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
                let p = ui.painter();
                p.circle_stroke(r.center() - vec2(1.5, 1.5), 5.0, Stroke::new(1.6, MUTED));
                p.line_segment([r.center() + vec2(2.2, 2.2), r.center() + vec2(6.0, 6.0)], Stroke::new(1.6, MUTED));
                let hint = if curated { format!("Search {} games", names.join(", ")) } else { "Search slot games".into() };
                let edit = egui::TextEdit::singleline(&mut app.catalog.query)
                    .id(ui.id().with("slot-search"))
                    .frame(Frame::NONE)
                    .hint_text(egui::RichText::new(hint).color(DIM))
                    .font(theme::font(13.5))
                    .margin(Margin::same(0))
                    .desired_width(ui.available_width());
                if ui.add(edit).changed() {
                    app.catalog.query_at = Some(Instant::now());
                }
            });
        });
}

fn section(app: &mut App, ui: &mut Ui, title: &str, games: &[SlotGame], loading: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(widgets::semi(title, 14.0, TEXT2));
        if !games.is_empty() {
            ui.label(widgets::semi(games.len().to_string(), 12.0, DIM));
        }
    });
    ui.add_space(10.0);
    if games.is_empty() {
        ui.label(widgets::semi(if loading { "Loading…" } else { "Nothing here" }, 11.5, DIM));
        return;
    }
    let size = tile_size(app);
    let cols = ((ui.available_width() + GAP) / (size.x + GAP)).floor().max(1.0) as usize;
    let rows = games.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * (size.y + GAP) - GAP), Sense::hover());
    let mut clicked: Option<SlotGame> = None;
    for (i, g) in games.iter().enumerate() {
        let col = (i % cols) as f32;
        let row = (i / cols) as f32;
        let rect = Rect::from_min_size(pos2(area.left() + col * (size.x + GAP), area.top() + row * (size.y + GAP)), size);
        if !ui.is_rect_visible(rect.expand(8.0)) {
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
    let t = ui.ctx().animate_bool_with_time(resp.id.with("lift"), resp.hovered(), 0.12);
    let r = rect.translate(vec2(0.0, -3.0 * t));
    let p = ui.painter();
    widgets::shadow(p, r, 10, 4.0 + 5.0 * t, 10.0 + 8.0 * t, (80.0 + 60.0 * t) as u8);
    match app.images.get(ui.ctx(), &g.image) {
        Some(tex) => widgets::image_cover(p, r, &tex, 10),
        None => {
            p.rect(r, CornerRadius::same(10), BG2, Stroke::new(1.0, LINE), StrokeKind::Inside);
            let galley = p.layout(g.name.clone(), theme::font_bold(13.0), TEXT, r.width() - 16.0);
            let size = galley.size();
            p.galley(pos2(r.center().x - size.x / 2.0, r.center().y - size.y / 2.0), galley, TEXT);
            if app.images.failed(&g.image) {
                p.text(pos2(r.center().x, r.bottom() - 14.0), Align2::CENTER_CENTER, g.maker(), theme::font(9.5), DIM);
            }
        }
    }
    // an untested game stays a little dim until hovered
    if !curated && !supported && t < 1.0 {
        p.rect_filled(r, CornerRadius::same(10), alpha(Color32::BLACK, (70.0 * (1.0 - t)) as u8));
    }
    let edge_color = if supported || curated { G1 } else { LINE2 };
    p.rect_stroke(r, CornerRadius::same(10), Stroke::new(if t > 0.0 { 1.5 } else { 1.0 }, alpha(mix(alpha(Color32::BLACK, 120), edge_color, t), (90.0 + 150.0 * t) as u8)), StrokeKind::Inside);

    // badges; in a curated catalog every game is playable, so only the edge shows
    if !curated {
        status_badge(p, r.left_top() + vec2(6.0, 6.0), supported);
    }
    if g.rtp > 0.0 {
        widgets::tile_badge(p, pos2(r.right() - 6.0, r.top() + 6.0), Align2::RIGHT_TOP, &format!("Edge: {:.2}%", 100.0 - g.rtp), alpha(Color32::BLACK, 175), Color32::WHITE);
    }
    let tip = match (supported, &g.studio) {
        (true, Some(studio)) => format!("{} · {studio}", g.name),
        (true, None) => g.name.clone(),
        (false, _) => format!("{} — {} is not supported yet", g.name, g.producer),
    };
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// "Tested" with a tick on green, or "Untested" with a warning sign on red, like Antebot's.
fn status_badge(p: &egui::Painter, at: egui::Pos2, tested: bool) {
    let (text, fill) = if tested { ("Tested", alpha(GREEN, 225)) } else { ("Untested", alpha(RED, 215)) };
    let g = p.layout_no_wrap(text.to_string(), theme::font_semi(10.0), Color32::WHITE);
    let r = Rect::from_min_size(at, vec2(g.size().x + 26.0, g.size().y + 6.0));
    p.rect_filled(r, CornerRadius::same(5), fill);
    let c = pos2(r.left() + 10.0, r.center().y);
    let ink = Stroke::new(1.5, Color32::WHITE);
    if tested {
        p.circle_stroke(c, 4.6, ink);
        p.add(egui::Shape::line(vec![c + vec2(-2.2, 0.1), c + vec2(-0.6, 1.8), c + vec2(2.3, -1.6)], ink));
    } else {
        let tri = vec![pos2(c.x, c.y - 4.6), pos2(c.x + 5.0, c.y + 4.0), pos2(c.x - 5.0, c.y + 4.0), pos2(c.x, c.y - 4.6)];
        p.add(egui::Shape::line(tri, ink));
        p.line_segment([pos2(c.x, c.y - 1.6), pos2(c.x, c.y + 1.2)], Stroke::new(1.3, Color32::WHITE));
        p.circle_filled(pos2(c.x, c.y + 2.6), 0.7, Color32::WHITE);
    }
    p.galley(pos2(r.left() + 19.0, r.top() + 3.0), g, Color32::WHITE);
}
