//! Video Poker (jacks or better): hold the cards to keep, draw the rest once.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui};
use serde_json::{json, Value};

use super::super::theme::{self, alpha, GOLD, MUTED, TEXT, TEXT2};
use super::super::widgets::Btn;
use super::super::App;
use super::{board_button, card, cards_of, hit, payout_color, times};
use crate::originals::{Game, Round, Step, VIDEO_POKER_PAYTABLE};

pub fn params(_app: &mut App, ui: &mut Ui) {
    ui.label(egui::RichText::new("Bet to deal five cards, click the ones to hold, then draw.").size(11.0).color(MUTED));
}

pub fn bet_params() -> Result<Value, String> {
    Ok(json!({}))
}

fn hand_name(key: &str) -> &'static str {
    VIDEO_POKER_PAYTABLE.iter().find(|(k, _, _)| *k == key).map(|(_, n, _)| *n).unwrap_or("Nothing")
}

pub fn result_text(r: &Round) -> String {
    hand_name(r.state["handResult"].as_str().unwrap_or("none")).to_string()
}

pub fn board(app: &mut App, ui: &mut Ui, rect: Rect) {
    let open = app.orig.open(Game::VideoPoker).cloned();
    let busy = app.orig.busy.contains(&Game::VideoPoker);
    let shown = open.clone().or_else(|| app.orig.last(Game::VideoPoker).cloned());
    // holds belong to the round they were set in
    if let Some(r) = &open {
        if app.orig.vp_hold.0 != r.id {
            app.orig.vp_hold = (r.id.clone(), [false; 5]);
        }
    }
    let table_w = 230.0;
    let cards_area = Rect::from_min_max(rect.min, pos2(rect.right() - table_w - 10.0, rect.bottom()));
    let cards = shown.as_ref().map(|r| cards_of(if r.active { &r.state["initialHand"] } else { &r.state["playerHand"] })).unwrap_or_default();
    let result = shown.as_ref().and_then(|r| r.state["handResult"].as_str().map(str::to_string)).unwrap_or_default();

    let cw = ((cards_area.width() - 60.0) / 5.0 - 10.0).min(cards_area.height() * 0.38).min(110.0);
    let ch = cw * 1.4;
    let x0 = cards_area.center().x - (cw * 5.0 + 40.0) / 2.0;
    let y = cards_area.center().y - ch / 2.0 - 20.0;
    let mut toggle = None;
    for i in 0..5 {
        let r = Rect::from_min_size(pos2(x0 + i as f32 * (cw + 10.0), y), vec2(cw, ch));
        let held = open.is_some() && app.orig.vp_hold.1[i];
        let resp = hit(ui, r, ("vp", i), open.is_some() && !busy);
        let p = ui.painter();
        match cards.get(i) {
            Some((rank, suit)) => {
                let r = if held { r.translate(vec2(0.0, -10.0)) } else { r };
                card(p, r, rank, suit, open.is_some() && !held && resp.hovered());
                if held {
                    p.rect_stroke(r.expand(2.0), CornerRadius::same(8), Stroke::new(2.0, GOLD), StrokeKind::Outside);
                    p.text(pos2(r.center().x, r.bottom() + 18.0), Align2::CENTER_CENTER, "HOLD", theme::font_bold(12.0), GOLD);
                }
            }
            None => super::card_back(p, r),
        }
        if resp.clicked() {
            toggle = Some(i);
        }
    }
    if let Some(i) = toggle {
        app.orig.vp_hold.1[i] = !app.orig.vp_hold.1[i];
    }
    if let Some(r) = &open {
        let b = Rect::from_center_size(pos2(cards_area.center().x, rect.bottom() - 30.0), vec2(200.0, 40.0));
        if board_button(ui, b, "Draw", Btn::Primary, !busy) {
            let held: Vec<Value> = cards_of(&r.state["initialHand"])
                .iter()
                .enumerate()
                .filter(|(i, _)| app.orig.vp_hold.1[*i])
                .map(|(_, (rank, suit))| json!({ "rank": rank, "suit": suit }))
                .collect();
            app.orig_step(Game::VideoPoker, Step::Next, json!({ "held": held }));
        }
    } else if shown.is_none() {
        super::note(ui.painter(), pos2(cards_area.center().x, rect.bottom() - 30.0), "Bet to deal");
    }

    // the paytable, the hand made lit
    let p = ui.painter();
    let tr = Rect::from_min_max(pos2(rect.right() - table_w, rect.top() + 14.0), pos2(rect.right() - 12.0, rect.bottom() - 14.0));
    let row_h = (tr.height() / VIDEO_POKER_PAYTABLE.len() as f32).min(30.0);
    for (i, (key, name, m)) in VIDEO_POKER_PAYTABLE.iter().enumerate() {
        let r = Rect::from_min_size(pos2(tr.left(), tr.top() + i as f32 * row_h), vec2(tr.width(), row_h - 4.0));
        let on = *key == result;
        let c = payout_color(*m);
        p.rect_filled(r, CornerRadius::same(6), if on { alpha(c, 60) } else { alpha(Color32::WHITE, 4) });
        p.text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, *name, theme::font_semi(11.5), if on { TEXT } else { TEXT2 });
        p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, times(*m), theme::font_semi(11.5), c);
    }
}
