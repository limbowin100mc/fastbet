//! Pump: pump a balloon; every pump raises the payout until it pops.
//! Manual: pump and cash out. Auto: pump a set number of times.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Shape, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG2, GOLD, MUTED, TEXT};
use super::super::widgets::{button, caption, field, segmented, Btn};
use super::super::App;
use super::{chip, times};
use crate::originals::{pump_multiplier, pump_pops, Game, Round, Step};
use crate::settings::Mode;

const LEVELS: [&str; 4] = ["easy", "medium", "hard", "expert"];

fn max_pumps(difficulty: &str) -> u32 {
    25 - pump_pops(difficulty)
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Pump).is_some();
    caption(ui, "Difficulty");
    let sel = LEVELS.iter().position(|d| *d == app.settings.orig.pump_difficulty).unwrap_or(0);
    if let Some(i) = segmented(ui, &["Easy", "Medium", "Hard", "Expert"], sel, !locked && !open) {
        app.settings.orig.pump_difficulty = LEVELS[i].into();
        let max = max_pumps(LEVELS[i]) as u8;
        app.settings.orig.pump_pumps = app.settings.orig.pump_pumps.min(max);
        app.mark_dirty();
    }
    if app.settings.mode == Mode::Auto {
        let mut s = app.settings.orig.pump_pumps.to_string();
        let max = max_pumps(&app.settings.orig.pump_difficulty);
        if field(ui, "Pumps per bet", &mut s, "3", None, !locked).changed() {
            if let Ok(n) = s.trim().parse::<u32>() {
                app.settings.orig.pump_pumps = n.clamp(1, max) as u8;
                app.mark_dirty();
            }
        }
        let n = app.settings.orig.pump_pumps as u32;
        super::readout(ui, format!("Cashes out after {} at {}", super::count(n as u64, "pump"), times(pump_multiplier(&app.settings.orig.pump_difficulty, n))));
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let d = &app.settings.orig.pump_difficulty;
    if !auto {
        return Ok(json!({ "difficulty": d, "round": 0 }));
    }
    let n = (app.settings.orig.pump_pumps as u32).clamp(1, max_pumps(d));
    Ok(json!({ "difficulty": d, "round": n }))
}

pub fn result_text(r: &Round) -> String {
    let pumps = r.state["round"].as_u64().unwrap_or(0);
    if r.multiplier > 0.0 {
        format!("{pumps} pumps")
    } else {
        format!("Popped on pump {}", pumps + 1)
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Pump).cloned();
    let busy = app.orig.busy.contains(&Game::Pump);
    let shown = open.clone().or_else(|| app.orig.last(Game::Pump).cloned());
    let difficulty = shown.as_ref().and_then(|r| r.state["difficulty"].as_str().map(str::to_string)).unwrap_or_else(|| app.settings.orig.pump_difficulty.clone());
    let pumps = shown.as_ref().map(|r| r.state["round"].as_u64().unwrap_or(0) as u32).unwrap_or(0);
    let popped = shown.as_ref().is_some_and(|r| !r.active && r.multiplier <= 0.0);
    let age = app.orig.age(Game::Pump);

    let ladder_h = 54.0;
    let stage = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - ladder_h - (if open.is_some() { 56.0 } else { 10.0 })));
    let max = max_pumps(&difficulty);
    let p = ui.painter();
    let grow = (pumps as f32 / max as f32).sqrt();
    let radius = (stage.height() * 0.18 + stage.height() * 0.2 * grow).min(stage.width() * 0.3);
    let c = pos2(stage.center().x, stage.center().y - 4.0);
    let red = Color32::from_rgb(0xff, 0x3d, 0x5f);
    if popped {
        // burst: shards flying out
        for i in 0..10 {
            let a = i as f32 / 10.0 * std::f32::consts::TAU;
            let d = radius * (0.6 + (age * 3.0).min(0.8));
            let q = pos2(c.x + d * a.cos(), c.y + d * a.sin());
            p.add(Shape::convex_polygon(vec![q, q + vec2(8.0 * (a + 0.5).cos(), 8.0 * (a + 0.5).sin()), q + vec2(10.0 * (a - 0.4).cos(), 10.0 * (a - 0.4).sin())], alpha(red, 200), Stroke::NONE));
        }
        p.text(c, Align2::CENTER_CENTER, "POP", theme::font_bold(34.0), red);
        if age < 0.5 {
            ui.ctx().request_repaint();
        }
    } else {
        let pulse = if age < 0.2 { 1.0 + 0.06 * (1.0 - age / 0.2) } else { 1.0 };
        let r = radius * pulse;
        p.line_segment([pos2(c.x, c.y + r), pos2(c.x, stage.bottom())], Stroke::new(1.5, alpha(TEXT, 80)));
        p.add(Shape::convex_polygon(vec![pos2(c.x - 7.0, c.y + r + 8.0), pos2(c.x + 7.0, c.y + r + 8.0), pos2(c.x, c.y + r - 2.0)], mix(red, Color32::BLACK, 0.3), Stroke::NONE));
        p.circle_filled(c, r, red);
        p.circle_filled(c - vec2(r * 0.35, r * 0.4), r * 0.18, alpha(Color32::WHITE, 120));
        let m = shown.as_ref().map(|r| r.state["payoutMultiplier"].as_f64().unwrap_or(1.0)).unwrap_or(1.0);
        p.text(c, Align2::CENTER_CENTER, times(if pumps == 0 { 1.0 } else { m }), theme::font_bold((r * 0.36).clamp(16.0, 40.0)), Color32::WHITE);
        if age < 0.2 {
            ui.ctx().request_repaint();
        }
    }

    // the Pump button while a round is open
    if open.is_some() {
        let b = Rect::from_center_size(pos2(rect.center().x, stage.bottom() + 26.0), vec2(200.0, 40.0));
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(b));
        let next = pump_multiplier(&difficulty, pumps + 1);
        if button(&mut bui, &format!("Pump  ·  {}", times(next)), Btn::Primary, !busy && pumps < max, Some(200.0)).clicked() {
            app.orig_step(Game::Pump, Step::Next, json!({}));
        }
    }

    // the multiplier ladder
    let p = ui.painter();
    let shown_n = max.min(12);
    let first = pumps.saturating_sub(4).min(max.saturating_sub(shown_n)) + 1;
    let strip = Rect::from_min_max(pos2(rect.left() + 24.0, rect.bottom() - ladder_h), pos2(rect.right() - 24.0, rect.bottom() - 14.0));
    let w = (strip.width() - 4.0 * (shown_n as f32 - 1.0)) / shown_n as f32;
    for k in 0..shown_n {
        let n = first + k;
        let cell = Rect::from_min_size(pos2(strip.left() + k as f32 * (w + 4.0), strip.top()), vec2(w, 22.0));
        let done = n <= pumps && !popped;
        let fill = if done { alpha(GOLD, 60) } else { BG2 };
        chip(p, cell, &times(pump_multiplier(&difficulty, n)), fill, if done { GOLD } else { TEXT });
        p.text(pos2(cell.center().x, cell.bottom() + 10.0), Align2::CENTER_CENTER, format!("{n}"), theme::font(10.0), MUTED);
    }
    let _ = CornerRadius::ZERO;
}
