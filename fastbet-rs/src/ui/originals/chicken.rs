//! Chicken: cross lane after lane; each one raises the payout, a car ends it.
//! Manual: cross and cash out. Auto: cross a set number of lanes.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Shape, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, GOLD, RED, TEXT};
use super::super::widgets::{field, Btn};
use super::super::App;
use super::{board_button, levels, times};
use crate::originals::{chicken_lanes, chicken_multiplier, Game, Round, Step, LEVELS, LEVEL_NAMES};
use crate::settings::Mode;

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Chicken).is_some();
    if levels(ui, &mut app.settings.orig.chicken_difficulty, &LEVELS, &LEVEL_NAMES, !locked && !open) {
        let max = chicken_lanes(&app.settings.orig.chicken_difficulty) as u8;
        app.settings.orig.chicken_lanes = app.settings.orig.chicken_lanes.clamp(1, max);
        app.mark_dirty();
    }
    if app.settings.mode == Mode::Auto {
        let max = chicken_lanes(&app.settings.orig.chicken_difficulty);
        let mut s = app.settings.orig.chicken_lanes.to_string();
        if field(ui, "Lanes per bet", &mut s, "3", None, !locked).changed() {
            if let Ok(n) = s.trim().parse::<u32>() {
                app.settings.orig.chicken_lanes = n.clamp(1, max) as u8;
                app.mark_dirty();
            }
        }
        let n = app.settings.orig.chicken_lanes as u32;
        super::readout(ui, format!("Cashes out after {} at {}", super::count(n as u64, "lane"), times(chicken_multiplier(&app.settings.orig.chicken_difficulty, n))));
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let d = &app.settings.orig.chicken_difficulty;
    let lanes = if auto { (app.settings.orig.chicken_lanes as u32).clamp(1, chicken_lanes(d)) } else { 0 };
    Ok(json!({ "difficulty": d, "round": lanes }))
}

pub fn result_text(r: &Round) -> String {
    let lanes = r.state["round"].as_u64().unwrap_or(0);
    if r.multiplier > 0.0 {
        format!("{lanes} lanes crossed")
    } else {
        format!("Hit on lane {}", lanes + 1)
    }
}

/// A chicken: round body, comb, beak, an eye.
fn chicken(p: &egui::Painter, c: egui::Pos2, s: f32, dead: bool) {
    let body = if dead { Color32::from_rgb(0x8a, 0x94, 0xaa) } else { Color32::WHITE };
    p.circle_filled(c, s * 0.4, body);
    p.circle_filled(c + vec2(0.0, -s * 0.42), s * 0.12, RED);
    p.add(Shape::convex_polygon(vec![c + vec2(s * 0.36, -s * 0.08), c + vec2(s * 0.58, 0.0), c + vec2(s * 0.36, s * 0.08)], GOLD, Stroke::NONE));
    p.circle_filled(c + vec2(s * 0.14, -s * 0.12), s * 0.05, Color32::BLACK);
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Chicken).cloned();
    let busy = app.orig.busy.contains(&Game::Chicken);
    let shown = open.clone().or_else(|| app.orig.last(Game::Chicken).cloned());
    let difficulty = shown.as_ref().and_then(|r| r.state["difficulty"].as_str().map(str::to_string)).unwrap_or_else(|| app.settings.orig.chicken_difficulty.clone());
    let crossed = shown.as_ref().map(|r| r.state["round"].as_u64().unwrap_or(0) as u32).unwrap_or(0);
    let dead = shown.as_ref().is_some_and(|r| !r.active && r.multiplier <= 0.0);
    let safe = chicken_lanes(&difficulty);

    // a window of lanes around the chicken
    let visible = 8u32;
    let first = crossed.saturating_sub(2).min(safe.saturating_sub(visible)) + 1;
    let road = Rect::from_min_max(pos2(rect.left() + 24.0, rect.top() + 40.0), pos2(rect.right() - 24.0, rect.bottom() - if open.is_some() { 80.0 } else { 40.0 }));
    let p = ui.painter();
    let curb = Rect::from_min_max(road.min, pos2(road.left() + road.width() / (visible as f32 + 1.0), road.max.y));
    p.rect_filled(curb, CornerRadius::same(10), Color32::from_rgb(0x2b, 0x3a, 0x2b));
    let lane_w = (road.width() - curb.width()) / visible as f32;
    for k in 0..visible {
        let n = first + k;
        if n > safe {
            break;
        }
        let lr = Rect::from_min_size(pos2(curb.right() + k as f32 * lane_w, road.top()), vec2(lane_w, road.height()));
        p.rect_filled(lr.shrink2(vec2(1.0, 0.0)), CornerRadius::ZERO, Color32::from_rgb(0x2a, 0x30, 0x3c));
        // dashes between lanes
        let mut y = lr.top() + 6.0;
        while y < lr.bottom() - 6.0 {
            p.line_segment([pos2(lr.right(), y), pos2(lr.right(), y + 10.0)], Stroke::new(2.0, alpha(Color32::WHITE, 60)));
            y += 22.0;
        }
        let done = n <= crossed && !dead;
        let m = chicken_multiplier(&difficulty, n);
        let chip = Rect::from_center_size(pos2(lr.center().x, lr.bottom() - 18.0), vec2(lane_w - 10.0, 22.0));
        super::chip(p, chip, &times(m), if done { alpha(GOLD, 70) } else { alpha(Color32::BLACK, 120) }, if done { GOLD } else { TEXT });
        if dead && n == crossed + 1 {
            // the car that got it
            let car = Rect::from_center_size(pos2(lr.center().x, lr.center().y - 20.0), vec2(lane_w * 0.6, lane_w * 0.9));
            p.rect_filled(car, CornerRadius::same(8), RED);
            p.rect_filled(car.shrink2(vec2(car.width() * 0.15, car.height() * 0.3)), CornerRadius::same(4), alpha(Color32::WHITE, 120));
        }
    }
    // the chicken stands on the curb or in the last lane it crossed
    let at = if crossed == 0 || crossed < first { curb.center().x } else { curb.right() + ((crossed - first) as f32 + 0.5) * lane_w };
    let at = if dead { curb.right() + ((crossed + 1).saturating_sub(first) as f32 + 0.5) * lane_w } else { at };
    chicken(p, pos2(at, road.center().y - 10.0), (lane_w * 0.8).min(70.0), dead);

    if open.is_some() {
        let next = chicken_multiplier(&difficulty, crossed + 1);
        let b = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - 44.0), vec2(220.0, 40.0));
        if board_button(ui, b, &format!("Cross  ·  {}", times(next)), Btn::Primary, !busy && crossed < safe) {
            app.orig_step(Game::Chicken, Step::Next, json!({}));
        }
    } else if shown.is_none() {
        super::note(ui.painter(), pos2(rect.center().x, rect.bottom() - 18.0), "Bet, then cross lane by lane");
    }
    let _ = (mix, theme::font(1.0), Align2::CENTER_CENTER);
}
