//! Rock Paper Scissors against the house; every win multiplies the payout.
//! Manual: throw hand by hand and cash out. Auto: set the throws up front.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Pos2, Rect, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GOLD, GREEN, MUTED, RED, TEXT2};
use super::super::widgets::{caption, chip_button_sized, Btn};
use super::super::App;
use super::{board_button, times};
use crate::originals::{Game, Round, Step};
use crate::settings::Mode;

const HANDS: [(&str, &str); 3] = [("rock", "Rock"), ("paper", "Paper"), ("scissors", "Scissors")];
const MAX_CALLS: usize = 20;

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect()).unwrap_or_default()
}

/// A hand: a fist, a flat hand or a V.
fn hand(p: &egui::Painter, c: Pos2, s: f32, which: &str, color: Color32) {
    match which {
        "paper" => {
            p.rect_filled(Rect::from_center_size(c, vec2(s * 0.7, s * 0.85)), CornerRadius::same((s * 0.12) as u8), color);
            for k in 0..3 {
                let y = c.y - s * 0.2 + k as f32 * s * 0.2;
                p.line_segment([pos2(c.x - s * 0.22, y), pos2(c.x + s * 0.22, y)], Stroke::new(1.5, alpha(Color32::BLACK, 90)));
            }
        }
        "scissors" => {
            p.line_segment([c + vec2(-s * 0.3, -s * 0.4), c + vec2(s * 0.1, s * 0.15)], Stroke::new(s * 0.12, color));
            p.line_segment([c + vec2(s * 0.3, -s * 0.4), c + vec2(-s * 0.1, s * 0.15)], Stroke::new(s * 0.12, color));
            p.circle_stroke(c + vec2(-s * 0.16, s * 0.28), s * 0.13, Stroke::new(s * 0.08, color));
            p.circle_stroke(c + vec2(s * 0.16, s * 0.28), s * 0.13, Stroke::new(s * 0.08, color));
        }
        _ => {
            p.circle_filled(c, s * 0.38, color);
            p.circle_filled(c - vec2(s * 0.12, s * 0.12), s * 0.1, alpha(Color32::WHITE, 70));
        }
    }
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if app.settings.mode != Mode::Auto {
        ui.label(egui::RichText::new("Bet, then throw a hand each round. Cash out any time.").size(11.0).color(MUTED));
        return;
    }
    caption(ui, "Throws, in order");
    let calls = app.settings.orig.rps_calls.clone();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, c) in calls.iter().enumerate() {
            let label = match c.as_str() {
                "paper" => "P",
                "scissors" => "S",
                _ => "R",
            };
            if chip_button_sized(ui, label, true, !locked, vec2(28.0, 28.0)).on_hover_text("Click to change").clicked() {
                let next = match c.as_str() {
                    "rock" => "paper",
                    "paper" => "scissors",
                    _ => "rock",
                };
                app.settings.orig.rps_calls[i] = next.into();
                changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 6.0) / 2.0;
        if chip_button_sized(ui, "+ throw", false, !locked && calls.len() < MAX_CALLS, vec2(w, 28.0)).clicked() {
            app.settings.orig.rps_calls.push("rock".into());
            changed = true;
        }
        if chip_button_sized(ui, "− throw", false, !locked && calls.len() > 1, vec2(w, 28.0)).clicked() {
            app.settings.orig.rps_calls.pop();
            changed = true;
        }
    });
    if changed {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    if !auto {
        return Ok(json!({}));
    }
    let calls = &app.settings.orig.rps_calls;
    if calls.is_empty() {
        return Err("Add at least one throw".into());
    }
    Ok(json!({ "guesses": calls }))
}

pub fn result_text(r: &Round) -> String {
    let n = strs(&r.state["playerRounds"]).len();
    if r.multiplier > 0.0 {
        format!("Cashed out after {n} throws")
    } else {
        format!("Lost on throw {n}")
    }
}

/// Who won a throw: 1 player, 0 draw, -1 house.
fn outcome(player: &str, house: &str) -> i32 {
    match (player, house) {
        (a, b) if a == b => 0,
        ("rock", "scissors") | ("paper", "rock") | ("scissors", "paper") => 1,
        _ => -1,
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::RockPaperScissors).cloned();
    let busy = app.orig.busy.contains(&Game::RockPaperScissors);
    let shown = open.clone().or_else(|| app.orig.last(Game::RockPaperScissors).cloned());
    let player = shown.as_ref().map(|r| strs(&r.state["playerRounds"])).unwrap_or_default();
    let house = shown.as_ref().map(|r| strs(&r.state["rounds"])).unwrap_or_default();
    let p = ui.painter();

    // the latest throw, large: you on the left, the house on the right
    let s = (rect.height() * 0.24).clamp(50.0, 110.0);
    let cy = rect.top() + 24.0 + s * 0.6;
    if let (Some(a), Some(b)) = (player.last(), house.last()) {
        let o = outcome(a, b);
        let (ca, cb) = match o {
            1 => (GREEN, TEXT2),
            -1 => (TEXT2, RED),
            _ => (GOLD, GOLD),
        };
        hand(p, pos2(rect.center().x - s * 1.2, cy), s, a, ca);
        hand(p, pos2(rect.center().x + s * 1.2, cy), s, b, cb);
        p.text(pos2(rect.center().x, cy), Align2::CENTER_CENTER, "vs", theme::font_bold(18.0), MUTED);
        p.text(pos2(rect.center().x - s * 1.2, cy + s * 0.7), Align2::CENTER_CENTER, "You", theme::font_semi(11.0), MUTED);
        p.text(pos2(rect.center().x + s * 1.2, cy + s * 0.7), Align2::CENTER_CENTER, "House", theme::font_semi(11.0), MUTED);
    }
    if let Some(r) = &shown {
        let m = if r.active { r.state["payoutMultiplier"].as_f64().unwrap_or(1.0) } else { r.multiplier };
        p.text(pos2(rect.center().x, cy + s * 0.7 + 22.0), Align2::CENTER_CENTER, times(m), theme::font_bold(20.0), if m > 0.0 { GREEN } else { RED });
    }

    // throw buttons while a round is open
    if open.is_some() {
        let row = Rect::from_center_size(pos2(rect.center().x, cy + s * 0.7 + 66.0), vec2((rect.width() - 60.0).min(420.0), 40.0));
        let w = (row.width() - 16.0) / 3.0;
        for (i, (key, label)) in HANDS.iter().enumerate() {
            let b = Rect::from_min_size(pos2(row.left() + i as f32 * (w + 8.0), row.top()), vec2(w, 40.0));
            if board_button(ui, b, label, Btn::Primary, !busy) {
                app.orig_step(Game::RockPaperScissors, Step::Next, json!({ "guess": key }));
            }
        }
    } else if shown.is_none() {
        super::note(p, pos2(rect.center().x, rect.center().y), "Bet, then throw");
    }

    // every throw of the round
    let p = ui.painter();
    let small = 26.0;
    let n = player.len().min(((rect.width() - 40.0) / (small + 10.0)) as usize);
    let skip = player.len() - n;
    let x0 = rect.center().x - (n as f32 * (small + 10.0)) / 2.0 + small / 2.0;
    let y = rect.bottom() - 46.0;
    for (k, (a, b)) in player.iter().zip(house.iter()).skip(skip).enumerate() {
        let x = x0 + k as f32 * (small + 10.0);
        let c = match outcome(a, b) {
            1 => GREEN,
            -1 => RED,
            _ => GOLD,
        };
        hand(p, pos2(x, y), small, a, c);
        hand(p, pos2(x, y + small + 4.0), small * 0.7, b, alpha(TEXT2, 160));
    }
}
