//! Primedice: roll 0–100 and win inside a range, inside either of two ranges, or outside one.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GREEN, LINE2, MUTED, RED};
use super::super::widgets::{caption, segmented, text_box};
use super::super::App;
use super::times;
use crate::money;
use crate::originals::{Game, Round};

const CONDITIONS: [&str; 3] = ["rollBetween", "rollBetweenTwo", "rollOutside"];

fn targets(app: &App) -> Vec<Option<f64>> {
    app.settings.orig.primedice_targets.iter().map(|t| money::num(t).filter(|v| (0.0..=100.0).contains(v))).collect()
}

/// The winning stretches of 0–100 and their total width.
fn ranges(condition: &str, t: &[f64]) -> Vec<(f64, f64)> {
    match (condition, t) {
        ("rollBetweenTwo", [a, b, c, d, ..]) => vec![(*a, *b), (*c, *d)],
        ("rollOutside", [a, b, ..]) => vec![(0.0, *a), (*b, 100.0)],
        (_, [a, b, ..]) => vec![(*a, *b)],
        _ => vec![],
    }
}

fn used(condition: &str) -> usize {
    if condition == "rollBetweenTwo" {
        4
    } else {
        2
    }
}

/// The targets in order, when every one in use is set and they don't cross.
fn valid(app: &App) -> Option<Vec<f64>> {
    let cond = app.settings.orig.primedice_condition.as_str();
    let mut t: Vec<f64> = targets(app).into_iter().take(used(cond)).collect::<Option<Vec<_>>>()?;
    if t.len() < used(cond) {
        return None;
    }
    t.sort_by(f64::total_cmp);
    t.windows(2).all(|w| w[1] - w[0] >= 0.01).then_some(t)
}

fn chance(cond: &str, t: &[f64]) -> f64 {
    ranges(cond, t).iter().map(|(a, b)| b - a).sum()
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Win when the roll is");
    let sel = CONDITIONS.iter().position(|c| *c == app.settings.orig.primedice_condition).unwrap_or(0);
    if let Some(i) = segmented(ui, &["Between", "In two ranges", "Outside"], sel, !locked) {
        app.settings.orig.primedice_condition = CONDITIONS[i].into();
        if i == 1 && app.settings.orig.primedice_targets.get(2).is_none_or(|t| t.trim().is_empty()) {
            app.settings.orig.primedice_targets = vec!["10".into(), "30".into(), "60".into(), "80".into()];
        }
        app.mark_dirty();
    }
    let n = used(&app.settings.orig.primedice_condition);
    app.settings.orig.primedice_targets.resize(4, String::new());
    let mut changed = false;
    for pair in 0..n / 2 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = (ui.available_width() - 8.0) / 2.0;
            for k in 0..2 {
                ui.scope(|ui| {
                    ui.set_min_width(w);
                    ui.set_max_width(w);
                    changed |= text_box(ui, &mut app.settings.orig.primedice_targets[pair * 2 + k], if k == 0 { "from" } else { "to" }, None, !locked, false).changed();
                });
            }
        });
    }
    if changed {
        app.mark_dirty();
    }
    match valid(app) {
        Some(t) => {
            let c = chance(&app.settings.orig.primedice_condition, &t);
            super::readout(ui, format!("{c:.2}% win  ·  about {}", times(if c > 0.0 { 99.0 / c } else { 0.0 })));
        }
        None => super::readout_bad(ui, "Set the range edges (0–100, in order)"),
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    let t = valid(app).ok_or("Set the range edges (0–100, in order)")?;
    let cond = &app.settings.orig.primedice_condition;
    let get = |i: usize| t.get(i).copied().map(Value::from).unwrap_or(Value::Null);
    Ok(json!({ "target1": get(0), "target2": get(1), "target3": get(2), "target4": get(3), "condition": cond }))
}

pub fn result_text(r: &Round) -> String {
    format!("rolled {:.2}", r.state["result"].as_f64().unwrap_or(0.0))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let cond = app.settings.orig.primedice_condition.clone();
    let t = valid(app).unwrap_or_default();
    let track = Rect::from_center_size(pos2(rect.center().x, rect.center().y), vec2((rect.width() - 120.0).min(760.0), 16.0));
    let x_of = |v: f64| track.left() + track.width() * (v / 100.0) as f32;
    let p = ui.painter();
    let frame = track.expand2(vec2(22.0, 20.0));
    p.rect(frame, CornerRadius::same(18), alpha(Color32::BLACK, 60), Stroke::new(1.0, LINE2), StrokeKind::Inside);
    p.rect_filled(track, CornerRadius::same(8), RED);
    for (a, b) in ranges(&cond, &t) {
        p.rect_filled(Rect::from_min_max(pos2(x_of(a), track.top()), pos2(x_of(b), track.bottom())), CornerRadius::same(3), GREEN);
    }
    for v in [0.0, 25.0, 50.0, 75.0, 100.0] {
        p.text(pos2(x_of(v), frame.top() - 16.0), Align2::CENTER_CENTER, format!("{}", v as i64), theme::font_semi(12.0), MUTED);
    }
    for v in &t {
        p.text(pos2(x_of(*v), frame.bottom() + 14.0), Align2::CENTER_CENTER, format!("{v:.2}"), theme::font_semi(11.0), theme::TEXT2);
    }
    if let Some(last) = app.orig.last(Game::Primedice) {
        if let Some(roll) = last.state["result"].as_f64() {
            let c = if last.multiplier > 0.0 { GREEN } else { RED };
            let x = x_of(roll);
            let bubble = Rect::from_center_size(pos2(x, track.top() - 50.0), vec2(70.0, 34.0));
            p.rect_filled(bubble, CornerRadius::same(8), c);
            p.add(egui::Shape::convex_polygon(vec![pos2(x - 7.0, bubble.bottom()), pos2(x + 7.0, bubble.bottom()), pos2(x, bubble.bottom() + 8.0)], c, Stroke::NONE));
            p.text(bubble.center(), Align2::CENTER_CENTER, format!("{roll:.2}"), theme::font_bold(16.0), super::text_color_on(c));
        }
    }
    if !t.is_empty() {
        let c = chance(&cond, &t);
        p.text(pos2(rect.center().x, frame.bottom() + 52.0), Align2::CENTER_CENTER, format!("{c:.2}% to win  ·  pays about {}", times(if c > 0.0 { 99.0 / c } else { 0.0 })), theme::font_semi(13.0), theme::TEXT2);
    }
}
