//! The Limbo page: header (title, session chips, tabs, start/stop), settings
//! on the left, the result stage and bet table in the middle.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::stats::{clock, HistEntry};
use super::theme::{self, alpha, BG, BG1, DIM, GOLD, GREEN, LINE, MUTED, RED, STAGE, TEXT, TEXT2};
use super::widgets::{self, button, button_key, tab_bar, Btn};
use super::{controls, App};
use crate::engine::BetRecord;
use crate::money::{fmt_int, fmt_multi, fmt_time};
use crate::settings::Mode;

const TABLE_ROWS: usize = 150;
const ROW_H: f32 = 30.0;
const SETTINGS_W: f32 = 320.0;
const PAD: f32 = 14.0;

fn kind_color(r: &BetRecord) -> Color32 {
    if r.hit {
        GOLD
    } else if r.win {
        GREEN
    } else {
        RED
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("limbo-head")
        .frame(Frame::new().fill(BG).inner_margin(Margin { left: PAD as i8, right: PAD as i8, top: 12, bottom: 10 }))
        .exact_size(60.0)
        .show_separator_line(false)
        .show(ui, |ui| header(app, ui));

    egui::Panel::left("limbo-settings")
        .resizable(false)
        .exact_size(SETTINGS_W)
        .frame(Frame::new().fill(BG).inner_margin(Margin::ZERO))
        .show_separator_line(false)
        .show(ui, |ui| controls::show(app, ui));

    egui::CentralPanel::default()
        .frame(Frame::new().fill(BG).inner_margin(Margin { left: 2, right: PAD as i8, top: 2, bottom: PAD as i8 }))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 12.0;
            stage(app, ui);
            table(app, ui);
        });
}

fn header(app: &mut App, ui: &mut Ui) {
    let hr = ui.max_rect();
    let running = app.auto.running;
    // mode tabs, centred over the stage (everything right of the settings panel)
    let cx = (hr.left() + SETTINGS_W + hr.right()) / 2.0;
    let right_w = if running { 360.0 } else { 140.0 };
    let tabs_w = 236.0;
    let x = (cx - tabs_w / 2.0).min(hr.right() - right_w - tabs_w - 12.0);
    let tabs_rect = Rect::from_min_size(pos2(x, hr.center().y - 18.0), vec2(tabs_w, 36.0));
    // a child ui does not move the parent's cursor, so the row below stays on the same line
    let mut tabs_ui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect).layout(Layout::left_to_right(Align::Center)));
    let sel = if app.settings.mode == Mode::Auto { 1 } else { 0 };
    if let Some(i) = tab_bar(&mut tabs_ui, &["Manual Bet", "Automatic"], sel, !running) {
        app.settings.mode = if i == 1 { Mode::Auto } else { Mode::Manual };
        app.mark_dirty();
    }
    ui.horizontal_centered(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(widgets::bold("Limbo", 21.0, TEXT));
        ui.add_space(8.0);
        let elapsed = match (app.stats.started, app.stats.last_at) {
            (Some(s), Some(l)) => (if running { Instant::now() } else { l }).duration_since(s).as_millis(),
            _ => 0,
        };
        widgets::meta_chip(ui, "clock", &fmt_time(elapsed), TEXT2).on_hover_text("Session time");
        widgets::meta_chip(ui, "stack", &fmt_int(app.stats.bets as i128), TEXT2).on_hover_text("Bets this session");
        if app.settings.async_bets > 1 && app.settings.mode == Mode::Auto {
            widgets::meta_chip(ui, "layers", &format!("Async ×{}", app.settings.async_bets), theme::G2)
                .on_hover_text("Bets in flight at once. Set it under No. of bets, delay & async.");
        }
        controls::restarts_chip(app, ui);

        // right: start / running controls
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
            } else if app.settings.mode == Mode::Auto {
                if button(ui, "Start", Btn::Primary, app.ready(), Some(120.0)).clicked() {
                    app.start_auto();
                }
            } else {
                let key = app.settings.hotkeys.then_some("Space");
                if button_key(ui, "Bet", key, Btn::Primary, app.ready(), Some(132.0)).clicked() {
                    app.manual_bet();
                }
            }
        });
    });
}

fn stage(app: &mut App, ui: &mut Ui) {
    let h = (ui.available_height() * 0.38).clamp(230.0, 320.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect(rect, CornerRadius::same(14), STAGE, Stroke::new(1.0, LINE), StrokeKind::Inside);

    let latest = app.history.latest_for(None).map(|(n, r)| (n, r.clone()));
    let paused = app.auto.paused;

    let mut scale = 1.0;
    if let Some(t0) = app.pop_at {
        let t = t0.elapsed().as_secs_f32();
        if t < 0.16 {
            let k = 1.0 - t / 0.16;
            scale = 1.0 + 0.1 * k * k;
            ui.ctx().request_repaint_after(Duration::from_millis(8));
        }
    }
    let center = pos2(rect.center().x, rect.center().y - 22.0);
    let mut open_card = None;
    match &latest {
        Some((n, r)) => {
            let color = kind_color(r);
            widgets::glow(&ui.painter_at(rect.shrink(1.0)), center, (h * 0.75).min(260.0), alpha(color, if r.hit { 46 } else { 22 }));
            let fg = if paused { alpha(color, 120) } else { color };
            p.text(center, Align2::CENTER_CENTER, fmt_multi(r.outcome), theme::font_bold(80.0 * scale), fg);
            // a win's number opens its card
            if r.win {
                let hot = Rect::from_center_size(center, vec2(460.0, 110.0)).intersect(rect);
                let resp = ui.interact(hot, ui.id().with("stage-card"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open the card");
                if resp.clicked() {
                    open_card = Some(*n);
                }
            }

            // the bet behind the number: four labelled values
            let items = [
                ("TARGET", fmt_multi(r.target), TEXT2),
                ("BET", app.prim(r.amount, &r.currency, false), TEXT2),
                ("PROFIT", app.prim(r.profit, &r.currency, true), theme::pnl(r.profit)),
                ("LATENCY", format!("{}ms", r.ms.round() as i64), TEXT2),
            ];
            let col_w = 118.0;
            let x0 = rect.center().x - col_w * items.len() as f32 / 2.0;
            let y = center.y + 66.0;
            for (i, (label, value, c)) in items.iter().enumerate() {
                let cx = x0 + col_w * (i as f32 + 0.5);
                p.text(pos2(cx, y - 9.0), Align2::CENTER_CENTER, *label, theme::font_semi(10.0), MUTED);
                widgets::fit_text(&p, pos2(cx, y + 10.0), Align2::CENTER_CENTER, value, theme::font_semi(14.0), *c, col_w - 12.0);
                if i > 0 {
                    let x = x0 + col_w * i as f32;
                    p.vline(x, (y - 14.0)..=(y + 18.0), Stroke::new(1.0, LINE));
                }
            }

            let mut id_txt = format!("#{}", fmt_int(*n as i128));
            if let Some(nonce) = r.nonce {
                id_txt += &format!("  ·  nonce {nonce}");
            }
            p.text(pos2(rect.left() + 18.0, rect.top() + 20.0), Align2::LEFT_CENTER, id_txt, theme::font_semi(11.0), DIM);
            if app.hit_badge_n == Some(*n) {
                let b = badge(&p, pos2(rect.right() - 18.0, rect.top() + 20.0), "HIT", GOLD);
                let resp = ui.interact(b, ui.id().with("hit-badge"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open the card");
                if resp.clicked() {
                    open_card = Some(*n);
                }
            }
        }
        None => {
            p.text(center, Align2::CENTER_CENTER, "0.00×", theme::font_bold(80.0), alpha(TEXT, 40));
            let hint = if app.settings.mode == Mode::Auto { "Press Start to run the strategy" } else { "Press Bet to place one" };
            p.text(pos2(center.x, center.y + 62.0), Align2::CENTER_CENTER, hint, theme::font(12.5), DIM);
        }
    }
    if paused {
        badge(&p, pos2(rect.right() - 18.0, rect.top() + 20.0), "PAUSED", GOLD);
    }
    // why the last run ended, centred along the top edge
    let msg = &app.auto.msg;
    if !app.auto.running && !msg.is_empty() && msg != "Stopped" && latest.is_some() {
        let (c, dot) = if msg.starts_with("HIT") {
            (GOLD, GOLD)
        } else if msg.starts_with("Error") || msg.starts_with("Seed") {
            (theme::TEXT2, RED)
        } else {
            (TEXT2, MUTED)
        };
        let g = p.layout_no_wrap(msg.clone(), theme::font_semi(11.5), c);
        let w = (g.size().x + 34.0).min(rect.width() - 260.0);
        let br = Rect::from_center_size(pos2(rect.center().x, rect.top() + 20.0), vec2(w, 26.0));
        p.rect(br, CornerRadius::same(13), alpha(dot, 18), Stroke::new(1.0, alpha(dot, 60)), StrokeKind::Inside);
        p.circle_filled(pos2(br.left() + 14.0, br.center().y), 3.5, dot);
        let clip = p.with_clip_rect(br.shrink(2.0));
        clip.galley(pos2(br.left() + 24.0, br.center().y - g.size().y / 2.0), g, c);
    }
    // recent results along the bottom, newest first, fading out to the right
    let recent: Vec<(u64, BetRecord)> = app
        .history
        .entries
        .iter()
        .filter_map(|e| match e {
            HistEntry::Bet { n, rec } if rec.game.is_none() => Some((*n, rec.clone())),
            _ => None,
        })
        .take(60)
        .collect();
    if !recent.is_empty() {
        let y = rect.bottom() - 24.0;
        let x_end = rect.right() - 18.0;
        let fade_from = x_end - 160.0;
        let mut x = rect.left() + 18.0;
        for (i, (n, r)) in recent.iter().enumerate() {
            let c = kind_color(r);
            let g = p.layout_no_wrap(fmt_multi(r.outcome), theme::font_semi(11.0), c);
            let w = g.size().x + 14.0;
            if x + w > x_end {
                break;
            }
            let fade = if x + w > fade_from { 1.0 - ((x + w - fade_from) / (x_end - fade_from)).clamp(0.0, 1.0) } else { 1.0 };
            let cr = Rect::from_min_size(pos2(x, y - 11.0), vec2(w, 22.0));
            let strong = if i == 0 { 46.0 } else { 22.0 };
            p.rect_filled(cr, CornerRadius::same(6), alpha(c, (strong * fade) as u8));
            p.galley_with_override_text_color(pos2(cr.left() + 7.0, cr.center().y - g.size().y / 2.0), g, alpha(c, (255.0 * fade) as u8));
            // a win's pill opens its card
            if r.win || r.hit {
                let resp = ui.interact(cr, ui.id().with(("recent-card", *n)), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open the card");
                if resp.clicked() {
                    open_card = Some(*n);
                }
            }
            x += w + 5.0;
        }
    }
    if let Some(n) = open_card {
        super::hits::open_bet(app, n);
    }
}

fn badge(p: &egui::Painter, right_center: egui::Pos2, text: &str, color: Color32) -> Rect {
    let g = p.layout_no_wrap(text.to_string(), theme::font_bold(11.0), color);
    let r = Rect::from_min_size(pos2(right_center.x - g.size().x - 20.0, right_center.y - 11.0), vec2(g.size().x + 20.0, 22.0));
    p.rect(r, CornerRadius::same(11), alpha(color, 30), Stroke::new(1.0, alpha(color, 150)), StrokeKind::Inside);
    p.galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, color);
    r
}

struct Col {
    name: &'static str,
    weight: f32,
    right: bool,
}

const COLS: &[Col] = &[
    Col { name: "#", weight: 0.7, right: false },
    Col { name: "Nonce", weight: 0.7, right: false },
    Col { name: "Time", weight: 0.9, right: false },
    Col { name: "Target", weight: 0.9, right: true },
    Col { name: "Result", weight: 1.0, right: true },
    Col { name: "Bet", weight: 1.1, right: true },
    Col { name: "Payout ×", weight: 0.9, right: true },
    Col { name: "Payout", weight: 1.1, right: true },
    Col { name: "Profit", weight: 1.1, right: true },
];

fn col_widths(total: f32) -> Vec<f32> {
    let sum: f32 = COLS.iter().map(|c| c.weight).sum();
    COLS.iter().map(|c| total * c.weight / sum).collect()
}

fn cell_pos(cell: Rect, right: bool) -> (egui::Pos2, Align2) {
    if right {
        (pos2(cell.right() - 12.0, cell.center().y), Align2::RIGHT_CENTER)
    } else {
        (pos2(cell.left() + 12.0, cell.center().y), Align2::LEFT_CENTER)
    }
}

fn table(app: &mut App, ui: &mut Ui) {
    let (card, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    ui.painter().rect(card, CornerRadius::same(14), BG1, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let inner = card.shrink2(vec2(8.0, 1.0));
    let mut tui = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Min)));
    let ui = &mut tui;
    ui.spacing_mut().item_spacing.y = 0.0;
    let width = inner.width();
    if super::hits::tabs(app, ui, width) {
        super::hits::table(app, ui, width);
        return;
    }
    // the last 30px hold the clear-history button
    let widths = col_widths(width - 8.0 - 30.0);

    let (hr, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    let p = ui.painter();
    let mut x = hr.left() + 8.0;
    for (c, w) in COLS.iter().zip(&widths) {
        let cell = Rect::from_min_size(pos2(x, hr.top()), vec2(*w, hr.height()));
        let (pos, al) = cell_pos(cell, c.right);
        p.text(pos, al, c.name.to_uppercase(), theme::font_semi(10.5), DIM);
        x += w;
    }
    p.hline(card.x_range(), hr.bottom(), Stroke::new(1.0, LINE));

    // the originals have their own pages; seed changes show everywhere
    let entries: Vec<HistEntry> = app
        .history
        .entries
        .iter()
        .filter(|e| !matches!(e, HistEntry::Bet { rec, .. } if rec.game.is_some()))
        .take(TABLE_ROWS)
        .cloned()
        .collect();
    if entries.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(width, 90.0), Sense::hover());
        ui.painter().text(pos2(r.center().x, r.center().y - 8.0), Align2::CENTER_CENTER, "No bets yet", theme::font_semi(13.0), MUTED);
        ui.painter().text(pos2(r.center().x, r.center().y + 12.0), Align2::CENTER_CENTER, "Results show up here as they land", theme::font(12.0), DIM);
        return;
    }
    // clear-history sits in the header's far right
    let clear_rect = Rect::from_center_size(pos2(hr.right() - 16.0, hr.center().y), vec2(28.0, 28.0));
    let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(clear_rect));
    if widgets::glyph_button(&mut cui, "trash", "Clear history").clicked() {
        super::clear_history(app);
        return;
    }

    let mut open_hit = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, entries.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            // a win's row opens its card
            let hit_n = match &entries[i] {
                HistEntry::Bet { n, rec } if rec.win || rec.hit => Some(*n),
                _ => None,
            };
            let (rr, resp) = ui.allocate_exact_size(vec2(width, ROW_H), if hit_n.is_some() { Sense::click() } else { Sense::hover() });
            if let Some(n) = hit_n {
                if resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open the card").clicked() {
                    open_hit = Some(n);
                }
            }
            let p = ui.painter();
            if resp.hovered() {
                p.rect_filled(rr, CornerRadius::same(6), alpha(Color32::WHITE, 7));
            } else if i % 2 == 1 {
                p.rect_filled(rr, CornerRadius::same(6), alpha(Color32::WHITE, 3));
            }
            match &entries[i] {
                HistEntry::Seed { at, client_seed, reason } => {
                    let short: String = client_seed.chars().take(12).collect();
                    let txt = format!("Seed changed ({reason})  ·  {short}…  ·  {}", clock(*at));
                    let g = p.layout_no_wrap(txt, theme::font_semi(11.0), alpha(theme::BLUE, 230));
                    let gw = g.size().x;
                    let cx = rr.center().x;
                    p.galley(pos2(cx - gw / 2.0, rr.center().y - g.size().y / 2.0), g, theme::BLUE);
                    let line = Stroke::new(1.0, alpha(theme::BLUE, 50));
                    p.hline((rr.left() + 12.0)..=(cx - gw / 2.0 - 12.0), rr.center().y, line);
                    p.hline((cx + gw / 2.0 + 12.0)..=(rr.right() - 12.0), rr.center().y, line);
                }
                HistEntry::Bet { n, rec } => {
                    let c = kind_color(rec);
                    if rec.hit {
                        p.rect_filled(rr, CornerRadius::same(6), alpha(GOLD, 20));
                    }
                    // win / loss marker on the left edge
                    let bar = Rect::from_min_size(pos2(rr.left() + 2.0, rr.center().y - 7.0), vec2(3.0, 14.0));
                    p.rect_filled(bar, CornerRadius::same(2), alpha(c, 200));
                    let mult = if rec.win { fmt_multi(rec.target) } else { "0.00×".to_string() };
                    let cells: [(String, Color32, bool); 9] = [
                        (fmt_int(*n as i128), DIM, false),
                        (rec.nonce.map(|n| n.to_string()).unwrap_or("–".into()), MUTED, false),
                        (clock(rec.at), MUTED, false),
                        (fmt_multi(rec.target), TEXT2, false),
                        (fmt_multi(rec.outcome), c, true),
                        (app.prim(rec.amount, &rec.currency, false), TEXT2, false),
                        (mult, if rec.win { TEXT2 } else { DIM }, false),
                        (app.prim(rec.payout, &rec.currency, false), if rec.win { TEXT2 } else { DIM }, false),
                        (app.prim(rec.profit, &rec.currency, true), theme::pnl(rec.profit), true),
                    ];
                    let mut x = rr.left() + 8.0;
                    for ((text, color, strong), (col, w)) in cells.iter().zip(COLS.iter().zip(&widths)) {
                        let cell = Rect::from_min_size(pos2(x, rr.top()), vec2(*w, ROW_H));
                        let (pos, al) = cell_pos(cell, col.right);
                        let font = if *strong { theme::font_semi(12.5) } else { theme::font(12.5) };
                        p.text(pos, al, text, font, *color);
                        x += w;
                    }
                }
            }
        }
    });
    if let Some(n) = open_hit {
        super::hits::open_bet(app, n);
    }
}
