//! Dragon Tower: climb nine rows, picking a tile per row; eggs are safe, a dragon ends it.
//! Manual: pick row by row and cash out. Auto: set the path up front.

use egui::{pos2, vec2, Align2, Color32, Pos2, Rect, Shape, Stroke, Ui};
use rand::Rng;
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GOLD, MUTED, TEXT2};
use super::super::widgets::{caption, chip_button_sized, segmented};
use super::super::App;
use super::{hit, tile, tile_face, times, Tile};
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

/// An egg standing on its tile.
fn egg(p: &egui::Painter, c: Pos2, size: f32, color: Color32) {
    let pts: Vec<Pos2> = (0..28)
        .map(|i| {
            let a = i as f32 / 28.0 * std::f32::consts::TAU;
            let (s, co) = a.sin_cos();
            // narrower at the top
            let rx = size * 0.3 * if s < 0.0 { 0.82 } else { 1.0 };
            pos2(c.x + rx * co, c.y + size * 0.38 * s)
        })
        .collect();
    p.add(Shape::convex_polygon(pts, color, Stroke::NONE));
    p.circle_filled(pos2(c.x - size * 0.09, c.y - size * 0.12), size * 0.07, alpha(Color32::WHITE, 150));
}

/// The dragon's eye: a red slit.
fn dragon(p: &egui::Painter, c: Pos2, size: f32) {
    let red = Color32::from_rgb(0xff, 0x3d, 0x5f);
    p.circle_filled(c, size * 0.3, alpha(red, 60));
    let pts = vec![pos2(c.x - size * 0.28, c.y), pos2(c.x, c.y - size * 0.16), pos2(c.x + size * 0.28, c.y), pos2(c.x, c.y + size * 0.16)];
    p.add(Shape::convex_polygon(pts, red, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![pos2(c.x, c.y - size * 0.13), pos2(c.x + size * 0.04, c.y), pos2(c.x, c.y + size * 0.13), pos2(c.x - size * 0.04, c.y)], Color32::BLACK, Stroke::NONE));
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

    let rows = DRAGON_ROWS as usize;
    let label_w = 70.0;
    let area = Rect::from_min_max(pos2(rect.left() + 24.0 + label_w, rect.top() + 14.0), pos2(rect.right() - 24.0, rect.bottom() - 14.0));
    let gap = 6.0;
    let cell_h = ((area.height() - gap * (rows as f32 - 1.0)) / rows as f32).min(54.0);
    let cell_w = ((area.width() - gap * (cols as f32 - 1.0)) / cols as f32).min(cell_h * 2.2).min(150.0);
    let total_w = cell_w * cols as f32 + gap * (cols as f32 - 1.0);
    let total_h = cell_h * rows as f32 + gap * (rows as f32 - 1.0);
    let x0 = area.center().x - total_w / 2.0;
    let y_bottom = area.center().y + total_h / 2.0;

    let mut clicked: Option<(usize, u8)> = None;
    for row in 0..rows {
        let y = y_bottom - (row as f32 + 1.0) * cell_h - row as f32 * gap;
        let m = dragon_multiplier(&difficulty, row as u32 + 1);
        let here = current == Some(row);
        ui.painter().text(pos2(x0 - 14.0, y + cell_h / 2.0), Align2::RIGHT_CENTER, times(m), theme::font_semi(12.0), if here { GOLD } else { MUTED });
        if here {
            // the row to pick from
            let band = Rect::from_min_size(pos2(x0 - 6.0, y - 4.0), vec2(total_w + 12.0, cell_h + 8.0));
            ui.painter().rect(band, egui::CornerRadius::same(8), alpha(GOLD, 14), Stroke::new(1.0, alpha(GOLD, 90)), egui::StrokeKind::Outside);
        }
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
            } else if auto_pick {
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
            tile(p, r, kind, resp.hovered() && can);
            let face = tile_face(r, kind);
            let s = face.height() * 0.85;
            match (kind, is_egg) {
                (Tile::Good, _) => egg(p, face.center(), s, Color32::from_rgb(0xff, 0xe8, 0xb0)),
                (Tile::Bad, _) => dragon(p, face.center(), s),
                (Tile::Faded, Some(true)) => egg(p, face.center(), s * 0.8, alpha(Color32::from_rgb(0xff, 0xe8, 0xb0), 70)),
                (Tile::Faded, Some(false)) => dragon(p, face.center(), s * 0.8),
                _ => {}
            }
            if auto && settled && app.settings.orig.dragon_path.get(row).copied() == Some(col as u8) {
                p.rect_stroke(r, egui::CornerRadius::same(6), Stroke::new(2.0, alpha(Color32::from_rgb(0x8b, 0x5c, 0xf6), 220)), egui::StrokeKind::Inside);
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
    let _ = TEXT2;
}
