//! Keno: pick up to 10 of 40 numbers; ten are drawn. Pays by risk and hits.

use egui::{pos2, vec2, Align2, Color32, Rect, Ui};
use rand::seq::SliceRandom;
use serde_json::{json, Value};

use super::super::theme::{self, alpha, BG2, GREEN, MUTED, RED, TEXT};
use super::super::widgets::{caption, chip_button_sized, segmented};
use super::super::App;
use super::{chip, gem, grid, hit, nums, payout_color, tile, tile_face, times, Tile};
use crate::originals::{Game, Round};

const RISKS: [&str; 4] = ["classic", "low", "medium", "high"];
const MAX_PICKS: usize = 10;

/// Named picks (0-based numbers on the 8 × 5 board).
const PATTERNS: &[(&str, &[u8])] = &[
    ("Top row", &[0, 1, 2, 3, 4, 5, 6, 7]),
    ("Diagonal", &[0, 9, 18, 27, 36]),
    ("Corners", &[0, 7, 32, 39]),
    ("Middle", &[10, 11, 12, 13, 18, 19, 20, 21, 26, 27]),
    ("Checker", &[0, 2, 4, 6, 9, 11, 13, 15, 16, 18]),
];

fn random_picks(n: usize) -> Vec<u8> {
    let mut all: Vec<u8> = (0..40).collect();
    all.shuffle(&mut rand::rng());
    let mut v = all[..n.clamp(1, MAX_PICKS)].to_vec();
    v.sort_unstable();
    v
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Risk");
    let sel = RISKS.iter().position(|r| *r == app.settings.orig.keno_risk).unwrap_or(0);
    if let Some(i) = segmented(ui, &["Classic", "Low", "Medium", "High"], sel, !locked) {
        app.settings.orig.keno_risk = RISKS[i].into();
        app.mark_dirty();
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 6.0) / 2.0;
        if chip_button_sized(ui, "Auto pick", false, !locked, vec2(w, 30.0)).on_hover_text("Random numbers (as many as picked now, else 10)").clicked() {
            let n = match app.settings.orig.keno_picks.len() {
                0 => MAX_PICKS,
                n => n,
            };
            app.settings.orig.keno_picks = random_picks(n);
            app.mark_dirty();
        }
        if chip_button_sized(ui, "Clear table", false, !locked && !app.settings.orig.keno_picks.is_empty(), vec2(w, 30.0)).clicked() {
            app.settings.orig.keno_picks.clear();
            app.mark_dirty();
        }
    });
    caption(ui, "Patterns");
    // an even grid, three to a row
    for row in PATTERNS.chunks(3) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = ((ui.available_width() - 12.0) / 3.0).floor();
            for (name, picks) in row {
                let on = app.settings.orig.keno_picks == *picks;
                if chip_button_sized(ui, name, on, !locked, vec2(w, 28.0)).clicked() {
                    app.settings.orig.keno_picks = picks.to_vec();
                    app.mark_dirty();
                }
            }
        });
    }
    let n = app.settings.orig.keno_picks.len();
    if n == 0 {
        super::readout_idle(ui, "Pick 1–10 numbers on the board");
    } else {
        super::readout(ui, format!("{n} of 10 picked"));
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    let picks = &app.settings.orig.keno_picks;
    if picks.is_empty() {
        return Err("Pick at least one number".into());
    }
    Ok(json!({ "numbers": picks, "risk": app.settings.orig.keno_risk }))
}

fn hits(r: &Round) -> (usize, usize) {
    let drawn = nums(&r.state["drawnNumbers"]);
    let picked = nums(&r.state["selectedNumbers"]);
    (picked.iter().filter(|n| drawn.contains(n)).count(), picked.len())
}

pub fn result_text(r: &Round) -> String {
    let (h, n) = hits(r);
    format!("{h} of {n} hit")
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let picks = app.settings.orig.keno_picks.clone();
    let last = app.orig.last(Game::Keno).cloned();
    // the latest draw stays on the board until the picks change
    let shown = last.filter(|r| nums(&r.state["selectedNumbers"]).iter().map(|n| *n as u8).collect::<Vec<_>>() == picks);
    let drawn: Vec<i64> = shown.as_ref().map(|r| nums(&r.state["drawnNumbers"])).unwrap_or_default();
    let age = app.orig.age(Game::Keno);

    let area = Rect::from_min_max(rect.min + vec2(24.0, 18.0), pos2(rect.max.x - 24.0, rect.max.y - 70.0));
    let cells = grid(area, 8, 5, 10.0, 86.0);
    let mut toggle: Option<u8> = None;
    for (i, cell) in cells.iter().enumerate() {
        let n = i as u8;
        let picked = picks.contains(&n);
        // drawn numbers appear one after another
        let order = drawn.iter().position(|d| *d == i as i64);
        let revealed = order.is_some_and(|k| age > k as f32 * 0.05);
        let kind = match (picked, revealed) {
            (true, true) => Tile::Good,
            (true, false) => Tile::Picked,
            (false, true) => Tile::Bad,
            (false, false) => Tile::Idle,
        };
        let can = !locked && (picked || picks.len() < MAX_PICKS);
        let resp = hit(ui, *cell, ("keno", i), can);
        let p = ui.painter();
        tile(p, *cell, kind, resp.hovered() && can);
        let face = tile_face(*cell, kind);
        let fs = (cell.height() * 0.3).clamp(11.0, 20.0);
        match kind {
            Tile::Good => {
                gem(p, face.center(), face.height() * 0.62, GREEN);
                p.text(face.center() + vec2(0.0, face.height() * 0.32), Align2::CENTER_CENTER, format!("{}", i + 1), theme::font_bold(fs * 0.62), TEXT);
            }
            Tile::Bad => {
                p.text(face.center(), Align2::CENTER_CENTER, format!("{}", i + 1), theme::font_bold(fs), RED);
            }
            _ => {
                p.text(face.center(), Align2::CENTER_CENTER, format!("{}", i + 1), theme::font_bold(fs), if picked { Color32::WHITE } else { TEXT });
            }
        }
        if resp.clicked() {
            toggle = Some(n);
        }
    }
    if order_pending(&drawn, age) {
        ui.ctx().request_repaint();
    }
    if let Some(n) = toggle {
        let list = &mut app.settings.orig.keno_picks;
        if let Some(i) = list.iter().position(|x| *x == n) {
            list.remove(i);
        } else if list.len() < MAX_PICKS {
            list.push(n);
            list.sort_unstable();
        }
        app.mark_dirty();
    }

    // what each number of hits pays for the current picks
    let p = ui.painter();
    let table = app.orig.tables.keno(&app.settings.orig.keno_risk, picks.len()).to_vec();
    let strip = Rect::from_min_max(pos2(cells[0].left(), rect.bottom() - 58.0), pos2(cells[7].right(), rect.bottom() - 16.0));
    if table.is_empty() {
        super::note(p, strip.center(), "Pick numbers to see what the hits pay");
        return;
    }
    let got = shown.as_ref().map(|r| hits(r).0);
    let w = (strip.width() - 4.0 * (table.len() as f32 - 1.0)) / table.len() as f32;
    for (h, m) in table.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(strip.left() + h as f32 * (w + 4.0), strip.top()), vec2(w, 20.0));
        let on = got == Some(h);
        let c = payout_color(*m);
        chip(p, cell, &times(*m), if on { c } else { BG2 }, if on { super::text_color_on(c) } else { alpha(c, 230) });
        // the number of hits, with a little gem
        let label = p.layout_no_wrap(format!("{h}"), theme::font_semi(10.5), if on { TEXT } else { MUTED });
        let lw = label.size().x + 12.0;
        let lx = cell.center().x - lw / 2.0;
        p.galley(pos2(lx, cell.bottom() + 5.0), label, MUTED);
        gem(p, pos2(lx + lw - 4.0, cell.bottom() + 12.0), 9.0, if on { GREEN } else { alpha(GREEN, 120) });
    }
}

fn order_pending(drawn: &[i64], age: f32) -> bool {
    !drawn.is_empty() && age < drawn.len() as f32 * 0.05 + 0.1
}
