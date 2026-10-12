//! The lobby: the casino's originals as artwork tiles, Limbo first, the way Antebot shows them.
//! Slots have their own page in the rail.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, mix, G1, MUTED, TEXT};
use super::widgets;
use super::App;
use crate::originals::Game as Original;

const TILE_W: f32 = 158.0;
const TILE_H: f32 = 204.0;
const GAP: f32 = 16.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let top = ui.max_rect().top();
        let used = content(app, ui, Rect::from_min_max(pos2(full.left(), top), pos2(full.right(), top + 10_000.0)));
        ui.allocate_space(vec2(full.width(), used));
    });
}

/// Draws the page from the top of `full`; returns the height used.
fn content(app: &mut App, ui: &mut Ui, full: Rect) -> f32 {
    let id = app.casino();
    let p = ui.painter_at(full);
    widgets::glow(&p, pos2(full.left() + 160.0, full.top() - 60.0), 560.0, alpha(G1, 22));

    let x0 = full.left() + 40.0;
    let mut y = full.top() + 40.0;
    p.text(pos2(x0, y), Align2::LEFT_TOP, "Originals", theme::font_bold(28.0), TEXT);
    y += 42.0;
    p.text(pos2(x0, y), Align2::LEFT_TOP, format!("Instant bets on {}. No animations, no delays.", id.name()), theme::font(13.5), MUTED);
    y += 44.0;

    // Limbo, then the casino's other originals
    let games: Vec<Option<Original>> = std::iter::once(None).chain(id.originals().iter().copied().map(Some)).collect();
    let avail = full.right() - 40.0 - x0;
    let cols = ((avail + GAP) / (TILE_W + GAP)).floor().max(1.0) as usize;
    let mut open: Option<Option<Original>> = None;
    for (i, g) in games.iter().enumerate() {
        let col = (i % cols) as f32;
        let row = (i / cols) as f32;
        let rect = Rect::from_min_size(pos2(x0 + col * (TILE_W + GAP), y + row * (TILE_H + GAP)), vec2(TILE_W, TILE_H));
        let slug = g.map(|g| g.slug()).unwrap_or("limbo");
        let name = g.map(|g| id.original_name(g)).unwrap_or("Limbo");
        let listed = app.orig.catalog.iter().find(|c| c.id == slug);
        let art = listed.map(|c| c.image.clone()).unwrap_or_default();
        // the edge the casino lists; Limbo's is known without the list
        let edge = match listed.map(|c| c.rtp).filter(|r| *r > 0.0) {
            Some(rtp) => Some(100.0 - rtp),
            None if g.is_none() => Some(100.0 - id.win_chance(1.0).min(100.0)),
            None => None,
        };
        if tile(app, ui, rect, slug, name, &art, edge).clicked() {
            open = Some(*g);
        }
    }
    match open {
        Some(Some(g)) => app.go_original(g),
        Some(None) => app.go_limbo(),
        None => {}
    }
    y += games.len().div_ceil(cols) as f32 * (TILE_H + GAP) - GAP;
    y - full.top() + 40.0
}

/// An original's tile: the casino's artwork with its edge, or the name on a gradient while
/// the artwork loads (or where the casino has none).
fn tile(app: &mut App, ui: &mut Ui, rect: Rect, slug: &str, name: &str, art: &str, edge: Option<f64>) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("orig-tile", slug)), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("lift"), resp.hovered(), 0.12);
    let r = rect.translate(vec2(0.0, -4.0 * t));
    let p = ui.painter();
    widgets::shadow(p, r, 12, 6.0 + 6.0 * t, 14.0 + 10.0 * t, (90.0 + 60.0 * t) as u8);
    match app.images.get(ui.ctx(), art) {
        Some(tex) => widgets::image_cover(p, r, &tex, 12),
        None => {
            let base = Color32::from_rgb(0x1e, 0x3a, 0x5f);
            widgets::gradient_rect(p, r, 12, [mix(base, G1, 0.25), base, mix(base, Color32::BLACK, 0.3), mix(base, Color32::BLACK, 0.45)]);
            let g = p.layout(name.to_uppercase(), theme::font_bold(18.0), Color32::WHITE, r.width() - 20.0);
            let size = g.size();
            p.galley(pos2(r.center().x - size.x / 2.0, r.bottom() - 26.0 - size.y), g, Color32::WHITE);
        }
    }
    if let Some(e) = edge {
        widgets::tile_badge(p, pos2(r.right() - 6.0, r.top() + 6.0), Align2::RIGHT_TOP, &format!("Edge: {}%", trim_pct(e)), alpha(Color32::BLACK, 175), Color32::WHITE);
    }
    p.rect_stroke(r, CornerRadius::same(12), Stroke::new(if t > 0.0 { 1.5 } else { 1.0 }, alpha(mix(Color32::WHITE, G1, t), (25.0 + 140.0 * t) as u8)), StrokeKind::Inside);
    resp.on_hover_text(name.to_string()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn trim_pct(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}
