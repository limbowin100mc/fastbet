//! The settings column every game page shares: the bet card, then the auto-run cards that fold
//! away (strategy, bets and speed, stops, force restart), seeds and sound. Limbo's own panel too.

use egui::{vec2, Align, Frame, Layout, Margin, RichText, Ui};

use super::theme::{self, MUTED, TEXT2};
use super::widgets::{self, caption, caption_help, card, check_field_help, chip_button_sized, field, fold, readout, segmented, text_box, toggle_help, Btn, FIELD_H};
use super::App;
use crate::money::{self, fmt_multi, mul_f};
use crate::settings::{Adjust, Mode, Unit};

const QUICK_TARGETS: &[&str] = &["1.5", "2", "3", "5", "10", "50", "100"];

pub fn show(app: &mut App, ui: &mut Ui) {
    let panel_w = ui.available_width();
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin { left: 14, right: 12, top: 2, bottom: 14 }).show(ui, |ui| {
            // keep every section the same width even if a row overflows by a pixel
            let w = panel_w - 26.0 - 4.0;
            ui.spacing_mut().item_spacing.y = 10.0;
            ui.set_max_width(w);
            general(app, ui, w);
            if app.settings.mode == Mode::Auto {
                strategy(app, ui, w);
                bets_and_speed(app, ui, w);
                stops(app, ui, w);
                force_restart(app, ui, w);
            }
            seed(app, ui, w);
            notifications(app, ui, w);
        });
    });
}

fn general(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.auto.running;
    let cur = app.active_currency();
    bet_card(app, ui, w);
    let right = target_readout(app, &cur);
    card(ui, w, "Target", Some("The multiplier a bet has to reach. Higher targets pay more and hit less often."), Some(right), |ui| {
        target(app, ui, locked);
    });
}

/// The bet card every game page starts with: bet size, and what it is worth in the other unit.
pub(super) fn bet_card(app: &mut App, ui: &mut Ui, w: f32) {
    let cur = app.active_currency();
    let other = app.to_base(&app.settings.amount, &cur).map(|b| app.sec(b, &cur, false)).unwrap_or_default();
    let right = (!other.is_empty()).then(|| widgets::semi(format!("≈ {other}"), 11.5, MUTED));
    card(ui, w, "Bet", None, right, |ui| bet_size(app, ui));
}

/// Bet size with ½ / 2× and the coin / fiat unit switch.
fn bet_size(app: &mut App, ui: &mut Ui) {
    let locked = app.auto.running;
    let cur = app.active_currency();
    let cur_up = cur.to_uppercase();
    {
        let fiat_up = app.fiat().to_uppercase();
        let unit_sfx = if app.fiat_mode() { fiat_up.clone() } else { cur_up.clone() };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = ui.available_width() - 2.0 * (40.0 + 6.0);
            ui.scope(|ui| {
                ui.set_max_width(w);
                ui.set_min_width(w);
                if text_box(ui, &mut app.settings.amount, "0.00", Some(&unit_sfx), !locked, false).changed() {
                    app.mark_dirty();
                }
            });
            if chip_button_sized(ui, "½", false, !locked, vec2(40.0, FIELD_H)).on_hover_text("Halve").clicked() {
                app.adjust_amount(false);
            }
            if chip_button_sized(ui, "2×", false, !locked, vec2(40.0, FIELD_H)).on_hover_text("Double").clicked() {
                app.adjust_amount(true);
            }
        });
        // unit
        widgets::row(ui, 32.0, |ui| {
            caption(ui, "Unit");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.scope(|ui| {
                    ui.set_min_width(132.0);
                    ui.set_max_width(132.0);
                    let coin_label = if cur_up.is_empty() { "COIN".to_string() } else { cur_up.clone() };
                    let sel = if app.fiat_mode() { 1 } else { 0 };
                    if let Some(i) = segmented(ui, &[&coin_label, &fiat_up], sel, !locked) {
                        app.set_unit(if i == 1 { Unit::Fiat } else { Unit::Coin });
                    }
                });
            });
        });
    }
}

/// Limbo's target card header: the chance to win and the profit, or the allowed range.
fn target_readout(app: &App, cur: &str) -> RichText {
    let id = app.casino();
    let txt = match app.target() {
        Some(t) => {
            let mut s = format!("{:.*}% win", if t > 9900.0 { 4 } else { 2 }, id.win_chance(t));
            if let Some(b) = app.to_base(&app.settings.amount, cur) {
                s += &format!("  ·  +{}", app.prim(mul_f(b, t) - b, cur, false));
            }
            s
        }
        None => format!("{}–{}×", id.min_target(), money::group_thousands(&format!("{}", id.max_target() as i64))),
    };
    widgets::semi(txt, 11.5, if app.target().is_some() { theme::G2 } else { MUTED })
}

fn target(app: &mut App, ui: &mut Ui, locked: bool) {
    {
        if text_box(ui, &mut app.settings.target, "2.00", Some("×"), !locked, false).changed() {
            app.mark_dirty();
        }
        ui.horizontal(|ui| {
            let gap = 5.0;
            ui.spacing_mut().item_spacing.x = gap;
            let n = QUICK_TARGETS.len() as f32;
            let cw = ((ui.available_width() - gap * (n - 1.0)) / n).floor();
            let t = money::num(&app.settings.target);
            for q in QUICK_TARGETS {
                let on = t == money::num(q);
                if chip_button_sized(ui, &format!("{q}×"), on, !locked, vec2(cw, 28.0)).clicked() {
                    app.settings.target = q.to_string();
                    app.mark_dirty();
                }
            }
        });
    }
}

pub(super) fn strategy(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.auto.running;
    let active = app.settings.on_win != Adjust::Reset || app.settings.on_loss != Adjust::Reset;
    let pct = |adj: Adjust, v: &str| (adj == Adjust::Increase).then(|| format!("{}%", v.trim()));
    let summary = match (pct(app.settings.on_win, &app.settings.on_win_pct), pct(app.settings.on_loss, &app.settings.on_loss_pct)) {
        (None, None) => "Off".to_string(),
        (Some(w), None) => format!("Win {w}"),
        (None, Some(l)) => format!("Loss {l}"),
        (Some(w), Some(l)) => format!("Win {w} · Loss {l}"),
    };
    let mut open = app.settings.open.strategy;
    fold(ui, w, "Increase on win / loss", &summary, active, &mut open, |ui| {
        for (label, which) in [("After a win", true), ("After a loss", false)] {
            if which {
                caption(ui, label);
            } else {
                caption_help(ui, label, "Change by % adds that much to the stake: +100% after a loss doubles it (martingale). Reset goes back to the base bet.");
            }
            let (adj, pct) = if which {
                (&mut app.settings.on_win, &mut app.settings.on_win_pct)
            } else {
                (&mut app.settings.on_loss, &mut app.settings.on_loss_pct)
            };
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let w = ui.available_width() - 90.0;
                ui.scope(|ui| {
                    ui.set_min_width(w);
                    ui.set_max_width(w);
                    let sel = if *adj == Adjust::Increase { 1 } else { 0 };
                    if let Some(i) = segmented(ui, &["Reset to base", "Change by %"], sel, !locked) {
                        *adj = if i == 1 { Adjust::Increase } else { Adjust::Reset };
                        changed = true;
                    }
                });
                ui.scope(|ui| {
                    ui.set_min_width(84.0);
                    ui.set_max_width(84.0);
                    changed |= text_box(ui, pct, "0", Some("%"), !locked && *adj == Adjust::Increase, false).changed();
                });
            });
            if changed {
                app.mark_dirty();
            }
        }
    });
    if open != app.settings.open.strategy {
        app.settings.open.strategy = open;
        app.mark_dirty();
    }
}

pub(super) fn bets_and_speed(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.auto.running;
    let cnt: u32 = app.settings.count.trim().parse().unwrap_or(0);
    let dl = money::num(&app.settings.delay).unwrap_or(0.0);
    let mut parts = vec![if cnt > 0 { format!("{cnt} bets") } else { "No limit".to_string() }];
    if dl > 0.0 {
        parts.push(format!("{} ms", app.settings.delay.trim()));
    }
    if app.settings.async_bets > 1 {
        parts.push(format!("{} in flight", app.settings.async_bets));
    }
    let mut open = app.settings.open.count;
    fold(ui, w, "Bets, delay & async", &parts.join(" · "), cnt > 0 || dl > 0.0 || app.settings.async_bets > 1, &mut open, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = (ui.available_width() - 8.0) / 2.0;
            ui.scope(|ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                if field(ui, "Number of bets", &mut app.settings.count, "0 = ∞", None, !locked).changed() {
                    app.mark_dirty();
                }
            });
            ui.scope(|ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                if field(ui, "Delay", &mut app.settings.delay, "0", Some("ms"), !locked).changed() {
                    app.mark_dirty();
                }
            });
        });
        caption_help(ui, "Async mode · bets in flight", "Sends the next bet before the previous one answers, for more bets per second. Flat stake only: with Change by % the bets go one at a time.");
        const LANES: [u32; 6] = [1, 2, 3, 4, 6, 8];
        let cur = app.settings.async_bets.max(1);
        let sel = LANES.iter().position(|n| *n == cur).unwrap_or(0);
        if let Some(i) = segmented(ui, &["1", "2", "3", "4", "6", "8"], sel, !locked) {
            app.settings.async_bets = LANES[i];
            app.mark_dirty();
            app.sync_concurrency();
        }
    });
    if open != app.settings.open.count {
        app.settings.open.count = open;
        app.mark_dirty();
    }
}

pub(super) fn stops(app: &mut App, ui: &mut Ui, w: f32) {
    let locked = app.auto.running;
    let s = &app.settings;
    let active = [&s.stop_profit, &s.stop_loss, &s.max_bet].iter().any(|v| !v.trim().is_empty()) || s.stop_on_hit;
    let unit = if app.fiat_mode() { app.fiat().to_uppercase() } else { app.active_currency().to_uppercase() };
    let mut parts = vec![];
    for (name, v) in [("Profit", &s.stop_profit), ("Loss", &s.stop_loss), ("Max", &s.max_bet)] {
        if !v.trim().is_empty() {
            parts.push(format!("{name} {}", v.trim()));
        }
    }
    if s.stop_on_hit {
        parts.push(if s.hit_multi.trim().is_empty() { "Hit on win".to_string() } else { format!("Hit {}×", s.hit_multi.trim()) });
    }
    let summary = if parts.is_empty() { "Off".to_string() } else { parts.join(" · ") };
    let mut open = app.settings.open.stops;
    fold(ui, w, "Stop on profit / loss / hit", &summary, active, &mut open, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let w = (ui.available_width() - 8.0) / 2.0;
            ui.scope(|ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                if field(ui, "Stop profit", &mut app.settings.stop_profit, "off", Some(&unit), !locked).changed() {
                    app.mark_dirty();
                }
            });
            ui.scope(|ui| {
                ui.set_min_width(w);
                ui.set_max_width(w);
                if field(ui, "Stop loss", &mut app.settings.stop_loss, "off", Some(&unit), !locked).changed() {
                    app.mark_dirty();
                }
            });
        });
        if field(ui, "Max bet", &mut app.settings.max_bet, "off", Some(&unit), !locked).changed() {
            app.mark_dirty();
        }
        // an empty hit line stops on a win: Limbo's target, any winning round elsewhere
        let help = if app.screen == super::Screen::Limbo {
            let target = app.target().map(|t| format!(" ({})", fmt_multi(t))).unwrap_or_default();
            format!("Stops the run on a bet that rolls at least this multiplier. Empty: the target{target}.")
        } else {
            "Stops the run on a round that pays at least this many times the stake. Empty: any winning round.".into()
        };
        let mut on = app.settings.stop_on_hit;
        if check_field_help(ui, "Stop on hit  ≥", Some(&help), &mut on, &mut app.settings.hit_multi, Some("×"), !locked) {
            app.settings.stop_on_hit = on;
            app.mark_dirty();
        }
    });
    if open != app.settings.open.stops {
        app.settings.open.stops = open;
        app.mark_dirty();
    }
}

/// Force restart betting: a run that ends by itself starts again at once. Not locked while
/// running, so it can be switched on or off mid-run.
pub(super) fn force_restart(app: &mut App, ui: &mut Ui, w: f32) {
    let mut open = app.settings.open.restart;
    let active = app.settings.force_restart || app.settings.restart_on_error;
    let summary = match (app.settings.force_restart, app.settings.restart_on_error) {
        (false, false) => "Off",
        (true, false) => "Always",
        (false, true) => "On errors",
        (true, true) => "Always · on errors",
    };
    fold(ui, w, "Force restart betting", summary, active, &mut open, |ui| {
        let (mut always, mut on_error) = (app.settings.force_restart, app.settings.restart_on_error);
        let help_always = "When the run ends by itself (stop profit or loss, a hit, the number of bets, max bet, an error), it starts \
                           again at once from the base bet, with its counters back at zero. Stop, Stop on next win and \
                           running out of balance still end it.";
        if toggle_help(ui, "Restart always", Some(help_always), &mut always, true) {
            app.set_force_restart(always, on_error);
        }
        let help_errors = "An error doesn't stop the run: it bets again at once, with the same bet size and counters. When errors \
                           keep coming, it waits 1, 2, 4, then 8 s between tries. Insufficient balance still stops it.";
        if toggle_help(ui, "Restart on errors", Some(help_errors), &mut on_error, true) {
            app.set_force_restart(always, on_error);
        }
        let a = &app.auto;
        if a.running && (a.restarts > 0 || a.errors > 0) {
            readout(ui, format!("Restarted {}×  ·  errors {}×  ·  {}", a.restarts, a.errors, a.msg), TEXT2, theme::G1);
        }
    });
    if open != app.settings.open.restart {
        app.settings.open.restart = open;
        app.mark_dirty();
    }
}

pub(super) fn seed(app: &mut App, ui: &mut Ui, w: f32) {
    let every = app.settings.seed_every();
    let summary = if every > 0 { format!("Every {every} bets") } else { "Off".to_string() };
    let mut open = app.settings.open.seed;
    fold(ui, w, "Auto seed changes", &summary, app.settings.seed_enabled, &mut open, |ui| {
        let mut en = app.settings.seed_enabled;
        let help = "Gets a new client seed (and server seed) after this many bets, without stopping the run.";
        if check_field_help(ui, "Change after bets", Some(help), &mut en, &mut app.settings.seed_every, None, true) {
            app.settings.seed_enabled = en;
            app.mark_dirty();
            app.sync_seed_policy();
        }
        let mut parts = vec![];
        if app.seed.busy {
            parts.push("Changing…".to_string());
        } else if every > 0 {
            parts.push(format!("Next change in {} bets", every.saturating_sub(app.seed.count)));
        }
        if app.seed.rotations > 0 {
            parts.push(format!("{} changed", app.seed.rotations));
        }
        if !parts.is_empty() {
            readout(ui, parts.join("  ·  "), TEXT2, theme::G1);
        }
        kv(ui, "Client seed", app.provable.client_seed.clone().unwrap_or("–".into()));
        kv(ui, "Nonce", app.provable.nonce.map(|n| n.to_string()).unwrap_or("–".into()));
        if let Some(h) = &app.provable.server_seed_hash {
            kv(ui, "Server hash", h.clone());
        }
        let can = app.ready() && !app.seed.busy;
        if widgets::button(ui, "Change seed now", Btn::Ghost, can, Some(ui.available_width())).clicked() {
            app.rotate_now();
        }
    });
    if open != app.settings.open.seed {
        app.settings.open.seed = open;
        app.mark_dirty();
    }
}

fn kv(ui: &mut Ui, k: &str, v: String) {
    ui.horizontal(|ui| {
        caption(ui, k);
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add(egui::Label::new(widgets::mono(v.clone(), 11.0, TEXT2)).truncate()).on_hover_text(v);
        });
    });
}

/// Header chips of a run force restart kept going: restarts and errors bet through, hover for the last why.
pub(super) fn restarts_chip(app: &App, ui: &mut Ui) {
    let a = &app.auto;
    if !a.running {
        return;
    }
    if a.restarts > 0 {
        widgets::meta_chip(ui, "refresh", &format!("Restarted ×{}", a.restarts), theme::G2).on_hover_text(&a.msg);
    }
    if a.errors > 0 {
        widgets::meta_chip(ui, "cross", &format!("Errors ×{}", a.errors), theme::GOLD).on_hover_text(&a.msg);
    }
}

pub(super) fn notifications(app: &mut App, ui: &mut Ui, w: f32) {
    let summary = match (app.settings.hit_sound, app.settings.hotkeys) {
        (false, false) => "Off",
        (true, false) => "Sound",
        (false, true) => "Hotkeys",
        (true, true) => "Sound · Hotkeys",
    };
    let mut open = app.settings.open.keys;
    fold(ui, w, "Sound & hotkeys", summary, app.settings.hit_sound || app.settings.hotkeys, &mut open, |ui| {
        let mut snd = app.settings.hit_sound;
        if toggle_help(ui, "Sound on hit", None, &mut snd, true) {
            app.settings.hit_sound = snd;
            app.mark_dirty();
        }
        let mut hk = app.settings.hotkeys;
        if toggle_help(ui, "Hotkeys", None, &mut hk, true) {
            app.settings.hotkeys = hk;
            app.mark_dirty();
        }
        for (k, what) in [("Space", "Place one bet"), ("P", "Pause / resume"), ("Esc", "Stop"), ("Ctrl+R", "Reset session: history, stats, seed")] {
            ui.horizontal(|ui| {
                let kw = ui.painter().layout_no_wrap(k.to_string(), theme::font_semi(10.0), TEXT2).size().x + 12.0;
                let (r, _) = ui.allocate_exact_size(vec2(kw.max(26.0), 20.0), egui::Sense::hover());
                widgets::keycap(ui.painter(), r, k, TEXT2);
                ui.add_space(4.0);
                ui.label(RichText::new(what).size(11.5).color(MUTED));
            });
        }
    });
    if open != app.settings.open.keys {
        app.settings.open.keys = open;
        app.mark_dirty();
    }
}
