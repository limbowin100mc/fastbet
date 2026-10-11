//! Moles: every round some of the seven holes hide a mole; whack a mole to go on.
//! Manual: pick a hole each round and cash out. Auto: plan the holes up front.

use egui::{pos2, vec2, Align2, Color32, Pos2, Rect, Stroke, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, GOLD, GREEN, MUTED, RED, TEXT};
use super::super::widgets::{caption, chip_button_sized, segmented};
use super::super::App;
use super::{hit, times};
use crate::originals::{moles_multiplier, moles_rounds, Game, Round, Step};
use crate::settings::Mode;

/// The holes, as fractions of the board: two on top, three across the middle, two below.
const HOLES: [(f32, f32); 7] = [(0.3, 0.18), (0.7, 0.18), (0.12, 0.5), (0.5, 0.5), (0.88, 0.5), (0.3, 0.82), (0.7, 0.82)];

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Moles).is_some();
    caption(ui, "Moles per round");
    let sel = (app.settings.orig.moles_count.clamp(1, 6) - 1) as usize;
    if let Some(i) = segmented(ui, &["1", "2", "3", "4", "5", "6"], sel, !locked && !open) {
        app.settings.orig.moles_count = i as u8 + 1;
        let max = moles_rounds(i as u32 + 1) as usize;
        app.settings.orig.moles_picks.truncate(max);
        app.mark_dirty();
    }
    if app.settings.mode == Mode::Auto {
        let n = app.settings.orig.moles_picks.len();
        let w = ui.available_width();
        if chip_button_sized(ui, "Clear plan", false, !locked && n > 0, vec2(w, 28.0)).clicked() {
            app.settings.orig.moles_picks.clear();
            app.mark_dirty();
        }
        let m = app.settings.orig.moles_count as u32;
        let txt = if n == 0 {
            "Click holes on the board: one per round, in order".to_string()
        } else {
            format!("{n} rounds planned  ·  pays {} if every one hits", times(moles_multiplier(m, n as u32)))
        };
        ui.label(egui::RichText::new(txt).size(11.0).color(MUTED));
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let m = app.settings.orig.moles_count.clamp(1, 6);
    if !auto {
        return Ok(json!({ "molesCount": m }));
    }
    let picks = &app.settings.orig.moles_picks;
    if picks.is_empty() {
        return Err("Plan at least one round on the board".into());
    }
    Ok(json!({ "molesCount": m, "picks": picks }))
}

struct Played {
    pick: Option<u64>,
    hit: Option<bool>,
    moles: Vec<i64>,
}

fn rounds(r: &Round) -> Vec<Played> {
    r.state["rounds"]
        .as_array()
        .map(|a| a.iter().map(|x| Played { pick: x["pick"].as_u64(), hit: x["hit"].as_bool(), moles: super::nums(&x["molePositions"]) }).collect())
        .unwrap_or_default()
}

pub fn result_text(r: &Round) -> String {
    let hits = rounds(r).iter().filter(|p| p.hit == Some(true)).count();
    if r.multiplier > 0.0 {
        format!("{hits} hits")
    } else {
        format!("Missed after {hits} hits")
    }
}

fn hole_pos(area: Rect, i: usize) -> Pos2 {
    let (x, y) = HOLES[i];
    pos2(area.left() + area.width() * x, area.top() + area.height() * y)
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let auto = app.settings.mode == Mode::Auto;
    let open = app.orig.open(Game::Moles).cloned();
    let busy = app.orig.busy.contains(&Game::Moles);
    let shown = open.clone().or_else(|| app.orig.last(Game::Moles).cloned());
    let played = shown.as_ref().map(rounds).unwrap_or_default();
    let current = shown.as_ref().map(|r| r.state["currentRound"].as_u64().unwrap_or(0) as usize).unwrap_or(0);
    let moles_count = shown.as_ref().and_then(|r| r.state["molesCount"].as_u64()).unwrap_or(app.settings.orig.moles_count as u64) as u32;

    let area = Rect::from_center_size(pos2(rect.center().x, rect.top() + (rect.height() - 70.0) / 2.0), vec2((rect.height() - 90.0).min(rect.width() - 80.0) * 1.2, rect.height() - 110.0));
    let r = (area.height() * 0.13).min(46.0);
    // the round the board shows: the open one, else the last one played
    let showing = if open.is_some() { None } else { played.iter().rev().find(|p| p.pick.is_some()) };
    let mut clicked = None;
    for i in 0..7 {
        let c = hole_pos(area, i);
        let hole_rect = Rect::from_center_size(c, vec2(r * 2.4, r * 1.6));
        let can = if open.is_some() { !busy } else { auto && !locked };
        let resp = hit(ui, hole_rect, ("mole", i), can);
        let p = ui.painter();
        // dirt mound and hole
        p.circle_filled(c + vec2(0.0, r * 0.25), r * 1.15, Color32::from_rgb(0x3d, 0x2b, 0x1e));
        p.circle_filled(c + vec2(0.0, r * 0.35), r * 0.85, if resp.hovered() && can { Color32::from_rgb(0x2a, 0x1c, 0x12) } else { Color32::from_rgb(0x14, 0x0d, 0x08) });
        if let Some(pl) = showing {
            let picked = pl.pick == Some(i as u64);
            let mole_here = pl.moles.contains(&(i as i64));
            if mole_here {
                // the mole: a brown head with a pink nose
                let body = if picked { mix(Color32::from_rgb(0x9a, 0x6b, 0x45), GREEN, 0.15) } else { Color32::from_rgb(0x9a, 0x6b, 0x45) };
                p.circle_filled(c - vec2(0.0, r * 0.1), r * 0.62, body);
                p.circle_filled(c + vec2(0.0, r * 0.08), r * 0.14, Color32::from_rgb(0xff, 0x9a, 0xb0));
                p.circle_filled(c + vec2(-r * 0.22, -r * 0.2), r * 0.07, Color32::BLACK);
                p.circle_filled(c + vec2(r * 0.22, -r * 0.2), r * 0.07, Color32::BLACK);
            }
            if picked {
                p.circle_stroke(c + vec2(0.0, r * 0.25), r * 1.2, Stroke::new(3.0, if mole_here { GREEN } else { RED }));
            }
        }
        if auto && open.is_none() {
            // the plan: which rounds hit this hole
            let rounds_here: Vec<String> = app.settings.orig.moles_picks.iter().enumerate().filter(|(_, h)| **h as usize == i).map(|(k, _)| format!("{}", k + 1)).collect();
            if !rounds_here.is_empty() {
                p.circle_stroke(c + vec2(0.0, r * 0.25), r * 1.2, Stroke::new(2.0, alpha(Color32::from_rgb(0x8b, 0x5c, 0xf6), 220)));
                p.text(c + vec2(0.0, r * 1.6), Align2::CENTER_CENTER, rounds_here.join(" "), theme::font_semi(11.0), Color32::from_rgb(0xb8, 0x9c, 0xff));
            }
        }
        if resp.clicked() {
            clicked = Some(i as u8);
        }
    }
    if let Some(i) = clicked {
        if open.is_some() {
            app.orig_step(Game::Moles, Step::Next, json!({ "pick": i }));
        } else {
            let max = moles_rounds(app.settings.orig.moles_count as u32) as usize;
            if app.settings.orig.moles_picks.len() < max {
                app.settings.orig.moles_picks.push(i);
                app.mark_dirty();
            }
        }
    }

    // the rounds: hits so far and what the next one pays
    let p = ui.painter();
    let total = moles_rounds(moles_count) as usize;
    let y = rect.bottom() - 34.0;
    let w = 46.0;
    let x0 = rect.center().x - (total as f32 * (w + 6.0)) / 2.0;
    for k in 0..total {
        let cell = Rect::from_min_size(pos2(x0 + k as f32 * (w + 6.0), y - 11.0), vec2(w, 22.0));
        let pl = played.get(k).filter(|pl| pl.pick.is_some());
        let (fill, fg) = match pl.and_then(|pl| pl.hit) {
            Some(true) => (alpha(GREEN, 60), GREEN),
            Some(false) => (alpha(RED, 60), RED),
            None if open.is_some() && k == current => (alpha(GOLD, 50), GOLD),
            None => (alpha(Color32::BLACK, 120), TEXT),
        };
        super::chip(p, cell, &times(moles_multiplier(moles_count, k as u32 + 1)), fill, fg);
    }
    if shown.is_none() && !auto {
        super::note(p, pos2(rect.center().x, y - 26.0), "Bet, then whack a hole each round");
    }
    let _ = MUTED;
}
