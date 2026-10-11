//! HiLo: guess whether the next card is higher or lower; each right guess multiplies the payout.

use egui::{pos2, vec2, Align2, Rect, Ui};
use rand::seq::IndexedRandom;
use serde_json::{json, Value};

use super::super::theme::{self, GREEN, MUTED, RED, TEXT2};
use super::super::widgets::{button, Btn};
use super::super::App;
use super::{card, hit, times};
use crate::originals::{hilo_guesses, hilo_multiplier, Game, Round, Step, RANKS, SUITS};

pub fn random_card() -> (String, String) {
    let mut rng = rand::rng();
    (RANKS.choose(&mut rng).unwrap_or(&"7").to_string(), SUITS.choose(&mut rng).unwrap_or(&"H").to_string())
}

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Hilo).is_some();
    let txt = if open {
        "Guess on the board, or cash out at the top."
    } else {
        "Click the card to draw another start card."
    };
    ui.label(egui::RichText::new(txt).size(11.0).color(MUTED));
    let _ = locked;
}

pub fn bet_params(app: &App) -> Result<Value, String> {
    let (rank, suit) = &app.orig.hilo_card;
    Ok(json!({ "startCard": { "rank": rank, "suit": suit } }))
}

fn cards(r: &Round) -> Vec<(String, String, Option<String>, f64)> {
    let mut out = vec![(
        r.state["startCard"]["rank"].as_str().unwrap_or("?").to_string(),
        r.state["startCard"]["suit"].as_str().unwrap_or("S").to_string(),
        None,
        0.0,
    )];
    for x in r.state["rounds"].as_array().into_iter().flatten() {
        out.push((
            x["card"]["rank"].as_str().unwrap_or("?").to_string(),
            x["card"]["suit"].as_str().unwrap_or("S").to_string(),
            x["guess"].as_str().map(str::to_string),
            x["payoutMultiplier"].as_f64().unwrap_or(0.0),
        ));
    }
    out
}

pub fn result_text(r: &Round) -> String {
    let n = cards(r).len().saturating_sub(1);
    if r.multiplier > 0.0 {
        format!("Cashed out after {n} cards")
    } else {
        format!("Wrong on card {n}")
    }
}

fn guess_label(g: &str) -> &'static str {
    match g {
        "higher" => "Higher",
        "lower" => "Lower",
        "equal" => "Same",
        "higherEqual" => "Higher or same",
        "lowerEqual" => "Lower or same",
        "skip" => "Skip",
        _ => "Guess",
    }
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Hilo).cloned();
    let busy = app.orig.busy.contains(&Game::Hilo);
    let shown = open.clone().or_else(|| app.orig.last(Game::Hilo).cloned());
    let trail = shown.as_ref().map(cards).unwrap_or_default();
    let (rank, suit) = match (&open, trail.last()) {
        (Some(_), Some(c)) => (c.0.clone(), c.1.clone()),
        _ => app.orig.hilo_card.clone(),
    };

    let ch = (rect.height() * 0.42).clamp(120.0, 210.0);
    let cw = ch * 0.7;
    let cr = Rect::from_center_size(pos2(rect.center().x, rect.top() + 24.0 + ch / 2.0), vec2(cw, ch));
    let resp = hit(ui, cr, "hilo-card", open.is_none() && !app.auto.running);
    card(ui.painter(), cr, &rank, &suit, false);
    if resp.on_hover_text("Draw another start card").clicked() {
        app.orig.hilo_card = random_card();
    }
    ui.painter().text(pos2(cr.center().x, cr.top() - 12.0), Align2::CENTER_CENTER, if open.is_some() { "Current card" } else { "Start card" }, theme::font_semi(11.0), MUTED);

    // the guesses, with chance and payout
    let row = Rect::from_center_size(pos2(rect.center().x, cr.bottom() + 36.0), vec2((cw * 3.6).min(rect.width() - 60.0), 40.0));
    if let Some(r) = &open {
        // what the round stands at now; each guess multiplies it
        let so_far = r.state["rounds"].as_array().and_then(|a| a.last()).and_then(|x| x["payoutMultiplier"].as_f64()).filter(|m| *m > 0.0).unwrap_or(1.0);
        let guesses = hilo_guesses(&rank);
        let w = (row.width() - 16.0) / 3.0;
        let mut pick: Option<&str> = None;
        for (i, (g, chance)) in guesses.iter().enumerate() {
            let b = Rect::from_min_size(pos2(row.left() + i as f32 * (w + 8.0), row.top()), vec2(w, 40.0));
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(b));
            let label = format!("{}  ·  {}", guess_label(g), times(so_far * hilo_multiplier(*chance)));
            if button(&mut bui, &label, if i == 0 { Btn::Primary } else { Btn::Ghost }, !busy, Some(w)).on_hover_text(format!("{:.2}% chance", chance * 100.0)).clicked() {
                pick = Some(g);
            }
        }
        let b = Rect::from_min_size(pos2(row.left() + 2.0 * (w + 8.0), row.top()), vec2(w, 40.0));
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(b));
        if button(&mut bui, "Skip card", Btn::Ghost, !busy, Some(w)).clicked() {
            pick = Some("skip");
        }
        if let Some(g) = pick {
            app.orig_step(Game::Hilo, Step::Next, json!({ "guess": g }));
        }
    } else {
        super::note(ui.painter(), row.center(), "Bet to start from this card");
    }

    // the cards drawn so far
    if trail.len() > 1 || open.is_some() {
        let p = ui.painter();
        let sw = 44.0;
        let sh = sw * 1.4;
        let n = trail.len();
        let max_n = ((rect.width() - 40.0) / (sw + 8.0)) as usize;
        let skip = n.saturating_sub(max_n);
        let total = (n - skip) as f32 * (sw + 8.0) - 8.0;
        let x0 = rect.center().x - total / 2.0;
        let y = rect.bottom() - sh - 24.0;
        for (k, (rk, st, guess, m)) in trail.iter().skip(skip).enumerate() {
            let r = Rect::from_min_size(pos2(x0 + k as f32 * (sw + 8.0), y), vec2(sw, sh));
            card(p, r, rk, st, false);
            if let Some(g) = guess {
                let lost = *m <= 0.0 && g != "skip";
                let c = if g == "skip" { MUTED } else if lost { RED } else { GREEN };
                let txt = if g == "skip" { "skip".to_string() } else { times(*m) };
                p.text(pos2(r.center().x, r.bottom() + 11.0), Align2::CENTER_CENTER, txt, theme::font_semi(10.5), c);
            } else {
                p.text(pos2(r.center().x, r.bottom() + 11.0), Align2::CENTER_CENTER, "start", theme::font(10.5), TEXT2);
            }
        }
    }
}
