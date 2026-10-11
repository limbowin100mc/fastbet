//! Blue Samurai: Stake's own slot with uneven reels (3-4-4-4-3). Bonus spins play out by themselves.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG2, GOLD, MUTED, TEXT2};
use super::super::App;
use super::{payout_color, times, trim};
use crate::originals::{Game, Round};

pub fn params(_app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("No settings: spin, and bonus spins play out by themselves.").size(11.0).color(MUTED));
}

pub fn bet_params() -> Result<Value, String> {
    Ok(json!({}))
}

fn rounds(r: &Round) -> Vec<Value> {
    r.state["rounds"].as_array().cloned().unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    let n = rounds(r).len();
    if n > 1 {
        format!("{} bonus spins", n - 1)
    } else {
        "base spin".into()
    }
}

/// Symbol colours: the samurai gold, the rest along the spectrum.
fn symbol(sym: &str) -> (String, Color32) {
    if sym.contains("samurai") {
        return ("S".into(), GOLD);
    }
    let Ok(n) = sym.trim_start_matches('s').parse::<u32>() else {
        // a named symbol (wild, scatter …): its first letters
        let short: String = sym.chars().take(2).collect::<String>().to_uppercase();
        return (short, Color32::from_rgb(0xff, 0xff, 0xff));
    };
    let palette = [
        Color32::from_rgb(0x5b, 0x8d, 0xef),
        Color32::from_rgb(0x2b, 0xe4, 0xa6),
        Color32::from_rgb(0xa8, 0x6b, 0xff),
        Color32::from_rgb(0xff, 0x8a, 0x3d),
        Color32::from_rgb(0xff, 0x3d, 0x5f),
        Color32::from_rgb(0x5b, 0xd8, 0xff),
        Color32::from_rgb(0xb6, 0xe3, 0x3a),
        Color32::from_rgb(0xff, 0x7a, 0xd9),
        Color32::from_rgb(0xc7, 0xcf, 0xdf),
    ];
    (n.to_string(), palette[(n as usize).saturating_sub(1) % palette.len()])
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::BlueSamurai).cloned();
    let age = app.orig.age(Game::BlueSamurai);
    let p = ui.painter();
    let Some(r) = last else {
        super::note(p, rect.center(), "Spin");
        return;
    };
    let all = rounds(&r);
    // the spin shown: the last round played (the final bonus spin, or the base spin)
    let shown = all.last().cloned().unwrap_or(Value::Null);
    let view: Vec<Vec<String>> = shown["data"]["view"].as_array().map(|reels| reels.iter().map(|reel| reel.as_array().map(|s| s.iter().filter_map(|x| x.as_str()).map(str::to_string).collect()).unwrap_or_default()).collect()).unwrap_or_default();
    let rows = view.iter().map(Vec::len).max().unwrap_or(4).max(1) as f32;
    let cell = ((rect.height() * 0.62) / rows).min((rect.width() - 80.0) / view.len().max(5) as f32).min(70.0);
    let total_w = cell * view.len() as f32 + 8.0 * (view.len() as f32 - 1.0);
    let x0 = rect.center().x - total_w / 2.0;
    let top = rect.top() + 20.0;
    for (i, reel) in view.iter().enumerate() {
        let x = x0 + i as f32 * (cell + 8.0);
        let pad = (rows - reel.len() as f32) * cell / 2.0;
        let landed = age > i as f32 * 0.08;
        for (k, sym) in reel.iter().enumerate() {
            let rr = Rect::from_min_size(pos2(x, top + pad + k as f32 * cell), vec2(cell, cell)).shrink(3.0);
            let (label, c) = symbol(sym);
            p.rect_filled(rr, CornerRadius::same(8), if landed { mix(c, Color32::BLACK, 0.55) } else { BG2 });
            if landed {
                p.text(rr.center(), Align2::CENTER_CENTER, label, theme::font_bold(cell * 0.4), c);
            }
        }
    }
    if age < 0.5 {
        ui.ctx().request_repaint();
    }
    let y = top + rows * cell + 22.0;
    let c = if r.multiplier > 0.0 { payout_color(r.multiplier) } else { MUTED };
    p.text(pos2(rect.center().x, y), Align2::CENTER_CENTER, times(r.multiplier), theme::font_bold(24.0), c);
    if all.len() > 1 {
        p.text(pos2(rect.center().x, y + 26.0), Align2::CENTER_CENTER, format!("{} bonus spins played", all.len() - 1), theme::font_semi(12.5), GOLD);
    }
    let lines: usize = all.iter().map(|x| x["data"]["paylines"].as_array().map(Vec::len).unwrap_or(0)).sum();
    if lines > 0 {
        p.text(pos2(rect.center().x, y + 48.0), Align2::CENTER_CENTER, format!("{lines} winning lines"), theme::font(12.0), TEXT2);
    }
    let _ = (alpha(GOLD, 1), trim(1.0));
}
