//! Plinko: a ball falls through rows of pegs into a bucket. Pays by risk and rows.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Pos2, Rect, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, TEXT};
use super::super::widgets::{caption, segmented};
use super::super::App;
use super::{payout_color, text_color_on, trim};
use crate::originals::{Game, Round};

const FALL: f32 = 0.7;

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Risk");
    let risks = app.casino().plinko_risks();
    // a risk this casino doesn't have (Shuffle has no expert) falls back to medium
    if !risks.iter().any(|(k, _)| *k == app.settings.orig.plinko_risk) {
        app.settings.orig.plinko_risk = "medium".into();
        app.mark_dirty();
    }
    let sel = risks.iter().position(|(k, _)| *k == app.settings.orig.plinko_risk).unwrap_or(1);
    let labels: Vec<&str> = risks.iter().map(|(_, l)| *l).collect();
    if let Some(i) = segmented(ui, &labels, sel, !locked) {
        app.settings.orig.plinko_risk = risks[i].0.into();
        app.mark_dirty();
    }
    caption(ui, "Rows");
    let rows: Vec<String> = (8..=16).map(|r| r.to_string()).collect();
    let labels: Vec<&str> = rows.iter().map(String::as_str).collect();
    let sel = (app.settings.orig.plinko_rows.clamp(8, 16) - 8) as usize;
    if let Some(i) = segmented(ui, &labels, sel, !locked) {
        app.settings.orig.plinko_rows = 8 + i as u8;
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "rows": app.settings.orig.plinko_rows.clamp(8, 16), "risk": app.settings.orig.plinko_risk }))
}

pub fn result_text(r: &Round) -> String {
    let path = r.state["path"].as_array().map(|a| a.len()).unwrap_or(0);
    format!("{}×  ·  {path} rows", trim(r.multiplier))
}

fn bucket_of(path: &[Value]) -> usize {
    path.iter().filter(|s| s.as_str() == Some("R")).count()
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let rows = app.settings.orig.plinko_rows.clamp(8, 16) as usize;
    let risk = app.settings.orig.plinko_risk.clone();
    let table = app.orig.tables.plinko(&risk, rows as u32).to_vec();
    let last = app.orig.last(Game::Plinko).cloned().filter(|r| r.state["rows"].as_u64() == Some(rows as u64) && r.state["risk"].as_str() == Some(risk.as_str()));
    let age = app.orig.age(Game::Plinko);

    let top = rect.top() + 34.0;
    let bucket_h = 30.0;
    let bottom = rect.bottom() - bucket_h - 26.0;
    let dy = (bottom - top) / rows as f32;
    let dx = ((rect.width() - 60.0) / (rows as f32 + 2.0)).min(dy * 1.25);
    let cx = rect.center().x;
    let peg = |i: usize, j: usize| pos2(cx + (j as f32 - (i as f32 + 2.0) / 2.0) * dx, top + i as f32 * dy);
    let p = ui.painter();
    let r = (dx * 0.09).clamp(2.0, 4.5);
    for i in 0..rows {
        for j in 0..i + 3 {
            p.circle_filled(peg(i, j), r, alpha(Color32::WHITE, 210));
        }
    }

    // buckets under the last row
    let landed = last.as_ref().and_then(|l| l.state["path"].as_array().map(|a| bucket_of(a)));
    let falling = age < FALL;
    let by = bottom + dy * 0.45;
    for (k, m) in table.iter().enumerate() {
        let x = cx + (k as f32 - rows as f32 / 2.0) * dx;
        let hit = landed == Some(k) && !falling;
        let bounce = if hit { (1.0 - ((age - FALL) / 0.25).clamp(0.0, 1.0)) * 5.0 } else { 0.0 };
        let b = Rect::from_center_size(pos2(x, by + bucket_h / 2.0 + bounce), vec2(dx - 4.0, bucket_h));
        let c = payout_color(*m);
        let fill = if hit { mix(c, Color32::WHITE, 0.15) } else { c };
        p.rect_filled(b.translate(vec2(0.0, 3.0)), CornerRadius::same(5), mix(c, Color32::BLACK, 0.45));
        p.rect_filled(b, CornerRadius::same(5), fill);
        let label = if *m >= 100.0 { format!("{}", m.round() as i64) } else { trim(*m) };
        super::super::widgets::fit_text(p, b.center(), Align2::CENTER_CENTER, &label, theme::font_bold((dx * 0.32).clamp(9.0, 13.0)), text_color_on(fill), b.width() - 2.0);
    }

    // the latest ball, falling along its path
    if let Some(path) = last.as_ref().and_then(|l| l.state["path"].as_array().cloned()) {
        let mut pts: Vec<Pos2> = vec![pos2(cx, top - dy * 0.8)];
        let mut off = 0.0f32;
        for (i, step) in path.iter().enumerate() {
            off += if step.as_str() == Some("R") { 0.5 } else { -0.5 };
            pts.push(pos2(cx + off * dx, top + (i as f32 + 0.5) * dy));
        }
        pts.push(pos2(cx + off * dx, by + bucket_h * 0.4));
        let t = (age / FALL).clamp(0.0, 1.0);
        let pos = along(&pts, t);
        if falling {
            ui.ctx().request_repaint();
        } else {
            p.add(egui::Shape::line(pts.clone(), Stroke::new(1.5, alpha(payout_color(last.as_ref().map(|l| l.multiplier).unwrap_or(0.0)), 70))));
        }
        p.circle_filled(pos, (dx * 0.22).clamp(4.0, 9.0), Color32::from_rgb(0xff, 0x3d, 0x5f));
        p.circle_filled(pos - vec2(1.5, 1.5), (dx * 0.08).clamp(1.5, 3.0), alpha(Color32::WHITE, 170));
    } else {
        super::note(p, pos2(cx, top - 16.0), "Drop a ball");
    }
    let _ = TEXT;
}

/// The point a fraction `t` of the way along a polyline.
fn along(pts: &[Pos2], t: f32) -> Pos2 {
    let segs = pts.len().saturating_sub(1).max(1) as f32;
    let x = t * segs;
    let i = (x.floor() as usize).min(pts.len().saturating_sub(2));
    let f = x - i as f32;
    let a = pts[i];
    let b = pts[(i + 1).min(pts.len() - 1)];
    // a little hop between pegs
    pos2(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f - (f * (1.0 - f)) * 10.0)
}
