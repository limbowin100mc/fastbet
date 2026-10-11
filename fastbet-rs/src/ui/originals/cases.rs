//! Cases: open a case; what's inside pays. Riskier difficulties hold bigger prizes.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG2, MUTED, TEXT};
use super::super::App;
use super::{chip, levels, payout_color, text_color_on, times, trim};
use crate::originals::{cases_table, Game, Round, LEVELS, LEVEL_NAMES};

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if levels(ui, &mut app.settings.orig.cases_difficulty, &LEVELS, &LEVEL_NAMES, !locked) {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "difficulty": app.settings.orig.cases_difficulty }))
}

pub fn result_text(r: &Round) -> String {
    format!("{}×  ·  {}", trim(r.state["result"].as_f64().unwrap_or(r.multiplier)), r.state["difficulty"].as_str().unwrap_or(""))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let difficulty = app.settings.orig.cases_difficulty.clone();
    let table = cases_table(&difficulty);
    let last = app.orig.last(Game::Cases).cloned().filter(|r| r.state["difficulty"].as_str() == Some(difficulty.as_str()));
    let age = app.orig.age(Game::Cases);
    let p = ui.painter();

    // the case, opening on a result
    let size = (rect.height() * 0.42).min(220.0);
    let body = Rect::from_center_size(pos2(rect.center().x, rect.top() + 30.0 + size * 0.6), vec2(size * 1.3, size * 0.8));
    let open = last.is_some();
    let t = if open { (age / 0.3).clamp(0.0, 1.0) } else { 0.0 };
    let gold = Color32::from_rgb(0xff, 0xc1, 0x3d);
    let shell = mix(Color32::from_rgb(0x8b, 0x5c, 0xf6), Color32::BLACK, 0.15);
    p.rect_filled(body.translate(vec2(0.0, 6.0)), CornerRadius::same(14), alpha(Color32::BLACK, 120));
    p.rect_filled(body, CornerRadius::same(14), shell);
    p.rect_stroke(body, CornerRadius::same(14), Stroke::new(2.0, mix(shell, Color32::WHITE, 0.25)), StrokeKind::Inside);
    let lid = Rect::from_min_size(body.left_top() - vec2(0.0, size * 0.22 + 28.0 * t), vec2(body.width(), size * 0.22));
    p.rect_filled(lid, CornerRadius::same(10), mix(shell, Color32::WHITE, 0.12));
    p.rect_filled(Rect::from_center_size(pos2(body.center().x, body.top() + 4.0), vec2(body.width() * 0.18, 14.0)), CornerRadius::same(4), gold);
    match &last {
        Some(r) => {
            let m = r.state["result"].as_f64().unwrap_or(r.multiplier);
            let c = payout_color(m);
            p.text(pos2(body.center().x, body.center().y + 6.0), Align2::CENTER_CENTER, times(m), theme::font_bold((size * 0.26 * (0.7 + 0.3 * t)).clamp(14.0, 44.0)), if m > 0.0 { c } else { MUTED });
            if age < 0.3 {
                ui.ctx().request_repaint();
            }
        }
        None => {
            p.text(pos2(body.center().x, body.center().y + 6.0), Align2::CENTER_CENTER, "?", theme::font_bold(size * 0.3), alpha(TEXT, 90));
        }
    }

    // what a case can hold, with the chances
    let strip = Rect::from_min_max(pos2(rect.left() + 24.0, rect.bottom() - 58.0), pos2(rect.right() - 24.0, rect.bottom() - 18.0));
    let n = table.len() as f32;
    let w = ((strip.width() - 6.0 * (n - 1.0)) / n).min(120.0);
    let x0 = strip.center().x - (w * n + 6.0 * (n - 1.0)) / 2.0;
    let got = last.as_ref().map(|r| r.state["result"].as_f64().unwrap_or(r.multiplier));
    for (i, (m, chance)) in table.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(x0 + i as f32 * (w + 6.0), strip.top()), vec2(w, 22.0));
        let c = payout_color(*m);
        let on = got.is_some_and(|g| (g - m).abs() < 1e-9);
        chip(p, cell, &times(*m), if on { c } else { BG2 }, if on { text_color_on(c) } else { c });
        p.text(pos2(cell.center().x, cell.bottom() + 10.0), Align2::CENTER_CENTER, format!("{}%", trim(chance * 100.0)), theme::font(10.0), MUTED);
    }
}
