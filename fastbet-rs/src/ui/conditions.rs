//! The Condition Builder's card: the run's rules as blocks that read like sentences ("Every 3
//! losses → Increase bet by 100%"), each opening into its editor, with presets to start from and
//! the run's live bet, target and streak while it goes.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder};

use super::theme::{self, alpha, mix, BG, BG2, BG3, DIM, G1, G2, LINE, LINE2, MUTED, TEXT, TEXT2};
use super::widgets::{self, caption, segmented, select, select_opt, text_box};
use super::App;
use crate::money::fmt_multi;
use crate::rules::{presets, Act, Arg, Cmp, Every, Of, Rule, Side, When};

const HELP: &str = "Rules checked from the top after every bet. Each one watches a count of bets, wins or losses (or a \
                    streak of them), the run's profit or loss, or the multiplier a bet landed on. When it fires it changes \
                    the next bet or target, resets the profit count, changes the seed or stops the run.";

const CMP_LABELS: [&str; 4] = ["≥  at least", ">  more than", "≤  at most", "<  less than"];

/// What a click on a rule asks for.
enum Op {
    Open,
    Delete,
    Up,
    Down,
    Copy,
}

pub(super) fn card(app: &mut App, ui: &mut Ui, w: f32, limbo: bool) {
    let on = app.settings.rules.iter().filter(|r| r.on && (limbo || !r.then.on_target())).count();
    let right = if on > 0 { widgets::semi(format!("{on} on"), 11.5, G2) } else { widgets::semi("None", 11.5, MUTED) };
    widgets::card(ui, w, "Conditions", Some(HELP), Some(right), |ui| body(app, ui, limbo));
}

fn body(app: &mut App, ui: &mut Ui, limbo: bool) {
    let locked = app.auto.running;
    let unit = if app.fiat_mode() { app.fiat().to_uppercase() } else { app.active_currency().to_uppercase() };
    if locked {
        live(app, ui, limbo);
    }
    let open_id = ui.id().with("cond-open");
    let mut open: Option<usize> = ui.data(|d| d.get_temp(open_id)).unwrap_or(None);
    // FASTBET_COND_OPEN=<n>: rule n's editor is open at start (UI work)
    if let Some(k) = std::env::var("FASTBET_COND_OPEN").ok().and_then(|v| v.parse().ok()).filter(|_| !ui.data(|d| d.get_temp::<bool>(open_id.with("dev")).unwrap_or(false))) {
        ui.data_mut(|d| d.insert_temp(open_id.with("dev"), true));
        open = Some(k);
    }
    if app.settings.rules.is_empty() {
        empty(ui);
    }
    let mut op = None;
    for i in 0..app.settings.rules.len() {
        if let Some(o) = block(app, ui, i, open == Some(i), limbo, locked, &unit) {
            op = Some((i, o));
        }
    }

    // add one, or start from a preset
    let list: Vec<_> = presets(&app.settings.amount).into_iter().filter(|p| limbo || !p.limbo_only).collect();
    let names: Vec<String> = list.iter().map(|p| p.name.to_string()).collect();
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let pw = 112.0;
        let aw = ui.available_width() - pw - 6.0;
        ui.scope(|ui| {
            ui.set_max_width(aw);
            if widgets::dashed_button(ui, "Add condition", widgets::FIELD_H, !locked).clicked() {
                app.settings.rules.push(Rule::default());
                open = Some(app.settings.rules.len() - 1);
                app.mark_dirty();
            }
        });
        ui.scope(|ui| {
            ui.set_max_width(pw);
            picked = select_opt(ui, "cond-presets", &names, None, "Presets", !locked);
        });
    });
    // the preset loaded says what it does, until its rules are changed
    let loaded_id = ui.id().with("cond-preset");
    if let Some(i) = picked {
        app.settings.rules = list[i].rules.clone();
        ui.data_mut(|d| d.insert_temp(loaded_id, list[i].name));
        open = None;
        app.mark_dirty();
    }
    let loaded: Option<&str> = ui.data(|d| d.get_temp(loaded_id));
    if let Some(p) = loaded.and_then(|name| list.iter().find(|p| p.name == name)).filter(|p| p.rules == app.settings.rules) {
        ui.label(widgets::semi(format!("{}: {}", p.name, p.about), 11.0, MUTED));
    }

    if let Some((i, o)) = op {
        let rules = &mut app.settings.rules;
        match o {
            Op::Open => open = if open == Some(i) { None } else { Some(i) },
            Op::Delete => {
                rules.remove(i);
                open = None;
            }
            Op::Up if i > 0 => {
                rules.swap(i, i - 1);
                open = Some(i - 1);
            }
            Op::Down if i + 1 < rules.len() => {
                rules.swap(i, i + 1);
                open = Some(i + 1);
            }
            Op::Copy => {
                let r = rules[i].clone();
                rules.insert(i + 1, r);
                open = Some(i + 1);
            }
            _ => {}
        }
        if !matches!(o, Op::Open) {
            app.mark_dirty();
        }
    }
    ui.data_mut(|d| d.insert_temp(open_id, open));
}

/// No rules yet: what to do.
fn empty(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
    let p = ui.painter();
    p.rect(r, CornerRadius::same(10), BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    widgets::icon(p, Rect::from_center_size(pos2(r.center().x, r.top() + 28.0), vec2(20.0, 20.0)), "branch", alpha(MUTED, 160));
    p.text(pos2(r.center().x, r.top() + 56.0), Align2::CENTER_CENTER, "No conditions yet", theme::font_semi(12.5), TEXT2);
    p.text(pos2(r.center().x, r.top() + 75.0), Align2::CENTER_CENTER, "Add one, or start from a preset", theme::font(11.5), MUTED);
}

/// The running run: next bet, target and streak, in one slim strip.
fn live(app: &App, ui: &mut Ui, limbo: bool) {
    let a = &app.auto;
    let cur = app.active_currency();
    let streak = if a.loss_streak > 0 {
        format!("{} loss{}", a.loss_streak, if a.loss_streak == 1 { "" } else { "es" })
    } else if a.win_streak > 0 {
        format!("{} win{}", a.win_streak, if a.win_streak == 1 { "" } else { "s" })
    } else {
        "–".into()
    };
    let mut cols = vec![("NEXT BET", app.prim(a.stake, &cur, false))];
    if limbo {
        cols.push(("TARGET", fmt_multi(a.target)));
    }
    cols.push(("STREAK", streak));
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::hover());
    let p = ui.painter();
    p.rect(r, CornerRadius::same(10), BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let cw = r.width() / cols.len() as f32;
    for (k, (label, value)) in cols.iter().enumerate() {
        let x = r.left() + cw * (k as f32 + 0.5);
        p.text(pos2(x, r.top() + 15.0), Align2::CENTER_CENTER, *label, theme::font_semi(9.5), MUTED);
        widgets::fit_text(p, pos2(x, r.top() + 31.0), Align2::CENTER_CENTER, value, theme::font_semi(12.5), TEXT, cw - 12.0);
        if k > 0 {
            p.vline(r.left() + cw * k as f32, (r.top() + 10.0)..=(r.bottom() - 10.0), Stroke::new(1.0, LINE));
        }
    }
}

/// One rule: its sentence, and its editor while open.
fn block(app: &mut App, ui: &mut Ui, i: usize, is_open: bool, limbo: bool, locked: bool, unit: &str) -> Option<Op> {
    let mut r = app.settings.rules[i].clone();
    let skipped = r.then.on_target() && !limbo;
    let fired = app.auto.fired.get(i).copied().unwrap_or(0);

    // a rule that just fired glows for a moment
    let now = ui.input(|inp| inp.time);
    let fired_id = ui.id().with(("cond-fired", i));
    let (seen, at): (u32, f64) = ui.data(|d| d.get_temp(fired_id)).unwrap_or((0, f64::MIN));
    let at = if fired > seen { now } else { at };
    ui.data_mut(|d| d.insert_temp(fired_id, (fired, at)));
    let glow = (1.0 - ((now - at) / 0.6) as f32).clamp(0.0, 1.0);
    if glow > 0.0 {
        ui.ctx().request_repaint();
    }

    let mut op = None;
    let stroke = if glow > 0.0 {
        mix(if is_open { LINE2 } else { LINE }, G1, glow)
    } else if is_open {
        LINE2
    } else {
        LINE
    };
    Frame::new()
        .fill(if glow > 0.0 { mix(BG, alpha(G1, 255), glow * 0.08) } else { BG })
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin { left: 10, right: 10, top: 8, bottom: if is_open { 10 } else { 8 } })
        .show(ui, |ui| {
            let w = ui.available_width();
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 8.0;

            // the sentence
            let (hr, hresp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
            let hresp = hresp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if hresp.clicked() {
                op = Some(Op::Open);
            }
            let sw = Rect::from_center_size(pos2(hr.right() - 15.0, hr.center().y), vec2(30.0, 16.0));
            let mut sui = ui.new_child(UiBuilder::new().max_rect(sw));
            if widgets::switch(&mut sui, r.on, !locked) {
                r.on = !r.on;
            }
            let live = r.on && !skipped;
            let p = ui.painter();
            if hresp.hovered() && !is_open {
                p.rect_filled(hr.expand2(vec2(6.0, 4.0)), CornerRadius::same(7), alpha(Color32::WHITE, 5));
            }
            let badge = pos2(hr.left() + 11.0, hr.center().y);
            p.circle(badge, 11.0, if live { BG3 } else { BG2 }, Stroke::new(1.0, if live { LINE2 } else { LINE }), );
            p.text(badge, Align2::CENTER_CENTER, format!("{}", i + 1), theme::font_semi(11.0), if live { TEXT } else { DIM });
            let mut room = sw.left() - 10.0 - (hr.left() + 30.0);
            if locked && fired > 0 {
                let g = p.layout_no_wrap(format!("×{fired}"), theme::font_mono(10.5), MUTED);
                let x = sw.left() - 8.0 - g.size().x;
                room -= g.size().x + 8.0;
                p.galley(pos2(x, hr.center().y - g.size().y / 2.0), g, MUTED);
            }
            let x = hr.left() + 30.0;
            widgets::fit_text(p, pos2(x, hr.center().y - 8.0), Align2::LEFT_CENTER, &r.when_text(unit), theme::font_semi(12.5), if live { TEXT } else { DIM }, room);
            p.text(pos2(x, hr.center().y + 9.0), Align2::LEFT_CENTER, "→", theme::font_semi(11.5), if live { G2 } else { DIM });
            let then = if skipped { format!("{}  ·  Limbo only", r.then_text(unit)) } else { r.then_text(unit) };
            widgets::fit_text(p, pos2(x + 16.0, hr.center().y + 9.0), Align2::LEFT_CENTER, &then, theme::font(11.5), if live { TEXT2 } else { DIM }, room - 16.0);

            if is_open {
                let (sep, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
                ui.painter().hline(sep.x_range(), sep.center().y, Stroke::new(1.0, LINE));
                editor(ui, i, &mut r, limbo, !locked, unit);
                if let Some(o) = footer(ui, i, app.settings.rules.len(), !locked) {
                    op = Some(o);
                }
            }
        });

    if r != app.settings.rules[i] {
        app.settings.rules[i] = r;
        app.mark_dirty();
    }
    op
}

/// The rule's fields: what it watches, then what it does.
fn editor(ui: &mut Ui, i: usize, r: &mut Rule, limbo: bool, enabled: bool, unit: &str) {
    caption(ui, "When");
    let kinds = [When::Bets, When::Profit, When::Result];
    let sel = kinds.iter().position(|k| *k == r.when).unwrap_or(0);
    if let Some(k) = segmented(ui, &["Bets", "Profit", "Result"], sel, enabled) {
        r.when = kinds[k];
    }
    let cmp_items: Vec<String> = CMP_LABELS.iter().map(|s| s.to_string()).collect();
    let cmp_sel = Cmp::ALL.iter().position(|c| *c == r.cmp).unwrap_or(0);
    match r.when {
        When::Bets => {
            pair(ui, 64.0, |ui| {
                let items: Vec<String> = Every::ALL.iter().map(|e| e.label().to_string()).collect();
                let sel = Every::ALL.iter().position(|e| *e == r.every).unwrap_or(0);
                if let Some(k) = select(ui, &format!("cond-every-{i}"), &items, sel, enabled) {
                    r.every = Every::ALL[k];
                }
            }, |ui| {
                text_box(ui, &mut r.n, "1", None, enabled, false);
            });
            let ofs: &[Of] = if r.every == Every::Every { &[Of::Bets, Of::Wins, Of::Losses] } else { &[Of::Wins, Of::Losses] };
            let labels: Vec<&str> = ofs.iter().map(|o| match o {
                Of::Bets => "Bets",
                Of::Wins => "Wins",
                Of::Losses => "Losses",
            }).collect();
            let cur = if r.every == Every::Every { r.of } else { r.streak_of() };
            let sel = ofs.iter().position(|o| *o == cur).unwrap_or(0);
            if let Some(k) = segmented(ui, &labels, sel, enabled) {
                r.of = ofs[k];
            }
        }
        When::Profit => {
            halves(ui, |ui| {
                let sel = if r.side == Side::Profit { 0 } else { 1 };
                if let Some(k) = segmented(ui, &["Profit", "Loss"], sel, enabled) {
                    r.side = if k == 0 { Side::Profit } else { Side::Loss };
                }
            }, |ui| {
                if let Some(k) = select(ui, &format!("cond-cmp-{i}"), &cmp_items, cmp_sel, enabled) {
                    r.cmp = Cmp::ALL[k];
                }
            });
            text_box(ui, &mut r.value, "0.00", Some(unit), enabled, false);
        }
        When::Result => {
            halves(ui, |ui| {
                if let Some(k) = select(ui, &format!("cond-cmp-{i}"), &cmp_items, cmp_sel, enabled) {
                    r.cmp = Cmp::ALL[k];
                }
            }, |ui| {
                text_box(ui, &mut r.value, "2.00", Some("×"), enabled, false);
            });
        }
    }

    caption(ui, "Then");
    // target actions only where there is a target (the one set stays listed so it shows)
    let acts: Vec<Act> = Act::ALL.into_iter().filter(|a| limbo || !a.on_target() || *a == r.then).collect();
    let items: Vec<String> = acts.iter().map(|a| a.label().to_string()).collect();
    let sel = acts.iter().position(|a| *a == r.then).unwrap_or(0);
    let suffix = match r.then.arg() {
        Arg::Pct => Some("%"),
        Arg::Amount => Some(unit),
        Arg::Multi => Some("×"),
        Arg::None => None,
    };
    match suffix {
        Some(sfx) => pair(ui, 92.0, |ui| {
            if let Some(k) = select(ui, &format!("cond-act-{i}"), &items, sel, enabled) {
                r.then = acts[k];
            }
        }, |ui| {
            text_box(ui, &mut r.by, "0", Some(sfx), enabled, false);
        }),
        None => {
            if let Some(k) = select(ui, &format!("cond-act-{i}"), &items, sel, enabled) {
                r.then = acts[k];
            }
        }
    }
    let note = match r.then {
        Act::BetDown | Act::BetSub => Some("Never takes the bet below the base bet."),
        Act::ResetProfit => Some("Profit and loss conditions count again from 0."),
        Act::Seed => Some("Waits for the bets in flight, then changes the seed."),
        a if a.on_target() && !limbo => Some("Only Limbo has a target: skipped on this game."),
        _ => None,
    };
    if let Some(n) = note {
        ui.label(widgets::semi(n, 10.5, DIM));
    }
}

/// Move up / down, duplicate on the left, delete on the right.
fn footer(ui: &mut Ui, i: usize, count: usize, enabled: bool) -> Option<Op> {
    let mut op = None;
    if !enabled {
        return None;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        if i > 0 && widgets::glyph_button(ui, "up", "Move up").clicked() {
            op = Some(Op::Up);
        }
        if i + 1 < count && widgets::glyph_button(ui, "down", "Move down").clicked() {
            op = Some(Op::Down);
        }
        if widgets::glyph_button(ui, "copy", "Duplicate").clicked() {
            op = Some(Op::Copy);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::glyph_button(ui, "trash", "Delete").clicked() {
                op = Some(Op::Delete);
            }
        });
    });
    op
}

/// Two controls side by side: the first takes what the second (`right_w` wide) leaves.
fn pair(ui: &mut Ui, right_w: f32, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        let lw = ui.available_width() - right_w - 6.0;
        ui.scope(|ui| {
            ui.set_min_width(lw);
            ui.set_max_width(lw);
            left(ui);
        });
        ui.scope(|ui| {
            ui.set_min_width(right_w);
            ui.set_max_width(right_w);
            right(ui);
        });
    });
}

/// Two controls of equal width side by side.
fn halves(ui: &mut Ui, left: impl FnOnce(&mut Ui), right: impl FnOnce(&mut Ui)) {
    let half = (ui.available_width() - 6.0) / 2.0;
    pair(ui, half, left, right);
}
