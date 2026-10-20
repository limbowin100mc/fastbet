//! Chicken: cross lane after lane; each one raises the payout, a car ends it.
//! Manual: cross and cash out. Auto: cross a set number of lanes.

use egui::emath::GuiRounding as _;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::logos::{self, Icon};
use super::super::theme::{self, alpha, mix, BG2, BG3, DIM, G1, LINE2, MUTED, RED, TEXT};
use super::super::widgets::{caption, fit_text, select, Btn};
use super::super::App;
use super::{board_button, levels, miss, times, times_full, times_short};
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
        // every lane count the level allows, with what cashing out there pays
        caption(ui, "Lanes per bet");
        let d = app.settings.orig.chicken_difficulty.clone();
        let max = chicken_lanes(&d);
        let items: Vec<String> = (1..=max).map(|n| format!("{n}  -  {}", times_full(chicken_multiplier(&d, n)))).collect();
        let sel = (app.settings.orig.chicken_lanes as u32).clamp(1, max) as usize - 1;
        if let Some(i) = select(ui, "chicken-lanes", &items, sel, !locked) {
            app.settings.orig.chicken_lanes = (i + 1) as u8;
            app.mark_dirty();
        }
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

const PURPLE: Color32 = Color32::from_rgb(0x8b, 0x5c, 0xf6);

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Chicken).cloned();
    let busy = app.orig.busy.contains(&Game::Chicken);
    let shown = open.clone().or_else(|| app.orig.last(Game::Chicken).cloned());
    let difficulty = shown.as_ref().and_then(|r| r.state["difficulty"].as_str().map(str::to_string)).unwrap_or_else(|| app.settings.orig.chicken_difficulty.clone());
    let crossed = shown.as_ref().map(|r| r.state["round"].as_u64().unwrap_or(0) as u32).unwrap_or(0);
    let dead = shown.as_ref().is_some_and(|r| !r.active && r.multiplier <= 0.0);
    let safe = chicken_lanes(&difficulty);
    let target = (app.settings.mode == Mode::Auto && difficulty == app.settings.orig.chicken_difficulty).then(|| (app.settings.orig.chicken_lanes as u32).clamp(1, safe));

    // the start pad behind a dashed line, then a tile per lane; the piece stands on top
    let gap = 7.0;
    let room = rect.width() - 56.0;
    // tiles as big as fit with every lane shown, else a window of 42-px tiles
    let all = ((room - 40.0 - gap * (safe as f32 - 1.0)) / (safe as f32 + 1.0)).min(62.0);
    let tile = all.max(42.0);
    let visible = (((room - tile - 40.0 + gap) / (tile + gap) + 0.01).floor().max(1.0) as u32).min(safe);
    // the whole row centred
    let width = tile + 40.0 + visible as f32 * (tile + gap) - gap;
    let left = rect.center().x - width / 2.0;
    let line_x = left + tile + 18.0;
    let x0 = line_x + 22.0;
    // keep the piece, the lane that got it and the cash-out lane in view
    let mut first = crossed.saturating_sub(2);
    if let Some(t) = target.filter(|t| *t > first + visible) {
        first = t - visible;
    }
    if dead {
        first = first.min(crossed);
    }
    let first = first.min(safe - visible) + 1;
    let mid_y = rect.center().y + tile * 0.45 - if open.is_some() { 30.0 } else { 6.0 };
    let lane = |n: u32| Rect::from_min_size(pos2(x0 + (n - first) as f32 * (tile + gap), mid_y - tile / 2.0), vec2(tile, tile));
    let pad = Rect::from_min_size(pos2(left, mid_y - tile / 2.0), vec2(tile, tile));
    let piece_h = tile * 1.3;

    let p = ui.painter();
    let line_top = mid_y - tile / 2.0 - piece_h;
    let line_bottom = mid_y + tile / 2.0 + 6.0;
    p.text(pos2(line_x, line_top - 10.0), Align2::CENTER_CENTER, "START", theme::font_semi(10.0), MUTED);
    let mut y = line_top;
    while y < line_bottom {
        p.line_segment([pos2(line_x, y), pos2(line_x, (y + 5.0).min(line_bottom))], Stroke::new(1.0, alpha(TEXT, 90)));
        y += 9.0;
    }
    p.rect(pad, CornerRadius::same(8), BG2, Stroke::new(1.0, LINE2), StrokeKind::Inside);
    for n in first..first + visible {
        let r = lane(n);
        let m = chicken_multiplier(&difficulty, n);
        let passed = n <= crossed;
        let hit = dead && n == crossed + 1;
        let aim = target == Some(n);
        let gone = dead && n > crossed + 1;
        let (fill, line, text) = if hit {
            (alpha(RED, 28), RED, RED)
        } else if passed {
            (alpha(G1, 26), G1, G1)
        } else if aim {
            (PURPLE, PURPLE, Color32::WHITE)
        } else {
            (mix(BG3, LINE2, 0.5), mix(BG3, LINE2, 0.5), TEXT)
        };
        let (fill, text) = if gone { (alpha(fill, 110), alpha(text, 110)) } else { (fill, text) };
        // raised like the site's tiles: a shadow under the face
        p.rect_filled(r.translate(vec2(0.0, 3.0)), CornerRadius::same(8), alpha(Color32::BLACK, 120));
        p.rect(r, CornerRadius::same(8), fill, Stroke::new(if hit || passed { 1.5 } else { 1.0 }, line), StrokeKind::Inside);
        if hit {
            miss(p, r.center(), tile * 0.46, RED);
        } else {
            fit_text(p, r.center(), Align2::CENTER_CENTER, &times_short(m), theme::font_bold((tile * 0.22).clamp(10.0, 13.0)), text, tile - 6.0);
        }
        p.text(pos2(r.center().x, r.bottom() + 14.0), Align2::CENTER_CENTER, format!("{n}"), theme::font(10.0), if aim { PURPLE } else { DIM });
    }

    // the piece hops tile to tile; the lane that got it knocks it over
    let on = if dead { crossed + 1 } else { crossed };
    let spot = if on == 0 || on < first { pad } else { lane(on.min(first + visible - 1)) };
    let ctx = ui.ctx().clone();
    let id = ui.id().with("chicken-piece");
    let x = ctx.animate_value_with_time(id, spot.center().x, 0.16);
    let fall = ctx.animate_value_with_time(id.with("fall"), if dead { 1.0 } else { 0.0 }, 0.22);
    let travel = ((x - spot.center().x).abs() / (tile + gap)).min(1.0);
    let lift = (travel * std::f32::consts::PI).sin() * tile * 0.35;
    let size = vec2(piece_h * 0.75, piece_h);
    // standing it rests on its base; lying on its side, on its width
    let ground = spot.top() + tile * 0.1;
    let half = size.y / 2.0 * (1.0 - fall) + size.x * 0.4 * fall;
    let piece = Rect::from_center_size(pos2(x, ground - half - lift), size);
    logos::paint_icon_turned(ui.painter(), piece.round_to_pixels(ctx.pixels_per_point()), Icon::Pawn, Color32::WHITE, fall * std::f32::consts::FRAC_PI_2);
    if (x - spot.center().x).abs() > 0.5 || (fall > 0.0 && fall < 1.0) {
        ctx.request_repaint();
    }

    if open.is_some() {
        let next = chicken_multiplier(&difficulty, crossed + 1);
        let b = Rect::from_center_size(pos2(rect.center().x, mid_y + tile / 2.0 + 56.0), vec2(220.0, 40.0));
        if board_button(ui, b, &format!("Cross  ·  {}", times(next)), Btn::Primary, !busy && crossed < safe) {
            app.orig_step(Game::Chicken, Step::Next, json!({}));
        }
    } else if shown.is_none() {
        super::note(ui.painter(), pos2(rect.center().x, mid_y + tile / 2.0 + 48.0), "Bet, then cross lane by lane");
    }
}
