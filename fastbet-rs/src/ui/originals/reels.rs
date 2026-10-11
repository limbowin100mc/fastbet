//! Scarab Spin and Tome of Life: Stake's own five-reel slots. A spin pays its winning
//! lines (and scatters); bonus rounds play out by themselves.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG2, GOLD, MUTED, TEXT, TEXT2};
use super::super::widgets::field;
use super::super::App;
use super::{payout_color, times, trim};
use crate::originals::{Game, Round};

fn lines_of(app: &mut App, game: Game) -> &mut u8 {
    if game == Game::TomeOfLife {
        &mut app.settings.orig.tome_lines
    } else {
        &mut app.settings.orig.scarab_lines
    }
}

pub fn params(app: &mut App, ui: &mut Ui, game: Game, locked: bool) {
    let mut s = lines_of(app, game).to_string();
    if field(ui, "Lines", &mut s, "20", None, !locked).changed() {
        if let Ok(n) = s.trim().parse::<u8>() {
            *lines_of(app, game) = n.clamp(1, 20);
            app.mark_dirty();
        }
    }
    ui.label(egui::RichText::new("The bet size is spread over the lines played (1–20).").size(11.0).color(MUTED));
}

pub fn bet_params(app: &App, game: Game) -> Result<Value, String> {
    let lines = if game == Game::TomeOfLife { app.settings.orig.tome_lines } else { app.settings.orig.scarab_lines };
    Ok(json!({ "lines": lines.clamp(1, 20) }))
}

/// Every round of the spin (one, or the base spin plus bonus spins).
fn rounds(r: &Round) -> Vec<Value> {
    r.state["rounds"].as_array().cloned().unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    let n = rounds(r).len();
    let lines: usize = rounds(r).iter().map(|x| x["paylines"].as_array().map(Vec::len).unwrap_or(0)).sum();
    if n > 1 {
        format!("{lines} lines · {} bonus spins", n - 1)
    } else {
        format!("{lines} winning lines")
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect, game: Game) {
    let last = app.orig.last(game).cloned();
    let age = app.orig.age(game);
    let p = ui.painter();
    // the reels' frame with the spin's multiplier across it
    let frame = Rect::from_center_size(pos2(rect.center().x, rect.top() + rect.height() * 0.3), vec2((rect.width() - 80.0).min(560.0), (rect.height() * 0.4).min(200.0)));
    let gold = Color32::from_rgb(0xc9, 0x9a, 0x3a);
    p.rect_filled(frame.expand(6.0), CornerRadius::same(16), mix(gold, Color32::BLACK, 0.45));
    p.rect_filled(frame, CornerRadius::same(12), Color32::from_rgb(0x1a, 0x14, 0x0c));
    let rw = frame.width() / 5.0;
    for i in 1..5 {
        p.vline(frame.left() + rw * i as f32, frame.y_range(), egui::Stroke::new(1.0, alpha(gold, 60)));
    }
    let spinning = last.is_some() && age < 0.5;
    match &last {
        Some(r) if !spinning => {
            let m = r.multiplier;
            let c = if m > 0.0 { payout_color(m) } else { MUTED };
            p.text(frame.center(), Align2::CENTER_CENTER, times(m), theme::font_bold((frame.height() * 0.32).clamp(20.0, 52.0)), c);
        }
        Some(_) => {
            // a blur of symbols while it lands
            for i in 0..5 {
                let x = frame.left() + rw * (i as f32 + 0.5);
                let off = (age * 900.0 + i as f32 * 37.0) % 40.0;
                for k in 0..6 {
                    let y = frame.top() + k as f32 * 40.0 - off;
                    if y > frame.top() && y < frame.bottom() - 10.0 {
                        p.circle_filled(pos2(x, y), 9.0, alpha(GOLD, 60));
                    }
                }
            }
            ui.ctx().request_repaint();
        }
        None => super::note(p, frame.center(), "Spin"),
    }

    // the winning lines
    let list: Vec<Value> = last.as_ref().map(rounds).unwrap_or_default().iter().flat_map(|x| x["paylines"].as_array().cloned().unwrap_or_default()).collect();
    let top = frame.bottom() + 24.0;
    let row_h = 22.0;
    let max_rows = ((rect.bottom() - top - 10.0) / row_h) as usize;
    if !spinning {
        if let Some(r) = &last {
            let rounds = rounds(r);
            let scatter: f64 = rounds.iter().filter_map(|x| x["scatterMultiplier"].as_f64()).sum();
            let mut y = top;
            if rounds.len() > 1 {
                p.text(pos2(rect.center().x, y), Align2::CENTER_CENTER, format!("{} bonus spins", rounds.len() - 1), theme::font_semi(13.0), GOLD);
                y += row_h;
            }
            if scatter > 0.0 {
                p.text(pos2(rect.center().x, y), Align2::CENTER_CENTER, format!("Scatter ×{}", trim(scatter)), theme::font_semi(12.5), GOLD);
                y += row_h;
            }
            for line in list.iter().take(max_rows.saturating_sub(2)) {
                let name = line["payline"].as_str().unwrap_or("line").replace("line", "Line ");
                let sym = line["symbol"].as_str().unwrap_or("");
                let hits = line["hits"].as_u64().unwrap_or(0);
                let m = line["multiplier"].as_f64().unwrap_or(0.0);
                let row = Rect::from_center_size(pos2(rect.center().x, y), vec2(360.0f32.min(rect.width() - 40.0), row_h - 4.0));
                p.rect_filled(row, CornerRadius::same(5), BG2);
                p.text(pos2(row.left() + 10.0, row.center().y), Align2::LEFT_CENTER, format!("{name}  ·  {hits}× {sym}"), theme::font(11.5), TEXT2);
                p.text(pos2(row.right() - 10.0, row.center().y), Align2::RIGHT_CENTER, format!("×{}", trim(m)), theme::font_semi(11.5), payout_color(m));
                y += row_h;
            }
            if list.is_empty() && scatter <= 0.0 {
                p.text(pos2(rect.center().x, y), Align2::CENTER_CENTER, "No winning lines", theme::font(12.0), MUTED);
            }
        }
    }
    let _ = TEXT;
}
