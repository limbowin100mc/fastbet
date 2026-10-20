//! Blitz (Shuffle): cards are drawn from an endless deck; it pays when every one
//! of them is different. The board is the whole deck, 4 suits by 13 ranks, and
//! the drawn cards light up in order; a repeat ends the round.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Painter, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, BG2, DIM, GREEN, LINE2, RED, TEXT2};
use super::super::widgets::int_field;
use super::super::App;
use super::{suit_color, suit_shape, times};
use crate::originals::{blitz_multiplier, Game, Round, BLITZ_PICKS};

/// Rows top to bottom and columns left to right, as the site lays out the deck.
const SUIT_ROWS: [&str; 4] = ["C", "S", "H", "D"];
const RANK_COLS: [&str; 13] = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];

fn picks(app: &App) -> u32 {
    (app.settings.orig.blitz_picks as u32).clamp(BLITZ_PICKS.0, BLITZ_PICKS.1)
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let mut n = picks(app);
    if int_field(ui, "Unique cards", &mut n, BLITZ_PICKS, !locked, true) {
        app.settings.orig.blitz_picks = n as u8;
        app.mark_dirty();
    }
    let txt = format!("Pays {} when all {n} cards are different", times(blitz_multiplier(n)));
    super::readout(ui, txt);
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    Ok(json!({ "picks": picks(app) }))
}

/// The drawn cards and where the first repeat is (its position in the draw), if any.
fn draw(r: &Round) -> (Vec<u32>, Option<usize>) {
    let cards: Vec<u32> = super::nums(&r.state["cards"]).into_iter().map(|c| c.clamp(0, 51) as u32).collect();
    let mut seen = [false; 52];
    let repeat = cards.iter().position(|c| std::mem::replace(&mut seen[*c as usize], true));
    (cards, repeat)
}

pub fn result_text(r: &Round) -> String {
    let (cards, repeat) = draw(r);
    match repeat {
        Some(i) => format!("Repeat on card {} of {}", i + 1, cards.len()),
        None => format!("{} different cards", cards.len()),
    }
}

/// A deck card: dark and grey until drawn, then white with its colours; `edge` outlines it.
fn deck_card(p: &Painter, r: Rect, rank: &str, suit: &str, drawn: bool, edge: Option<Color32>) {
    let cr = CornerRadius::same(((r.width() * 0.12) as u8).clamp(3, 6));
    let (fill, fg) = if drawn { (Color32::WHITE, suit_color(suit)) } else { (BG2, DIM) };
    p.rect(r, cr, fill, Stroke::new(1.0, if drawn { Color32::WHITE } else { LINE2 }), StrokeKind::Inside);
    p.text(r.left_top() + vec2(r.width() * 0.14, r.height() * 0.07), Align2::LEFT_TOP, rank, theme::font_bold((r.width() * 0.3).clamp(8.0, 15.0)), fg);
    suit_shape(p, pos2(r.center().x, r.bottom() - r.height() * 0.3), r.width() * 0.36, suit, fg);
    if let Some(c) = edge {
        p.rect_stroke(r.expand(1.0), CornerRadius::same(cr.nw + 1), Stroke::new(2.0, c), StrokeKind::Outside);
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::Blitz).cloned();
    let age = app.orig.age(Game::Blitz);
    let (cards, repeat) = last.as_ref().map(draw).unwrap_or_default();
    // the draw plays out in about a second, stopping at the repeat
    let shown_len = repeat.map(|i| i + 1).unwrap_or(cards.len());
    let step = if shown_len == 0 { 0.0 } else { (0.9 / shown_len as f32).min(0.06) };
    let revealed = if step == 0.0 { 0 } else { ((age / step) as usize).saturating_add(1).min(shown_len) };
    let done = revealed >= shown_len;
    if last.is_some() && (!done || age < 1.2) {
        ui.ctx().request_repaint();
    }

    // the deck: 13 ranks by 4 suits, cards 0.72 as wide as tall, never bigger than a real card
    let footer = 46.0;
    let area = Rect::from_min_max(pos2(rect.left() + 24.0, rect.top() + 16.0), pos2(rect.right() - 24.0, rect.bottom() - footer - 16.0));
    let gap = 6.0;
    let ch = ((area.height() - gap * 3.0) / 4.0).min((area.width() - gap * 12.0) / 13.0 / 0.72).min(74.0);
    let cw = ch * 0.72;
    let total = vec2(cw * 13.0 + gap * 12.0, ch * 4.0 + gap * 3.0);
    let origin = pos2(area.center().x - total.x / 2.0, area.center().y - total.y / 2.0 + footer / 2.0 - 8.0);
    let cell = |row: usize, col: usize| Rect::from_min_size(origin + vec2(col as f32 * (cw + gap), row as f32 * (ch + gap)), vec2(cw, ch));

    // how often each card has come up so far, and when it last came up
    let mut hits = [0u8; 52];
    let mut when = [0usize; 52];
    for (i, c) in cards[..revealed].iter().enumerate() {
        hits[*c as usize] = hits[*c as usize].saturating_add(1);
        when[*c as usize] = i;
    }
    let won = last.as_ref().is_some_and(|r| r.multiplier > 0.0);
    let lost = repeat.is_some_and(|i| revealed > i);
    let p = ui.painter();
    for (row, suit) in SUIT_ROWS.iter().enumerate() {
        for (col, rank) in RANK_COLS.iter().enumerate() {
            let index = blitz_index(rank, suit) as usize;
            let n = hits[index];
            let mut r = cell(row, col);
            if n > 0 {
                // a card pops a little as it is drawn
                let since = age - when[index] as f32 * step;
                if since < 0.14 {
                    r = r.expand(cw * 0.08 * (1.0 - since / 0.14));
                }
            }
            let edge = match n {
                0 => None,
                1 if won && done => Some(GREEN),
                1 => Some(alpha(GREEN, 170)),
                _ => Some(RED),
            };
            deck_card(p, r, rank, suit, n > 0, edge);
            if n > 1 {
                // the repeat: how many times it came up
                let badge = Rect::from_center_size(r.right_top() + vec2(-1.0, 1.0), vec2(20.0, 15.0));
                p.rect_filled(badge, CornerRadius::same(7), RED);
                p.text(badge.center(), Align2::CENTER_CENTER, format!("×{n}"), theme::font_bold(10.0), Color32::WHITE);
            }
        }
    }

    // progress through the draw, and the outcome
    let line_y = origin.y + total.y + 20.0;
    let bar = Rect::from_min_size(pos2(origin.x, line_y + 12.0), vec2(total.x, 4.0));
    p.rect_filled(bar, CornerRadius::same(2), alpha(Color32::WHITE, 16));
    if let Some(l) = &last {
        let len = l.state["picks"].as_u64().map(|n| n as usize).unwrap_or(cards.len()).max(1);
        p.text(pos2(origin.x, line_y), Align2::LEFT_CENTER, format!("{} / {len}", revealed.min(len)), theme::font_semi(12.0), TEXT2);
        let frac = (revealed as f32 / len as f32).clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(2), if lost { RED } else { GREEN });
        if done {
            let (txt, color) = match repeat {
                None if won => (format!("All unique  ·  {}", times(l.multiplier)), GREEN),
                Some(i) => (format!("Repeat on card {}", i + 1), RED),
                None => (String::new(), RED),
            };
            p.text(pos2(origin.x + total.x, line_y), Align2::RIGHT_CENTER, txt, theme::font_semi(12.0), color);
        }
    } else {
        super::note(p, pos2(rect.center().x, bar.bottom() + 18.0), "Every card different pays; one repeat ends it");
    }
}

/// Grid card -> Blitz card index (twos first, aces last; diamonds, hearts, spades, clubs).
fn blitz_index(rank: &str, suit: &str) -> u32 {
    const RANKS: [&str; 13] = ["2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K", "A"];
    const SUITS: [&str; 4] = ["D", "H", "S", "C"];
    let r = RANKS.iter().position(|x| *x == rank).unwrap_or(0) as u32;
    let s = SUITS.iter().position(|x| *x == suit).unwrap_or(0) as u32;
    r * 4 + s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::originals::blitz_card;

    #[test]
    fn grid_and_draw_agree() {
        for i in 0..52 {
            let (rank, suit) = blitz_card(i);
            assert_eq!(blitz_index(rank, suit), i);
        }
    }
}
