//! Mines: 25 tiles, some hide mines. Manual: reveal one at a time and cash out.
//! Auto: pick the tiles up front; every bet reveals them all.

use egui::{pos2, vec2, Align2, Rect, Ui};
use rand::seq::IndexedRandom;
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GREEN, MUTED, TEXT2};
use super::super::widgets::{caption, chip_button_sized};
use super::super::App;
use super::{bomb, gem, grid, hit, nums, tile, tile_face, times, Tile};
use crate::originals::{mines_multiplier, Game, Round, Step};
use crate::settings::Mode;

fn auto_mode(app: &App) -> bool {
    app.settings.mode == Mode::Auto
}

/// Tiles revealed so far in a round.
fn revealed(r: &Round) -> Vec<u8> {
    r.state["rounds"].as_array().map(|a| a.iter().filter_map(|x| x["field"].as_u64()).map(|f| f as u8).collect()).unwrap_or_default()
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Mines).is_some();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let w = (ui.available_width() - 8.0) / 2.0;
        ui.vertical(|ui| {
            ui.set_width(w);
            caption(ui, "Mines");
            let mut n = app.settings.orig.mines_count;
            ui.add_enabled_ui(!locked && !open, |ui| {
                egui::ComboBox::from_id_salt("mines-count").width(w).selected_text(format!("{n}")).show_ui(ui, |ui| {
                    for k in 1..=24u8 {
                        ui.selectable_value(&mut n, k, format!("{k}"));
                    }
                });
            });
            if n != app.settings.orig.mines_count {
                app.settings.orig.mines_count = n;
                app.settings.orig.mines_picks.truncate((25 - n) as usize);
                app.mark_dirty();
            }
        });
        ui.vertical(|ui| {
            ui.set_width(w);
            caption(ui, "Gems");
            let gems = 25 - app.settings.orig.mines_count;
            let (r, _) = ui.allocate_exact_size(vec2(w, super::super::widgets::FIELD_H), egui::Sense::hover());
            ui.painter().rect(r, egui::CornerRadius::same(8), theme::BG2, egui::Stroke::new(1.0, theme::LINE), egui::StrokeKind::Inside);
            ui.painter().text(pos2(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, format!("{gems}"), theme::font_semi(13.0), TEXT2);
        });
    });
    if auto_mode(app) {
        let n = app.settings.orig.mines_picks.len() as u32;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = (ui.available_width() - 6.0) / 2.0;
            if chip_button_sized(ui, "Random pick", false, !locked, vec2(w, 30.0)).on_hover_text("Random tiles (as many as picked now, else 3)").clicked() {
                let k = if n == 0 { 3 } else { n as usize };
                let all: Vec<u8> = (0..25).collect();
                let mut v: Vec<u8> = all.choose_multiple(&mut rand::rng(), k.min(25 - app.settings.orig.mines_count as usize)).copied().collect();
                v.sort_unstable();
                app.settings.orig.mines_picks = v;
                app.mark_dirty();
            }
            if chip_button_sized(ui, "Clear picks", false, !locked && n > 0, vec2(w, 30.0)).clicked() {
                app.settings.orig.mines_picks.clear();
                app.mark_dirty();
            }
        });
        let txt = if n == 0 {
            "Click tiles on the board: every bet reveals them".to_string()
        } else {
            format!("{n} tiles  ·  pays {} if all are gems", times(mines_multiplier(app.settings.orig.mines_count as u32, n)))
        };
        ui.label(egui::RichText::new(txt).size(11.0).color(MUTED));
    } else if open {
        let w = ui.available_width();
        let busy = app.orig.busy.contains(&Game::Mines);
        if chip_button_sized(ui, "Pick random tile", false, !busy, vec2(w, 30.0)).clicked() {
            reveal_random(app);
        }
    }
}

fn reveal_random(app: &mut App) {
    let Some(r) = app.orig.open(Game::Mines) else { return };
    let shown = revealed(r);
    let left: Vec<u8> = (0..25).filter(|i| !shown.contains(i)).collect();
    if let Some(i) = left.choose(&mut rand::rng()).copied() {
        app.orig_step(Game::Mines, Step::Next, json!({ "fields": [i] }));
    }
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let mines = app.settings.orig.mines_count.clamp(1, 24);
    if !auto {
        return Ok(json!({ "minesCount": mines }));
    }
    let picks = &app.settings.orig.mines_picks;
    if picks.is_empty() {
        return Err("Pick the tiles every bet reveals".into());
    }
    Ok(json!({ "minesCount": mines, "fields": picks }))
}

pub fn result_text(r: &Round) -> String {
    let gems = r.state["rounds"].as_array().map(|a| a.iter().filter(|x| x["payoutMultiplier"].as_f64().unwrap_or(0.0) > 0.0).count()).unwrap_or(0);
    let mines = r.state["minesCount"].as_u64().unwrap_or(0);
    if r.multiplier > 0.0 {
        format!("{gems} gems · {mines} mines")
    } else {
        format!("Mine after {gems} · {mines} mines")
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let auto = auto_mode(app);
    let open = app.orig.open(Game::Mines).cloned();
    let busy = app.orig.busy.contains(&Game::Mines);
    let last = app.orig.last(Game::Mines).cloned();
    let shown = open.clone().or(last);
    let picked: Vec<u8> = shown.as_ref().map(revealed).unwrap_or_default();
    let mines: Vec<i64> = shown.as_ref().filter(|r| !r.active).map(|r| nums(&r.state["mines"])).unwrap_or_default();
    let settled = shown.as_ref().is_some_and(|r| !r.active);

    let area = Rect::from_min_max(rect.min + vec2(24.0, 16.0), pos2(rect.max.x - 24.0, rect.max.y - 44.0));
    let cells = grid(area, 5, 5, 10.0, 96.0);
    let mut clicked: Option<u8> = None;
    for (i, cell) in cells.iter().enumerate() {
        let n = i as u8;
        let was_picked = picked.contains(&n);
        let is_mine = mines.contains(&(i as i64));
        let auto_pick = auto && open.is_none() && app.settings.orig.mines_picks.contains(&n);
        let kind = if open.is_some() {
            if was_picked {
                Tile::Good
            } else if busy {
                Tile::Off
            } else {
                Tile::Idle
            }
        } else if auto_pick && !settled {
            Tile::Picked
        } else if settled {
            match (was_picked, is_mine) {
                (true, true) => Tile::Bad,
                (true, false) => Tile::Good,
                _ => Tile::Faded,
            }
        } else if auto_pick {
            Tile::Picked
        } else {
            Tile::Idle
        };
        let can = if open.is_some() { !was_picked && !busy } else { auto && !locked };
        let resp = hit(ui, *cell, ("mines", i), can);
        let p = ui.painter();
        // in auto mode the settled board keeps showing the picks over the result
        tile(p, *cell, kind, resp.hovered() && can);
        let face = tile_face(*cell, kind);
        let s = face.height() * 0.62;
        match kind {
            Tile::Good => gem(p, face.center(), s, GREEN),
            Tile::Bad => bomb(p, face.center(), s * 1.1),
            Tile::Faded => {
                if is_mine {
                    let c = face.center();
                    bomb(&p.with_clip_rect(face), c, s * 0.8);
                    p.rect_filled(face, egui::CornerRadius::same(6), alpha(egui::Color32::BLACK, 120));
                } else {
                    gem(p, face.center(), s * 0.75, alpha(GREEN, 90));
                }
            }
            _ => {}
        }
        if auto && settled && app.settings.orig.mines_picks.contains(&n) {
            p.rect_stroke(*cell, egui::CornerRadius::same(8), egui::Stroke::new(2.0, alpha(egui::Color32::from_rgb(0x8b, 0x5c, 0xf6), 220)), egui::StrokeKind::Inside);
        }
        if resp.clicked() {
            clicked = Some(n);
        }
    }
    if let Some(n) = clicked {
        if open.is_some() {
            app.orig_step(Game::Mines, Step::Next, json!({ "fields": [n] }));
        } else if auto {
            let list = &mut app.settings.orig.mines_picks;
            if let Some(i) = list.iter().position(|x| *x == n) {
                list.remove(i);
            } else if list.len() < (25 - app.settings.orig.mines_count) as usize {
                list.push(n);
                list.sort_unstable();
            }
            app.mark_dirty();
        }
    }

    // the line under the board: what the next tile pays
    let p = ui.painter();
    let y = rect.bottom() - 24.0;
    let m = app.settings.orig.mines_count as u32;
    let txt = match &open {
        Some(r) => {
            let gems = revealed(r).len() as u32;
            format!("{} so far  ·  next tile pays {}", times(r.state["rounds"].as_array().and_then(|a| a.last()).and_then(|x| x["payoutMultiplier"].as_f64()).unwrap_or(1.0)), times(mines_multiplier(r.state["minesCount"].as_u64().unwrap_or(m as u64) as u32, gems + 1)))
        }
        None if auto => format!("{m} mines  ·  click tiles to pick them"),
        None => format!("{m} mines  ·  first gem pays {}", times(mines_multiplier(m, 1))),
    };
    super::note(p, pos2(rect.center().x, y), &txt);
}
