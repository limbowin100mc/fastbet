//! Bars: pick up to five of 30 tiles; each hides a multiplier, and the picked ones add up.

use egui::{pos2, vec2, Align2, Color32, Rect, Ui};
use rand::seq::IndexedRandom;
use serde_json::{json, Value};

use super::super::theme::{self, MUTED, TEXT};
use super::super::widgets::chip_button_sized;
use super::super::App;
use super::{grid, hit, levels, payout_color, tile, tile_face, trim, Tile};
use crate::originals::{Game, Round, LEVELS, LEVEL_NAMES};

const TILES: usize = 30;
const MAX_PICKS: usize = 5;

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    if levels(ui, &mut app.settings.orig.bars_difficulty, &LEVELS, &LEVEL_NAMES, !locked) {
        app.mark_dirty();
    }
    let n = app.settings.orig.bars_tiles.len();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 6.0) / 2.0;
        if chip_button_sized(ui, "Random pick", false, !locked, vec2(w, 30.0)).clicked() {
            let all: Vec<u8> = (0..TILES as u8).collect();
            let mut v: Vec<u8> = all.choose_multiple(&mut rand::rng(), if n == 0 { MAX_PICKS } else { n }).copied().collect();
            v.sort_unstable();
            app.settings.orig.bars_tiles = v;
            app.mark_dirty();
        }
        if chip_button_sized(ui, "Clear", false, !locked && n > 0, vec2(w, 30.0)).clicked() {
            app.settings.orig.bars_tiles.clear();
            app.mark_dirty();
        }
    });
    if n == 0 {
        super::readout_idle(ui, "Pick 1–5 tiles on the board");
    } else {
        super::readout(ui, format!("{n} of 5 picked"));
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    let tiles = &app.settings.orig.bars_tiles;
    if tiles.is_empty() {
        return Err("Pick at least one tile".into());
    }
    Ok(json!({ "difficulty": app.settings.orig.bars_difficulty, "tiles": tiles }))
}

pub fn result_text(r: &Round) -> String {
    format!("{} tiles · {}", super::nums(&r.state["tiles"]).len(), r.state["difficulty"].as_str().unwrap_or(""))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let picks = app.settings.orig.bars_tiles.clone();
    let last = app.orig.last(Game::Bars).cloned();
    let shown = last.filter(|r| super::nums(&r.state["tiles"]).iter().map(|t| *t as u8).collect::<Vec<_>>() == picks);
    let values: Vec<f64> = shown.as_ref().and_then(|r| r.state["board"].as_array().map(|a| a.iter().filter_map(|v| v.as_f64()).collect())).unwrap_or_default();
    let area = Rect::from_min_max(rect.min + vec2(24.0, 14.0), pos2(rect.max.x - 24.0, rect.max.y - 40.0));
    let cells = grid(area, 6, 5, 10.0, 84.0);
    let mut toggle = None;
    for (i, cell) in cells.iter().enumerate() {
        let picked = picks.contains(&(i as u8));
        let value = values.get(i).copied();
        let kind = match (value, picked) {
            (Some(_), true) => Tile::Good,
            (Some(_), false) => Tile::Faded,
            (None, true) => Tile::Picked,
            (None, false) => Tile::Idle,
        };
        let can = !locked && (picked || picks.len() < MAX_PICKS);
        let resp = hit(ui, *cell, ("bars", i), can);
        let p = ui.painter();
        tile(p, *cell, kind, resp.hovered() && can);
        let face = tile_face(*cell, kind);
        if let Some(v) = value {
            let c = payout_color(v);
            // a bar as tall as the multiplier is big, against the board's best
            let best = values.iter().copied().fold(0.0, f64::max).max(1.0);
            let h = face.height() * 0.7 * (v / best).sqrt() as f32;
            let bar = Rect::from_min_max(pos2(face.center().x - face.width() * 0.16, face.bottom() - 6.0 - h), pos2(face.center().x + face.width() * 0.16, face.bottom() - 6.0));
            p.rect_filled(bar, egui::CornerRadius::same(3), if picked { c } else { super::alpha(c, 90) });
            p.text(pos2(face.center().x, face.top() + 12.0), Align2::CENTER_CENTER, format!("{}×", trim(v)), theme::font_semi(11.0), if picked { TEXT } else { MUTED });
        } else if picked {
            p.text(face.center(), Align2::CENTER_CENTER, "✓", theme::font_bold(16.0), Color32::WHITE);
        }
        if resp.clicked() {
            toggle = Some(i as u8);
        }
    }
    if let Some(n) = toggle {
        let list = &mut app.settings.orig.bars_tiles;
        if let Some(i) = list.iter().position(|x| *x == n) {
            list.remove(i);
        } else if list.len() < MAX_PICKS {
            list.push(n);
            list.sort_unstable();
        }
        app.mark_dirty();
    }
    let p = ui.painter();
    let txt = match &shown {
        Some(r) => format!("Picked tiles add up to {}", super::times(r.multiplier)),
        None => "Pick up to five tiles, then bet".into(),
    };
    super::note(p, pos2(rect.center().x, rect.bottom() - 20.0), &txt);
}
