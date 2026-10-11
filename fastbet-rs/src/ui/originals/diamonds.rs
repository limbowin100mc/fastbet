//! Diamonds: five gems are drawn; pairs and better pay, like poker hands.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};

use super::super::theme::{self, alpha, BG2, LINE, MUTED, TEXT, TEXT2};
use super::super::App;
use super::{gem, times};
use crate::originals::{diamonds_hand, Game, Round, DIAMONDS_PAYTABLE};

pub fn params(_app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("No settings: five gems are drawn, and a pair or better pays.").size(11.0).color(MUTED));
}

fn hand(r: &Round) -> Vec<String> {
    r.state["hand"].as_array().map(|a| a.iter().filter_map(|c| c.as_str()).map(str::to_string).collect()).unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    diamonds_hand(&hand(r)).0.to_string()
}

fn gem_color(name: &str) -> Color32 {
    match name {
        "green" => Color32::from_rgb(0x2b, 0xe4, 0x7a),
        "purple" => Color32::from_rgb(0xa8, 0x6b, 0xff),
        "yellow" => Color32::from_rgb(0xff, 0xd1, 0x4a),
        "red" => Color32::from_rgb(0xff, 0x3d, 0x5f),
        "lightBlue" | "cyan" => Color32::from_rgb(0x5b, 0xd8, 0xff),
        "pink" => Color32::from_rgb(0xff, 0x7a, 0xd9),
        "blue" => Color32::from_rgb(0x4b, 0x7b, 0xff),
        "orange" => Color32::from_rgb(0xff, 0x8a, 0x3d),
        _ => Color32::from_rgb(0xc7, 0xcf, 0xdf),
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::Diamonds).cloned();
    let age = app.orig.age(Game::Diamonds);
    let gems = last.as_ref().map(hand).unwrap_or_default();
    let won = gems.len() == 5;
    let (name, _) = if won { diamonds_hand(&gems) } else { ("", 0.0) };

    let p = ui.painter();
    let slot = ((rect.width() - 80.0) / 5.0 - 12.0).min(rect.height() * 0.32).min(120.0);
    let x0 = rect.center().x - (slot + 12.0) * 2.5 + 6.0;
    let y = rect.top() + 26.0;
    for i in 0..5 {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (slot + 12.0), y), vec2(slot, slot));
        p.rect(r, CornerRadius::same(12), BG2, Stroke::new(1.0, LINE), StrokeKind::Inside);
        // gems drop in one by one
        let show = won && age > i as f32 * 0.08;
        if show {
            let c = gem_color(&gems[i]);
            let t = ((age - i as f32 * 0.08) / 0.15).clamp(0.0, 1.0);
            gem(p, pos2(r.center().x, r.center().y - (1.0 - t) * 14.0), slot * 0.62, c);
            // the gems that make the hand get a glow under them
            if gems.iter().filter(|g| *g == &gems[i]).count() > 1 {
                p.rect_filled(Rect::from_center_size(pos2(r.center().x, r.bottom() - 6.0), vec2(slot * 0.6, 4.0)), CornerRadius::same(2), alpha(c, 200));
            }
        }
    }
    if won && age < 0.6 {
        ui.ctx().request_repaint();
    }

    // the paytable
    let rows = DIAMONDS_PAYTABLE.len() as f32;
    let top = y + slot + 24.0;
    let row_h = ((rect.bottom() - 16.0 - top) / rows).clamp(18.0, 30.0);
    let w = (slot + 12.0) * 5.0 - 12.0;
    for (i, (hand_name, m)) in DIAMONDS_PAYTABLE.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0, top + i as f32 * row_h), vec2(w, row_h - 4.0));
        let on = *hand_name == name && age > 0.4;
        let c = super::payout_color(*m);
        p.rect_filled(r, CornerRadius::same(6), if on { alpha(c, 50) } else { alpha(Color32::WHITE, 4) });
        if on {
            p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, alpha(c, 160)), StrokeKind::Inside);
        }
        p.text(pos2(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, *hand_name, theme::font_semi(12.0), if on { TEXT } else { TEXT2 });
        p.text(pos2(r.right() - 12.0, r.center().y), Align2::RIGHT_CENTER, times(*m), theme::font_semi(12.0), if *m > 0.0 { c } else { MUTED });
    }
}
