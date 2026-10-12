//! Flip: call heads or tails; every right call pays ×1.96 more, a wrong one ends it.
//! Manual: call coin by coin and cash out. Auto: set the calls up front.

use egui::{pos2, vec2, Align2, Color32, Pos2, Rect, Stroke, Ui};
use rand::Rng;
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, GREEN, MUTED, RED};
use super::super::widgets::{button, caption, chip_button_sized, Btn};
use super::super::App;
use super::times;
use crate::originals::{flip_multiplier, Game, Round, Step};
use crate::settings::Mode;

const MAX_CALLS: usize = 20;

fn strs(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect()).unwrap_or_default()
}

/// A coin: gold heads, steel tails, or a blank still to flip.
fn coin(p: &egui::Painter, c: Pos2, r: f32, side: Option<&str>, ring: Option<Color32>) {
    let (face, mark) = match side {
        Some("heads") => (Color32::from_rgb(0xff, 0xc1, 0x3d), "H"),
        Some("tails") => (Color32::from_rgb(0x7f, 0xa6, 0xd9), "T"),
        _ => (Color32::from_rgb(0x2a, 0x34, 0x4a), ""),
    };
    p.circle_filled(c + vec2(0.0, r * 0.12), r, mix(face, Color32::BLACK, 0.4));
    p.circle_filled(c, r, face);
    p.circle_stroke(c, r * 0.78, Stroke::new((r * 0.06).max(1.0), alpha(Color32::WHITE, 90)));
    if !mark.is_empty() {
        p.text(c, Align2::CENTER_CENTER, mark, theme::font_bold(r * 0.9), mix(face, Color32::BLACK, 0.55));
    }
    if let Some(rc) = ring {
        p.circle_stroke(c, r + 3.0, Stroke::new(2.0, rc));
    }
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if app.settings.mode != Mode::Auto {
        super::nothing_to_set(ui, "Bet, then call each flip on the board and cash out any time.");
        return;
    }
    caption(ui, "Calls, in order");
    let calls = app.settings.orig.flip_calls.clone();
    let mut changed = false;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for (i, c) in calls.iter().enumerate() {
            let label = if c == "heads" { "H" } else { "T" };
            if chip_button_sized(ui, label, c == "heads", !locked, vec2(28.0, 28.0)).on_hover_text("Click to switch").clicked() {
                app.settings.orig.flip_calls[i] = if c == "heads" { "tails".into() } else { "heads".into() };
                changed = true;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 12.0) / 3.0;
        if chip_button_sized(ui, "+ flip", false, !locked && calls.len() < MAX_CALLS, vec2(w, 28.0)).clicked() {
            app.settings.orig.flip_calls.push("heads".into());
            changed = true;
        }
        if chip_button_sized(ui, "− flip", false, !locked && calls.len() > 1, vec2(w, 28.0)).clicked() {
            app.settings.orig.flip_calls.pop();
            changed = true;
        }
        if chip_button_sized(ui, "Random", false, !locked, vec2(w, 28.0)).clicked() {
            let mut rng = rand::rng();
            for c in app.settings.orig.flip_calls.iter_mut() {
                *c = if rng.random_bool(0.5) { "heads".into() } else { "tails".into() };
            }
            changed = true;
        }
    });
    let n = app.settings.orig.flip_calls.len() as u32;
    super::readout(ui, format!("{}  ·  pays {} if every call is right", super::count(n as u64, "flip"), times(flip_multiplier(n))));
    if changed {
        app.mark_dirty();
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    if !auto {
        return Ok(json!({ "guesses": [] }));
    }
    let calls = &app.settings.orig.flip_calls;
    if calls.is_empty() {
        return Err("Add at least one call".into());
    }
    Ok(json!({ "guesses": calls }))
}

pub fn result_text(r: &Round) -> String {
    let n = strs(&r.state["playedRounds"]).len();
    if r.multiplier > 0.0 {
        format!("{n} right calls")
    } else {
        format!("Wrong on flip {n}")
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Flip).cloned();
    let busy = app.orig.busy.contains(&Game::Flip);
    let shown = open.clone().or_else(|| app.orig.last(Game::Flip).cloned());
    let calls = shown.as_ref().map(|r| strs(&r.state["playedRounds"])).unwrap_or_default();
    let flips = shown.as_ref().map(|r| strs(&r.state["flips"])).unwrap_or_default();
    let age = app.orig.age(Game::Flip);

    // the big coin: the latest flip
    let big = (rect.height() * 0.2).clamp(40.0, 80.0);
    let c = pos2(rect.center().x, rect.top() + 30.0 + big);
    let latest = if open.is_some() { calls.last().cloned() } else { calls.len().checked_sub(1).and_then(|i| flips.get(i).cloned()).or(calls.last().cloned()) };
    let spin = (age / 0.35).clamp(0.0, 1.0);
    let p = ui.painter();
    let squash = if spin < 1.0 { (spin * std::f32::consts::PI * 3.0).cos().abs().max(0.15) } else { 1.0 };
    let r = big * squash.sqrt();
    coin(p, c, r, latest.as_deref(), None);
    if spin < 1.0 {
        ui.ctx().request_repaint();
    }
    let so_far = shown.as_ref().map(|r| if r.active { flip_multiplier(calls.len() as u32) } else { r.multiplier }).unwrap_or(0.0);
    if shown.is_some() {
        p.text(pos2(c.x, c.y + big + 22.0), Align2::CENTER_CENTER, times(so_far), theme::font_bold(20.0), if so_far > 0.0 { GREEN } else { RED });
    }

    // call buttons while a round is open
    if open.is_some() {
        let next = flip_multiplier(calls.len() as u32 + 1);
        let row = Rect::from_center_size(pos2(rect.center().x, c.y + big + 66.0), vec2(360.0f32.min(rect.width() - 40.0), 40.0));
        let w = (row.width() - 10.0) / 2.0;
        let mut pick: Option<&str> = None;
        for (i, (side, label)) in [("heads", "Heads"), ("tails", "Tails")].iter().enumerate() {
            let b = Rect::from_min_size(pos2(row.left() + i as f32 * (w + 10.0), row.top()), vec2(w, 40.0));
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(b));
            if button(&mut bui, &format!("{label}  ·  {}", times(next)), Btn::Primary, !busy && calls.len() < MAX_CALLS, Some(w)).clicked() {
                pick = Some(side);
            }
        }
        if let Some(side) = pick {
            app.orig_step(Game::Flip, Step::Next, json!({ "condition": side }));
        }
    }

    // the calls so far: green when the coin agreed, red where it didn't
    let n = calls.len().max(if open.is_none() && app.settings.mode == Mode::Auto { app.settings.orig.flip_calls.len() } else { 0 });
    if n > 0 {
        let p = ui.painter();
        let sr = 16.0;
        let total = n as f32 * (sr * 2.0 + 8.0) - 8.0;
        let x0 = rect.center().x - total / 2.0 + sr;
        let y = rect.bottom() - 34.0;
        for i in 0..n {
            let cc = pos2(x0 + i as f32 * (sr * 2.0 + 8.0), y);
            match calls.get(i) {
                Some(call) => {
                    let actual = flips.get(i).cloned().unwrap_or_else(|| call.clone());
                    let right = &actual == call;
                    coin(p, cc, sr, Some(&actual), Some(if right { GREEN } else { RED }));
                    p.text(pos2(cc.x, cc.y + sr + 10.0), Align2::CENTER_CENTER, if call == "heads" { "H" } else { "T" }, theme::font(10.0), MUTED);
                }
                None => {
                    let planned = app.settings.orig.flip_calls.get(i).cloned();
                    coin(p, cc, sr, None, None);
                    if let Some(pl) = planned {
                        p.text(pos2(cc.x, cc.y + sr + 10.0), Align2::CENTER_CENTER, if pl == "heads" { "H" } else { "T" }, theme::font(10.0), MUTED);
                    }
                }
            }
        }
    } else if open.is_none() {
        super::note(ui.painter(), pos2(rect.center().x, rect.bottom() - 30.0), "Bet, then call heads or tails");
    }
}
