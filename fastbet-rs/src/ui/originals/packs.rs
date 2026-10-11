//! Packs: open a pack of five cards; their multipliers add up.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG2, LINE, MUTED};
use super::super::App;
use super::{payout_color, times};
use crate::originals::{Game, Round};

pub fn params(_app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("No settings: a pack holds five cards, and their multipliers add up.").size(11.0).color(MUTED));
}

pub fn bet_params() -> Result<Value, String> {
    Ok(json!({}))
}

fn cards(r: &Round) -> Vec<(f64, bool)> {
    r.state["cards"].as_array().map(|a| a.iter().map(|c| (c["multiplier"].as_f64().unwrap_or(0.0), c["isNew"].as_bool().unwrap_or(false))).collect()).unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    let best = cards(r).iter().map(|c| c.0).fold(0.0, f64::max);
    format!("best card {}", times(best))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::Packs).cloned();
    let age = app.orig.age(Game::Packs);
    let p = ui.painter();
    let n = 5.0;
    let cw = ((rect.width() - 80.0) / n - 14.0).min(rect.height() * 0.42).min(140.0);
    let ch = cw * 1.4;
    let x0 = rect.center().x - (cw * n + 14.0 * (n - 1.0)) / 2.0;
    let y = rect.center().y - ch / 2.0 - 14.0;
    let list = last.as_ref().map(cards).unwrap_or_default();
    for i in 0..5 {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (cw + 14.0), y), vec2(cw, ch));
        let shown = list.get(i).filter(|_| age > i as f32 * 0.1);
        p.rect_filled(r.translate(vec2(0.0, 5.0)), CornerRadius::same(10), alpha(Color32::BLACK, 110));
        match shown {
            Some((m, new)) => {
                let c = payout_color(*m);
                p.rect_filled(r, CornerRadius::same(10), mix(c, Color32::BLACK, 0.55));
                p.rect_stroke(r, CornerRadius::same(10), Stroke::new(2.0, c), StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, times(*m), theme::font_bold((cw * 0.22).clamp(12.0, 28.0)), c);
                if *new {
                    p.text(pos2(r.center().x, r.top() + 14.0), Align2::CENTER_CENTER, "NEW", theme::font_bold(10.0), Color32::WHITE);
                }
            }
            None => {
                p.rect(r, CornerRadius::same(10), BG2, Stroke::new(1.0, LINE), StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, "?", theme::font_bold(cw * 0.3), alpha(MUTED, 120));
            }
        }
    }
    if let Some(l) = &last {
        if age < 0.6 {
            ui.ctx().request_repaint();
        } else {
            p.text(pos2(rect.center().x, y + ch + 34.0), Align2::CENTER_CENTER, format!("Total {}", times(l.multiplier)), theme::font_bold(20.0), payout_color(l.multiplier));
        }
    }
}
