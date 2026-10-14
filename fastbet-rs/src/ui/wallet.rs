//! The balance pill on the top bar and its wallet dropdown: pick the coin to
//! bet with, show balances in fiat or crypto (and which fiat), hide empty wallets.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG, BG1, BG2, BG3, DIM, G1, LINE, LINE2, MUTED, TEXT, TEXT2};
use super::widgets::{self, semi};
use super::{coins, App};
use crate::currency;
use crate::money::{self, format_units};
use crate::settings::Unit;

/// One line for a wallet amount, following the display mode.
fn amount_text(app: &App, cur: &str, units: i128) -> String {
    let Some(info) = app.info(cur) else { return "–".into() };
    match app.fiat_value(units, cur) {
        // dust shows as zero, not as "$0.0000"
        Some(v) if app.fiat_mode() => money::fmt_fiat(if v.abs() < 0.00005 { 0.0 } else { v }, &app.fiat()),
        _ => format_units(units, info.decimals, 8),
    }
}

/// The pill. Toggles the dropdown.
pub fn pill(app: &mut App, ui: &mut Ui) {
    let cur = app.active_currency();
    let bal = app.market.balance(&cur).unwrap_or(0);
    let main = if cur.is_empty() { "—".to_string() } else { amount_text(app, &cur, bal) };
    let label = if !app.fiat_mode() && !cur.is_empty() { format!("{main} {}", cur.to_uppercase()) } else { main };

    let font = theme::font_bold(13.5);
    let tw = ui.painter().layout_no_wrap(label.clone(), font.clone(), TEXT).size().x;
    let w = tw + 16.0 + 22.0 + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered() || app.wallet_open;
    p.rect(rect, CornerRadius::same(16), if hovered { BG2 } else { BG1 }, Stroke::new(1.0, if app.wallet_open { alpha(G1, 140) } else { LINE }), StrokeKind::Inside);
    p.text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, label, font, TEXT);
    if !cur.is_empty() {
        coins::paint(p, pos2(rect.left() + 16.0 + tw + 14.0, rect.center().y), 16.0, app.casino(), &cur);
    }
    let c = pos2(rect.right() - 16.0, rect.center().y + (if app.wallet_open { -1.0 } else { 1.0 }));
    let dir = if app.wallet_open { -1.0 } else { 1.0 };
    p.line_segment([c + vec2(-4.0, -2.0 * dir), c + vec2(0.0, 2.0 * dir)], Stroke::new(1.6, MUTED));
    p.line_segment([c + vec2(0.0, 2.0 * dir), c + vec2(4.0, -2.0 * dir)], Stroke::new(1.6, MUTED));
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        app.wallet_open = !app.wallet_open;
        app.wallet_search.clear();
    }
    if app.wallet_open {
        dropdown(app, ui, rect);
    }
}

fn dropdown(app: &mut App, ui: &mut Ui, anchor: Rect) {
    let width = 300.0;
    let pos = pos2(anchor.center().x - width / 2.0, anchor.bottom() + 8.0);
    let area = egui::Area::new(egui::Id::new("wallet-dropdown"))
        .fixed_pos(pos)
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(BG1)
                .stroke(Stroke::new(1.0, LINE2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(12))
                .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(170) })
                .show(ui, |ui| {
                    ui.set_width(width - 24.0);
                    ui.spacing_mut().item_spacing.y = 8.0;
                    if app.wallet_fiats {
                        fiat_picker(app, ui, width - 24.0);
                    } else {
                        contents(app, ui, width - 24.0);
                    }
                });
        });
    // click outside closes it
    let rect = area.response.rect;
    let clicked_outside = ui.input(|i| {
        i.pointer.any_pressed() && i.pointer.interact_pos().map(|p| !rect.contains(p) && !anchor.contains(p)).unwrap_or(false)
    });
    if clicked_outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.wallet_open = false;
    }
}

fn search(app: &mut App, ui: &mut Ui, width: f32) {
    Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0, alpha(G1, 140)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(width - 22.0);
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                let p = ui.painter();
                p.circle_stroke(r.center() - vec2(1.5, 1.5), 4.5, Stroke::new(1.5, MUTED));
                p.line_segment([r.center() + vec2(2.0, 2.0), r.center() + vec2(5.5, 5.5)], Stroke::new(1.5, MUTED));
                let edit = egui::TextEdit::singleline(&mut app.wallet_search)
                    .frame(Frame::NONE)
                    .hint_text(egui::RichText::new("Search").color(DIM))
                    .font(theme::font(12.5))
                    .margin(Margin::same(0))
                    .desired_width(ui.available_width());
                let r = ui.add(edit);
                if !app.wallet_search_focused {
                    r.request_focus();
                    app.wallet_search_focused = true;
                }
            });
        });
}

fn contents(app: &mut App, ui: &mut Ui, width: f32) {
    search(app, ui, width);

    // coin list: the casino's coins, plus fiat wallets that hold money (Stake has ~150 fiats)
    let site = app.market.site_currency.clone().unwrap_or_default();
    let active = app.active_currency();
    let q = app.wallet_search.trim().to_ascii_lowercase();
    let mut list: Vec<(String, i128, f64)> = app
        .market
        .currencies
        .iter()
        .filter(|(_, c)| !c.hidden)
        .map(|(k, c)| {
            let b = app.market.balance(k).unwrap_or(0);
            (k.clone(), b, c.fiat)
        })
        .filter(|(k, b, fiat)| !fiat || *b > 0 || *k == active)
        .map(|(k, b, _)| {
            let usd = app.usd_value(b, &k).unwrap_or(0.0);
            (k, b, usd)
        })
        .filter(|(k, b, _)| (q.is_empty() || k.contains(&q)) && (!app.settings.hide_empty || *b > 0 || *k == active))
        .collect();
    // richest first; empty wallets in Stake's order
    list.sort_by(|a, b| {
        let (ra, rb) = (currency::rank(&a.0), currency::rank(&b.0));
        b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal).then(ra.cmp(&rb)).then_with(|| a.0.cmp(&b.0))
    });

    let mut pick: Option<String> = None;
    let (row_h, gap) = (30.0, 2.0);
    // sized from the list itself: a size remembered from an emptier frame would clip it
    let height = (list.len().max(1) as f32 * (row_h + gap) - gap).min(row_h * 6.5);
    egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = gap;
        if list.is_empty() {
            ui.label(semi("No wallets match", 11.5, DIM));
        }
        for (k, b, _) in &list {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
            let on = *k == active;
            let p = ui.painter();
            if on {
                p.rect_filled(rect, CornerRadius::same(8), alpha(G1, 28));
            } else if resp.hovered() {
                p.rect_filled(rect, CornerRadius::same(8), BG3);
            }
            coins::paint(p, pos2(rect.left() + 18.0, rect.center().y), 16.0, app.casino(), k);
            p.text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, k.to_uppercase(), theme::font_bold(12.5), if on { G1 } else { TEXT });
            let amt = amount_text(app, k, *b);
            p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, amt, theme::font_semi(12.5), if *b > 0 { TEXT } else { MUTED });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                pick = Some(k.clone());
            }
        }
    });
    if let Some(k) = pick {
        let changed = k != active;
        app.settings.currency = if k == site { String::new() } else { k };
        app.mark_dirty();
        app.wallet_open = false;
        if changed {
            app.slot_money_changed();
        }
    }

    widgets::hsep(ui);

    // toggles
    let locked = app.auto.running;
    let mut fiat = app.fiat_mode();
    if widgets::toggle(ui, "Display in fiat", &mut fiat, !locked) {
        app.set_unit(if fiat { Unit::Fiat } else { Unit::Coin });
    }
    if app.market.fiats.len() > 1 {
        fiat_row(app, ui, !locked);
    }
    let mut hide = app.settings.hide_empty;
    if widgets::toggle(ui, "Hide empty balances", &mut hide, true) {
        app.settings.hide_empty = hide;
        app.mark_dirty();
    }
}

/// "Fiat currency  [flag] EUR ›": opens the fiat picker.
fn fiat_row(app: &mut App, ui: &mut Ui, enabled: bool) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), if enabled { Sense::click() } else { Sense::hover() });
    let fiat = app.fiat();
    let p = ui.painter();
    let text = if enabled { TEXT2 } else { DIM };
    p.text(pos2(rect.left(), rect.center().y), Align2::LEFT_CENTER, "Fiat currency", theme::font(12.5), text);
    let c = pos2(rect.right() - 4.0, rect.center().y);
    let edge = if enabled && resp.hovered() { TEXT } else { MUTED };
    p.line_segment([c + vec2(-3.0, -4.0), c], Stroke::new(1.6, edge));
    p.line_segment([c, c + vec2(-3.0, 4.0)], Stroke::new(1.6, edge));
    let code = p.text(pos2(rect.right() - 14.0, rect.center().y), Align2::RIGHT_CENTER, fiat.to_uppercase(), theme::font_bold(12.5), if enabled { TEXT } else { DIM });
    coins::paint(p, pos2(code.left() - 12.0, rect.center().y), 16.0, app.casino(), &fiat);
    if enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        app.wallet_fiats = true;
        app.wallet_search.clear();
        app.wallet_search_focused = false;
    }
}

/// The casino's fiats in a grid, in its own order; picking one also shows amounts in fiat.
fn fiat_picker(app: &mut App, ui: &mut Ui, width: f32) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click());
    let p = ui.painter();
    let c = pos2(rect.left() + 5.0, rect.center().y);
    let edge = if resp.hovered() { TEXT } else { MUTED };
    p.line_segment([c + vec2(3.0, -4.0), c], Stroke::new(1.6, edge));
    p.line_segment([c, c + vec2(3.0, 4.0)], Stroke::new(1.6, edge));
    p.text(pos2(rect.left() + 18.0, rect.center().y), Align2::LEFT_CENTER, "Fiat currency", theme::font_bold(13.0), TEXT);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        app.wallet_fiats = false;
        app.wallet_search.clear();
        app.wallet_search_focused = false;
        return;
    }

    search(app, ui, width);

    let q = app.wallet_search.trim().to_ascii_lowercase();
    let current = app.fiat();
    let fiats: Vec<String> = app.market.fiats.iter().filter(|f| q.is_empty() || f.contains(&q)).cloned().collect();
    let locked = app.auto.running;
    let (cols, gap, row_h) = (3usize, 4.0, 30.0);
    let mut pick: Option<String> = None;
    let rows = fiats.len().div_ceil(cols).max(1) as f32;
    let height = (rows * (row_h + gap) - gap).min((row_h + gap) * 7.5);
    egui::ScrollArea::vertical().max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(gap, gap);
        if fiats.is_empty() {
            ui.label(semi("No currency matches", 11.5, DIM));
        }
        let cell_w = ((ui.available_width() - gap * (cols - 1) as f32) / cols as f32).floor();
        for row in fiats.chunks(cols) {
            ui.horizontal(|ui| {
                for f in row {
                    let (rect, resp) = ui.allocate_exact_size(vec2(cell_w, row_h), if locked { Sense::hover() } else { Sense::click() });
                    let on = *f == current;
                    let p = ui.painter();
                    if on {
                        p.rect_filled(rect, CornerRadius::same(8), alpha(G1, 28));
                    } else if resp.hovered() && !locked {
                        p.rect_filled(rect, CornerRadius::same(8), BG3);
                    }
                    coins::paint(p, pos2(rect.left() + 18.0, rect.center().y), 18.0, app.casino(), f);
                    let color = if on { G1 } else if locked { DIM } else { TEXT };
                    p.text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, f.to_uppercase(), theme::font_bold(12.5), color);
                    if !locked && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        pick = Some(f.clone());
                    }
                }
            });
        }
    });
    if let Some(f) = pick {
        app.set_fiat(&f);
        app.set_unit(Unit::Fiat);
        app.wallet_fiats = false;
        app.wallet_search.clear();
        app.wallet_search_focused = false;
    }
}
