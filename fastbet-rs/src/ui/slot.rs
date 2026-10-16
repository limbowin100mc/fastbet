//! A slot game page: bet size, feature spin, stop on multiplier, Real/Demo,
//! the spin table and the Start / Pause / Stop controls.

use std::time::{Duration, Instant};

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::stats::clock;
use super::theme::{self, alpha, BG1, BG2, DIM, G1, GOLD, GREEN, LINE, MUTED, RED, STAGE, TEXT, TEXT2};
use super::widgets::{self, button, caption, check_field, field, icon, section, tab_bar, Btn};
use super::App;
use crate::engine::slots::SpinRecord;
use crate::money::{self, fmt_int, fmt_multi, fmt_time};
use crate::slots::{fmt_money, PlayMode};

const SETTINGS_W: f32 = 300.0;
const ROW_H: f32 = 23.0;

pub fn show(app: &mut App, ui: &mut Ui) {
    egui::Panel::top("slot-head")
        .frame(Frame::new().fill(theme::BG).inner_margin(Margin::symmetric(16, 8)))
        .exact_size(54.0)
        .show_separator_line(false)
        .show(ui, |ui| header(app, ui));

    egui::Panel::left("slot-settings")
        .resizable(false)
        .exact_size(SETTINGS_W)
        .frame(Frame::new().fill(theme::BG).inner_margin(Margin::same(0)))
        .show_separator_line(false)
        .show(ui, |ui| settings(app, ui));

    egui::CentralPanel::default()
        .frame(Frame::new().fill(STAGE).inner_margin(Margin::same(0)))
        .show(ui, |ui| {
            stage(app, ui);
            table(app, ui);
        });
}

fn header(app: &mut App, ui: &mut Ui) {
    let hr = ui.max_rect();
    let cx = (hr.left() + SETTINGS_W + hr.right()) / 2.0;
    let running = app.slot.running;
    let opening = app.slot.opening;
    // keep the tabs clear of the buttons on the right when the window is narrow
    let right_w = if running { 330.0 } else { 110.0 };
    let tabs_w = 190.0;
    let x = (cx - tabs_w / 2.0).min(hr.right() - right_w - tabs_w - 12.0);
    let tabs_rect = Rect::from_min_size(pos2(x, hr.center().y - 16.0), vec2(tabs_w, 32.0));
    let mut tabs_ui = ui.new_child(egui::UiBuilder::new().max_rect(tabs_rect).layout(Layout::left_to_right(Align::Center)));
    let sel = if app.settings.slot_demo { 1 } else { 0 };
    if let Some(i) = tab_bar(&mut tabs_ui, &["Real Play", "Demo"], sel, !running && !opening) {
        app.slot_set_mode(i == 1);
    }

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let name = app.slot.game.as_ref().map(|g| g.name.clone()).unwrap_or_else(|| "Slot".into());
        let producer = app.slot.game.as_ref().map(|g| g.maker().to_string()).unwrap_or_default();
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(widgets::semi(producer, 10.0, MUTED));
            ui.label(widgets::bold(name, 18.0, TEXT));
        });
        ui.add_space(4.0);
        let elapsed = match (app.slot_stats.started, app.slot_stats.last_at) {
            (Some(s), Some(l)) => (if running { Instant::now() } else { l }).duration_since(s).as_millis(),
            _ => 0,
        };
        let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
        icon(ui.painter(), r, "clock", MUTED);
        ui.label(widgets::mono(fmt_time(elapsed), 12.5, TEXT2));
        let (r, _) = ui.allocate_exact_size(vec2(13.0, 13.0), Sense::hover());
        icon(ui.painter(), r, "refresh", MUTED);
        ui.label(widgets::mono(fmt_int(app.slot_stats.bets as i128), 12.5, TEXT2));

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if running {
                if button(ui, "Stop", Btn::Danger, true, Some(78.0)).clicked() {
                    app.stop_slot();
                }
                let paused = app.slot.paused;
                if button(ui, if paused { "Resume" } else { "Pause" }, Btn::Warn, true, Some(78.0)).clicked() {
                    app.pause_slot();
                }
                let armed = app.slot.stop_on_win;
                if button(ui, if armed { "Stopping on win…" } else { "Stop on next win" }, if armed { Btn::Ghost } else { Btn::Danger }, true, None).clicked() {
                    app.slot_stop_on_next_win();
                }
            } else {
                let ready = app.slot.info.is_some() && !opening;
                if button(ui, if opening { "Opening…" } else { "Start" }, Btn::Primary, ready, Some(96.0)).clicked() {
                    app.start_slot();
                }
                if app.slot.error.is_some() && !opening {
                    if button(ui, "Retry", Btn::Ghost, true, Some(72.0)).clicked() {
                        app.reopen_slot();
                    }
                }
            }
        });
    });
}

fn settings(app: &mut App, ui: &mut Ui) {
    let panel_w = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            let w = panel_w - 24.0 - 8.0;
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.set_max_width(w);
            super::challenges::slot_cards(app, ui, w);
            general(app, ui, w);
            counts(app, ui, w);
            stops(app, ui, w);
        });
    });
}

fn general(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.slot.running || app.slot.opening;
    let mut open = true;
    let info = app.slot.info.clone();
    let game_id = app.slot.game.as_ref().map(|g| g.id.clone()).unwrap_or_default();
    section(ui, w, "General", true, &mut open, |ui| {
        caption(ui, "Bet size");
        let (cur, dec) = info.as_ref().map(|i| (i.currency.clone(), i.decimals)).unwrap_or(("USD".into(), 2));
        let levels = info.as_ref().map(|i| i.bet_levels.clone()).unwrap_or_default();
        let prefs = app.settings.slot_prefs.get(&game_id).cloned().unwrap_or_default();
        let mut bet = prefs.bet.or(info.as_ref().and_then(|i| i.default_bet)).unwrap_or(levels.first().map(|l| l.minor).unwrap_or(0));
        if !levels.is_empty() && !levels.iter().any(|l| l.minor == bet) {
            bet = info.as_ref().and_then(|i| i.default_bet).unwrap_or(levels[0].minor);
        }
        let before = bet;
        ui.add_enabled_ui(!locked && !levels.is_empty(), |ui| {
            egui::ComboBox::from_id_salt("slot-bet")
                .selected_text(widgets::mono(if levels.is_empty() { "–".into() } else { fmt_money(bet, &cur, dec) }, 12.5, TEXT))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for l in &levels {
                        ui.selectable_value(&mut bet, l.minor, fmt_money(l.minor, &cur, dec));
                    }
                });
        });
        if bet != before || prefs.bet != Some(bet) {
            if !levels.is_empty() {
                app.settings.slot_prefs.entry(game_id.clone()).or_default().bet = Some(bet);
                app.mark_dirty();
            }
        }

        caption(ui, "Feature spin");
        let features = info.as_ref().map(|i| i.features.clone()).unwrap_or_default();
        let mut feat = prefs.feature.clone().filter(|f| features.iter().any(|x| &x.id == f));
        let before = feat.clone();
        ui.add_enabled_ui(!locked && !features.is_empty(), |ui| {
            let label = feat.as_ref().and_then(|f| features.iter().find(|x| &x.id == f)).map(|f| f.name.clone()).unwrap_or_else(|| "Normal spins".into());
            egui::ComboBox::from_id_salt("slot-feature")
                .selected_text(widgets::semi(label, 12.0, TEXT))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut feat, None, "Normal spins");
                    for f in &features {
                        let cost = fmt_money((bet as f64 * f.cost_mult).round() as i64, &cur, dec);
                        ui.selectable_value(&mut feat, Some(f.id.clone()), format!("{}  ·  {cost}", f.name));
                    }
                });
        });
        if feat != before {
            app.settings.slot_prefs.entry(game_id.clone()).or_default().feature = feat.clone();
            app.mark_dirty();
        }
        if let Some(f) = feat.as_ref().and_then(|f| features.iter().find(|x| &x.id == f)) {
            let cost = (bet as f64 * f.cost_mult).round() as i64;
            ui.label(egui::RichText::new(format!("Each spin costs {}{}", fmt_money(cost, &cur, dec), f.rtp.map(|r| format!(" · RTP {r:.2}%")).unwrap_or_default())).size(10.5).color(MUTED));
        }

        widgets::hsep(ui);
        let mut on = app.settings.slot_stop_mult_on;
        if check_field(ui, "Stop on multiplier  ≥", &mut on, &mut app.settings.slot_stop_mult, Some("×"), !locked) {
            app.settings.slot_stop_mult_on = on;
            app.mark_dirty();
        }
        ui.label(egui::RichText::new("Pauses the moment a round pays this many × the stake. Resume to carry on.").size(10.5).color(DIM));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut fs = app.settings.slot_stop_fs;
            if widgets::checkbox(ui, "Stop before free spins", &mut fs, !locked) {
                app.settings.slot_stop_fs = fs;
                app.mark_dirty();
            }
            widgets::help_icon(
                ui,
                "Stops autoplay when a spin triggers free spins, before they are played, so you can play them in your                  browser: open the game on the casino site and it carries on with them. Some providers (Hacksaw, Stake                  Engine …) decide the whole bonus in the spin that triggers it; there the result is already set and the                  browser shows it.",
            );
        });
    });
}

fn counts(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.slot.running || app.slot.opening;
    let cnt: u32 = app.settings.slot_count.trim().parse().unwrap_or(0);
    let mut open = app.settings.open.count;
    section(ui, w, "No. of spins", cnt > 0, &mut open, |ui| {
        if field(ui, "Number of spins", &mut app.settings.slot_count, "0 = ∞", None, !locked).changed() {
            app.mark_dirty();
        }
    });
    if open != app.settings.open.count {
        app.settings.open.count = open;
        app.mark_dirty();
    }
}

fn stops(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.slot.running || app.slot.opening;
    let active = !app.settings.slot_stop_profit.trim().is_empty() || !app.settings.slot_stop_loss.trim().is_empty();
    let cur = app.slot.info.as_ref().map(|i| i.currency.clone()).unwrap_or("USD".into());
    let mut open = app.settings.open.stops;
    section(ui, w, "Stop on profit / loss", active, &mut open, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let half = (ui.available_width() - 8.0) / 2.0;
            ui.scope(|ui| {
                ui.set_min_width(half);
                ui.set_max_width(half);
                if field(ui, "Stop profit", &mut app.settings.slot_stop_profit, "off", Some(&cur), !locked).changed() {
                    app.mark_dirty();
                }
            });
            ui.scope(|ui| {
                ui.set_min_width(half);
                ui.set_max_width(half);
                if field(ui, "Stop loss", &mut app.settings.slot_stop_loss, "off", Some(&cur), !locked).changed() {
                    app.mark_dirty();
                }
            });
        });
    });
    if open != app.settings.open.stops {
        app.settings.open.stops = open;
        app.mark_dirty();
    }
}

/// Gold on a stop-multiplier hit, green from 1× up (stake back or better), red below.
fn kind_color(r: &SpinRecord) -> Color32 {
    if r.hit {
        GOLD
    } else if r.multiplier >= 1.0 {
        GREEN
    } else {
        RED
    }
}

fn stage(app: &mut App, ui: &mut Ui) {
    let h = 150.0;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::ZERO, STAGE);
    let center = pos2(rect.center().x, rect.top() + 62.0);
    let mut scale = 1.0;
    if let Some(t0) = app.slot_pop {
        let t = t0.elapsed().as_secs_f32();
        if t < 0.16 {
            scale = 1.0 + 0.12 * (1.0 - t / 0.16).powi(2);
            ui.ctx().request_repaint_after(Duration::from_millis(8));
        }
    }
    match app.spins.front().cloned() {
        Some(r) if r.held => {
            p.text(center, Align2::CENTER_CENTER, "Free spins ready", theme::font_bold(34.0 * scale), GOLD);
            let sub = format!("Bet {}   ·   play them in your browser", fmt_money(r.cost, &r.currency, r.decimals));
            p.text(pos2(center.x, center.y + 34.0), Align2::CENTER_CENTER, sub, theme::font_semi(11.5), MUTED);
        }
        Some(r) => {
            let c = kind_color(&r);
            if r.hit {
                for i in 0..5 {
                    p.circle_filled(center, 40.0 + i as f32 * 18.0, alpha(GOLD, (30.0 * (1.0 - i as f32 / 5.0)) as u8));
                }
            }
            p.text(center, Align2::CENTER_CENTER, fmt_multi(r.multiplier), theme::font_bold(44.0 * scale), c);
            let sub = format!("Bet {}   ·   Win {}   ·   {}ms", fmt_money(r.cost, &r.currency, r.decimals), fmt_money(r.win, &r.currency, r.decimals), r.ms.round() as i64);
            p.text(pos2(center.x, center.y + 34.0), Align2::CENTER_CENTER, sub, theme::font_semi(11.5), MUTED);
        }
        None => {
            p.text(center, Align2::CENTER_CENTER, "0.00×", theme::font_bold(44.0), alpha(TEXT, 60));
        }
    }
    // session line: status + balance
    let st = &app.slot;
    let (msg, color) = if let Some(e) = &st.error {
        (e.clone(), RED)
    } else if st.opening {
        ("Opening session…".to_string(), GOLD)
    } else if st.paused {
        (st.msg.clone(), GOLD)
    } else if st.running {
        (st.msg.clone(), GREEN)
    } else if st.held {
        (st.msg.clone(), GOLD)
    } else if st.info.is_some() {
        (if st.msg.is_empty() { "Ready".into() } else { st.msg.clone() }, MUTED)
    } else {
        ("No session".to_string(), DIM)
    };
    p.text(pos2(rect.left() + 16.0, rect.bottom() - 16.0), Align2::LEFT_CENTER, msg, theme::font_semi(11.5), color);
    if let (Some(info), Some(bal)) = (&st.info, st.balance) {
        let mode = match st.mode {
            Some(PlayMode::Demo) => "demo balance",
            _ => "balance",
        };
        p.text(pos2(rect.right() - 16.0, rect.bottom() - 16.0), Align2::RIGHT_CENTER, format!("{mode}  {}", fmt_money(bal, &info.currency, info.decimals)), theme::font_semi(11.5), TEXT2);
    }
    if st.opening {
        ui.ctx().request_repaint_after(Duration::from_millis(200));
    }
    let _ = (BG1, BG2, G1, LINE, money::fmt_usd);
}

struct Col {
    name: &'static str,
    weight: f32,
    right: bool,
}

const COLS: &[Col] = &[
    Col { name: "#", weight: 0.5, right: false },
    Col { name: "Time", weight: 0.9, right: false },
    Col { name: "Feature", weight: 1.4, right: false },
    Col { name: "Bet size", weight: 1.0, right: true },
    Col { name: "Multiplier", weight: 1.0, right: true },
    Col { name: "Payout", weight: 1.0, right: true },
    Col { name: "Balance", weight: 1.2, right: true },
];

fn table(app: &mut App, ui: &mut Ui) {
    let width = ui.available_width();
    if super::hits::tabs(app, ui, width) {
        super::hits::table(app, ui, width);
        return;
    }
    let sum: f32 = COLS.iter().map(|c| c.weight).sum();
    let widths: Vec<f32> = COLS.iter().map(|c| width * c.weight / sum).collect();
    let (hr, _) = ui.allocate_exact_size(vec2(width, 24.0), Sense::hover());
    let p = ui.painter();
    p.hline(hr.x_range(), hr.top(), Stroke::new(1.0, LINE));
    let mut x = hr.left();
    let cell_pos = |cell: Rect, right: bool| if right { (pos2(cell.right() - 10.0, cell.center().y), Align2::RIGHT_CENTER) } else { (pos2(cell.left() + 10.0, cell.center().y), Align2::LEFT_CENTER) };
    for (c, w) in COLS.iter().zip(&widths) {
        let cell = Rect::from_min_size(pos2(x, hr.top()), vec2(*w, hr.height()));
        let (pos, al) = cell_pos(cell, c.right);
        p.text(pos, al, c.name, theme::font_semi(10.5), MUTED);
        x += w;
    }
    p.hline(hr.x_range(), hr.bottom(), Stroke::new(1.0, LINE));
    let n = app.spins.len();
    if n == 0 {
        let (r, _) = ui.allocate_exact_size(vec2(width, 60.0), Sense::hover());
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "No spins yet", theme::font(12.0), DIM);
        return;
    }
    let total = app.spins_total;
    let mut open_hit = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, n.min(200), |ui, range| {
        for i in range {
            let r = &app.spins[i];
            // a win's row opens its card
            let opens = r.hit || (!r.held && r.multiplier >= 1.0);
            let (rr, resp) = ui.allocate_exact_size(vec2(width, ROW_H), if opens { Sense::click() } else { Sense::hover() });
            let resp = if opens { resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Open the card") } else { resp };
            if opens && resp.clicked() {
                open_hit = Some(total.saturating_sub(i as u64));
            }
            if !r.round_id.is_empty() {
                resp.on_hover_text(format!("round {}", r.round_id));
            }
            let p = ui.painter();
            if i % 2 == 1 {
                p.rect_filled(rr, CornerRadius::ZERO, alpha(Color32::WHITE, 4));
            }
            if r.hit {
                p.rect_filled(rr, CornerRadius::ZERO, alpha(GOLD, 22));
            }
            let c = kind_color(r);
            let cells: [(String, Color32); 7] = [
                ((total as usize - i).to_string(), MUTED),
                (clock(r.at), MUTED),
                (
                    if r.held { "free spins ready · browser".into() } else { r.feature.clone().unwrap_or_else(|| if r.free_spins { "free spins".into() } else { "–".into() }) },
                    if r.free_spins { GOLD } else { TEXT2 },
                ),
                (fmt_money(r.cost, &r.currency, r.decimals), TEXT2),
                if r.held { ("–".into(), MUTED) } else { (fmt_multi(r.multiplier), c) },
                if r.held { ("–".into(), MUTED) } else { (fmt_money(r.win, &r.currency, r.decimals), if r.multiplier >= 1.0 { GREEN } else { TEXT2 }) },
                (r.balance.map(|b| fmt_money(b, &r.currency, r.decimals)).unwrap_or("–".into()), MUTED),
            ];
            let mut x = rr.left();
            for ((text, color), (col, w)) in cells.iter().zip(COLS.iter().zip(&widths)) {
                let cell = Rect::from_min_size(pos2(x, rr.top()), vec2(*w, ROW_H));
                let (pos, al) = cell_pos(cell, col.right);
                p.text(pos, al, text, theme::font_mono(10.5), *color);
                x += w;
            }
        }
    });
    if let Some(n) = open_hit {
        super::hits::open_spin(app, n);
    }
    let _ = StrokeKind::Inside;
}
