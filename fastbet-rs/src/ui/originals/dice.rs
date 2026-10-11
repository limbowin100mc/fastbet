//! Dice: a 0–100 track split at the target, drag the handle to move it.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, BG2, GREEN, LINE2, MUTED, RED, TEXT, TEXT2};
use super::super::widgets::{caption, chip_button_sized, segmented, text_box};
use super::super::App;
use crate::money;
use crate::originals::{dice_chance, dice_multiplier, dice_target, Game, Round};

const QUICK: &[f64] = &[1.1, 1.5, 2.0, 3.0, 5.0, 10.0];

fn target(app: &App) -> Option<f64> {
    money::num(&app.settings.orig.dice_target)
}

/// The target when it gives a chance Stake accepts.
fn valid(app: &App) -> Option<f64> {
    let t = target(app)?;
    let chance = dice_chance(t, app.settings.orig.dice_over);
    let (lo, hi) = app.casino().dice_chance();
    (chance >= lo - 1e-9 && chance <= hi + 1e-9).then_some(t)
}

fn set_target(app: &mut App, t: f64) {
    let over = app.settings.orig.dice_over;
    let (lo, hi) = app.casino().dice_chance();
    let chance = dice_chance(t, over).clamp(lo, hi);
    app.settings.orig.dice_target = format!("{:.2}", dice_target(chance, over));
    app.mark_dirty();
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Condition");
    let sel = if app.settings.orig.dice_over { 0 } else { 1 };
    if let Some(i) = segmented(ui, &["Roll over", "Roll under"], sel, !locked) {
        let over = i == 0;
        if over != app.settings.orig.dice_over {
            // same chance on the other side: over 50.50 <-> under 49.50
            if let Some(t) = target(app) {
                app.settings.orig.dice_target = format!("{:.2}", 100.0 - t);
            }
            app.settings.orig.dice_over = over;
            app.mark_dirty();
        }
    }
    ui.horizontal(|ui| {
        caption(ui, if app.settings.orig.dice_over { "Roll over" } else { "Roll under" });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let txt = match valid(app) {
                Some(t) => {
                    let c = dice_chance(t, app.settings.orig.dice_over);
                    format!("{c:.2}% win  ·  {:.4}×", dice_multiplier(c))
                }
                None => chance_range(app),
            };
            ui.label(super::super::widgets::semi(txt, 11.0, if valid(app).is_some() { theme::G2 } else { RED }));
        });
    });
    if text_box(ui, &mut app.settings.orig.dice_target, "50.50", None, !locked, false).changed() {
        app.mark_dirty();
    }
    ui.horizontal(|ui| {
        let gap = 5.0;
        ui.spacing_mut().item_spacing.x = gap;
        let n = QUICK.len() as f32;
        let cw = ((ui.available_width() - gap * (n - 1.0)) / n).floor();
        let cur = valid(app).map(|t| dice_multiplier(dice_chance(t, app.settings.orig.dice_over)));
        for m in QUICK {
            let on = cur.is_some_and(|c| (c - m).abs() < 0.005);
            if chip_button_sized(ui, &format!("{}×", super::trim(*m)), on, !locked, vec2(cw, 28.0)).clicked() {
                let chance = 99.0 / m;
                app.settings.orig.dice_target = format!("{:.2}", dice_target(chance, app.settings.orig.dice_over));
                app.mark_dirty();
            }
        }
    });
}

/// "Win chance 0.01%–98%", with this casino's limits.
fn chance_range(app: &App) -> String {
    let (lo, hi) = app.casino().dice_chance();
    format!("Win chance {}%–{}%", super::trim(lo), super::trim(hi))
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    let t = valid(app).ok_or_else(|| format!("Pick a target with a {} win chance", chance_range(app).trim_start_matches("Win chance ")))?;
    Ok(json!({ "target": t, "condition": if app.settings.orig.dice_over { "above" } else { "below" } }))
}

pub fn result_text(r: &Round) -> String {
    let roll = r.state["result"].as_f64().unwrap_or(0.0);
    let t = r.state["target"].as_f64().unwrap_or(0.0);
    let over = r.state["condition"].as_str() == Some("above");
    format!("{roll:.2}  ({} {t:.2})", if over { ">" } else { "<" })
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let over = app.settings.orig.dice_over;
    let t = target(app).unwrap_or(50.5).clamp(0.0, 100.0);
    let track = Rect::from_center_size(pos2(rect.center().x, rect.center().y + 10.0), vec2((rect.width() - 120.0).min(760.0), 14.0));
    let x_of = |v: f64| track.left() + track.width() * (v / 100.0) as f32;

    // the frame around the track, like a ruler
    let frame = track.expand2(vec2(22.0, 20.0));
    let p = ui.painter();
    p.rect(frame, CornerRadius::same(18), alpha(Color32::BLACK, 60), Stroke::new(1.0, LINE2), StrokeKind::Inside);
    for v in [0.0, 25.0, 50.0, 75.0, 100.0] {
        let x = x_of(v);
        p.text(pos2(x, frame.top() - 16.0), Align2::CENTER_CENTER, format!("{}", v as i64), theme::font_semi(12.0), MUTED);
        p.line_segment([pos2(x, frame.top() - 6.0), pos2(x, frame.top())], Stroke::new(1.0, LINE2));
    }
    let split = x_of(t);
    let (left_c, right_c) = if over { (RED, GREEN) } else { (GREEN, RED) };
    let r = CornerRadius::same(7);
    p.rect_filled(Rect::from_min_max(track.min, pos2(split, track.max.y)), r, left_c);
    p.rect_filled(Rect::from_min_max(pos2(split, track.min.y), track.max), r, right_c);

    // the handle: drag it to move the target
    let knob = Rect::from_center_size(pos2(split, track.center().y), vec2(30.0, 30.0));
    let resp = ui.interact(knob.expand(6.0), ui.id().with("dice-knob"), if locked { Sense::hover() } else { Sense::drag() });
    let track_resp = ui.interact(track.expand2(vec2(0.0, 8.0)), ui.id().with("dice-track"), if locked { Sense::hover() } else { Sense::click() });
    let pointer = ui.input(|i| i.pointer.interact_pos());
    if let Some(pos) = pointer {
        if resp.dragged() || track_resp.clicked() {
            let v = (((pos.x - track.left()) / track.width()) as f64 * 100.0).clamp(0.0, 100.0);
            set_target(app, (v * 100.0).round() / 100.0);
        }
    }
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter();
    p.rect_filled(knob.translate(vec2(0.0, 2.0)), CornerRadius::same(6), alpha(Color32::BLACK, 120));
    p.rect_filled(knob, CornerRadius::same(6), if hot { Color32::from_rgb(0x7f, 0xa6, 0xff) } else { theme::BLUE });
    for dx in [-5.0, 0.0, 5.0] {
        p.line_segment([pos2(knob.center().x + dx, knob.top() + 9.0), pos2(knob.center().x + dx, knob.bottom() - 9.0)], Stroke::new(1.5, alpha(Color32::WHITE, 170)));
    }
    if !locked {
        resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    }

    // the latest roll, a marker above the track
    if let Some(last) = app.orig.last(Game::Dice) {
        if let Some(roll) = last.state["result"].as_f64() {
            let won = last.multiplier > 0.0;
            let c = if won { GREEN } else { RED };
            let age = app.orig.age(Game::Dice);
            let slide = (age / 0.25).clamp(0.0, 1.0);
            let x = x_of(roll);
            let y = track.top() - 46.0 - 8.0 * (1.0 - slide);
            let bubble = Rect::from_center_size(pos2(x, y), vec2(70.0, 34.0));
            p.rect_filled(bubble, CornerRadius::same(8), alpha(c, 230));
            p.add(egui::Shape::convex_polygon(vec![pos2(x - 7.0, bubble.bottom()), pos2(x + 7.0, bubble.bottom()), pos2(x, bubble.bottom() + 8.0)], alpha(c, 230), Stroke::NONE));
            p.text(bubble.center(), Align2::CENTER_CENTER, format!("{roll:.2}"), theme::font_bold(16.0), super::text_color_on(c));
            if age < 0.4 {
                ui.ctx().request_repaint();
            }
        }
    }

    // three readouts under the track
    let chance = dice_chance(t, over);
    let items = [
        ("Multiplier", format!("{:.4}×", dice_multiplier(chance))),
        (if over { "Roll over" } else { "Roll under" }, format!("{t:.2}")),
        ("Win chance", format!("{chance:.4}%")),
    ];
    let w = (track.width() - 24.0) / 3.0;
    for (i, (label, value)) in items.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(track.left() + i as f32 * (w + 12.0), frame.bottom() + 26.0), vec2(w, 54.0));
        p.text(pos2(cell.left() + 2.0, cell.top()), Align2::LEFT_TOP, *label, theme::font_semi(11.0), MUTED);
        let field = Rect::from_min_max(pos2(cell.left(), cell.top() + 18.0), cell.max);
        p.rect(field, CornerRadius::same(8), BG2, Stroke::new(1.0, LINE2), StrokeKind::Inside);
        p.text(pos2(field.left() + 12.0, field.center().y), Align2::LEFT_CENTER, value, theme::font_semi(14.0), if i == 1 { TEXT } else { TEXT2 });
    }
    // clicking the middle readout swaps the condition, like the site
    let swap = Rect::from_min_size(pos2(track.left() + w + 12.0, frame.bottom() + 44.0), vec2(w, 36.0));
    if super::hit(ui, swap, "dice-swap", !locked).on_hover_text("Swap roll over / under").clicked() {
        app.settings.orig.dice_target = format!("{:.2}", 100.0 - t);
        app.settings.orig.dice_over = !over;
        app.mark_dirty();
    }
}
