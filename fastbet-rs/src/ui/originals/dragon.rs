//! Dragon Tower: climb nine rows, picking a tile per row; eggs are safe, a dragon ends it.
//! Manual: pick row by row and cash out. Auto: set the path up front.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use rand::Rng;
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, BG1, BG2, BG3, DIM, G1, LINE, LINE2, MUTED, RED};
use super::super::widgets::{caption, chip_button_sized, segmented};
use super::super::App;
use super::{hit, mark, miss, times, Tile};
use crate::originals::{dragon_layout, dragon_multiplier, Game, Round, Step, DRAGON_ROWS};
use crate::settings::Mode;

const LEVELS: [&str; 5] = ["easy", "medium", "hard", "expert", "master"];

fn rows_of(v: &Value) -> Vec<Vec<i64>> {
    v.as_array().map(|a| a.iter().map(super::nums).collect()).unwrap_or_default()
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::DragonTower).is_some();
    caption(ui, "Difficulty");
    let sel = LEVELS.iter().position(|d| *d == app.settings.orig.dragon_difficulty).unwrap_or(0);
    if let Some(i) = segmented(ui, &["Easy", "Medium", "Hard", "Expert", "Master"], sel, !locked && !open) {
        app.settings.orig.dragon_difficulty = LEVELS[i].into();
        let cols = dragon_layout(LEVELS[i]).0 as u8;
        app.settings.orig.dragon_path.retain(|c| *c < cols);
        app.mark_dirty();
    }
    if app.settings.mode == Mode::Auto {
        let n = app.settings.orig.dragon_path.len();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = (ui.available_width() - 6.0) / 2.0;
            if chip_button_sized(ui, "Random path", false, !locked, vec2(w, 30.0)).on_hover_text("Random tiles for as many rows as set now, else 3").clicked() {
                let rows = if n == 0 { 3 } else { n };
                let cols = dragon_layout(&app.settings.orig.dragon_difficulty).0;
                let mut rng = rand::rng();
                app.settings.orig.dragon_path = (0..rows).map(|_| rng.random_range(0..cols) as u8).collect();
                app.mark_dirty();
            }
            if chip_button_sized(ui, "Clear path", false, !locked && n > 0, vec2(w, 30.0)).clicked() {
                app.settings.orig.dragon_path.clear();
                app.mark_dirty();
            }
        });
        if n == 0 {
            super::readout_idle(ui, "Click a tile per row on the board, from the bottom: every bet climbs that path");
        } else {
            super::readout(ui, format!("{}  ·  pays {} at the top of the path", super::count(n as u64, "row"), times(dragon_multiplier(&app.settings.orig.dragon_difficulty, n as u32))));
        }
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let d = &app.settings.orig.dragon_difficulty;
    if !auto {
        return Ok(json!({ "difficulty": d, "eggs": [] }));
    }
    let path = &app.settings.orig.dragon_path;
    if path.is_empty() {
        return Err("Set a path on the board".into());
    }
    Ok(json!({ "difficulty": d, "eggs": path }))
}

pub fn result_text(r: &Round) -> String {
    let climbed = super::nums(&r.state["tilesSelected"]).len();
    if r.multiplier > 0.0 {
        format!("{climbed} rows · {}", r.state["difficulty"].as_str().unwrap_or(""))
    } else {
        format!("Dragon on row {climbed}")
    }
}

const PURPLE: Color32 = Color32::from_rgb(0x8b, 0x5c, 0xf6);

/// A flat tile in the tower's frame. `live`: in the row to pick from.
fn cell(p: &egui::Painter, r: Rect, kind: Tile, hovered: bool, live: bool) {
    let (fill, line, width) = match kind {
        Tile::Idle if hovered => (BG3, G1, 1.5),
        Tile::Idle if live => (mix(BG2, BG3, 0.6), alpha(G1, 120), 1.0),
        Tile::Idle => (BG2, LINE2, 1.0),
        Tile::Off => (alpha(BG2, 150), LINE, 1.0),
        Tile::Picked => (alpha(PURPLE, 80), PURPLE, 1.5),
        Tile::Good => (alpha(G1, 24), G1, 1.5),
        Tile::Bad => (alpha(RED, 24), RED, 1.5),
        Tile::Faded => (BG2, LINE, 1.0),
    };
    p.rect(r, CornerRadius::same(6), fill, Stroke::new(width, line), StrokeKind::Inside);
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let auto = app.settings.mode == Mode::Auto;
    let open = app.orig.open(Game::DragonTower).cloned();
    let busy = app.orig.busy.contains(&Game::DragonTower);
    let shown = open.clone().or_else(|| app.orig.last(Game::DragonTower).cloned());
    let difficulty = shown.as_ref().filter(|r| r.active).and_then(|r| r.state["difficulty"].as_str().map(str::to_string)).unwrap_or_else(|| app.settings.orig.dragon_difficulty.clone());
    let (cols, _) = dragon_layout(&difficulty);
    let settled = shown.as_ref().is_some_and(|r| !r.active) && shown.as_ref().and_then(|r| r.state["difficulty"].as_str()) == Some(difficulty.as_str());
    let picks: Vec<i64> = shown.as_ref().filter(|_| open.is_some() || settled).map(|r| super::nums(&r.state["tilesSelected"])).unwrap_or_default();
    let layout = shown.as_ref().filter(|_| open.is_some() || settled).map(|r| {
        let full = rows_of(&r.state["rounds"]);
        if full.is_empty() {
            rows_of(&r.state["playedRounds"])
        } else {
            full
        }
    });
    let current = open.as_ref().map(|r| r.state["currentRound"].as_u64().unwrap_or(0) as usize);

    // the tower in a frame, the payout of each row on its left
    let rows = DRAGON_ROWS as usize;
    let label_w = 70.0;
    let pad = 10.0;
    let area = Rect::from_min_max(pos2(rect.left() + 24.0 + label_w + pad, rect.top() + 14.0 + pad), pos2(rect.right() - 24.0 - pad, rect.bottom() - 14.0 - pad));
    let gap = 6.0;
    let cell_h = ((area.height() - gap * (rows as f32 - 1.0)) / rows as f32).min(50.0);
    let cell_w = ((area.width() - gap * (cols as f32 - 1.0)) / cols as f32).min(cell_h * 2.4).min(150.0);
    let total_w = cell_w * cols as f32 + gap * (cols as f32 - 1.0);
    let total_h = cell_h * rows as f32 + gap * (rows as f32 - 1.0);
    let x0 = area.center().x - total_w / 2.0;
    let y_bottom = area.center().y + total_h / 2.0;
    let frame = Rect::from_min_max(pos2(x0 - pad, y_bottom - total_h - pad), pos2(x0 + total_w + pad, y_bottom + pad));
    ui.painter().rect(frame, CornerRadius::same(10), BG1, Stroke::new(1.0, LINE2), StrokeKind::Inside);
    let path_top = (auto && !app.settings.orig.dragon_path.is_empty()).then(|| app.settings.orig.dragon_path.len() - 1);

    let mut clicked: Option<(usize, u8)> = None;
    for row in 0..rows {
        let y = y_bottom - (row as f32 + 1.0) * cell_h - row as f32 * gap;
        let m = dragon_multiplier(&difficulty, row as u32 + 1);
        let here = current == Some(row);
        let label = if here {
            G1
        } else if path_top == Some(row) && open.is_none() {
            PURPLE
        } else {
            MUTED
        };
        ui.painter().text(pos2(frame.left() - 12.0, y + cell_h / 2.0), Align2::RIGHT_CENTER, times(m), theme::font_semi(12.0), label);
        for col in 0..cols as usize {
            let r = Rect::from_min_size(pos2(x0 + col as f32 * (cell_w + gap), y), vec2(cell_w, cell_h));
            let eggs = layout.as_ref().and_then(|l| l.get(row)).cloned();
            let picked = picks.get(row).copied() == Some(col as i64);
            let is_egg = eggs.as_ref().map(|e| e.contains(&(col as i64)));
            let auto_pick = auto && open.is_none() && app.settings.orig.dragon_path.get(row).copied() == Some(col as u8);
            let kind = if here {
                if busy {
                    Tile::Off
                } else {
                    Tile::Idle
                }
            } else if picked {
                if is_egg == Some(false) {
                    Tile::Bad
                } else {
                    Tile::Good
                }
            } else if auto_pick && !settled {
                Tile::Picked
            } else if is_egg.is_some() {
                Tile::Faded
            } else if open.is_some() {
                Tile::Off
            } else {
                Tile::Idle
            };
            let can = if open.is_some() { here && !busy } else { auto && !locked };
            let resp = hit(ui, r, ("dragon", row, col), can);
            let p = ui.painter();
            cell(p, r, kind, resp.hovered() && can, here);
            let s = (cell_h * 0.5).min(cell_w * 0.42);
            match (kind, is_egg) {
                (Tile::Good, _) => mark(p, r.center(), s, G1),
                (Tile::Bad, _) => miss(p, r.center(), s, RED),
                (Tile::Faded, Some(true)) => mark(p, r.center(), s, DIM),
                (Tile::Faded, Some(false)) => miss(p, r.center(), s, DIM),
                _ => {}
            }
            // after a round the auto path stays outlined where it wasn't played
            if auto_pick && settled && !picked {
                p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.5, alpha(PURPLE, 220)), StrokeKind::Inside);
            }
            if resp.clicked() {
                clicked = Some((row, col as u8));
            }
        }
    }
    if let Some((row, col)) = clicked {
        if open.is_some() {
            app.orig_step(Game::DragonTower, Step::Next, json!({ "egg": col }));
        } else {
            // the auto path climbs from the bottom without gaps
            let path = &mut app.settings.orig.dragon_path;
            if row < path.len() {
                if path[row] == col {
                    path.truncate(row);
                } else {
                    path[row] = col;
                }
            } else if row == path.len() {
                path.push(col);
            }
            app.mark_dirty();
        }
    }
    if open.is_none() && !auto && shown.is_none() {
        super::note(ui.painter(), pos2(rect.center().x, rect.bottom() - 8.0), "Bet to start climbing");
    }
}
