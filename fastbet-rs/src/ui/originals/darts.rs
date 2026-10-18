//! Darts: one dart at a board of rings; the ring it lands in pays.

use std::f32::consts::{PI, TAU};

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Mesh, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};
use serde_json::{json, Value};

use super::super::stats::HistEntry;
use super::super::theme::{self, alpha, mix, BG, BG1, BG3, G1, LINE, LINE2, MUTED, RED, TEXT};
use super::super::widgets;
use super::super::App;
use super::{levels, times, trim};
use crate::originals::{darts_table, Game, Round, LEVELS, LEVEL_NAMES};

/// Wires around the board, like a real dartboard's 20 sectors.
const SECTORS: usize = 20;
/// Seconds the dart flies before it lands.
const FLIGHT: f32 = 0.22;
/// Earlier darts left on the board.
const TRAIL: usize = 12;

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if levels(ui, &mut app.settings.orig.darts_difficulty, &LEVELS, &LEVEL_NAMES, !locked) {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "difficulty": app.settings.orig.darts_difficulty }))
}

pub fn result_text(r: &Round) -> String {
    format!("{}×  ·  {}", trim(r.multiplier), r.state["difficulty"].as_str().unwrap_or(""))
}

/// Ring colours, outside in: slate for the rings that pay back less than the stake, then
/// yellow, orange, red and a green bullseye for the ones that win.
fn ring_color(table: &[(f64, f64)], i: usize) -> Color32 {
    const SLATE: [Color32; 3] = [Color32::from_rgb(0x1b, 0x25, 0x34), Color32::from_rgb(0x2b, 0x38, 0x4b), Color32::from_rgb(0x3a, 0x48, 0x5e)];
    const PAY: [Color32; 5] = [
        Color32::from_rgb(0x1e, 0xd3, 0x80),
        Color32::from_rgb(0xf0, 0x34, 0x56),
        Color32::from_rgb(0xff, 0x84, 0x2e),
        Color32::from_rgb(0xff, 0xc9, 0x3c),
        Color32::from_rgb(0xb6, 0xe3, 0x3a),
    ];
    if table[i].0 < 1.0 {
        SLATE[i.min(2)]
    } else {
        PAY[(table.len() - 1 - i).min(4)]
    }
}

/// Outer edge of each ring, outside in, as a share of the board: halfway between even steps
/// and areas true to the chances, so rare rings stay small but readable.
fn radii(table: &[(f64, f64)]) -> Vec<f32> {
    let n = table.len();
    let mut cum = 0.0;
    let mut out = vec![0.0f32; n];
    for i in (0..n).rev() {
        cum += table[i].1;
        out[i] = cum.min(1.0).sqrt() as f32 * 0.45 + (n - i) as f32 / n as f32 * 0.55;
    }
    let outer = out[0].max(1e-6);
    out.iter().map(|r| r / outer).collect()
}

/// The ring that pays `m`.
fn ring_of(table: &[(f64, f64)], m: f64) -> usize {
    table.iter().position(|(x, _)| (x - m).abs() < 1e-9).unwrap_or(0)
}

/// "1:154.11", "1:2.5K": one win in how many throws.
fn odds(chance: f64) -> String {
    let x = 1.0 / chance.max(1e-12);
    let short = |v: f64, unit: &str| format!("1:{}{unit}", format!("{v:.1}").trim_end_matches(".0"));
    if x >= 1e6 {
        short(x / 1e6, "M")
    } else if x >= 1e3 {
        short(x / 1e3, "K")
    } else {
        format!("1:{x:.2}")
    }
}

/// Where a round's dart sits: in the ring that paid, at the depth and angle the casino drew
/// (a spot made from the round id when the reply has none).
fn spot(table: &[(f64, f64)], rad: &[f32], c: Pos2, r: &Round) -> Pos2 {
    let ring = ring_of(table, r.multiplier);
    let r_out = rad[ring];
    let r_in = rad.get(ring + 1).copied().unwrap_or(0.0);
    let h = r.id.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3));
    let frac = r.state["distance"].as_f64().unwrap_or(((h >> 8) % 1000) as f64 / 1000.0).clamp(0.12, 0.88) as f32;
    let turn = r.state["rotation"].as_f64().unwrap_or(((h >> 24) % 1000) as f64 / 1000.0) as f32;
    let dist = if ring + 1 == table.len() { r_out * frac * 0.8 } else { r_in + (r_out - r_in) * frac };
    let a = turn * TAU;
    pos2(c.x + dist * a.cos(), c.y + dist * a.sin())
}

/// An annulus slice from angle `a0` to `a1`, added to `mesh`.
fn slice(mesh: &mut Mesh, c: Pos2, r_in: f32, r_out: f32, a0: f32, a1: f32, color: Color32) {
    let steps = (((a1 - a0) * r_out / 5.0).ceil() as u32).clamp(2, 96);
    let base = mesh.vertices.len() as u32;
    for k in 0..=steps {
        let a = a0 + (a1 - a0) * k as f32 / steps as f32;
        let (s, co) = a.sin_cos();
        mesh.colored_vertex(pos2(c.x + r_out * co, c.y + r_out * s), color);
        mesh.colored_vertex(pos2(c.x + r_in * co, c.y + r_in * s), color);
    }
    for k in 0..steps {
        let i = base + k * 2;
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i + 1, i + 3, i + 2);
    }
}

/// A dart stuck in the board at `tip`, its tail toward the top right. `fly` runs from 1 (far
/// away, big) to 0 (landed).
fn dart(p: &Painter, tip: Pos2, len: f32, fly: f32) {
    let scale = 1.0 + 0.9 * fly;
    let tip = tip + vec2(len * 3.2, len * 1.4) * fly;
    let len = len * scale;
    let dir = vec2(0.6, -0.8);
    let side = vec2(-dir.y, dir.x);
    let at = |t: f32| tip + dir * len * t;
    if fly <= 0.0 {
        // its shadow on the board, thinning toward the tail
        let s = vec2(0.8, -0.5).normalized();
        p.line_segment([tip, tip + s * len * 0.45], Stroke::new(len * 0.08, alpha(Color32::BLACK, 70)));
        p.line_segment([tip + s * len * 0.45, tip + s * len * 0.8], Stroke::new(len * 0.05, alpha(Color32::BLACK, 45)));
    }
    let steel = Color32::from_rgb(0xc9, 0xd1, 0xdc);
    let barrel = Color32::from_rgb(0x8a, 0x95, 0xa6);
    p.line_segment([at(0.0), at(0.18)], Stroke::new((len * 0.025).max(1.4), steel));
    p.line_segment([at(0.16), at(0.47)], Stroke::new(len * 0.1, barrel));
    for k in 0..5 {
        let t = 0.2 + k as f32 * 0.055;
        let w = side * len * 0.05;
        p.line_segment([at(t) - w, at(t) + w], Stroke::new(1.0, mix(barrel, Color32::BLACK, 0.4)));
    }
    p.line_segment([at(0.17), at(0.46)], Stroke::new(len * 0.025, alpha(Color32::WHITE, 90)));
    p.line_segment([at(0.47), at(0.8)], Stroke::new(len * 0.045, Color32::from_rgb(0x2b, 0x32, 0x40)));
    // flights
    let wing = |s: f32, color: Color32| {
        p.add(Shape::convex_polygon(vec![at(0.64), at(0.8) + side * len * 0.17 * s, at(1.0) + side * len * 0.13 * s, at(1.0)], color, Stroke::NONE));
    };
    wing(1.0, G1);
    wing(-1.0, mix(G1, Color32::BLACK, 0.3));
    p.line_segment([at(0.64), at(1.0)], Stroke::new(1.0, alpha(Color32::WHITE, 120)));
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let difficulty = app.settings.orig.darts_difficulty.clone();
    let table = darts_table(&difficulty);
    let n = table.len();
    let mine = |r: &Round| r.state["difficulty"].as_str() == Some(difficulty.as_str());
    let last = app.orig.last(Game::Darts).cloned().filter(|r| mine(r));
    let age = app.orig.age(Game::Darts);
    let landed = age >= FLIGHT;

    // the board on the left, the payout list on its right, the pair centred
    let (row_h, row_gap, legend_w, gap) = (34.0, 8.0, 168.0, 56.0);
    let d = (rect.height() - 56.0).min(rect.width() - legend_w - gap - 48.0).max(120.0);
    let r = d / 2.0;
    let x0 = rect.center().x - (d + gap + legend_w) / 2.0;
    let c = pos2(x0 + r, rect.center().y - 8.0);
    let play = r * 0.84;
    let rad: Vec<f32> = radii(table).iter().map(|f| f * play).collect();
    let legend_h = n as f32 * row_h + (n as f32 - 1.0) * row_gap;
    let row_rect = |i: usize| {
        let k = (n - 1 - i) as f32;
        Rect::from_min_size(pos2(x0 + d + gap, c.y - legend_h / 2.0 + k * (row_h + row_gap)), vec2(legend_w, row_h))
    };

    // the ring under the pointer, on the board or in the list
    let mut hovered = None;
    let board_resp = ui.interact(Rect::from_center_size(c, Vec2::splat(play * 2.0)), ui.id().with("darts-board"), Sense::hover());
    if let Some(pos) = board_resp.hover_pos() {
        let dist = (pos - c).length();
        hovered = (0..n).rev().find(|&i| dist <= rad[i]);
    }
    for i in 0..n {
        let (m, chance) = table[i];
        let resp = ui.interact(row_rect(i), ui.id().with(("darts-row", i)), Sense::hover());
        if resp.hovered() {
            hovered = Some(i);
        }
        resp.on_hover_text(format!("{}%  ·  pays {}×", trim(chance * 100.0), trim(m)));
    }
    if let (Some(i), true) = (hovered, board_resp.hovered()) {
        board_resp.on_hover_text(format!("{}×  ·  {}%", trim(table[i].0), trim(table[i].1 * 100.0)));
    }

    let p = ui.painter();
    // shadow, bezel and its ticks
    widgets::glow(p, c + vec2(0.0, r * 0.08), r * 1.3, alpha(Color32::BLACK, 220));
    p.circle_filled(c, r, Color32::from_rgb(0x25, 0x2f, 0x3e));
    p.circle_stroke(c, r - 0.5, Stroke::new(1.0, alpha(Color32::WHITE, 26)));
    let step = TAU / SECTORS as f32;
    let start = -PI / 2.0 - step / 2.0;
    for k in 0..SECTORS {
        let a = start + step * (k as f32 + 0.5);
        let u = vec2(a.cos(), a.sin());
        p.line_segment([c + u * r * 0.9, c + u * r * 0.95], Stroke::new(1.5, alpha(Color32::WHITE, 45)));
    }
    p.circle_filled(c, play + 3.0, BG);

    // the rings, in sectors of two shades (each a hair wider, so no seam shows); the bullseye whole
    let mut mesh = Mesh::default();
    for i in 0..n - 1 {
        let color = ring_color(table, i);
        let alt = mix(color, Color32::BLACK, if table[i].0 < 1.0 { 0.25 } else { 0.16 });
        for k in 0..SECTORS {
            let a0 = start + step * k as f32;
            slice(&mut mesh, c, rad[i + 1] - 0.5, rad[i] + 0.5, a0 - 0.002, a0 + step + 0.002, if k % 2 == 0 { color } else { alt });
        }
    }
    p.add(Shape::mesh(mesh));
    p.circle_filled(c, rad[n - 1], ring_color(table, n - 1));
    // wires: a dark groove under a thin bright line, which also smooths the rings' edges
    let groove = Stroke::new(2.4, alpha(Color32::BLACK, 110));
    let wire = Stroke::new(1.0, alpha(Color32::from_rgb(0xc8, 0xd2, 0xe0), 70));
    for k in 0..SECTORS {
        let a = start + step * k as f32;
        let u = vec2(a.cos(), a.sin());
        p.line_segment([c + u * rad[n - 1], c + u * rad[0]], Stroke::new(1.6, alpha(Color32::BLACK, 70)));
        p.line_segment([c + u * rad[n - 1], c + u * rad[0]], wire);
    }
    for rr in &rad {
        p.circle_stroke(c, *rr, groove);
        p.circle_stroke(c, *rr, wire);
    }

    // the hovered ring lit up
    if let Some(h) = hovered {
        let inner = rad.get(h + 1).copied().unwrap_or(0.0);
        let mut glow = Mesh::default();
        slice(&mut glow, c, inner, rad[h], 0.0, TAU, alpha(Color32::WHITE, 30));
        p.add(Shape::mesh(glow));
        let edge = Stroke::new(2.0, mix(ring_color(table, h), Color32::WHITE, 0.35));
        p.circle_stroke(c, rad[h], edge);
        if inner > 0.0 {
            p.circle_stroke(c, inner, edge);
        }
    }

    // earlier darts, fading
    let last_id = last.as_ref().map(|r| r.id.clone());
    let trail: Vec<Pos2> = app
        .history
        .entries
        .iter()
        .filter_map(|e| match e {
            HistEntry::Bet { rec, .. } if rec.game == Some(Game::Darts) => rec.round.clone(),
            _ => None,
        })
        .filter(|r| mine(r) && Some(&r.id) != last_id.as_ref())
        .take(TRAIL)
        .map(|r| spot(table, &rad, c, &r))
        .collect();
    for (k, q) in trail.iter().enumerate().rev() {
        let a = (150.0 * (1.0 - k as f32 / TRAIL as f32)) as u8 + 30;
        p.circle_filled(*q, 3.0, alpha(Color32::BLACK, a));
        p.circle_filled(*q, 2.0, alpha(Color32::WHITE, a));
    }

    // the dart, the ripple where it lands, and the result under the board
    let result = last.as_ref().map(|r| (r.multiplier, if r.multiplier >= 1.0 { G1 } else { RED }));
    if let Some(rd) = &last {
        let hit = spot(table, &rad, c, rd);
        let len = (r * 0.36).clamp(40.0, 96.0);
        if landed {
            let ring = ring_of(table, rd.multiplier);
            let u = ((age - FLIGHT) / 0.45).clamp(0.0, 1.0);
            if u < 1.0 {
                let col = mix(ring_color(table, ring), Color32::WHITE, 0.4);
                p.circle_stroke(hit, 5.0 + 26.0 * u, Stroke::new(2.5 * (1.0 - u), alpha(col, (230.0 * (1.0 - u)) as u8)));
            }
            p.circle_filled(hit, 5.5, alpha(Color32::BLACK, 120));
            p.circle_stroke(hit, 5.0, Stroke::new(1.6, Color32::WHITE));
            dart(p, hit, len, 0.0);
        } else {
            let t = age / FLIGHT;
            dart(p, hit, len, (1.0 - t).powi(3));
        }
        if age < FLIGHT + 0.45 {
            ui.ctx().request_repaint();
        }
    }
    let badge = Rect::from_center_size(pos2(c.x, c.y + r + 2.0), vec2(96.0, 32.0));
    match result {
        Some((m, col)) if landed => {
            p.rect_filled(badge, CornerRadius::same(8), BG1);
            p.rect(badge, CornerRadius::same(8), alpha(col, 24), Stroke::new(1.0, alpha(col, 200)), StrokeKind::Inside);
            p.text(badge.center(), Align2::CENTER_CENTER, times(m), theme::font_semi(13.5), col);
        }
        Some(_) => {}
        None => {
            p.rect(badge, CornerRadius::same(8), BG1, Stroke::new(1.0, LINE), StrokeKind::Inside);
            p.text(badge.center(), Align2::CENTER_CENTER, "Throw a dart", theme::font_semi(11.5), MUTED);
        }
    }

    // the payout list, best on top
    let landed_ring = last.as_ref().filter(|_| landed).map(|r| ring_of(table, r.multiplier));
    for i in 0..n {
        let (m, chance) = table[i];
        let row = row_rect(i);
        let won = landed_ring == Some(i);
        let col = result.map(|(_, c)| c).unwrap_or(TEXT);
        let (fill, stroke) = if won {
            (alpha(col, 22), alpha(col, 200))
        } else if hovered == Some(i) {
            (BG3, LINE2)
        } else {
            (BG1, LINE)
        };
        p.rect_filled(row, CornerRadius::same(8), BG1);
        p.rect(row, CornerRadius::same(8), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        let dot = pos2(row.left() + 18.0, row.center().y);
        p.circle_filled(dot, 6.5, ring_color(table, i));
        if m < 1.0 {
            p.circle_stroke(dot, 6.5, Stroke::new(1.0, LINE2));
        }
        p.text(pos2(row.left() + 34.0, row.center().y), Align2::LEFT_CENTER, format!("{}×", trim(m)), theme::font_semi(13.0), if won { col } else { TEXT });
        p.text(pos2(row.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, odds(chance), theme::font_semi(12.0), if won { col } else { MUTED });
    }
}
