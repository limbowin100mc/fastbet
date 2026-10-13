//! Mines: 25 tiles, some hide mines. Manual: reveal one at a time and cash out.
//! Auto: pick the tiles up front; every bet reveals them all.

use egui::{pos2, vec2, Align2, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use rand::seq::IndexedRandom;
use serde_json::{json, Value};

use super::super::theme::{self, alpha, BG1, BG2, DIM, GREEN, LINE2, MUTED, TEXT, TEXT2};
use super::super::widgets::{self, caption, chip_button_sized};
use super::super::App;
use super::{bomb, gem, grid, hit, nums, tile, tile_face, times, Tile};
use crate::originals::{Game, Round, Step};
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
                super::super::widgets::field_combos(ui);
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
        if n == 0 {
            super::readout_idle(ui, "Click tiles on the board: every bet reveals them");
        } else {
            super::readout(ui, format!("{}  ·  pays {} if all are gems", super::count(n as u64, "tile"), times(app.orig.tables.mines(app.settings.orig.mines_count as u32, n))));
        }
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

    let area = Rect::from_min_max(rect.min + vec2(24.0, 16.0), pos2(rect.max.x - 24.0, rect.max.y - BAR_ROOM));
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

    stat_bar(app, ui, rect, open.as_ref());
}

/// Height of the readouts under the board, and the room they take with their margins.
const BAR_H: f32 = 32.0;
const BAR_ROOM: f32 = BAR_H + 30.0;

/// One readout under the board: "Payout  52,598.70× | $52.60".
struct Stat {
    label: &'static str,
    /// A multiplier shown before the value, when the value is money.
    mult: Option<String>,
    value: String,
}

/// Chance that the next `k` tiles are all gems, with `gems` already found (in %).
fn gem_chance(mines: u32, gems: u32, k: u32) -> f64 {
    (0..k).map(|i| (25.0 - (mines + gems + i) as f64) / (25.0 - (gems + i) as f64)).product::<f64>().max(0.0) * 100.0
}

/// A chance with enough digits to tell small ones apart: 91.67%, 0.001882%.
fn fmt_chance(pct: f64) -> String {
    if pct >= 1.0 || pct <= 0.0 {
        return format!("{pct:.2}%");
    }
    let decimals = (3 - pct.log10().floor() as i32).clamp(2, 10) as usize;
    format!("{pct:.decimals$}%")
}

/// A multiplier with two decimals, thousands grouped: 52,598.70×.
fn fmt_mult(m: f64) -> String {
    if m >= 1000.0 {
        format!("{}×", crate::money::group_thousands(&format!("{m:.2}")))
    } else {
        times(m)
    }
}

/// The readouts under the board, Antebot's: what the picked tiles pay and the chance they are all
/// gems (auto), or the cash out, the next gem and its chance (a manual round).
fn stat_bar(app: &mut App, ui: &mut Ui, rect: Rect, open: Option<&Round>) {
    let auto = auto_mode(app);
    let m = app.settings.orig.mines_count as u32;
    let picks = app.settings.orig.mines_picks.len() as u32;
    let cur = app.active_currency();
    let bet = app.to_base(&app.settings.amount, &cur);
    // money when the bet size is known, else just the multiplier
    let paid = |label: &'static str, mult: f64| match bet {
        Some(b) => Stat { label, mult: Some(fmt_mult(mult)), value: app.prim(crate::money::mul_f(b, mult), &cur, false) },
        None => Stat { label, mult: None, value: fmt_mult(mult) },
    };
    let stats: Vec<Stat> = match open {
        Some(r) => {
            let mines = r.state["minesCount"].as_u64().unwrap_or(m as u64) as u32;
            let gems = revealed(r).len() as u32;
            let now = r.state["rounds"].as_array().and_then(|a| a.last()).and_then(|x| x["payoutMultiplier"].as_f64()).unwrap_or(0.0);
            let cash = if gems == 0 { Stat { label: "Cash out", mult: None, value: "–".into() } } else { paid("Cash out", now) };
            vec![
                cash,
                paid("Next gem", app.orig.tables.mines(mines, gems + 1)),
                Stat { label: "Gem chance", mult: None, value: fmt_chance(gem_chance(mines, gems, 1)) },
            ]
        }
        None if auto && picks == 0 => vec![],
        None if auto => vec![
            paid("Payout", app.orig.tables.mines(m, picks)),
            Stat { label: "Win chance", mult: None, value: fmt_chance(gem_chance(m, 0, picks)) },
        ],
        None => vec![
            paid("First gem", app.orig.tables.mines(m, 1)),
            Stat { label: "Gem chance", mult: None, value: fmt_chance(gem_chance(m, 0, 1)) },
        ],
    };

    let mid_y = rect.bottom() - 15.0 - BAR_H / 2.0;
    if stats.is_empty() {
        super::note(ui.painter(), pos2(rect.center().x, mid_y), &format!("{m} mines  ·  click tiles to pick them, every bet reveals them"));
        return;
    }

    // lay the pills out first, to centre the row
    let p = ui.painter().clone();
    let (pad, gap, inner) = (12.0, 8.0, 8.0);
    let text = |s: &str, color| p.layout_no_wrap(s.to_string(), theme::font_semi(12.0), color);
    let pills: Vec<_> = stats
        .iter()
        .map(|s| {
            let label = text(s.label, MUTED);
            let mult = s.mult.as_deref().map(|t| text(t, TEXT2));
            let value = text(&s.value, TEXT);
            let w = pad * 2.0 + label.size().x + inner + mult.as_ref().map_or(0.0, |g| g.size().x + inner * 2.0 + 1.0) + value.size().x;
            (label, mult, value, w)
        })
        .collect();
    let clear = auto && open.is_none();
    let clear_label = text("Clear board", TEXT2);
    let clear_w = pad * 2.0 + 18.0 + clear_label.size().x;
    let total = pills.iter().map(|x| x.3).sum::<f32>() + gap * (pills.len() as f32 - 1.0) + if clear { gap + clear_w } else { 0.0 };
    let mut x = rect.center().x - total / 2.0;

    let pill = |r: Rect, hot: bool| {
        p.rect(r, CornerRadius::same(8), if hot { BG2 } else { BG1 }, Stroke::new(1.0, if hot { LINE2 } else { theme::LINE }), StrokeKind::Inside);
    };
    let at = |x: f32, g: &std::sync::Arc<egui::Galley>| pos2(x, mid_y - g.size().y / 2.0);
    for (label, mult, value, w) in pills {
        let r = Rect::from_min_size(pos2(x, mid_y - BAR_H / 2.0), vec2(w, BAR_H));
        pill(r, false);
        let mut cx = r.left() + pad;
        let lw = label.size().x;
        p.galley(at(cx, &label), label, MUTED);
        cx += lw + inner;
        if let Some(g) = mult {
            let gw = g.size().x;
            p.galley(at(cx, &g), g, TEXT2);
            cx += gw + inner;
            p.vline(cx, (mid_y - 7.0)..=(mid_y + 7.0), Stroke::new(1.0, LINE2));
            cx += 1.0 + inner;
        }
        p.galley(at(cx, &value), value, TEXT);
        x += w + gap;
    }
    if clear {
        let r = Rect::from_min_size(pos2(x, mid_y - BAR_H / 2.0), vec2(clear_w, BAR_H));
        let can = !app.auto.running && picks > 0;
        let resp = hit(ui, r, "mines-clear", can);
        let hot = can && resp.hovered();
        pill(r, hot);
        let color = if !can { DIM } else if hot { TEXT } else { TEXT2 };
        widgets::icon(&p, Rect::from_center_size(pos2(r.left() + pad + 6.0, mid_y), vec2(12.0, 12.0)), "trash", color);
        p.galley(at(r.left() + pad + 18.0, &clear_label), clear_label, color);
        if resp.clicked() {
            app.settings.orig.mines_picks.clear();
            app.mark_dirty();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readouts_match_antebot() {
        // 20 mines, 5 tiles picked: 0.99 × 53,130 (Antebot cuts it to 52,598.69×) and 0.001882%
        assert_eq!(fmt_mult(crate::originals::mines_multiplier(20, 5)), "52,598.70×");
        assert_eq!(fmt_chance(gem_chance(20, 0, 5)), "0.001882%");
        assert_eq!(fmt_chance(gem_chance(1, 0, 2)), "92.00%");
        // one mine left among 23 unrevealed tiles after 2 gems
        assert!((gem_chance(1, 2, 1) - 100.0 * 22.0 / 23.0).abs() < 1e-9);
    }
}
