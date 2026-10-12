//! Blitz (Shuffle): cards are drawn from an endless deck; it pays when every one
//! of them is different. The board is the whole deck, 4 suits by 13 ranks, and
//! the drawn cards light up in order; a repeat ends the round.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GREEN, RED};
use super::super::widgets::{chip_button_sized, field};
use super::super::App;
use super::{card, payout_color, times};
use crate::originals::{blitz_card, blitz_multiplier, Game, Round, BLITZ_PICKS};

const QUICK: &[u8] = &[5, 8, 10, 13, 16, 20];
/// Rows top to bottom and columns left to right, as the site lays out the deck.
const SUIT_ROWS: [&str; 4] = ["C", "S", "H", "D"];
const RANK_COLS: [&str; 13] = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];

fn picks(app: &App) -> u32 {
    (app.settings.orig.blitz_picks as u32).clamp(BLITZ_PICKS.0, BLITZ_PICKS.1)
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let mut s = app.settings.orig.blitz_picks.to_string();
    let range = format!("{}–{}", BLITZ_PICKS.0, BLITZ_PICKS.1);
    if field(ui, "Unique cards", &mut s, "10", Some(&range), !locked).changed() {
        if let Ok(n) = s.trim().parse::<u32>() {
            app.settings.orig.blitz_picks = n.clamp(BLITZ_PICKS.0, BLITZ_PICKS.1) as u8;
            app.mark_dirty();
        }
    }
    ui.horizontal(|ui| {
        let gap = 5.0;
        ui.spacing_mut().item_spacing.x = gap;
        let n = QUICK.len() as f32;
        let cw = ((ui.available_width() - gap * (n - 1.0)) / n).floor();
        for q in QUICK {
            if chip_button_sized(ui, &q.to_string(), app.settings.orig.blitz_picks == *q, !locked, vec2(cw, 28.0)).clicked() {
                app.settings.orig.blitz_picks = *q;
                app.mark_dirty();
            }
        }
    });
    let n = picks(app);
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

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let last = app.orig.last(Game::Blitz).cloned();
    let age = app.orig.age(Game::Blitz);
    let (cards, repeat) = last.as_ref().map(draw).unwrap_or_default();
    // the draw plays out in about a second, stopping at the repeat
    let shown_len = repeat.map(|i| i + 1).unwrap_or(cards.len());
    let step = if shown_len == 0 { 0.0 } else { (0.9 / shown_len as f32).min(0.06) };
    let revealed = if step == 0.0 { 0 } else { ((age / step) as usize + 1).min(shown_len) };
    let done = revealed >= shown_len;
    if last.is_some() && !done {
        ui.ctx().request_repaint();
    }

    // the deck: 13 columns by 4 rows, cards 0.7 as wide as tall
    let footer = 70.0;
    let area = Rect::from_min_max(pos2(rect.left() + 20.0, rect.top() + 16.0), pos2(rect.right() - 20.0, rect.bottom() - footer));
    let gap = 5.0;
    let ch = ((area.height() - gap * 3.0) / 4.0).min((area.width() - gap * 12.0) / 13.0 / 0.7);
    let cw = ch * 0.7;
    let total = vec2(cw * 13.0 + gap * 12.0, ch * 4.0 + gap * 3.0);
    let origin = pos2(area.center().x - total.x / 2.0, area.center().y - total.y / 2.0);
    let cell = |index: u32| {
        let (rank, suit) = blitz_card(index);
        let row = SUIT_ROWS.iter().position(|s| *s == suit).unwrap_or(0) as f32;
        let col = RANK_COLS.iter().position(|r| *r == rank).unwrap_or(0) as f32;
        Rect::from_min_size(origin + vec2(col * (cw + gap), row * (ch + gap)), vec2(cw, ch))
    };

    // how often each card has come up so far, and the step it first came up
    let mut hits = [0u8; 52];
    for c in &cards[..revealed] {
        hits[*c as usize] = hits[*c as usize].saturating_add(1);
    }
    let won = last.as_ref().is_some_and(|r| r.multiplier > 0.0);
    let p = ui.painter();
    for (row, suit) in SUIT_ROWS.iter().enumerate() {
        for (col, rank) in RANK_COLS.iter().enumerate() {
            let r = Rect::from_min_size(origin + vec2(col as f32 * (cw + gap), row as f32 * (ch + gap)), vec2(cw, ch));
            let index = blitz_index(rank, suit);
            let n = hits[index as usize];
            card(p, r, rank, suit, n == 0);
            if n == 0 {
                p.rect_filled(r, CornerRadius::same((cw * 0.09) as u8), alpha(Color32::BLACK, 120));
            }
        }
    }
    // the drawn cards: outlined (green on a win), the repeat in red with a second card on top
    for c in hits.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, _)| i as u32) {
        let r = cell(c);
        let twice = hits[c as usize] > 1;
        let color = if twice { RED } else if won && done { GREEN } else { theme::G2 };
        p.rect_stroke(r.expand(1.5), CornerRadius::same((cw * 0.12) as u8), Stroke::new(2.0, color), StrokeKind::Outside);
        if twice {
            let (rank, suit) = blitz_card(c);
            let over = r.translate(vec2(cw * 0.22, -ch * 0.08));
            card(p, over, rank, suit, false);
            p.rect_stroke(over.expand(1.5), CornerRadius::same((cw * 0.12) as u8), Stroke::new(2.0, RED), StrokeKind::Outside);
        }
    }

    // progress through the draw, and the outcome
    let bar = Rect::from_center_size(pos2(rect.center().x, rect.bottom() - footer + 22.0), vec2(total.x.min(rect.width() - 40.0), 6.0));
    p.rect_filled(bar, CornerRadius::same(3), alpha(Color32::WHITE, 18));
    if let Some(l) = &last {
        let len = l.state["picks"].as_u64().map(|n| n as usize).unwrap_or(cards.len()).max(1);
        let frac = (revealed as f32 / len as f32).clamp(0.0, 1.0);
        let lost = repeat.is_some_and(|i| revealed > i);
        let fill = Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height()));
        p.rect_filled(fill, CornerRadius::same(3), if lost { RED } else { GREEN });
        if done {
            let (txt, color) = if won { (times(l.multiplier), payout_color(l.multiplier)) } else { ("Repeat".to_string(), RED) };
            p.text(pos2(rect.center().x, bar.bottom() + 24.0), Align2::CENTER_CENTER, txt, theme::font_bold(20.0), color);
        }
    } else {
        super::note(p, pos2(rect.center().x, bar.bottom() + 24.0), "Every card different pays; one repeat ends it");
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

    #[test]
    fn grid_and_draw_agree() {
        for i in 0..52 {
            let (rank, suit) = blitz_card(i);
            assert_eq!(blitz_index(rank, suit), i);
        }
    }
}
