//! An original's page, shaped like Limbo's: header (title, tabs, session chips),
//! settings on the left with bet / cash out / start pinned under them, the game's
//! board and its bet table in the middle.

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::originals::{self, times};
use super::stats::{clock, HistEntry};
use super::theme::{self, alpha, BG, BG1, DIM, GOLD, GREEN, LINE, MUTED, RED, STAGE, TEXT, TEXT2};
use super::widgets;
use super::{controls, App};
use crate::money::fmt_int;
use crate::originals::{dragon_multiplier, Game, Round};
use crate::settings::Mode;

const SETTINGS_W: f32 = 320.0;
const PAD: f32 = 14.0;
const ROW_H: f32 = 30.0;
const TABLE_ROWS: usize = 150;

/// What an open round would pay if cashed out now.
pub fn cashout_multiplier(r: &Round) -> f64 {
    let last_round = || r.state["rounds"].as_array().and_then(|a| a.last()).and_then(|x| x["payoutMultiplier"].as_f64());
    match r.game {
        Game::Mines => last_round().unwrap_or(0.0),
        // skipped cards alone pay nothing to cash out (0.99× on chips.gg, which refuses it)
        Game::Hilo => last_round().filter(|m| *m > 1.0).unwrap_or(0.0),
        Game::DragonTower => {
            let rows = r.state["currentRound"].as_u64().unwrap_or(0) as u32;
            if rows == 0 {
                0.0
            } else {
                dragon_multiplier(r.state["difficulty"].as_str().unwrap_or("easy"), rows)
            }
        }
        Game::Pump | Game::Flip | Game::Chicken => {
            let played = r.state["round"].as_u64().or(r.state["currentRound"].as_u64()).unwrap_or(0);
            if played == 0 {
                0.0
            } else {
                r.state["payoutMultiplier"].as_f64().unwrap_or(0.0)
            }
        }
        Game::Snakes => last_round().unwrap_or(0.0),
        Game::Moles => r.state["rounds"]
            .as_array()
            .and_then(|a| a.iter().rev().find(|x| x["hit"].as_bool() == Some(true)))
            .and_then(|x| x["multiplier"].as_f64())
            .unwrap_or(0.0),
        Game::RockPaperScissors => {
            let played = r.state["playerRounds"].as_array().map(Vec::len).unwrap_or(0);
            if played == 0 {
                0.0
            } else {
                r.state["payoutMultiplier"].as_f64().unwrap_or(0.0)
            }
        }
        _ => r.multiplier,
    }
}

pub fn show(app: &mut App, ui: &mut Ui, game: Game) {
    // blackjack and video poker have no auto mode: their pages are always manual
    if !game.auto() && app.settings.mode == Mode::Auto && !app.auto.running {
        app.settings.mode = Mode::Manual;
    }
    egui::Panel::top("orig-head")
        .frame(Frame::new().fill(BG).inner_margin(Margin { left: PAD as i8, right: PAD as i8, top: 12, bottom: 10 }))
        .exact_size(60.0)
        .show_separator_line(false)
        .show(ui, |ui| header(app, ui, game));

    egui::Panel::left("orig-settings")
        .resizable(false)
        .exact_size(SETTINGS_W)
        .frame(Frame::new().fill(BG).inner_margin(Margin::ZERO))
        .show_separator_line(false)
        .show(ui, |ui| {
            controls::action_bar(app, ui, Some(game));
            settings(app, ui, game);
        });

    egui::CentralPanel::default()
        .frame(Frame::new().fill(BG).inner_margin(Margin { left: 2, right: PAD as i8, top: 2, bottom: PAD as i8 }))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            let h = (ui.available_height() * 0.64).clamp(320.0, 620.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
            ui.painter().rect(rect, CornerRadius::same(14), STAGE, Stroke::new(1.0, LINE), StrokeKind::Inside);
            let inner = rect.shrink(6.0);
            let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(inner));
            bui.set_clip_rect(inner);
            if app.code_mode() && game.auto() {
                super::code::panel(app, &mut bui, inner, game.slug());
            } else {
                originals::board(app, &mut bui, game, inner);
            }
            stage_badges(app, ui, game, rect);
            table(app, ui, game);
        });
}

fn header(app: &mut App, ui: &mut Ui, game: Game) {
    let hr = ui.max_rect();
    let running = app.auto.running;
    let open = app.orig.open(game).is_some();
    let (mut left, mut right) = (hr.left(), hr.right());
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(widgets::bold(app.casino().original_name(game), 21.0, TEXT));
        left = ui.min_rect().right();
        // right: the session at a glance (the buttons are under the settings)
        let runs_async = !game.has_steps() || app.casino().auto_settles(game);
        let async_on = app.settings.async_bets > 1 && app.settings.mode == Mode::Auto && game.auto() && runs_async && !app.strategy_sequential(false);
        right = ui
            .with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                controls::session_chips(app, ui, async_on);
            })
            .response
            .rect
            .left();
    });
    if game.auto() {
        // the stage starts right of the settings (the header has its own left margin)
        let stage = (hr.left() - PAD + SETTINGS_W + 2.0, hr.right());
        controls::mode_tabs(app, ui, stage, left, right, !running && !open);
    }
}

fn settings(app: &mut App, ui: &mut Ui, game: Game) {
    let panel_w = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 14, right: 12, top: 2, bottom: 14 }).show(ui, |ui| {
            let w = panel_w - 26.0 - 4.0;
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.set_max_width(w);
            controls::bet_card(app, ui, w);
            // the game's own settings, under its name; the "?" says how it plays
            let help = originals::help(app.casino(), game);
            widgets::card(ui, w, app.casino().original_name(game), Some(&help), None, |ui| originals::params(app, ui, game, app.auto.running));
            controls::run_settings(app, ui, w, game.auto(), false);
        });
    });
}

/// Paused / why the run ended, over the board's top edge.
fn stage_badges(app: &App, ui: &Ui, game: Game, rect: Rect) {
    let p = ui.painter();
    if app.auto.paused {
        badge(p, pos2(rect.right() - 16.0, rect.top() + 18.0), "PAUSED", GOLD);
    }
    let msg = &app.auto.msg;
    let mine = app.history.latest_for(Some(game)).is_some();
    if !app.auto.running && !msg.is_empty() && msg != "Stopped" && mine {
        let c = if msg.starts_with("HIT") { GOLD } else if msg.starts_with("Error") || msg.starts_with("The round") { RED } else { MUTED };
        let g = p.layout_no_wrap(msg.clone(), theme::font_semi(11.0), TEXT2);
        let br = Rect::from_center_size(pos2(rect.center().x, rect.top() + 18.0), vec2(g.size().x + 34.0, 24.0));
        p.rect(br, CornerRadius::same(12), alpha(c, 20), Stroke::new(1.0, alpha(c, 70)), StrokeKind::Inside);
        p.circle_filled(pos2(br.left() + 13.0, br.center().y), 3.5, c);
        p.galley(pos2(br.left() + 23.0, br.center().y - g.size().y / 2.0), g, TEXT2);
    }
}

fn badge(p: &egui::Painter, right_center: egui::Pos2, text: &str, color: Color32) {
    let g = p.layout_no_wrap(text.to_string(), theme::font_bold(11.0), color);
    let r = Rect::from_min_size(pos2(right_center.x - g.size().x - 20.0, right_center.y - 11.0), vec2(g.size().x + 20.0, 22.0));
    p.rect(r, CornerRadius::same(11), alpha(color, 30), Stroke::new(1.0, alpha(color, 150)), StrokeKind::Inside);
    p.galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, color);
}

struct Col {
    name: &'static str,
    weight: f32,
    right: bool,
}

const COLS: &[Col] = &[
    Col { name: "#", weight: 0.6, right: false },
    Col { name: "Time", weight: 0.9, right: false },
    Col { name: "Result", weight: 2.0, right: false },
    Col { name: "Bet", weight: 1.1, right: true },
    Col { name: "Payout ×", weight: 0.9, right: true },
    Col { name: "Payout", weight: 1.1, right: true },
    Col { name: "Profit", weight: 1.1, right: true },
];

fn table(app: &mut App, ui: &mut Ui, game: Game) {
    let (card, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    ui.painter().rect_filled(card, CornerRadius::same(14), BG1);
    let inner = card.shrink2(vec2(8.0, 1.0));
    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    let ui = &mut tui;
    ui.spacing_mut().item_spacing.y = 0.0;
    let width = inner.width();
    if super::hits::tabs(app, ui, width) {
        super::hits::table(app, ui, width);
        return;
    }
    let sum: f32 = COLS.iter().map(|c| c.weight).sum();
    let widths: Vec<f32> = COLS.iter().map(|c| (width - 8.0) * c.weight / sum).collect();
    let cell_pos = |cell: Rect, right: bool| if right { (pos2(cell.right() - 12.0, cell.center().y), Align2::RIGHT_CENTER) } else { (pos2(cell.left() + 12.0, cell.center().y), Align2::LEFT_CENTER) };

    let (hr, _) = ui.allocate_exact_size(vec2(width, 36.0), Sense::hover());
    let p = ui.painter();
    let mut x = hr.left() + 8.0;
    for (c, w) in COLS.iter().zip(&widths) {
        let cell = Rect::from_min_size(pos2(x, hr.top()), vec2(*w, hr.height()));
        let (pos, al) = cell_pos(cell, c.right);
        p.text(pos, al, c.name.to_uppercase(), theme::font_semi(10.5), DIM);
        x += w;
    }
    p.hline(card.x_range(), hr.bottom(), Stroke::new(1.0, LINE));

    let rows: Vec<(u64, crate::engine::BetRecord)> = app
        .history
        .entries
        .iter()
        .filter_map(|e| match e {
            HistEntry::Bet { n, rec } if rec.game == Some(game) => Some((*n, rec.clone())),
            _ => None,
        })
        .take(TABLE_ROWS)
        .collect();
    if rows.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(width, 70.0), Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "No rounds yet", theme::font_semi(13.0), MUTED);
        return;
    }
    let mut open_hit = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let (n, rec) = &rows[i];
            // a win's row opens its card
            let opens = rec.win || rec.hit;
            let (rr, resp) = ui.allocate_exact_size(vec2(width, ROW_H), if opens { Sense::click() } else { Sense::hover() });
            if opens && resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open the card").clicked() {
                open_hit = Some(*n);
            }
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(rr, CornerRadius::same(6), alpha(Color32::WHITE, 7));
            } else if i % 2 == 1 {
                p.rect_filled(rr, CornerRadius::same(6), alpha(Color32::WHITE, 3));
            }
            let c = if rec.hit {
                GOLD
            } else if rec.win {
                GREEN
            } else {
                RED
            };
            let bar = Rect::from_min_size(pos2(rr.left() + 2.0, rr.center().y - 7.0), vec2(3.0, 14.0));
            p.rect_filled(bar, CornerRadius::same(2), alpha(c, 200));
            let result = rec.round.as_deref().map(originals::result_text).unwrap_or_default();
            let cells: [(String, Color32, bool); 7] = [
                (fmt_int(*n as i128), DIM, false),
                (clock(rec.at), MUTED, false),
                (result, TEXT2, false),
                (app.prim(rec.amount, &rec.currency, false), TEXT2, false),
                (times(rec.outcome), c, true),
                (app.prim(rec.payout, &rec.currency, false), if rec.payout > 0 { TEXT2 } else { DIM }, false),
                (app.prim(rec.profit, &rec.currency, true), theme::pnl(rec.profit), true),
            ];
            let mut x = rr.left() + 8.0;
            for ((text, color, strong), (col, w)) in cells.iter().zip(COLS.iter().zip(&widths)) {
                let cell = Rect::from_min_size(pos2(x, rr.top()), vec2(*w, ROW_H));
                let (pos, al) = cell_pos(cell, col.right);
                let font = if *strong { theme::font_semi(12.5) } else { theme::font(12.5) };
                widgets::fit_text(p, pos, al, text, font, *color, w - 16.0);
                x += w;
            }
        }
    });
    if let Some(n) = open_hit {
        super::hits::open_bet(app, n);
    }
}
