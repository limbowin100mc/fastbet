//! Drill: three drills dig; pick one and a depth (multiplier). Wins when it gets that far.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, GOLD, GREEN, MUTED, RED, TEXT};
use super::super::widgets::{caption, segmented, text_box};
use super::super::App;
use super::{times, trim};
use crate::money;
use crate::originals::{Game, Round};

fn target(app: &App) -> Option<f64> {
    money::num(&app.settings.orig.drill_target).filter(|t| *t >= 1.01 && *t <= 1_000_000.0)
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Drill");
    let sel = (app.settings.orig.drill_pick.clamp(1, 3) - 1) as usize;
    if let Some(i) = segmented(ui, &["1", "2", "3"], sel, !locked) {
        app.settings.orig.drill_pick = i as u8 + 1;
        app.mark_dirty();
    }
    ui.horizontal(|ui| {
        caption(ui, "Target");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let txt = match target(app) {
                Some(t) => format!("≈{}% win", trim(98.0 / t)),
                None => "1.01× or more".into(),
            };
            ui.label(super::super::widgets::semi(txt, 11.0, if target(app).is_some() { theme::G2 } else { RED }));
        });
    });
    if text_box(ui, &mut app.settings.orig.drill_target, "2.00", Some("×"), !locked, false).changed() {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    let t = target(app).ok_or("Set a target of 1.01× or more")?;
    Ok(json!({ "pick": app.settings.orig.drill_pick.clamp(1, 3), "target": t }))
}

fn results(r: &Round) -> Vec<(u64, f64)> {
    r.state["drillResults"].as_array().map(|a| a.iter().map(|d| (d["drillNumber"].as_u64().unwrap_or(0), d["multiplier"].as_f64().unwrap_or(0.0))).collect()).unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    let pick = r.state["pick"].as_u64().unwrap_or(0);
    let got = results(r).into_iter().find(|(n, _)| *n == pick).map(|(_, m)| m).unwrap_or(0.0);
    format!("drill {pick} reached {}  (target {})", times(got), times(r.state["target"].as_f64().unwrap_or(0.0)))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let pick = app.settings.orig.drill_pick.clamp(1, 3) as u64;
    let t = target(app).unwrap_or(2.0);
    let last = app.orig.last(Game::Drill).cloned();
    let age = app.orig.age(Game::Drill);
    let res = last.as_ref().map(results).unwrap_or_default();
    let p = ui.painter();
    let ground = rect.top() + 46.0;
    let bottom = rect.bottom() - 40.0;
    // depth on a log scale: 1× at the surface, the deepest of target and results at the bottom
    let deepest = res.iter().map(|r| r.1).fold(t, f64::max).max(2.0) * 1.3;
    let y_of = |m: f64| ground + (bottom - ground) * ((m.max(1.0)).ln() / deepest.ln()) as f32;
    let soil = Rect::from_min_max(pos2(rect.left() + 30.0, ground), pos2(rect.right() - 30.0, bottom + 10.0));
    p.rect_filled(soil, CornerRadius::same(10), Color32::from_rgb(0x2a, 0x1f, 0x16));
    // the target line
    let ty = y_of(t);
    p.hline(soil.x_range(), ty, Stroke::new(2.0, alpha(GOLD, 200)));
    p.text(pos2(soil.left() + 8.0, ty - 10.0), Align2::LEFT_CENTER, format!("target {}", times(t)), theme::font_semi(11.0), GOLD);
    let w = soil.width() / 3.0;
    let grow = (age / 0.6).clamp(0.0, 1.0);
    for k in 1..=3u64 {
        let cx = soil.left() + w * (k as f32 - 0.5);
        let picked = k == pick;
        p.text(pos2(cx, ground - 18.0), Align2::CENTER_CENTER, format!("Drill {k}"), theme::font_semi(12.0), if picked { TEXT } else { MUTED });
        let reached = res.iter().find(|(n, _)| *n == k).map(|(_, m)| *m);
        let depth = match reached {
            Some(m) => ground + (y_of(m) - ground) * grow,
            None => ground,
        };
        let shaft = Rect::from_min_max(pos2(cx - 14.0, ground), pos2(cx + 14.0, depth));
        let c = match reached {
            Some(m) if picked && m >= t => GREEN,
            Some(_) if picked => RED,
            _ => Color32::from_rgb(0x8a, 0x94, 0xaa),
        };
        p.rect_filled(shaft, CornerRadius::same(4), alpha(c, if picked { 200 } else { 90 }));
        // the bit
        p.add(egui::Shape::convex_polygon(vec![pos2(cx - 16.0, depth), pos2(cx + 16.0, depth), pos2(cx, depth + 18.0)], mix(c, Color32::WHITE, 0.2), Stroke::NONE));
        if let Some(m) = reached.filter(|_| grow >= 1.0) {
            p.text(pos2(cx, (depth + 32.0).min(bottom)), Align2::CENTER_CENTER, times(m), theme::font_bold(14.0), c);
        }
        if picked {
            p.rect_stroke(Rect::from_min_max(pos2(cx - w / 2.0 + 6.0, ground + 2.0), pos2(cx + w / 2.0 - 6.0, bottom + 6.0)), CornerRadius::same(8), Stroke::new(1.5, alpha(GOLD, 120)), egui::StrokeKind::Inside);
        }
    }
    if grow < 1.0 {
        ui.ctx().request_repaint();
    }
    if last.is_none() {
        super::note(p, pos2(rect.center().x, rect.bottom() - 14.0), "Pick a drill and a target, then bet");
    }
    let _ = vec2(0.0, 0.0);
}
