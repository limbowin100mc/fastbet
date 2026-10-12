//! HiLo: guess whether the next card is higher or lower; each right guess multiplies the payout.
//! chips.gg also takes guesses on the next card's colour and kind (number, face, ace).
//! Manual: guess on the board and cash out. Auto: every bet starts from the chosen card and
//! plays the pattern card by card until the round pays the Cash out or Stop multiplier.

use egui::{pos2, vec2, Align2, Rect, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, G1, G2, GREEN, MUTED, RED, TEXT, TEXT2};
use super::super::widgets::{button, caption, check_field_help, field_combos, help_icon_ui, semi, text_box, toggle_help, Btn};
use super::super::App;
use super::{card, card_back, times};
use crate::money;
use crate::originals::{chips_hilo_next, hilo_guesses, hilo_multiplier, hilo_side_guesses, parse_hilo_pattern, Game, Round, Step, HILO_CODES, RANKS};
use crate::settings::Mode;

const SUITS: [(&str, &str); 4] = [("S", "Spades"), ("H", "Hearts"), ("D", "Diamonds"), ("C", "Clubs")];

pub fn params(app: &mut App, ui: &mut Ui, locked: bool) {
    let open = app.orig.open(Game::Hilo).is_some();
    let o = &mut app.settings.orig;
    let mut dirty = false;

    caption(ui, "Start card");
    let pick = !locked && !open && !o.hilo_random;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        let w = ui.available_width();
        ui.add_enabled_ui(pick, |ui| {
            field_combos(ui);
            let mut rank = o.hilo_rank.clone();
            egui::ComboBox::from_id_salt("hilo-rank").width(64.0).selected_text(rank.as_str()).show_ui(ui, |ui| {
                for r in RANKS {
                    ui.selectable_value(&mut rank, r.to_string(), r);
                }
            });
            let mut suit = o.hilo_suit.clone();
            let name = SUITS.iter().find(|(k, _)| *k == suit).map(|(_, n)| *n).unwrap_or("Spades");
            egui::ComboBox::from_id_salt("hilo-suit").width(w - 64.0 - 8.0).selected_text(name).show_ui(ui, |ui| {
                for (k, n) in SUITS {
                    ui.selectable_value(&mut suit, k.to_string(), n);
                }
            });
            if rank != o.hilo_rank || suit != o.hilo_suit {
                (o.hilo_rank, o.hilo_suit) = (rank, suit);
                dirty = true;
            }
        });
    });
    dirty |= toggle_help(ui, "Random card every bet", Some("Every bet starts from a new random card instead of the one picked above."), &mut o.hilo_random, !locked && !open);

    if app.settings.mode == Mode::Auto {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            caption(ui, "Pattern");
            help_icon_ui(ui, pattern_help);
        });
        dirty |= text_box(ui, &mut o.hilo_pattern, "5", None, !locked, false).changed();
        let cash_help = "Takes the winnings once a round pays this much, and bets on.";
        dirty |= check_field_help(ui, "Cash out at  ≥", Some(cash_help), &mut o.hilo_cashout_on, &mut o.hilo_cashout, Some("×"), !locked);
        let stop_help = "Takes the winnings once a round pays this much, and ends the run.";
        dirty |= check_field_help(ui, "Stop at  ≥", Some(stop_help), &mut o.hilo_stop_on, &mut o.hilo_stop, Some("×"), !locked);
        match parse_hilo_pattern(&o.hilo_pattern).and(targets(o)) {
            Err(e) => super::readout_bad(ui, e),
            Ok(t) => super::readout(ui, plan_text(t)),
        }
    }
    if dirty {
        app.mark_dirty();
    }
}

/// The pattern codes, for the "?" by the pattern.
fn pattern_help(ui: &mut Ui) {
    ui.set_max_width(300.0);
    ui.label(semi("Pattern", 13.0, TEXT));
    ui.add(egui::Label::new(egui::RichText::new("One code per card, separated by commas, e.g. 2,2,2,5,5. The pattern starts over when it runs out.").size(11.5).color(MUTED)).wrap());
    ui.add_space(2.0);
    for (code, name, what) in HILO_CODES {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            let (r, _) = ui.allocate_exact_size(vec2(22.0, 20.0), egui::Sense::hover());
            ui.painter().rect_filled(r, egui::CornerRadius::same(5), alpha(G1, 34));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, code.to_string(), theme::font_bold(11.5), G2);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(semi(name, 12.0, TEXT));
                ui.add(egui::Label::new(egui::RichText::new(what).size(11.0).color(MUTED)).wrap());
            });
        });
    }
}

/// What the auto targets do, in a line: "Cashes out at 2× · stops at 100×".
fn plan_text((cash, stop): (Option<f64>, Option<f64>)) -> String {
    let x = |m: f64| format!("{}×", super::trim(m));
    match (cash, stop) {
        (Some(c), Some(s)) if c < s => format!("Cashes out at {} and bets on · stops at {}", x(c), x(s)),
        (_, Some(s)) => format!("Cashes out at {} and stops", x(s)),
        (Some(c), None) => format!("Cashes out at {} and bets on", x(c)),
        (None, None) => String::new(),
    }
}

/// Auto: the multipliers to take the winnings at and bet on (Cash out), and to take them at and
/// end the run (Stop), where they are turned on.
fn targets(o: &crate::settings::OriginalPrefs) -> Result<(Option<f64>, Option<f64>), String> {
    let read = |on: bool, value: &str, what: &str| match (on, money::num(value)) {
        (false, _) => Ok(None),
        (true, Some(m)) if m > 1.0 => Ok(Some(m)),
        (true, _) => Err(format!("{what} has to be more than 1×")),
    };
    let t = (read(o.hilo_cashout_on, &o.hilo_cashout, "Cash out")?, read(o.hilo_stop_on, &o.hilo_stop, "Stop")?);
    if t == (None, None) {
        return Err("Turn on Cash out or Stop: an auto round needs a multiplier to take the winnings at".into());
    }
    Ok(t)
}

/// Auto: the Stop multiplier, which ends the run like a hit.
pub fn stop_line(app: &App) -> Option<f64> {
    targets(&app.settings.orig).ok().and_then(|(_, stop)| stop)
}

pub fn bet_params(app: &App, auto: bool) -> Result<Value, String> {
    let o = &app.settings.orig;
    // a random card is drawn for every bet when it is placed
    let start = if o.hilo_random { json!("random") } else { json!({ "rank": o.hilo_rank, "suit": o.hilo_suit }) };
    if !auto {
        return Ok(json!({ "startCard": start }));
    }
    let at = match targets(o)? {
        (Some(c), Some(s)) => c.min(s),
        (Some(m), None) | (None, Some(m)) => m,
        (None, None) => unreachable!("targets wants one of them"),
    };
    let pattern = parse_hilo_pattern(&o.hilo_pattern)?;
    Ok(json!({ "startCard": start, "plan": { "cashout": at, "pattern": pattern } }))
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
        format!("Cashed out after {n} card{}", if n == 1 { "" } else { "s" })
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
        "red" => "Red",
        "black" => "Black",
        "number" => "Number",
        "face" => "Face card",
        "ace" => "Ace",
        _ => "Guess",
    }
}

/// What a right guess means, for the button's tooltip.
fn guess_hint(g: &str) -> &'static str {
    match g {
        "red" => "The next card is red (hearts or diamonds)",
        "black" => "The next card is black (clubs or spades)",
        "number" => "The next card is a number, 2 to 10",
        "face" => "The next card is a jack, queen or king",
        "ace" => "The next card is an ace",
        _ => "",
    }
}

/// One guess button in `slot`: the guess and what the round pays if it is right.
fn guess_button(ui: &mut Ui, slot: Rect, g: &str, chance: f64, pays: f64, kind: Btn, enabled: bool) -> bool {
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(slot));
    let label = format!("{}  ·  {}", guess_label(g), times(pays));
    let hint = match guess_hint(g) {
        "" => format!("{:.2}% chance", chance * 100.0),
        h => format!("{h} · {:.2}% chance", chance * 100.0),
    };
    button(&mut bui, &label, kind, enabled, Some(slot.width())).on_hover_text(hint).clicked()
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::Hilo).cloned();
    let busy = app.orig.busy.contains(&Game::Hilo);
    let shown = open.clone().or_else(|| app.orig.last(Game::Hilo).cloned());
    let trail = shown.as_ref().map(cards).unwrap_or_default();
    let random = open.is_none() && app.settings.orig.hilo_random;
    let (rank, suit) = match (&open, trail.last()) {
        (Some(_), Some(c)) => (c.0.clone(), c.1.clone()),
        _ => (app.settings.orig.hilo_rank.clone(), app.settings.orig.hilo_suit.clone()),
    };
    // the board is the auto run's while it plays
    let can = !busy && !app.auto.running;

    // chips.gg's side guesses take a second row: a smaller card makes room
    let sides = app.casino().hilo_side_guesses();
    let ch = if sides { (rect.height() * 0.33).clamp(110.0, 190.0) } else { (rect.height() * 0.42).clamp(120.0, 210.0) };
    let cw = ch * 0.7;
    // below the pill that says why an auto run ended
    let cr = Rect::from_center_size(pos2(rect.center().x, rect.top() + 48.0 + ch / 2.0), vec2(cw, ch));
    if random {
        card_back(ui.painter(), cr);
    } else {
        card(ui.painter(), cr, &rank, &suit, false);
    }
    let label = match (&open, random) {
        (Some(_), _) => "Current card",
        (None, true) => "Random start card",
        (None, false) => "Start card",
    };
    ui.painter().text(pos2(cr.center().x, cr.top() - 12.0), Align2::CENTER_CENTER, label, theme::font_semi(11.0), MUTED);

    // the guesses, with chance and payout
    let row_w = if sides { 540.0 } else { cw * 3.6 };
    let row = Rect::from_center_size(pos2(rect.center().x, cr.bottom() + 36.0), vec2(row_w.min(rect.width() - 60.0), 40.0));
    if let Some(r) = &open {
        // what the round stands at now; each guess multiplies it
        let last = r.state["rounds"].as_array().and_then(|a| a.last()).and_then(|x| x["payoutMultiplier"].as_f64());
        let pays = |chance: f64| if sides { chips_hilo_next(last, chance) } else { last.filter(|m| *m > 0.0).unwrap_or(1.0) * hilo_multiplier(chance) };
        let w = (row.width() - 16.0) / 3.0;
        let slot = |row: Rect, i: usize| Rect::from_min_size(pos2(row.left() + i as f32 * (w + 8.0), row.top()), vec2(w, 40.0));
        let mut pick: Option<&str> = None;
        for (i, (g, chance)) in hilo_guesses(&rank).iter().enumerate() {
            if guess_button(ui, slot(row, i), g, *chance, pays(*chance), if i == 0 { Btn::Primary } else { Btn::Ghost }, can) {
                pick = Some(*g);
            }
        }
        let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(slot(row, 2)));
        if button(&mut bui, "Skip card", Btn::Ghost, can, Some(w)).clicked() {
            pick = Some("skip");
        }
        if sides {
            let row = row.translate(vec2(0.0, 48.0));
            for (i, (g, chance)) in hilo_side_guesses(&rank).iter().enumerate() {
                if guess_button(ui, slot(row, i), g, *chance, pays(*chance), Btn::Ghost, can) {
                    pick = Some(*g);
                }
            }
        }
        if let Some(g) = pick {
            app.orig_step(Game::Hilo, Step::Next, json!({ "guess": g }));
        }
    } else {
        let note = match (app.settings.mode == Mode::Auto, random) {
            (true, true) => "Every bet starts from a random card",
            (true, false) => "Every bet starts from this card",
            (false, true) => "Bet to start from a random card",
            (false, false) => "Bet to start from this card",
        };
        super::note(ui.painter(), row.center(), note);
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
