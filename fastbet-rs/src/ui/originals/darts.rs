//! Darts: one dart at a board of rings; the ring it lands in pays.

use std::f32::consts::TAU;

use egui::{pos2, vec2, Align2, Color32, Rect, Shape, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, MUTED};
use super::super::App;
use super::{levels, payout_color, text_color_on, times, trim};
use crate::originals::{darts_table, Game, Round, LEVELS, LEVEL_NAMES};

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if levels(ui, &mut app.settings.orig.darts_difficulty, &LEVELS, &LEVEL_NAMES, !locked) {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "difficulty": app.settings.orig.darts_difficulty }))
}

pub fn result_text(r: &Round) -> String {
    format!("{}×  ·  {}", trim(r.multiplier), r.state["difficulty"].as_str().unwrap_or(""))
}

/// Ring colours, outside in, like the site's board.
fn ring_color(i: usize, m: f64) -> Color32 {
    match i {
        0 => Color32::from_rgb(0x21, 0x37, 0x43),
        1 => Color32::from_rgb(0x32, 0x4e, 0x5e),
        _ => payout_color(m.max(1.0)),
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let difficulty = app.settings.orig.darts_difficulty.clone();
    let table = darts_table(&difficulty);
    let last = app.orig.last(Game::Darts).cloned().filter(|r| r.state["difficulty"].as_str() == Some(difficulty.as_str()));
    let age = app.orig.age(Game::Darts);
    let p = ui.painter();
    let c = pos2(rect.center().x, rect.center().y);
    let outer = (rect.height() / 2.0 - 20.0).min(rect.width() / 2.0 - 140.0).max(60.0);

    // ring radii from the chances: an equal-area board, inner rings the rarest
    let mut cum = 0.0;
    let mut radii = vec![0.0f32; table.len()];
    for i in (0..table.len()).rev() {
        cum += table[i].1;
        radii[i] = (cum.sqrt() as f32) * outer;
    }
    for (i, (m, _)) in table.iter().enumerate() {
        p.circle_filled(c, radii[i], ring_color(i, *m));
        p.circle_stroke(c, radii[i], Stroke::new(1.5, alpha(Color32::BLACK, 120)));
    }
    // labels down the right-hand side
    for (i, (m, chance)) in table.iter().enumerate() {
        let r_in = radii.get(i + 1).copied().unwrap_or(0.0);
        let y = c.y - (radii[i] + r_in) / 2.0;
        let fill = ring_color(i, *m);
        let lx = c.x + outer + 22.0;
        p.line_segment([pos2(c.x, y), pos2(lx - 4.0, y)], Stroke::new(1.0, alpha(Color32::WHITE, 40)));
        let label = Rect::from_min_size(pos2(lx, y - 10.0), vec2(64.0, 20.0));
        super::chip(p, label, &times(*m), fill, text_color_on(fill));
        p.text(pos2(label.right() + 8.0, y), Align2::LEFT_CENTER, format!("{}%", trim(chance * 100.0)), theme::font(10.5), MUTED);
    }

    // the dart: in the ring that paid, at the angle the casino drew
    if let Some(r) = &last {
        let ring = table.iter().position(|(m, _)| (m - r.multiplier).abs() < 1e-9).unwrap_or(0);
        let r_out = radii[ring];
        let r_in = radii.get(ring + 1).copied().unwrap_or(0.0);
        let frac = r.state["distance"].as_f64().unwrap_or(0.5).clamp(0.05, 0.95) as f32;
        let dist = r_in + (r_out - r_in) * frac;
        let a = r.state["rotation"].as_f64().unwrap_or(0.0) as f32 * TAU;
        let hit = pos2(c.x + dist * a.cos(), c.y + dist * a.sin());
        let t = (age / 0.25).clamp(0.0, 1.0);
        let from = pos2(hit.x + 60.0 * (1.0 - t), hit.y - 60.0 * (1.0 - t));
        let red = Color32::from_rgb(0xff, 0x3d, 0x5f);
        p.line_segment([from, from + vec2(22.0, -22.0)], Stroke::new(3.0, mix(red, Color32::BLACK, 0.2)));
        p.add(Shape::convex_polygon(vec![from + vec2(18.0, -26.0), from + vec2(28.0, -16.0), from + vec2(30.0, -30.0)], red, Stroke::NONE));
        p.circle_filled(from, 4.0, Color32::WHITE);
        if age < 0.25 {
            ui.ctx().request_repaint();
        } else {
            let fill = ring_color(ring, r.multiplier);
            let badge = Rect::from_center_size(pos2(c.x - outer - 70.0, c.y), vec2(96.0, 40.0));
            super::chip(p, badge, &times(r.multiplier), fill, text_color_on(fill));
        }
    } else {
        super::note(p, pos2(c.x, rect.bottom() - 10.0), "Throw a dart");
    }
}
