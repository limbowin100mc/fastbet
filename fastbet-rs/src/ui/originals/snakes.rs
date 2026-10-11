//! Snakes: roll two dice; the total multiplies the payout, a snake total ends it.
//! Manual: roll and cash out. Auto: roll a set number of times.

use egui::{pos2, vec2, Align2, Color32, Rect, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GREEN, MUTED, RED, TEXT};
use super::super::widgets::{field, Btn};
use super::super::App;
use super::{board_button, die, levels, payout_color, text_color_on, times, trim};
use crate::originals::{snakes_payout, Game, Round, Step, SNAKES_LEVELS};
use crate::settings::Mode;

const NAMES: [&str; 5] = ["Easy", "Medium", "Hard", "Expert", "Master"];

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Snakes).is_some();
    if levels(ui, &mut app.settings.orig.snakes_difficulty, &SNAKES_LEVELS, &NAMES, !locked && !open) {
        app.mark_dirty();
    }
    if app.settings.mode == Mode::Auto {
        let mut s = app.settings.orig.snakes_rolls.to_string();
        if field(ui, "Rolls per bet", &mut s, "3", None, !locked).changed() {
            if let Ok(n) = s.trim().parse::<u32>() {
                app.settings.orig.snakes_rolls = n.clamp(1, 20) as u8;
                app.mark_dirty();
            }
        }
        ui.label(egui::RichText::new("Cashes out after that many rolls without a snake.").size(11.0).color(MUTED));
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let rolls = if auto { app.settings.orig.snakes_rolls.clamp(1, 20) } else { 0 };
    Ok(json!({ "difficulty": app.settings.orig.snakes_difficulty, "rollCount": rolls }))
}

/// The rolls that happened: an auto round lists the planned ones after a snake too (all 0×).
fn rolls(r: &Round) -> Vec<(u32, u32, f64)> {
    let mut all = planned(r);
    if let Some(snake) = all.iter().position(|x| x.2 <= 0.0) {
        all.truncate(snake + 1);
    }
    all
}

fn planned(r: &Round) -> Vec<(u32, u32, f64)> {
    r.state["rounds"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| {
                    let d = super::nums(&x["roll"]);
                    (d.first().copied().unwrap_or(1) as u32, d.get(1).copied().unwrap_or(1) as u32, x["payoutMultiplier"].as_f64().unwrap_or(0.0))
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    let n = rolls(r).len();
    if r.multiplier > 0.0 {
        format!("{n} rolls")
    } else {
        format!("Snake on roll {n}")
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Snakes).cloned();
    let busy = app.orig.busy.contains(&Game::Snakes);
    let shown = open.clone().or_else(|| app.orig.last(Game::Snakes).cloned());
    let difficulty = shown.as_ref().and_then(|r| r.state["difficulty"].as_str().map(str::to_string)).unwrap_or_else(|| app.settings.orig.snakes_difficulty.clone());
    let list = shown.as_ref().map(rolls).unwrap_or_default();
    let age = app.orig.age(Game::Snakes);
    let p = ui.painter();

    // the latest roll, big
    let ds = (rect.height() * 0.2).clamp(44.0, 86.0);
    let cy = rect.top() + 24.0 + ds / 2.0;
    match list.last() {
        Some((a, b, m)) => {
            let shake = if age < 0.25 { (age * 60.0).sin() * 4.0 } else { 0.0 };
            die(p, Rect::from_center_size(pos2(rect.center().x - ds * 0.6 + shake, cy), vec2(ds, ds)), *a, Color32::WHITE);
            die(p, Rect::from_center_size(pos2(rect.center().x + ds * 0.6 - shake, cy), vec2(ds, ds)), *b, Color32::WHITE);
            let total = a + b;
            let pay = snakes_payout(&difficulty, total);
            let txt = if pay > 0.0 { format!("{total}  ·  ×{}  ·  {}", trim(pay), times(*m)) } else { format!("{total}  ·  snake") };
            p.text(pos2(rect.center().x, cy + ds * 0.5 + 18.0), Align2::CENTER_CENTER, txt, theme::font_bold(15.0), if pay > 0.0 { GREEN } else { RED });
            if age < 0.25 {
                ui.ctx().request_repaint();
            }
        }
        None => {
            die(p, Rect::from_center_size(pos2(rect.center().x - ds * 0.6, cy), vec2(ds, ds)), 6, alpha(Color32::WHITE, 60));
            die(p, Rect::from_center_size(pos2(rect.center().x + ds * 0.6, cy), vec2(ds, ds)), 6, alpha(Color32::WHITE, 60));
        }
    }

    // what every total pays on this difficulty
    let strip = Rect::from_min_max(pos2(rect.left() + 30.0, cy + ds * 0.5 + 44.0), pos2(rect.right() - 30.0, cy + ds * 0.5 + 66.0));
    let w = (strip.width() - 4.0 * 10.0) / 11.0;
    let last_total = list.last().map(|(a, b, _)| a + b);
    for (i, total) in (2..=12u32).enumerate() {
        let cell = Rect::from_min_size(pos2(strip.left() + i as f32 * (w + 4.0), strip.top()), vec2(w, 22.0));
        let pay = snakes_payout(&difficulty, total);
        let c = if pay > 0.0 { payout_color(pay) } else { RED };
        let on = last_total == Some(total);
        super::chip(p, cell, &if pay > 0.0 { format!("×{}", trim(pay)) } else { "snake".into() }, if on { c } else { alpha(c, 40) }, if on { text_color_on(c) } else { TEXT });
        p.text(pos2(cell.center().x, cell.bottom() + 10.0), Align2::CENTER_CENTER, format!("{total}"), theme::font(10.0), MUTED);
    }

    // the rolls so far
    let hy = strip.bottom() + 46.0;
    let small = 22.0;
    let n = list.len().min(((rect.width() - 60.0) / (small * 2.0 + 18.0)) as usize);
    let skip = list.len() - n;
    let total_w = n as f32 * (small * 2.0 + 18.0);
    let x0 = rect.center().x - total_w / 2.0;
    for (k, (a, b, m)) in list.iter().skip(skip).enumerate() {
        let x = x0 + k as f32 * (small * 2.0 + 18.0);
        die(p, Rect::from_min_size(pos2(x, hy), vec2(small, small)), *a, Color32::WHITE);
        die(p, Rect::from_min_size(pos2(x + small + 3.0, hy), vec2(small, small)), *b, Color32::WHITE);
        p.text(pos2(x + small + 1.5, hy + small + 10.0), Align2::CENTER_CENTER, if *m > 0.0 { times(*m) } else { "×".into() }, theme::font_semi(10.0), if *m > 0.0 { GREEN } else { RED });
    }

    if open.is_some() {
        let b = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 30.0), vec2(200.0, 40.0));
        if board_button(ui, b, "Roll", Btn::Primary, !busy) {
            app.orig_step(Game::Snakes, Step::Next, json!({}));
        }
    } else if shown.is_none() {
        super::note(ui.painter(), pos2(rect.center().x, rect.bottom() - 18.0), "Bet, then roll");
    }
}
