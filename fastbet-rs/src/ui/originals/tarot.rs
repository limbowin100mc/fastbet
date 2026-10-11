//! Tarot: three cards are drawn; their multipliers multiply.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, MUTED, TEXT};
use super::super::App;
use super::{levels, payout_color, times, trim};
use crate::originals::{Game, Round, LEVELS, LEVEL_NAMES};

const SLOTS: [(&str, &str); 3] = [("leftMultiplier", "Past"), ("middleMultiplier", "Present"), ("rightMultiplier", "Future")];

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if levels(ui, &mut app.settings.orig.tarot_difficulty, &LEVELS, &LEVEL_NAMES, !locked) {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "difficulty": app.settings.orig.tarot_difficulty }))
}

fn cards(r: &Round) -> [f64; 3] {
    SLOTS.map(|(k, _)| r.state[k]["multiplier"].as_f64().unwrap_or(0.0))
}

pub fn result_text(r: &Round) -> String {
    let [a, b, c] = cards(r);
    format!("{} × {} × {}", trim(a), trim(b), trim(c))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::Tarot).cloned();
    let age = app.orig.age(Game::Tarot);
    let p = ui.painter();
    let ch = (rect.height() * 0.6).min(260.0);
    let cw = ch * 0.62;
    let gap = 26.0;
    let x0 = rect.center().x - (cw * 3.0 + gap * 2.0) / 2.0;
    let y = rect.center().y - ch / 2.0 - 16.0;
    let purple = Color32::from_rgb(0x4c, 0x2a, 0x85);
    for (i, (_, name)) in SLOTS.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (cw + gap), y), vec2(cw, ch));
        let turned = last.is_some() && age > i as f32 * 0.15;
        p.rect_filled(r.translate(vec2(0.0, 5.0)), CornerRadius::same(12), alpha(Color32::BLACK, 110));
        if turned {
            let m = last.as_ref().map(|l| cards(l)[i]).unwrap_or(0.0);
            let c = payout_color(m);
            p.rect_filled(r, CornerRadius::same(12), mix(purple, Color32::BLACK, 0.3));
            p.rect_stroke(r.shrink(6.0), CornerRadius::same(9), Stroke::new(1.5, alpha(c, 200)), StrokeKind::Inside);
            p.text(r.center(), Align2::CENTER_CENTER, times(m), theme::font_bold((cw * 0.24).clamp(14.0, 34.0)), if m > 0.0 { c } else { MUTED });
        } else {
            p.rect_filled(r, CornerRadius::same(12), purple);
            p.rect_stroke(r.shrink(6.0), CornerRadius::same(9), Stroke::new(1.5, alpha(Color32::from_rgb(0xff, 0xd1, 0x4a), 150)), StrokeKind::Inside);
            p.circle_stroke(r.center(), cw * 0.22, Stroke::new(1.5, alpha(Color32::from_rgb(0xff, 0xd1, 0x4a), 150)));
        }
        p.text(pos2(r.center().x, r.bottom() + 16.0), Align2::CENTER_CENTER, *name, theme::font_semi(12.0), MUTED);
    }
    if let Some(l) = &last {
        if age < 0.5 {
            ui.ctx().request_repaint();
        } else {
            let c = payout_color(l.multiplier);
            p.text(pos2(rect.center().x, y + ch + 50.0), Align2::CENTER_CENTER, format!("= {}", times(l.multiplier)), theme::font_bold(22.0), if l.multiplier > 0.0 { c } else { MUTED });
        }
    }
    let _ = TEXT;
}
