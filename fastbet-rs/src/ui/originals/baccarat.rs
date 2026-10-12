//! Baccarat: back the player, the banker or a tie; the hand closest to nine wins.

use egui::{pos2, vec2, Align2, Rect, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, GOLD, GREEN, MUTED, TEXT2};
use super::super::widgets::{caption, chip_button_sized};
use super::super::App;
use super::{card_row, cards_of};
use crate::originals::{Game, Round};

const SIDES: [(&str, &str); 3] = [("player", "Player  2×"), ("banker", "Banker  1.95×"), ("tie", "Tie  9×")];

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    caption(ui, "Bet on");
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let w = (ui.available_width() - 12.0) / 3.0;
        for (key, label) in SIDES {
            let on = app.settings.orig.baccarat_sides.iter().any(|s| s == key);
            if chip_button_sized(ui, label, on, !locked, vec2(w, 32.0)).clicked() {
                let sides = &mut app.settings.orig.baccarat_sides;
                if on {
                    sides.retain(|s| s != key);
                } else {
                    sides.push(key.to_string());
                }
                changed = true;
            }
        }
    });
    if changed {
        app.mark_dirty();
    }
    let n = app.settings.orig.baccarat_sides.len();
    if n == 0 {
        super::readout_idle(ui, "Pick at least one side");
    } else {
        super::readout(ui, format!("Stakes {n}× the bet size per round"));
    }
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    if app.settings.orig.baccarat_sides.is_empty() {
        return Err("Pick a side to bet on".into());
    }
    Ok(json!({ "sides": app.settings.orig.baccarat_sides }))
}

/// Baccarat value of a hand: tens and pictures count 0, the total mod 10.
fn total(cards: &[(String, String)]) -> u32 {
    cards
        .iter()
        .map(|(r, _)| match r.as_str() {
            "A" => 1,
            "10" | "J" | "Q" | "K" => 0,
            n => n.parse().unwrap_or(0),
        })
        .sum::<u32>()
        % 10
}

pub fn result_text(r: &Round) -> String {
    let player = total(&cards_of(&r.state["playerCards"]));
    let banker = total(&cards_of(&r.state["bankerCards"]));
    let winner = match player.cmp(&banker) {
        std::cmp::Ordering::Greater => "Player",
        std::cmp::Ordering::Less => "Banker",
        std::cmp::Ordering::Equal => "Tie",
    };
    format!("{winner}  {player}–{banker}")
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::Baccarat).cloned();
    let p = ui.painter();
    let Some(r) = last else {
        super::note(p, rect.center(), "Pick a side, then bet");
        return;
    };
    let player = cards_of(&r.state["playerCards"]);
    let banker = cards_of(&r.state["bankerCards"]);
    let (pt, bt) = (total(&player), total(&banker));
    let cw = (rect.height() * 0.2).clamp(50.0, 90.0);
    let y = rect.center().y - 10.0;
    let half = rect.width() / 2.0;
    for (i, (name, cards, t, other)) in [("Player", &player, pt, bt), ("Banker", &banker, bt, pt)].into_iter().enumerate() {
        let cx = rect.left() + half * (i as f32 + 0.5);
        p.text(pos2(cx, y - cw * 0.7 - 34.0), Align2::CENTER_CENTER, name, theme::font_semi(14.0), TEXT2);
        card_row(p, pos2(cx, y), cards, cw, half - 60.0);
        let c = if t > other { GREEN } else if t == other { GOLD } else { MUTED };
        p.text(pos2(cx, y + cw * 0.7 + 26.0), Align2::CENTER_CENTER, format!("{t}"), theme::font_bold(26.0), c);
    }
    let winner = match pt.cmp(&bt) {
        std::cmp::Ordering::Greater => "Player wins",
        std::cmp::Ordering::Less => "Banker wins",
        std::cmp::Ordering::Equal => "Tie",
    };
    p.text(pos2(rect.center().x, rect.bottom() - 26.0), Align2::CENTER_CENTER, winner, theme::font_bold(18.0), if r.multiplier >= 1.0 { GREEN } else { MUTED });
    let _ = vec2(0.0, 0.0);
}
