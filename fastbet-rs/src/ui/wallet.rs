//! The balance pill on the top bar and its wallet dropdown: pick the coin to
//! bet with, show balances in fiat or crypto, hide empty wallets.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, mix, BG, BG1, BG2, BG3, DIM, G1, LINE, LINE2, MUTED, TEXT, TEXT2};
use super::widgets::{self, semi};
use super::App;
use crate::money::{format_units, fmt_usd};
use crate::settings::Unit;

/// Brand-ish colour for a coin symbol, grey for unknown ones.
fn coin_color(sym: &str) -> Color32 {
    match sym.to_ascii_lowercase().as_str() {
        "btc" => Color32::from_rgb(0xf7, 0x93, 0x1a),
        "eth" => Color32::from_rgb(0x62, 0x7e, 0xea),
        "ltc" => Color32::from_rgb(0xbf, 0xbb, 0xbb),
        "bnb" => Color32::from_rgb(0xf3, 0xba, 0x2f),
        "xrp" => Color32::from_rgb(0xd0, 0xd4, 0xdc),
        "usdt" => Color32::from_rgb(0x26, 0xa1, 0x7b),
        "usdc" => Color32::from_rgb(0x27, 0x75, 0xca),
        "dai" => Color32::from_rgb(0xf5, 0xac, 0x37),
        "doge" => Color32::from_rgb(0xc2, 0xa6, 0x33),
        "trx" => Color32::from_rgb(0xef, 0x00, 0x27),
        "sol" => Color32::from_rgb(0x9b, 0x5d, 0xe5),
        "matic" | "pol" => Color32::from_rgb(0x82, 0x47, 0xe5),
        "shib" => Color32::from_rgb(0xf0, 0x6b, 0x25),
        "ton" => Color32::from_rgb(0x00, 0x98, 0xea),
        "chips" => G1,
        _ => Color32::from_rgb(0x7c, 0x86, 0x9c),
    }
}

fn coin_icon(p: &egui::Painter, center: egui::Pos2, radius: f32, sym: &str) {
    let c = coin_color(sym);
    p.circle_filled(center, radius, c);
    let letter: String = sym.chars().next().map(|ch| ch.to_ascii_uppercase().to_string()).unwrap_or_default();
    p.text(center, Align2::CENTER_CENTER, letter, theme::font_bold(radius * 1.15), Color32::WHITE);
}

/// One line for a wallet amount, following the display mode.
fn amount_text(app: &App, cur: &str, units: i128) -> String {
    match (app.usd_mode(), app.info(cur)) {
        (true, Some(i)) if i.price > 0.0 => fmt_usd(app.usd_value(units, cur).unwrap_or(0.0)),
        (_, Some(i)) => format_units(units, i.decimals, 8),
        _ => "–".into(),
    }
    .replace("0.00000000", "0")
    .replace("$0.0000", "$0.00")
    .replace("$0.000", "$0.00")
}

/// The pill. Toggles the dropdown.
pub fn pill(app: &mut App, ui: &mut Ui) {
    let cur = app.active_currency();
    let bal = app.market.balance(&cur).unwrap_or(0);
    let main = if cur.is_empty() { "—".to_string() } else { amount_text(app, &cur, bal) };
    let crypto_mode = !app.usd_mode();
    let label = if crypto_mode && !cur.is_empty() { format!("{main} {}", cur.to_uppercase()) } else { main };

    let font = theme::font_bold(13.5);
    let tw = ui.painter().layout_no_wrap(label.clone(), font.clone(), TEXT).size().x;
    let w = tw + 16.0 + 22.0 + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered() || app.wallet_open;
    p.rect(rect, CornerRadius::same(16), if hovered { BG2 } else { BG1 }, Stroke::new(1.0, if app.wallet_open { alpha(G1, 140) } else { LINE }), StrokeKind::Inside);
    p.text(pos2(rect.left() + 16.0, rect.center().y), Align2::LEFT_CENTER, label, font, TEXT);
    if !cur.is_empty() {
        coin_icon(p, pos2(rect.left() + 16.0 + tw + 14.0, rect.center().y), 8.0, &cur);
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
                    contents(app, ui, width - 24.0);
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

fn contents(app: &mut App, ui: &mut Ui, width: f32) {
    // search
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

    // coin list
    let site = app.market.site_currency.clone().unwrap_or_default();
    let active = app.active_currency();
    let q = app.wallet_search.trim().to_ascii_lowercase();
    let mut coins: Vec<(String, i128, f64)> = app
        .market
        .currencies
        .iter()
        .filter(|(_, c)| !c.hidden)
        .map(|(k, _)| {
            let b = app.market.balance(k).unwrap_or(0);
            (k.clone(), b, app.usd_value(b, k).unwrap_or(0.0))
        })
        .filter(|(k, b, _)| (q.is_empty() || k.contains(&q)) && (!app.settings.hide_empty || *b > 0 || *k == active))
        .collect();
    coins.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.0.cmp(&b.0)));

    let mut pick: Option<String> = None;
    let row_h = 30.0;
    egui::ScrollArea::vertical().max_height(row_h * 6.5).auto_shrink([false, true]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        if coins.is_empty() {
            ui.label(semi("No wallets match", 11.5, DIM));
        }
        for (k, b, _) in &coins {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click());
            let on = *k == active;
            let p = ui.painter();
            if on {
                p.rect_filled(rect, CornerRadius::same(8), alpha(G1, 28));
            } else if resp.hovered() {
                p.rect_filled(rect, CornerRadius::same(8), BG3);
            }
            coin_icon(p, pos2(rect.left() + 18.0, rect.center().y), 8.0, k);
            p.text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, k.to_uppercase(), theme::font_bold(12.5), if on { G1 } else { TEXT });
            let amt = amount_text(app, k, *b);
            p.text(pos2(rect.right() - 10.0, rect.center().y), Align2::RIGHT_CENTER, amt, theme::font_semi(12.5), if *b > 0 { TEXT } else { MUTED });
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                pick = Some(k.clone());
            }
        }
    });
    if let Some(k) = pick {
        app.settings.currency = if k == site { String::new() } else { k };
        app.mark_dirty();
        app.wallet_open = false;
    }

    widgets::hsep(ui);

    // toggles
    let locked = app.auto.running;
    let mut fiat = app.usd_mode();
    if widgets::toggle(ui, "Display in fiat", &mut fiat, !locked) {
        app.set_unit(if fiat { Unit::Usd } else { Unit::Coin });
    }
    let mut hide = app.settings.hide_empty;
    if widgets::toggle(ui, "Hide empty balances", &mut hide, true) {
        app.settings.hide_empty = hide;
        app.mark_dirty();
    }
    let _ = (mix(TEXT2, DIM, 0.5), BG2);
}
