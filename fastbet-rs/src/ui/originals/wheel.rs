//! Wheel: a spin lands on one of 10–50 segments. Pays by risk and segments.

use std::f32::consts::{PI, TAU};

use egui::{epaint::Mesh, pos2, vec2, Align2, Color32, Rect, Shape, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG1, MUTED, TEXT};
use super::super::widgets::{caption, segmented};
use super::super::App;
use super::{chip, payout_color, text_color_on, times};
use crate::originals::Game;

const RISKS: [&str; 3] = ["low", "medium", "high"];
const SEGMENTS: [u8; 5] = [10, 20, 30, 40, 50];
const SPIN: f32 = 1.1;

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Risk");
    let sel = RISKS.iter().position(|r| *r == app.settings.orig.wheel_risk).unwrap_or(1);
    if let Some(i) = segmented(ui, &["Low", "Medium", "High"], sel, !locked) {
        app.settings.orig.wheel_risk = RISKS[i].into();
        app.mark_dirty();
    }
    caption(ui, "Segments");
    let sel = SEGMENTS.iter().position(|s| *s == app.settings.orig.wheel_segments).unwrap_or(2);
    if let Some(i) = segmented(ui, &["10", "20", "30", "40", "50"], sel, !locked) {
        app.settings.orig.wheel_segments = SEGMENTS[i];
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "risk": app.settings.orig.wheel_risk, "segments": app.settings.orig.wheel_segments }))
}

fn color(m: f64) -> Color32 {
    if m <= 0.0 {
        Color32::from_rgb(0x2a, 0x34, 0x4a)
    } else {
        payout_color(m)
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let segs = app.settings.orig.wheel_segments as usize;
    let risk = app.settings.orig.wheel_risk.clone();
    let table = app.orig.tables.wheel(&risk, segs as u32).to_vec();
    if table.is_empty() {
        super::note(ui.painter(), rect.center(), "No payout table for this wheel");
        return;
    }
    let last = app.orig.last(Game::Wheel).cloned().filter(|r| r.state["segments"].as_u64() == Some(segs as u64) && r.state["risk"].as_str() == Some(risk.as_str()));
    let age = app.orig.age(Game::Wheel);

    let legend_h = 44.0;
    let center = pos2(rect.center().x, rect.top() + (rect.height() - legend_h) / 2.0 + 6.0);
    let outer = ((rect.height() - legend_h) / 2.0 - 22.0).min(rect.width() / 2.0 - 30.0).max(60.0);
    let inner = outer * 0.72;
    let seg = TAU / segs as f32;
    // the landed segment ends under the pointer at the top, after a couple of turns
    let rot = match &last {
        Some(l) => {
            let res = l.state["result"].as_u64().unwrap_or(0) as f32;
            let end = -PI / 2.0 - (res + 0.5) * seg;
            let t = (age / SPIN).clamp(0.0, 1.0);
            let ease = 1.0 - (1.0 - t).powi(3);
            if t < 1.0 {
                ui.ctx().request_repaint();
            }
            end - (1.0 - ease) * 2.0 * TAU
        }
        None => -PI / 2.0,
    };
    let p = ui.painter();
    p.circle_filled(center, outer + 10.0, mix(BG1, Color32::BLACK, 0.3));
    let mut mesh = Mesh::default();
    for (i, m) in table.iter().enumerate() {
        let a0 = rot + i as f32 * seg + 0.006;
        let a1 = rot + (i + 1) as f32 * seg - 0.006;
        let c = color(*m);
        let steps = 6;
        for k in 0..steps {
            let b0 = a0 + (a1 - a0) * k as f32 / steps as f32;
            let b1 = a0 + (a1 - a0) * (k + 1) as f32 / steps as f32;
            let base = mesh.vertices.len() as u32;
            for (a, r) in [(b0, outer), (b1, outer), (b1, inner), (b0, inner)] {
                mesh.colored_vertex(pos2(center.x + r * a.cos(), center.y + r * a.sin()), c);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
    }
    p.add(Shape::mesh(mesh));
    p.circle_filled(center, inner - 6.0, BG1);
    p.circle_stroke(center, inner - 6.0, Stroke::new(1.0, alpha(Color32::WHITE, 20)));

    // pointer
    let tip = pos2(center.x, center.y - outer + 10.0);
    p.add(Shape::convex_polygon(vec![pos2(tip.x - 11.0, tip.y - 26.0), pos2(tip.x + 11.0, tip.y - 26.0), tip], Color32::from_rgb(0xff, 0x3d, 0x5f), Stroke::new(1.5, mix(Color32::from_rgb(0xff, 0x3d, 0x5f), Color32::BLACK, 0.3))));

    match &last {
        Some(l) if age >= SPIN => {
            let c = color(l.multiplier);
            p.text(center, Align2::CENTER_CENTER, times(l.multiplier), theme::font_bold((inner * 0.32).clamp(18.0, 44.0)), if l.multiplier > 0.0 { c } else { MUTED });
        }
        Some(_) => {}
        None => super::note(p, center, "Spin the wheel"),
    }

    // every payout on this wheel with its chance
    let mut values: Vec<(f64, usize)> = Vec::new();
    for m in &table {
        match values.iter_mut().find(|(v, _)| (v - m).abs() < 1e-9) {
            Some((_, n)) => *n += 1,
            None => values.push((*m, 1)),
        }
    }
    values.sort_by(|a, b| a.0.total_cmp(&b.0));
    let w = ((rect.width() - 40.0) / values.len() as f32 - 6.0).min(110.0);
    let x0 = rect.center().x - (w + 6.0) * values.len() as f32 / 2.0 + 3.0;
    let y = rect.bottom() - legend_h + 4.0;
    for (i, (m, n)) in values.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (w + 6.0), y), vec2(w, 22.0));
        let c = color(*m);
        let on = last.as_ref().is_some_and(|l| (l.multiplier - m).abs() < 1e-9) && age >= SPIN;
        chip(p, r, &times(*m), if on { c } else { alpha(c, 60) }, if on { text_color_on(c) } else { TEXT });
        p.text(pos2(r.center().x, r.bottom() + 9.0), Align2::CENTER_CENTER, format!("{:.0}%", *n as f64 * 100.0 / segs as f64), theme::font(10.0), MUTED);
    }
    let _ = vec2(0.0, 0.0);
}
