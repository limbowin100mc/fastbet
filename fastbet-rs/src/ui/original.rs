//! An original's page, shaped like Limbo's: header (title, session chips, tabs,
//! bet / cash out / start), settings on the left, the game's board and its bet
//! table in the middle.

use std::time::Instant;

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::originals::{self, times};
use super::stats::{clock, HistEntry};
use super::theme::{self, alpha, BG, BG1, DIM, GOLD, GREEN, LINE, MUTED, RED, STAGE, TEXT, TEXT2};
use super::widgets::{self, button, button_key, section, tab_bar, Btn};
use super::{controls, App};
use crate::money::{fmt_int, fmt_time};
use crate::originals::{dragon_multiplier, Game, Round, Step};
use crate::settings::Mode;

const SETTINGS_W: f32 = 320.0;
const PAD: f32 = 14.0;
const ROW_H: f32 = 30.0;
const TABLE_ROWS: usize = 150;

/// What an open round would pay if cashed out now.
pub fn cashout_multiplier(r: &Round) -> f64 {
    let last_round = || r.state["rounds"].as_array().and_then(|a| a.last()).and_then(|x| x["payoutMultiplier"].as_f64());
    match r.game {
        Game::Mines | Game::Hilo => last_round().unwrap_or(0.0),
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
    // HiLo has no auto mode: its page is always manual
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
        .show(ui, |ui| settings(app, ui, game));

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
            originals::board(app, &mut bui, game, inner);
            stage_badges(app, ui, game, rect);
            table(app, ui, game);
        });
}

fn header(app: &mut App, ui: &mut Ui, game: Game) {
    let hr = ui.max_rect();
    let running = app.auto.running;
    let open = app.orig.open(game).cloned();
    let busy = app.orig.busy.contains(&game);
    if game.auto() {
        let cx = (hr.left() + SETTINGS_W + hr.right()) / 2.0;
        let right_w = if running { 360.0 } else { 170.0 };
        let tabs_w = 236.0;
        let x = (cx - tabs_w / 2.0).min(hr.right() - right_w - tabs_w - 12.0);
        let tabs_rect = Rect::from_min_size(pos2(x, hr.center().y - 18.0), vec2(tabs_w, 36.0));
        let mut tabs_ui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect).layout(Layout::left_to_right(Align::Center)));
        let sel = if app.settings.mode == Mode::Auto { 1 } else { 0 };
        if let Some(i) = tab_bar(&mut tabs_ui, &["Manual Bet", "Automatic"], sel, !running && open.is_none()) {
            app.settings.mode = if i == 1 { Mode::Auto } else { Mode::Manual };
            app.mark_dirty();
        }
    }
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(widgets::bold(app.casino().original_name(game), 21.0, TEXT));
        ui.add_space(8.0);
        let elapsed = match (app.stats.started, app.stats.last_at) {
            (Some(s), Some(l)) => (if running { Instant::now() } else { l }).duration_since(s).as_millis(),
            _ => 0,
        };
        widgets::meta_chip(ui, "clock", &fmt_time(elapsed), TEXT2).on_hover_text("Session time");
        widgets::meta_chip(ui, "stack", &fmt_int(app.stats.bets as i128), TEXT2).on_hover_text("Bets this session");
        controls::restarts_chip(app, ui);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if running {
                if button(ui, "Stop", Btn::Danger, true, Some(84.0)).clicked() {
                    app.stop_auto();
                }
                let paused = app.auto.paused;
                if button(ui, if paused { "Resume" } else { "Pause" }, Btn::Warn, true, Some(84.0)).clicked() {
                    app.toggle_pause();
                }
                let armed = app.auto.stop_on_win;
                if button(ui, if armed { "Stopping on win…" } else { "Stop on next win" }, Btn::Ghost, true, None).clicked() {
                    app.stop_on_next_win();
                }
            } else if open.is_some() && game.step_path(Step::Cashout).is_none() {
                // blackjack and video poker end on the board, not with a cash out
                ui.label(widgets::semi("Round in play", 12.0, theme::GOLD));
            } else if let Some(r) = &open {
                let m = cashout_multiplier(r);
                let label = if m > 0.0 { format!("Cash out  {}", times(m)) } else { "Cash out".to_string() };
                if button(ui, &label, Btn::Primary, m > 0.0 && !busy, Some(170.0)).clicked() {
                    app.orig_step(game, Step::Cashout, serde_json::json!({}));
                }
            } else if app.settings.mode == Mode::Auto && game.auto() {
                if button(ui, "Start", Btn::Primary, app.ready(), Some(120.0)).clicked() {
                    app.orig_start_auto(game);
                }
            } else {
                let key = app.settings.hotkeys.then_some("Space");
                if button_key(ui, "Bet", key, Btn::Primary, app.ready() && !busy, Some(132.0)).clicked() {
                    app.orig_bet(game);
                }
            }
        });
    });
}

fn settings(app: &mut App, ui: &mut Ui, game: Game) {
    let panel_w = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 14, right: 12, top: 2, bottom: 14 }).show(ui, |ui| {
            let w = panel_w - 26.0 - 4.0;
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.set_max_width(w);
            let locked = app.auto.running || app.orig.open(game).is_some();
            let mut open = true;
            section(ui, w, "General", true, &mut open, |ui| {
                controls::bet_size(app, ui);
                widgets::hsep(ui);
                originals::params(app, ui, game, app.auto.running);
            });
            let _ = locked;
            if app.settings.mode == Mode::Auto && game.auto() {
                controls::strategy(app, ui, w);
                controls::bets_and_speed(app, ui, w);
                controls::stops(app, ui, w);
                controls::force_restart(app, ui, w);
            }
            controls::seed(app, ui, w);
            controls::notifications(app, ui, w);
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
    ui.painter().rect(card, CornerRadius::same(14), BG1, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let inner = card.shrink2(vec2(8.0, 1.0));
    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    let ui = &mut tui;
    ui.spacing_mut().item_spacing.y = 0.0;
    let width = inner.width();
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
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let (n, rec) = &rows[i];
            let (rr, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::hover());
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
}
