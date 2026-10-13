//! Account screens: pick an account, then log in (with the connection check).

use std::time::Duration;

use egui::{pos2, vec2, Align, Align2, CornerRadius, Frame, Layout, Margin, Rect, RichText, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, DIM, G1, G2, GOLD, GREEN, LINE, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, icon, list_row, pill_button, Btn};
use super::{App, Stage};
use crate::casino::{CasinoId, ConnStatus};
use crate::settings::Account;
use crate::cloudflare;

fn header(ui: &mut Ui, full: Rect) {
    ui.add_space(full.height() * 0.07 + 4.0);
    widgets::wordmark(ui, 32.0);
    ui.add_space(4.0);
}

fn footer(ui: &Ui, full: Rect) {
    ui.painter().text(pos2(full.left() + 20.0, full.bottom() - 16.0), Align2::LEFT_CENTER, format!("v{}", env!("CARGO_PKG_VERSION")), theme::font(10.5), DIM);
}

const CASINO_H: f32 = 48.0;
const ACCOUNT_H: f32 = 40.0;
/// Height of [`list_row`] ("More casinos").
const MORE_H: f32 = 42.0;
/// A casino with this many accounts gets a search box over them.
const SEARCH_FROM: usize = 8;
const SEARCH_H: f32 = 46.0;

/// What a click on the account list asks for.
enum Pick {
    Toggle(CasinoId),
    Open(String),
    Add(CasinoId),
    Arm(String),
    Remove(String),
}

/// "Pick your casino": one row per casino with its account count. A click opens it on its
/// accounts (scrolling once there are more than fit), each with a remove button, then "Add account".
pub fn accounts(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    let mut pick = None;
    // the page scrolls too, for windows too short for the open casino's two first accounts
    egui::ScrollArea::vertical().id_salt("account-list").auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
        let page_top = ui.cursor().top();
        header(ui, full);
        ui.add_space(full.height() * 0.045);
        ui.label(widgets::semi("Pick your casino", 13.5, TEXT2));
        ui.add_space(18.0);
        let w = 500.0_f32.min(full.width() - 40.0);
        // what the open casino's accounts may take: the window under the card's top, less the
        // other rows, "Add account" and a margin (the version sits left of the card). Measured
        // from the page's top, so it doesn't change as the page scrolls.
        let rows = CasinoId::ALL.len() as f32 * CASINO_H + MORE_H + ACCOUNT_H;
        let room = full.height() - (ui.cursor().top() - page_top) - rows - 20.0;
        Frame::new()
            .fill(BG1)
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(0))
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 0.0;
                for &casino in CasinoId::ALL {
                    casino_group(app, ui, casino, room, &mut pick);
                }
                list_row(ui, |p, r| widgets::letter_mark(p, r, "?", DIM), "More casinos", "Coming soon", false);
            });
        ui.add_space(16.0);
    }));
    // a click anywhere but the armed remove button (or Esc) takes it back
    let clicked = ui.input(|i| i.pointer.any_click() || i.key_pressed(egui::Key::Escape));
    if clicked && !matches!(pick, Some(Pick::Arm(_) | Pick::Remove(_))) {
        app.remove_row = None;
    }
    match pick {
        Some(Pick::Toggle(casino)) => app.toggle_casino(casino),
        Some(Pick::Open(id)) => app.open_account(&id),
        Some(Pick::Add(casino)) => app.add_account(casino),
        Some(Pick::Arm(id)) => app.remove_row = Some(id),
        Some(Pick::Remove(id)) => app.forget_account(&id),
        None => {}
    }
    footer(ui, full);
    gear_menu(app, ui, full);
}

/// One casino of the account list: its row, and under it (while open) its accounts and "Add account".
fn casino_group(app: &mut App, ui: &mut Ui, casino: CasinoId, room: f32, pick: &mut Option<Pick>) {
    let n = app.settings.accounts.iter().filter(|a| a.casino == casino).count();
    let t = ui.ctx().animate_bool_with_time(egui::Id::new(("casino-group", casino.key())), app.open_casino == Some(casino), 0.16);
    let w = ui.available_width();

    let (rect, resp) = ui.allocate_exact_size(vec2(w, CASINO_H), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let hot = resp.hovered();
    let p = ui.painter();
    if hot {
        p.rect_filled(rect, CornerRadius::ZERO, alpha(egui::Color32::WHITE, 6));
    }
    let cy = rect.center().y;
    // the open casino's marker on the card's edge
    if t > 0.0 {
        let bar = Rect::from_center_size(pos2(rect.left() + 1.5, cy), vec2(3.0, 20.0));
        p.rect_filled(bar, CornerRadius::same(2), alpha(G1, (255.0 * t) as u8));
    }
    app.logos.paint(p, Rect::from_center_size(pos2(rect.left() + 26.0, cy), vec2(22.0, 22.0)).shrink(1.0), casino, TEXT);
    p.text(pos2(rect.left() + 48.0, cy), Align2::LEFT_CENTER, casino.name(), theme::font_semi(13.5), theme::mix(TEXT2, TEXT, t.max(f32::from(u8::from(hot)))));
    let count = match n {
        0 => "No accounts yet".to_string(),
        1 => "1 account".to_string(),
        n => format!("{n} accounts"),
    };
    p.text(pos2(rect.right() - 38.0, cy), Align2::RIGHT_CENTER, count, theme::font(12.0), if n == 0 { DIM } else { MUTED });
    chevron(p, pos2(rect.right() - 18.0, cy), t, if hot { TEXT2 } else { MUTED });
    if resp.clicked() {
        *pick = Some(Pick::Toggle(casino));
    }

    if t > 0.0 {
        group_body(app, ui, casino, room, t, pick);
    }
    let bottom = ui.cursor().top();
    ui.painter().hline(rect.x_range(), bottom - 0.5, Stroke::new(1.0, LINE));
}

/// An open casino: search (with many accounts), its accounts in a scroll area, "Add account".
/// `t` is how far open it is, for the slide.
fn group_body(app: &mut App, ui: &mut Ui, casino: CasinoId, room: f32, t: f32, pick: &mut Option<Pick>) {
    let all: Vec<Account> = app.settings.accounts.iter().filter(|a| a.casino == casino).cloned().collect();
    let search = all.len() >= SEARCH_FROM;
    let q = app.account_search.trim().to_lowercase();
    let matches = |a: &Account| q.is_empty() || app.account_name(a).is_some_and(|u| u.to_lowercase().contains(&q)) || a.mirror.as_ref().is_some_and(|m| m.contains(&q));
    let shown: Vec<&Account> = all.iter().filter(|a| matches(a)).collect();
    let search_h = if search { SEARCH_H } else { 0.0 };
    // whole rows, at least two
    let fit = ((room - search_h) / ACCOUNT_H).floor().max(2.0);
    let list_h = if shown.is_empty() && search { ACCOUNT_H } else { shown.len() as f32 * ACCOUNT_H }.min(fit * ACCOUNT_H);
    let body_h = search_h + list_h + ACCOUNT_H;

    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, (body_h * t).round()), egui::Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::ZERO, alpha(theme::BG, 110));
    ui.painter().hline(rect.x_range(), rect.top(), Stroke::new(1.0, alpha(LINE, 160)));
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(Rect::from_min_size(rect.min, vec2(w, body_h))).layout(Layout::top_down(Align::Min)));
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    ui.spacing_mut().item_spacing.y = 0.0;

    if search {
        Frame::new().inner_margin(Margin { left: 46, right: 14, top: 8, bottom: 6 }).show(&mut ui, |ui| {
            let count = format!("{} of {}", shown.len(), all.len());
            widgets::text_box(ui, &mut app.account_search, "Search accounts", Some(&count), true, false);
        });
    }
    let scroll = egui::ScrollArea::vertical().id_salt(("accounts", casino.key())).max_height(list_h).auto_shrink([false, true]).show(&mut ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for a in &shown {
            account_row(app, ui, a, pick);
        }
        if shown.is_empty() && search {
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), ACCOUNT_H), egui::Sense::hover());
            ui.painter().text(pos2(r.left() + 78.0, r.center().y), Align2::LEFT_CENTER, "No account matches", theme::font_semi(12.0), MUTED);
        }
    });
    // more accounts below: the last row fades out
    let view = scroll.inner_rect;
    if scroll.state.offset.y + view.height() < scroll.content_size.y - 1.0 {
        for i in 0..12 {
            let y = view.bottom() - 24.0 + i as f32 * 2.0;
            let strip = Rect::from_min_max(pos2(view.left(), y), pos2(view.right() - 8.0, y + 2.0));
            ui.painter().rect_filled(strip, CornerRadius::ZERO, alpha(BG1, (i * 18) as u8));
        }
    }
    if scroll.content_size.y > view.height() + 1.0 {
        ui.painter().hline(view.x_range(), view.bottom(), Stroke::new(1.0, alpha(LINE, 160)));
    }

    let (r, resp) = ui.allocate_exact_size(vec2(w, ACCOUNT_H), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(r, CornerRadius::ZERO, alpha(egui::Color32::WHITE, 6));
    }
    let c = pos2(r.left() + 60.0, r.center().y);
    p.circle_stroke(c, 9.5, Stroke::new(1.2, alpha(G2, if resp.hovered() { 200 } else { 120 })));
    p.line_segment([c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)], Stroke::new(1.6, G2));
    p.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], Stroke::new(1.6, G2));
    p.text(pos2(r.left() + 78.0, r.center().y), Align2::LEFT_CENTER, "Add account", theme::font_semi(12.5), G2);
    if resp.clicked() {
        *pick = Some(Pick::Add(casino));
    }
}

/// One account of an open casino: initial, username, mirror (when it isn't the main site),
/// and on hover a bin that asks for a second press.
fn account_row(app: &App, ui: &mut Ui, a: &Account, pick: &mut Option<Pick>) {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, ACCOUNT_H), egui::Sense::click());
    let armed = app.remove_row.as_deref() == Some(a.id.as_str());
    let over = ui.rect_contains_pointer(rect);
    let cy = rect.center().y;

    // the bin, or once pressed, "Remove" in red
    let bin = if armed {
        Rect::from_min_max(pos2(rect.right() - 98.0, cy - 12.0), pos2(rect.right() - 34.0, cy + 12.0))
    } else {
        Rect::from_center_size(pos2(rect.right() - 46.0, cy), vec2(26.0, 26.0))
    };
    let bin_resp = (over || armed).then(|| {
        let hint = if armed { "Forgets this login on this PC (the casino account itself stays)" } else { "Remove account" };
        ui.interact(bin, ui.id().with(("remove", &a.id)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(hint)
    });

    let p = ui.painter();
    if over && !armed {
        p.rect_filled(rect, CornerRadius::ZERO, alpha(egui::Color32::WHITE, 6));
    }
    let name = app.account_name(a);
    let c = pos2(rect.left() + 60.0, cy);
    p.circle_filled(c, 10.0, theme::BG3);
    let initial = name.as_deref().and_then(|u| u.chars().next()).map(|ch| ch.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    p.text(c, Align2::CENTER_CENTER, initial, theme::font_bold(10.5), if name.is_some() { TEXT2 } else { DIM });
    let (label, color) = match &name {
        Some(u) => (u.clone(), TEXT),
        None => ("Not logged in yet".to_string(), MUTED),
    };
    p.text(pos2(rect.left() + 78.0, cy), Align2::LEFT_CENTER, label, theme::font_semi(12.5), color);
    let mirror = a.mirror.as_deref().filter(|m| Some(*m) != a.casino.default_mirror());
    if let (Some(m), false) = (mirror, armed) {
        p.text(pos2(rect.right() - 66.0, cy), Align2::RIGHT_CENTER, m, theme::font(11.5), MUTED);
    }
    match &bin_resp {
        Some(b) if armed => {
            p.rect(bin, CornerRadius::same(12), alpha(RED, if b.hovered() { 60 } else { 36 }), Stroke::new(1.0, alpha(RED, 150)), StrokeKind::Inside);
            p.text(bin.center(), Align2::CENTER_CENTER, "Remove", theme::font_semi(11.0), RED);
        }
        Some(b) => {
            if b.hovered() {
                p.rect_filled(bin, CornerRadius::same(theme::RADIUS_SM), alpha(RED, 30));
            }
            icon(p, Rect::from_center_size(bin.center(), vec2(13.0, 13.0)), "trash", if b.hovered() { RED } else { MUTED });
        }
        None => {}
    }
    chevron(p, pos2(rect.right() - 18.0, cy), 0.0, if over { TEXT2 } else { DIM });

    if bin_resp.as_ref().is_some_and(|b| b.clicked()) {
        *pick = Some(if armed { Pick::Remove(a.id.clone()) } else { Pick::Arm(a.id.clone()) });
    } else if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *pick = Some(Pick::Open(a.id.clone()));
    }
}

/// A chevron pointing right, turned to point down as `t` goes to 1.
fn chevron(p: &egui::Painter, c: egui::Pos2, t: f32, color: egui::Color32) {
    let (sin, cos) = (t * std::f32::consts::FRAC_PI_2).sin_cos();
    let turn = |x: f32, y: f32| c + vec2(x * cos - y * sin, x * sin + y * cos);
    p.add(egui::Shape::line(vec![turn(-2.0, -4.0), turn(2.0, 0.0), turn(-2.0, 4.0)], Stroke::new(1.6, color)));
}

/// The gear in the top-right corner of the welcome screen and its menu (version, Check for updates).
fn gear_menu(app: &mut App, ui: &mut Ui, full: Rect) {
    let slot = Rect::from_min_size(pos2(full.right() - 20.0 - 36.0, full.top() + 18.0), vec2(36.0, 36.0));
    let anchor = ui.scope_builder(egui::UiBuilder::new().max_rect(slot), |ui| {
        if widgets::round_icon_button(ui, "gear", app.welcome_menu_open, "Settings").clicked() {
            app.welcome_menu_open = !app.welcome_menu_open;
        }
    });
    let anchor = anchor.response.rect;
    if !app.welcome_menu_open {
        return;
    }
    let mut prefs = false;
    let mut updates = false;
    let area = egui::Area::new(egui::Id::new("welcome-menu"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(anchor.right(), anchor.bottom() + 8.0))
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(BG1)
                .stroke(Stroke::new(1.0, theme::LINE2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(6))
                .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(170) })
                .show(ui, |ui| {
                    let w = 210.0;
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    let (hr, _) = ui.allocate_exact_size(vec2(w, 40.0), egui::Sense::hover());
                    let p = ui.painter();
                    p.text(pos2(hr.left() + 12.0, hr.center().y), Align2::LEFT_CENTER, "FastBet", theme::font_bold(13.0), TEXT);
                    p.text(pos2(hr.right() - 12.0, hr.center().y), Align2::RIGHT_CENTER, format!("v{}", env!("CARGO_PKG_VERSION")), theme::font(11.5), MUTED);
                    widgets::hsep(ui);
                    if widgets::menu_item(ui, "gear", "Settings").clicked() {
                        prefs = true;
                    }
                    if widgets::menu_item(ui, "download", "Check for updates").clicked() {
                        updates = true;
                    }
                });
        });
    // a click anywhere else, or Esc, closes it
    let rect = area.response.rect;
    let outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !anchor.contains(p)));
    if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.welcome_menu_open = false;
    }
    if prefs {
        app.open_prefs();
    }
    if updates {
        app.check_updates();
    }
}

/// One account: avatar, name, Settings / Login, plus the settings card and the connection check.
pub fn account(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    let id = app.casino();
    let name = app.active_name();
    let busy = matches!(app.stage, Stage::Checking(_) | Stage::Login(_));

    // scrolls once the settings card (with the Cloudflare section open) outgrows a small window
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
        header(ui, full);
        ui.add_space(full.height() * 0.14);
        let (rect, _) = ui.allocate_exact_size(vec2(76.0, 76.0), egui::Sense::hover());
        let p = ui.painter();
        p.circle_filled(rect.center(), 38.0, BG2);
        p.circle_stroke(rect.center(), 38.0, Stroke::new(1.5, alpha(TEXT, 180)));
        app.logos.paint(p, Rect::from_center_size(rect.center(), vec2(38.0, 38.0)), id, TEXT);
        ui.add_space(8.0);
        ui.label(widgets::bold(name.clone().unwrap_or_else(|| "New account".into()), 16.0, TEXT));
        ui.label(widgets::semi(app.casino_site(), 12.0, MUTED));
        ui.add_space(22.0);

        let mut open_settings = false;
        let mut login = false;
        ui.horizontal(|ui| {
            let total = 110.0 + 100.0 + 10.0;
            ui.add_space((ui.available_width() - total) / 2.0);
            ui.spacing_mut().item_spacing.x = 10.0;
            if button(ui, "⚙  Settings", Btn::Ghost, !busy, Some(110.0)).clicked() {
                open_settings = true;
            }
            if button(ui, "Login  ›", Btn::Primary, !busy && app.settings.token().is_some(), Some(100.0)).clicked() {
                login = true;
            }
        });
        if app.settings.token().is_none() && !matches!(app.stage, Stage::Settings) {
            ui.add_space(8.0);
            let note = if id.window_only() { "Not logged in yet. Open Settings to log in.".to_string() } else { format!("No {} yet. Open Settings to add one.", id.key_label()) };
            ui.label(widgets::semi(note, 11.5, MUTED));
        }
        if open_settings {
            app.open_settings();
        }
        if login {
            app.begin_login();
        }

        if matches!(app.stage, Stage::Settings) {
            ui.add_space(18.0);
            settings_card(app, ui);
            ui.add_space(10.0);
            remove_button(app, ui);
        }

        ui.add_space(22.0);
        if pill_button(ui, "‹  Account selection", MUTED, false).clicked() && !busy {
            app.go_accounts();
        }
        ui.add_space(28.0);
    }));
    footer(ui, full);

    match &app.stage {
        Stage::Checking(since) => {
            let since = *since;
            check_modal(app, ui, since);
        }
        Stage::Login(_) => login_modal(app, ui),
        _ => {}
    }
}

fn settings_card(app: &mut App, ui: &mut Ui) {
    let id = app.casino();
    if id.window_only() {
        return window_settings_card(app, ui);
    }
    let w = 460.0_f32.min(ui.available_width() - 32.0);
    let mut save = false;
    let mut close = false;
    let mut window = false;
    Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(w);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    ui.label(widgets::bold("Account settings", 14.0, TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, "×", MUTED, true, "Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add(egui::Label::new(RichText::new(id.token_hint()).size(11.5).color(TEXT2)).wrap());
                ui.add_space(2.0);
                let mirror = (!app.mirror_input.is_empty()).then(|| app.mirror_input.clone());
                ui.horizontal(|ui| {
                    widgets::caption(ui, id.key_label());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(ui, format!("Get {} ↗", id.key_label()), G2, true).clicked() {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(id.api_key_url(mirror.as_deref())));
                        }
                    });
                });
                let masked = !app.key_input.is_empty() && !app.token_visible;
                let r = widgets::text_box(ui, &mut app.key_input, &format!("paste your {}", id.key_label()), None, true, masked);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                // a logged-in browser's cookies log in by themselves
                let typed = app.typed_clearance().ok().flatten();
                let browser_login = typed.as_ref().and_then(|c| c.session()).is_some();
                if browser_login && app.typed_login(typed.as_ref()) != Some(app.key_input.trim().to_string()) {
                    ui.label(widgets::semi("Not needed: the login from your browser's cookies is used", 11.0, G2));
                }
                let can_save = app.typed_login(typed.as_ref()).is_some();
                if !id.mirrors().is_empty() {
                    ui.add_space(2.0);
                    widgets::caption(ui, "Mirror");
                    mirror_picker(app, ui);
                }
                if id.behind_cloudflare() {
                    ui.add_space(4.0);
                    cloudflare_section(app, ui);
                    ui.add_space(2.0);
                }
                ui.horizontal(|ui| {
                    if button(ui, "Save", Btn::Primary, can_save, Some(90.0)).clicked() || enter {
                        save = true;
                    }
                    if pill_button(ui, if app.token_visible { "hide" } else { "show" }, MUTED, app.token_visible).clicked() {
                        app.token_visible = !app.token_visible;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if id.window_login() {
                            if pill_button(ui, "log in in a window instead", MUTED, false).clicked() {
                                window = true;
                            }
                        } else if let Some(m) = &mirror {
                            if pill_button(ui, "Casino website ↗", MUTED, false).clicked() {
                                ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://{m}/")));
                            }
                        }
                    });
                });
                if let Some(e) = &app.onboard_error {
                    ui.label(widgets::semi(e.clone(), 11.5, RED));
                }
            });
        });
    if save {
        app.save_key();
    }
    if close {
        app.close_settings();
    }
    if window {
        app.start_login();
    }
}

/// "Remove account", under the settings card: a second press confirms.
fn remove_button(app: &mut App, ui: &mut Ui) {
    let armed = app.remove_armed;
    let label = if armed { "Press again to remove this account" } else { "Remove account" };
    let hint = "Forgets this account's login on this PC (the casino account itself stays)";
    if pill_button(ui, label, RED, armed).on_hover_text(hint).clicked() {
        if armed {
            app.remove_account();
        } else {
            app.remove_armed = true;
        }
    }
}

/// Settings of a casino linked only through the login window (Shuffle), Antebot-style: the
/// mirror, and a button that opens it to log in.
fn window_settings_card(app: &mut App, ui: &mut Ui) {
    let id = app.casino();
    let w = 460.0_f32.min(ui.available_width() - 32.0);
    let (mut open, mut close, mut clear, mut manual_save) = (false, false, false, false);
    Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(18, 16))
        .show(ui, |ui| {
            ui.set_width(w);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    ui.label(widgets::bold("Account settings", 14.0, TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if widgets::icon_button(ui, "×", MUTED, true, "Close").clicked() {
                            close = true;
                        }
                    });
                });
                if !id.mirrors().is_empty() {
                    widgets::caption(ui, "Mirror");
                    mirror_picker(app, ui);
                }
                if id.manual_tokens() {
                    widgets::caption(ui, "Authentication method");
                    if let Some(i) = widgets::segmented(ui, &["Automatic", "Manual"], usize::from(app.login_manual), true) {
                        app.login_manual = i == 1;
                        app.onboard_error = None;
                    }
                }
                if app.login_manual && id.manual_tokens() {
                    manual_tokens(app, ui, &mut manual_save);
                    return;
                }
                ui.add(egui::Label::new(RichText::new(id.token_hint()).size(11.5).color(TEXT2)).wrap());
                ui.add_space(2.0);
                let (state, color) = match (app.settings.token().is_some(), app.active_name()) {
                    (true, Some(u)) => (format!("Saved login: {u}"), G2),
                    (true, None) => ("Login saved".to_string(), G2),
                    (false, _) => ("Not logged in yet".to_string(), MUTED),
                };
                ui.label(widgets::semi(state, 11.5, color));
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let site = if id.mirrors().is_empty() { id.name().to_ascii_lowercase() } else { picked_mirror(app) };
                    if button(ui, &format!("Open {site} to log in"), Btn::Primary, true, None).clicked() {
                        open = true;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let hint = format!("Logs the login window out of {}, to log in to another account", id.name());
                        if pill_button(ui, "Clear cookies", RED, false).on_hover_text(hint).clicked() {
                            clear = true;
                        }
                    });
                });
                if app.cookies_cleared {
                    ui.label(widgets::semi("Cookies cleared: the login window opens logged out", 11.5, G2));
                }
                if let Some(e) = &app.onboard_error {
                    ui.label(widgets::semi(e.clone(), 11.5, RED));
                }
            });
        });
    if open {
        app.open_login_window();
    }
    if close {
        app.close_settings();
    }
    if clear {
        app.clear_login_cookies();
    }
    if manual_save {
        app.save_manual_tokens();
    }
}

/// Manual login (Goated): the site's access token and refresh token, copied from the browser.
fn manual_tokens(app: &mut App, ui: &mut Ui, save: &mut bool) {
    let note = |ui: &mut Ui, text: &str| {
        ui.add(egui::Label::new(RichText::new(text).size(11.0).color(MUTED)).wrap());
    };
    widgets::caption(ui, "Access token");
    let masked = !app.token_visible && !app.manual_access.is_empty();
    widgets::text_box(ui, &mut app.manual_access, "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9…", None, true, masked);
    note(
        ui,
        "Open Goated in your normal browser, press F12 and go to the Console tab. Type localStorage.getItem('g3_token') and press Enter, then copy the value shown, without the quotes around it.",
    );
    widgets::caption(ui, "Refresh token");
    let masked = !app.token_visible && !app.manual_refresh.is_empty();
    widgets::text_box(ui, &mut app.manual_refresh, "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9…", None, true, masked);
    note(
        ui,
        "In the same F12 tools open the Application tab, then Cookies and the Goated site. Find the refresh_token cookie, click it and copy its value.",
    );
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        let ready = !app.manual_access.trim().is_empty() && !app.manual_refresh.trim().is_empty();
        if button(ui, "Authenticate with tokens", Btn::Primary, ready, None).clicked() {
            *save = true;
        }
        if pill_button(ui, if app.token_visible { "hide" } else { "show" }, MUTED, app.token_visible).clicked() {
            app.token_visible = !app.token_visible;
        }
    });
    if let Some(e) = &app.onboard_error {
        ui.label(widgets::semi(e.clone(), 11.5, RED));
    }
}

/// The casino's mirrors: a field showing the pick, opening into a searchable list (Stake has over a hundred).
fn mirror_picker(app: &mut App, ui: &mut Ui) {
    let id = app.casino();
    let current = if app.mirror_input.is_empty() { id.default_mirror().unwrap_or_default().to_string() } else { app.mirror_input.clone() };
    let w = ui.available_width();

    let (rect, resp) = ui.allocate_exact_size(vec2(w, widgets::FIELD_H), egui::Sense::click());
    let p = ui.painter();
    let edge = if app.mirror_open { alpha(G1, 170) } else if resp.hovered() { theme::LINE2 } else { LINE };
    p.rect(rect, CornerRadius::same(theme::RADIUS), theme::BG, Stroke::new(1.0, edge), StrokeKind::Inside);
    p.text(pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, &current, theme::font_semi(13.5), TEXT);
    let c = pos2(rect.right() - 16.0, rect.center().y);
    let dy = if app.mirror_open { -1.8 } else { 1.8 };
    p.add(egui::Shape::line(vec![c + vec2(-4.0, -dy), c + vec2(0.0, dy), c + vec2(4.0, -dy)], Stroke::new(1.6, MUTED)));
    let opened_now = resp.clicked() && !app.mirror_open;
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        app.mirror_open = !app.mirror_open;
        app.mirror_search.clear();
    }
    if !app.mirror_open {
        return;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.mirror_open = false;
        return;
    }

    let q = app.mirror_search.trim().to_ascii_lowercase();
    let list: Vec<&str> = id.mirrors().iter().copied().filter(|m| m.contains(q.as_str())).collect();
    let mut picked = None;
    // floats over the page like a dropdown, so the card below doesn't move
    let frame = Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, theme::LINE2))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::same(6))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: egui::Color32::from_black_alpha(150) });
    // opens upward when the window has no room for it below the field
    let screen = ui.ctx().content_rect();
    let (below, above) = (screen.bottom() - rect.bottom() - 12.0, rect.top() - screen.top() - 12.0);
    let up = below < 220.0 && above > below;
    let list_h = ((if up { above } else { below }) - widgets::FIELD_H - 20.0).clamp(84.0, 240.0);
    let area = egui::Area::new(ui.id().with("mirror-list")).order(egui::Order::Foreground);
    let area = if up { area.pivot(Align2::LEFT_BOTTOM).fixed_pos(rect.left_top() - vec2(0.0, 4.0)) } else { area.fixed_pos(rect.left_bottom() + vec2(0.0, 4.0)) };
    let area = area.show(ui.ctx(), |ui| {
            frame.show(ui, |ui| {
                ui.set_width(w - 12.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                let count = format!("{} of {}", list.len(), id.mirrors().len());
                let search = widgets::text_box(ui, &mut app.mirror_search, "Search mirrors", Some(&count), true, false);
                if opened_now || ui.memory(|m| m.focused().is_none()) {
                    search.request_focus();
                }
                egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for m in &list {
                        let (r, row) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), egui::Sense::click());
                        let on = *m == current;
                        let p = ui.painter();
                        if row.hovered() {
                            p.rect_filled(r, CornerRadius::same(5), alpha(egui::Color32::WHITE, 8));
                        }
                        p.text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, *m, theme::font_semi(12.5), if on { G2 } else { TEXT2 });
                        if on {
                            icon(p, egui::Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(12.0, 12.0)), "check", G2);
                        }
                        if row.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            picked = Some(m.to_string());
                        }
                    }
                    if list.is_empty() {
                        ui.add_space(6.0);
                        ui.label(widgets::semi("No mirror matches", 12.0, MUTED));
                        ui.add_space(6.0);
                    }
                });
            });
        });
    // a click anywhere else closes it
    let outside = ui.input(|i| i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|pos| !area.response.rect.contains(pos) && !rect.contains(pos)));
    if outside {
        app.mirror_open = false;
    }
    if let Some(m) = picked {
        app.mirror_input = m;
        app.mirror_search.clear();
        app.mirror_open = false;
    }
}

/// The mirror picked in the settings card.
fn picked_mirror(app: &App) -> String {
    let m = crate::casino::stake::normalize_mirror(&app.mirror_input);
    if m.is_empty() {
        app.casino().default_mirror().unwrap_or_default().to_string()
    } else {
        m
    }
}

/// "Log in with your browser": the User Agent and cookies of the user's own browser on the
/// picked mirror (same PC, same IP). The app then connects as that browser, and a logged-in
/// browser's `session` cookie logs the account in without an API token.
/// The settings card's Save stores them with the token and mirror.
fn cloudflare_section(app: &mut App, ui: &mut Ui) {
    let mirror = picked_mirror(app);
    let saved = app.settings.clearance();
    let w = ui.available_width();
    let mut remove = false;
    Frame::new().stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(10)).inner_margin(Margin::ZERO).show(ui, |ui| {
        let inner_w = w - 2.0;
        ui.set_width(inner_w);
        ui.spacing_mut().item_spacing.y = 0.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(inner_w, 40.0), egui::Sense::click());
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if resp.clicked() {
            app.cf_open = !app.cf_open;
        }
        let p = ui.painter();
        if resp.hovered() {
            let r = if app.cf_open { CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 } } else { CornerRadius::same(9) };
            p.rect_filled(rect, r, alpha(egui::Color32::WHITE, 6));
        }
        // chevron points up while open
        let c = pos2(rect.left() + 18.0, rect.center().y);
        let dy = if app.cf_open { -2.0 } else { 2.0 };
        let chevron = vec![c + vec2(-4.0, -dy), c + vec2(0.0, dy), c + vec2(4.0, -dy)];
        p.add(egui::Shape::line(chevron, Stroke::new(1.6, if resp.hovered() { TEXT2 } else { MUTED })));
        p.text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, "Log in with your browser", theme::font_semi(13.0), if app.cf_open { TEXT } else { TEXT2 });
        let (tag, tag_color) = match &saved {
            Some(s) if s.mirror == mirror && s.session().is_some() => ("saved · logged in", G2),
            Some(s) if s.mirror == mirror => ("saved", G2),
            Some(_) => ("other mirror", GOLD),
            None => ("optional", DIM),
        };
        p.text(pos2(rect.right() - 14.0, rect.center().y), Align2::RIGHT_CENTER, tag, theme::font_semi(11.0), tag_color);
        if !app.cf_open {
            return;
        }

        Frame::new().inner_margin(Margin { left: 14, right: 14, top: 0, bottom: 14 }).show(ui, |ui| {
            ui.set_width(inner_w - 28.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            let note = |ui: &mut Ui, text: String, color: egui::Color32| {
                ui.add(egui::Label::new(RichText::new(text).size(11.0).color(color)).wrap());
            };
            ui.add(
                egui::Label::new(
                    RichText::new(format!(
                        "For when {mirror} opens in a browser on this computer but the app can't get past Cloudflare. Copy that browser's \
                         User Agent and its cookies for {mirror} here: FastBet then connects as that browser. If you're logged in there, \
                         your login comes with the cookies and no API token is needed."
                    ))
                    .size(11.5)
                    .color(TEXT2),
                )
                .wrap(),
            );
            ip_row(app, ui, &mirror);

            ui.add_space(2.0);
            ui.horizontal(|ui| {
                widgets::caption(ui, "User Agent");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let get = pill_button(ui, "Get User Agent ↗", G2, true).on_hover_text("Opens Cloudflare's info page for this mirror in your browser");
                    if get.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(cloudflare::trace_url(&mirror)));
                    }
                });
            });
            widgets::text_box(ui, &mut app.cf_agent_input, "Mozilla/5.0 (Windows NT…", None, true, false);
            note(
                ui,
                format!("Open {mirror}/cdn-cgi/trace in the browser you take the cookies from (Get User Agent does that in your default browser) and copy everything after uag="),
                MUTED,
            );

            ui.add_space(2.0);
            widgets::caption(ui, "Cookies");
            widgets::text_box(ui, &mut app.cf_cookie_input, "Paste the cookie line", None, true, false);
            note(
                ui,
                format!(
                    "In that browser, on {mirror}: press F12 → Network, reload the page, click the first request and copy the value of \
                     cookie under Request Headers. Only the cf_clearance value works too."
                ),
                MUTED,
            );
            // what the pasted line holds
            if !app.cf_cookie_input.trim().is_empty() {
                let found = cloudflare::parse_cookies(&app.cf_cookie_input);
                let has = |name: &str| found.iter().any(|(k, _)| k == name);
                let mut parts = Vec::new();
                if has(cloudflare::SESSION_COOKIE) {
                    parts.push("your Stake login");
                }
                if has(cloudflare::COOKIE) {
                    parts.push("cf_clearance");
                }
                if has(cloudflare::BOT_COOKIE) {
                    parts.push("__cf_bm");
                }
                let (txt, color) = if parts.is_empty() {
                    ("No Stake login or Cloudflare cookie in that line".to_string(), GOLD)
                } else if !has(cloudflare::COOKIE) && !has(cloudflare::BOT_COOKIE) {
                    (format!("Found {}, but no Cloudflare cookie: the app passes Cloudflare by itself", parts.join(", ")), GOLD)
                } else {
                    (format!("Found {}", parts.join(" · ")), G2)
                };
                note(ui, txt, color);
            }

            let typed = (app.cf_agent_input.trim(), app.cf_cookie_input.trim());
            let changed = match &saved {
                Some(s) => typed != (s.user_agent.as_str(), s.cookie.as_str()),
                None => typed != ("", ""),
            };
            if changed || saved.is_some() {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    match &saved {
                        _ if changed => ui.label(widgets::semi("Press Save to use these values", 11.5, MUTED)),
                        Some(s) if s.mirror == mirror => ui.label(widgets::semi(format!("Saved for {}", s.mirror), 11.5, G2)),
                        Some(s) => ui.label(widgets::semi(format!("Saved for {}; Save again to use them on {mirror}", s.mirror), 11.5, GOLD)),
                        None => ui.label(""),
                    };
                    if saved.is_some() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill_button(ui, "Remove", RED, false).on_hover_text("Forget the saved cookies and User Agent").clicked() {
                                remove = true;
                            }
                        });
                    }
                });
            }
        });
    });
    if remove {
        app.remove_clearance();
    }
}

/// "Your IP address": the one Cloudflare sees for the mirror, which the cookie is tied to.
fn ip_row(app: &mut App, ui: &mut Ui, mirror: &str) {
    let ip = app.public_ip(mirror);
    Frame::new()
        .fill(theme::BG)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(theme::RADIUS))
        .inner_margin(Margin::symmetric(12, 7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                icon(ui.painter(), r, "globe", MUTED);
                ui.label(widgets::semi("Your IP address:", 12.5, TEXT2));
                match ip {
                    Some(Ok(ip)) => ui.label(widgets::bold(ip, 12.5, TEXT)),
                    Some(Err(e)) => ui.label(widgets::semi("couldn't check", 12.5, MUTED)).on_hover_text(e),
                    None => ui.label(widgets::semi("checking…", 12.5, MUTED)),
                };
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let compare = pill_button(ui, "Compare in browser ↗", MUTED, false).on_hover_text("Shows the IP your browser uses (the ip= line). It must be the same.");
                    if compare.clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(cloudflare::trace_url(mirror)));
                    }
                });
            });
        });
}

fn modal_frame(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) -> bool {
    let mut close = false;
    let full = ui.max_rect();
    ui.painter().rect_filled(full, CornerRadius::ZERO, alpha(theme::BG, 170));
    egui::Area::new(egui::Id::new(("modal", title)))
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(BG1)
                .stroke(Stroke::new(1.0, theme::LINE2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(0))
                .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(160) })
                .show(ui, |ui| {
                    ui.set_width(440.0);
                    Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(widgets::bold(title, 14.5, TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if widgets::icon_button(ui, "×", MUTED, true, "").clicked() {
                                    close = true;
                                }
                            });
                        });
                    });
                    ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
                    Frame::new().inner_margin(Margin::symmetric(18, 16)).show(ui, |ui| {
                        ui.set_width(440.0 - 36.0);
                        ui.spacing_mut().item_spacing.y = 12.0;
                        add(ui);
                    });
                });
        });
    close
}

fn step(ui: &mut Ui, state: u8, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        match state {
            1 => icon(ui.painter(), r, "check", GREEN),
            2 => icon(ui.painter(), r, "cross", RED),
            _ => icon(ui.painter(), r.shrink(2.0), "spin", MUTED),
        }
        ui.label(widgets::semi(text, 13.0, if state == 0 { TEXT } else { TEXT2 }));
    });
}

fn check_modal(app: &mut App, ui: &mut Ui, since: std::time::Instant) {
    ui.ctx().request_repaint_after(Duration::from_millis(50));
    let reach = match app.status {
        ConnStatus::Connecting => 0,
        ConnStatus::Offline => {
            if since.elapsed() > Duration::from_secs(8) {
                2
            } else {
                0
            }
        }
        _ => 1,
    };
    let cred = match app.status {
        ConnStatus::Ready => 1,
        ConnStatus::AuthFailed | ConnStatus::NoToken => 2,
        _ => 0,
    };
    // Cloudflare still hasn't let the app through: offer the manual cookie
    let cloudflare = app.casino().behind_cloudflare();
    let stuck = cloudflare && reach == 0 && since.elapsed() > Duration::from_secs(25);
    let mut open_settings = false;
    let mut set_cookie = false;
    let mut relogin = false;
    let window_only = app.casino().window_only();
    let close = modal_frame(ui, "Account connection check", |ui| {
        step(ui, reach, &format!("Reaching {}", app.casino_site()));
        // e.g. Stake's Cloudflare check, which can take a few seconds
        if reach == 0 && !app.status_detail.is_empty() {
            ui.label(widgets::semi(app.status_detail.clone(), 11.5, MUTED));
        }
        step(ui, if reach == 1 { cred } else { 0 }, "Checking account credentials");
        if cred == 2 || reach == 2 {
            ui.add_space(2.0);
            let msg = if app.status == ConnStatus::Offline && !app.status_detail.is_empty() { app.status_detail.clone() } else { app.status.long(&app.status_detail) };
            ui.add(egui::Label::new(widgets::semi(msg, 11.5, RED)).wrap());
            ui.horizontal(|ui| {
                if window_only && cred == 2 && button(ui, "Log in again", Btn::Primary, true, None).clicked() {
                    relogin = true;
                }
                if button(ui, "Open settings", Btn::Ghost, true, None).clicked() {
                    open_settings = true;
                }
                if cloudflare && reach == 2 && button(ui, "Log in with your browser", Btn::Ghost, true, None).clicked() {
                    set_cookie = true;
                }
            });
        } else if stuck {
            ui.add_space(2.0);
            ui.add(egui::Label::new(widgets::semi("Stuck on Cloudflare? If the casino website opens in your browser, copy that browser's User Agent and cookies into the app: it then connects as your browser.", 11.5, TEXT2)).wrap());
            ui.horizontal(|ui| {
                if button(ui, "Set cookie manually", Btn::Ghost, true, None).clicked() {
                    set_cookie = true;
                }
                if button(ui, "Casino website ↗", Btn::Ghost, true, None).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(format!("https://{}/", app.settings.mirror().unwrap_or_default())));
                }
            });
        }
    });
    if close {
        app.cancel_check();
    }
    if open_settings {
        app.cancel_check();
        app.open_settings();
    }
    if set_cookie {
        app.open_cookie_settings();
    }
    if relogin {
        app.cancel_check();
        app.open_settings();
        app.open_login_window();
    }
    let _ = StrokeKind::Inside;
}

fn login_modal(app: &mut App, ui: &mut Ui) {
    let mut cancel = false;
    let close = modal_frame(ui, "Log in", |ui| {
        step(ui, 0, &format!("Waiting for you to log in to {} in the window that opened", app.casino().name()));
        ui.label(widgets::semi("It closes by itself the moment you're in.", 11.5, MUTED));
        if button(ui, "Cancel", Btn::Ghost, true, None).clicked() {
            cancel = true;
        }
    });
    if close || cancel {
        app.cancel_login("Login cancelled.");
    }
    let _ = G1;
}
