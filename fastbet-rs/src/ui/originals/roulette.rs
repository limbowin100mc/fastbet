//! Roulette (single zero): put chips on numbers and outside bets; click adds a chip of the
//! bet size, right-click takes one off.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, mix, GOLD, GREEN, MUTED, TEXT};
use super::super::widgets::chip_button_sized;
use super::super::App;
use crate::originals::{roulette_wins, Game, Round, ROULETTE_REDS};

fn red(n: u32) -> bool {
    ROULETTE_REDS.contains(&n)
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let chips: u32 = app.settings.orig.roulette_chips.values().sum();
    if chips == 0 {
        super::readout_idle(ui, "Click the table to place chips of the bet size; right-click removes one");
    } else {
        super::readout(ui, format!("{chips} chips on the table"));
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 6.0) / 2.0;
        if chip_button_sized(ui, "Double chips", false, !locked && chips > 0, vec2(w, 30.0)).clicked() {
            for v in app.settings.orig.roulette_chips.values_mut() {
                *v *= 2;
            }
            app.mark_dirty();
        }
        if chip_button_sized(ui, "Clear table", false, !locked && chips > 0, vec2(w, 30.0)).clicked() {
            app.settings.orig.roulette_chips.clear();
            app.mark_dirty();
        }
    });
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    if app.settings.orig.roulette_chips.values().all(|c| *c == 0) {
        return Err("Put a chip on the table".into());
    }
    Ok(json!({ "chips": app.settings.orig.roulette_chips }))
}

pub fn result_text(r: &Round) -> String {
    let n = r.state["result"].as_u64().unwrap_or(0) as u32;
    let colour = if n == 0 {
        "green"
    } else if red(n) {
        "red"
    } else {
        "black"
    };
    format!("{n} {colour}")
}

/// One spot on the table: its key, label, area and fill.
struct Spot {
    key: String,
    label: String,
    rect: Rect,
    fill: Color32,
}

fn layout(rect: Rect) -> Vec<Spot> {
    let red_c = Color32::from_rgb(0xe8, 0x33, 0x4f);
    let black_c = Color32::from_rgb(0x2a, 0x34, 0x4a);
    let green_c = Color32::from_rgb(0x1e, 0xa8, 0x5a);
    let outside = Color32::from_rgb(0x1d, 0x2a, 0x3d);
    let cols = 14.0; // zero, 12 numbers, 2:1
    let cell = (rect.width() / cols).min(rect.height() / 5.2);
    let w = cell * cols;
    let x0 = rect.center().x - w / 2.0;
    let y0 = rect.top() + (rect.height() - cell * 5.0) / 2.0;
    let mut v = Vec::new();
    v.push(Spot { key: "number0".into(), label: "0".into(), rect: Rect::from_min_size(pos2(x0, y0), vec2(cell, cell * 3.0)), fill: green_c });
    for col in 0..12u32 {
        for row in 0..3u32 {
            let n = col * 3 + (3 - row);
            v.push(Spot {
                key: format!("number{n}"),
                label: n.to_string(),
                rect: Rect::from_min_size(pos2(x0 + cell * (col as f32 + 1.0), y0 + cell * row as f32), vec2(cell, cell)),
                fill: if red(n) { red_c } else { black_c },
            });
        }
    }
    for row in 0..3u32 {
        v.push(Spot {
            key: format!("row{}", 3 - row),
            label: "2:1".into(),
            rect: Rect::from_min_size(pos2(x0 + cell * 13.0, y0 + cell * row as f32), vec2(cell, cell)),
            fill: outside,
        });
    }
    for (i, (key, label)) in [("range0112", "1 to 12"), ("range1324", "13 to 24"), ("range2536", "25 to 36")].iter().enumerate() {
        v.push(Spot { key: key.to_string(), label: label.to_string(), rect: Rect::from_min_size(pos2(x0 + cell * (1.0 + 4.0 * i as f32), y0 + cell * 3.0), vec2(cell * 4.0, cell)), fill: outside });
    }
    let even: [(&str, &str, Color32); 6] = [
        ("range0118", "1 to 18", outside),
        ("parityEven", "Even", outside),
        ("colorRed", "", red_c),
        ("colorBlack", "", black_c),
        ("parityOdd", "Odd", outside),
        ("range1936", "19 to 36", outside),
    ];
    for (i, (key, label, fill)) in even.iter().enumerate() {
        v.push(Spot { key: key.to_string(), label: label.to_string(), rect: Rect::from_min_size(pos2(x0 + cell * (1.0 + 2.0 * i as f32), y0 + cell * 4.0), vec2(cell * 2.0, cell)), fill: *fill });
    }
    v
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let locked = app.auto.running;
    let last = app.orig.last(Game::Roulette).cloned();
    let result = last.as_ref().and_then(|r| r.state["result"].as_u64()).map(|n| n as u32);
    let age = app.orig.age(Game::Roulette);

    // the ball's number, top centre
    let head = Rect::from_min_size(rect.min, vec2(rect.width(), 70.0));
    let p = ui.painter();
    if let Some(n) = result {
        let c = if n == 0 {
            Color32::from_rgb(0x1e, 0xa8, 0x5a)
        } else if red(n) {
            Color32::from_rgb(0xe8, 0x33, 0x4f)
        } else {
            Color32::from_rgb(0x2a, 0x34, 0x4a)
        };
        let pop = if age < 0.2 { 1.0 + 0.2 * (1.0 - age / 0.2) } else { 1.0 };
        p.circle_filled(head.center(), 26.0 * pop, c);
        p.circle_stroke(head.center(), 26.0 * pop, Stroke::new(2.0, alpha(Color32::WHITE, 120)));
        p.text(head.center(), Align2::CENTER_CENTER, n.to_string(), theme::font_bold(20.0), Color32::WHITE);
        if age < 0.2 {
            ui.ctx().request_repaint();
        }
    } else {
        super::note(p, head.center(), "Place chips, then spin");
    }

    let table = Rect::from_min_max(pos2(rect.left() + 16.0, head.bottom()), pos2(rect.right() - 16.0, rect.bottom() - 10.0));
    let spots = layout(table);
    let mut change: Option<(String, i32)> = None;
    for s in &spots {
        let resp = ui.interact(s.rect, ui.id().with(("roulette", &s.key)), if locked { egui::Sense::hover() } else { egui::Sense::click() });
        let p = ui.painter();
        let won = result.is_some_and(|n| roulette_wins(&s.key, n));
        let fill = if resp.hovered() && !locked { mix(s.fill, Color32::WHITE, 0.12) } else { s.fill };
        p.rect_filled(s.rect.shrink(1.5), CornerRadius::same(4), fill);
        if won {
            p.rect_stroke(s.rect.shrink(1.5), CornerRadius::same(4), Stroke::new(2.0, GOLD), StrokeKind::Inside);
        }
        if !s.label.is_empty() {
            p.text(s.rect.center(), Align2::CENTER_CENTER, &s.label, theme::font_semi((s.rect.height() * 0.32).clamp(9.0, 14.0)), TEXT);
        } else {
            // red / black diamonds
            let c = s.rect.center();
            let d = s.rect.height() * 0.28;
            p.add(egui::Shape::convex_polygon(vec![pos2(c.x, c.y - d), pos2(c.x + d * 1.4, c.y), pos2(c.x, c.y + d), pos2(c.x - d * 1.4, c.y)], mix(s.fill, Color32::WHITE, 0.25), Stroke::NONE));
        }
        let chips = app.settings.orig.roulette_chips.get(&s.key).copied().unwrap_or(0);
        if chips > 0 {
            let cr = (s.rect.height() * 0.24).clamp(8.0, 14.0);
            let cc = pos2(s.rect.right() - cr - 2.0, s.rect.top() + cr + 2.0);
            p.circle_filled(cc, cr, if won { GREEN } else { GOLD });
            p.circle_stroke(cc, cr * 0.75, Stroke::new(1.0, alpha(Color32::BLACK, 120)));
            p.text(cc, Align2::CENTER_CENTER, chips.to_string(), theme::font_bold(cr * 1.0), Color32::BLACK);
        }
        if resp.clicked() {
            change = Some((s.key.clone(), 1));
        } else if resp.secondary_clicked() {
            change = Some((s.key.clone(), -1));
        }
    }
    if let Some((key, d)) = change {
        let entry = app.settings.orig.roulette_chips.entry(key.clone()).or_insert(0);
        *entry = (*entry as i32 + d).max(0) as u32;
        if *entry == 0 {
            app.settings.orig.roulette_chips.remove(&key);
        }
        app.mark_dirty();
    }
    let _ = MUTED;
}
