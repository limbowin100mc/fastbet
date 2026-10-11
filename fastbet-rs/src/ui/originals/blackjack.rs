//! Blackjack: beat the dealer's hand without going over 21. Hit, stand, double, split, insurance.

use egui::{pos2, vec2, Align2, Rect, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, GOLD, GREEN, MUTED, RED, TEXT, TEXT2};
use super::super::widgets::Btn;
use super::super::App;
use super::{board_button, card, card_back, cards_of, times};
use crate::originals::{Game, Round, Step};

pub fn params(_app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("Bet to deal, then play the hand on the board. Blackjack pays 3:2, insurance 2:1.").size(11.0).color(MUTED));
}

pub fn bet_params() -> Result<Value, String> {
    Ok(json!({}))
}

struct Hand {
    value: i64,
    actions: Vec<String>,
    cards: Vec<(String, String)>,
}

fn hands(v: &Value) -> Vec<Hand> {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|h| Hand {
                    value: h["value"].as_i64().unwrap_or(0),
                    actions: h["actions"].as_array().map(|x| x.iter().filter_map(|s| s.as_str()).map(str::to_string).collect()).unwrap_or_default(),
                    cards: cards_of(&h["cards"]),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn done(h: &Hand) -> bool {
    h.actions.iter().any(|a| matches!(a.as_str(), "stand" | "bust" | "full" | "double" | "blackjack"))
}

pub fn result_text(r: &Round) -> String {
    let player = hands(&r.state["player"]);
    let dealer = hands(&r.state["dealer"]);
    let you: Vec<String> = player.iter().map(|h| h.value.to_string()).collect();
    format!("{} vs {}", you.join(" / "), dealer.first().map(|d| d.value).unwrap_or(0))
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Blackjack).cloned();
    let busy = app.orig.busy.contains(&Game::Blackjack);
    let shown = open.clone().or_else(|| app.orig.last(Game::Blackjack).cloned());
    let Some(r) = shown else {
        super::note(ui.painter(), rect.center(), "Bet to deal a hand");
        return;
    };
    let player = hands(&r.state["player"]);
    let dealer = hands(&r.state["dealer"]);
    let cw = (rect.height() * 0.17).clamp(48.0, 84.0);
    let p = ui.painter();

    // dealer: the hole card stays face down while the round is open
    let dy = rect.top() + 20.0 + cw * 0.7;
    if let Some(d) = dealer.first() {
        let mut cards = d.cards.clone();
        let hidden = open.is_some() && cards.len() == 1;
        let n = cards.len() + hidden as usize;
        let total = n as f32 * (cw + 8.0) - 8.0;
        let x0 = rect.center().x - total / 2.0;
        for (i, (rank, suit)) in cards.drain(..).enumerate() {
            card(p, Rect::from_min_size(pos2(x0 + i as f32 * (cw + 8.0), dy - cw * 0.7), vec2(cw, cw * 1.4)), &rank, &suit, false);
        }
        if hidden {
            card_back(p, Rect::from_min_size(pos2(x0 + (n - 1) as f32 * (cw + 8.0), dy - cw * 0.7), vec2(cw, cw * 1.4)));
        }
        p.text(pos2(x0 - 14.0, dy), Align2::RIGHT_CENTER, format!("Dealer {}", d.value), theme::font_semi(13.0), TEXT2);
    }

    // the player's hands (two after a split); the one being played is lit
    let py = dy + cw * 1.4 + 40.0;
    let current = if open.is_some() { player.iter().position(|h| !done(h)) } else { None };
    let slot = rect.width() / player.len().max(1) as f32;
    for (k, h) in player.iter().enumerate() {
        let cx = rect.left() + slot * (k as f32 + 0.5);
        let total = h.cards.len() as f32 * (cw * 0.62) + cw * 0.38;
        let x0 = cx - total / 2.0;
        for (i, (rank, suit)) in h.cards.iter().enumerate() {
            card(p, Rect::from_min_size(pos2(x0 + i as f32 * cw * 0.62, py - cw * 0.7 + i as f32 * 4.0), vec2(cw, cw * 1.4)), rank, suit, false);
        }
        let c = if current == Some(k) {
            GOLD
        } else if h.value > 21 {
            RED
        } else {
            TEXT
        };
        p.text(pos2(cx, py + cw * 0.85 + 12.0), Align2::CENTER_CENTER, format!("{}", h.value), theme::font_bold(16.0), c);
    }

    // actions, or how the round ended
    let row_y = rect.bottom() - 34.0;
    if open.is_some() {
        let hand = current.and_then(|i| player.get(i));
        let two = hand.is_some_and(|h| h.cards.len() == 2 && h.actions.len() <= 2);
        let pair = hand.is_some_and(|h| h.cards.len() == 2 && h.cards[0].0 == h.cards[1].0) && player.len() == 1;
        let dealer_ace = dealer.first().and_then(|d| d.cards.first()).is_some_and(|c| c.0 == "A");
        let insurance_open = dealer_ace && player.len() == 1 && player[0].actions.iter().all(|a| a == "deal");
        let mut actions: Vec<(&str, &str)> = Vec::new();
        if insurance_open {
            actions.push(("insurance", "Insurance"));
            actions.push(("noInsurance", "No insurance"));
        } else {
            actions.push(("hit", "Hit"));
            actions.push(("stand", "Stand"));
            if two {
                actions.push(("double", "Double"));
            }
            if pair {
                actions.push(("split", "Split"));
            }
        }
        let w = ((rect.width() - 60.0) / actions.len() as f32 - 8.0).min(150.0);
        let x0 = rect.center().x - (w + 8.0) * actions.len() as f32 / 2.0 + 4.0;
        let mut pick = None;
        for (i, (key, label)) in actions.iter().enumerate() {
            let b = Rect::from_min_size(pos2(x0 + i as f32 * (w + 8.0), row_y - 20.0), vec2(w, 40.0));
            if board_button(ui, b, label, if i == 0 { Btn::Primary } else { Btn::Ghost }, !busy) {
                pick = Some(*key);
            }
        }
        if let Some(a) = pick {
            app.orig_step(Game::Blackjack, Step::Next, json!({ "action": a }));
        }
    } else {
        let (txt, c) = if r.multiplier > 1.0 {
            (format!("You win  ·  {}", times(r.multiplier)), GREEN)
        } else if (r.multiplier - 1.0).abs() < 1e-9 {
            ("Push".to_string(), GOLD)
        } else {
            ("Dealer wins".to_string(), RED)
        };
        p.text(pos2(rect.center().x, row_y), Align2::CENTER_CENTER, txt, theme::font_bold(18.0), c);
    }
}
