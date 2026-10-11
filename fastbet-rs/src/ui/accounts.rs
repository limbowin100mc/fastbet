//! Account screens: pick an account, then log in (with the connection check).

use std::time::Duration;

use egui::{pos2, vec2, Align, Align2, CornerRadius, Frame, Layout, Margin, Rect, RichText, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, DIM, G1, G2, GOLD, GREEN, LINE, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, icon, list_row, pill_button, Btn};
use super::{App, Stage};
use crate::casino::{CasinoId, ConnStatus};
use crate::cloudflare;

fn header(ui: &mut Ui, full: Rect) {
    ui.add_space(full.height() * 0.11);
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 190.0) / 2.0);
        widgets::logo_mark(ui, 40.0);
        ui.add_space(8.0);
        widgets::wordmark(ui, 40.0);
    });
}

fn footer(ui: &Ui, full: Rect) {
    ui.painter().text(pos2(full.left() + 20.0, full.bottom() - 16.0), Align2::LEFT_CENTER, format!("v{}", env!("CARGO_PKG_VERSION")), theme::font(10.5), DIM);
}

/// "Pick your account": one row per casino.
pub fn accounts(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    ui.vertical_centered(|ui| {
        header(ui, full);
        ui.add_space(full.height() * 0.12);
        ui.label(widgets::semi("Pick your account", 13.5, TEXT2));
        ui.add_space(22.0);
        let w = 500.0_f32.min(full.width() - 40.0);
        let mut picked = None;
        Frame::new()
            .fill(BG1)
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::same(0))
            .show(ui, |ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 0.0;
                for id in CasinoId::ALL {
                    let name = app.username_for(*id);
                    let value = name.clone().unwrap_or_else(|| "Add account".to_string());
                    let logos = &mut app.logos;
                    let mark = |p: &egui::Painter, r: Rect| logos.paint(p, r.shrink(1.0), *id, TEXT);
                    if list_row(ui, mark, id.name(), &value, true).clicked() {
                        picked = Some(*id);
                    }
                }
                list_row(ui, |p, r| widgets::letter_mark(p, r, "?", DIM), "More casinos", "Coming soon", false);
            });
        if let Some(id) = picked {
            app.open_account(id);
        }
    });
    footer(ui, full);
    gear_menu(app, ui, full);
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
    if updates {
        app.check_updates();
    }
}

/// One account: avatar, name, Settings / Login, plus the settings card and the connection check.
pub fn account(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    let id = app.casino();
    let name = app.username_for(id);
    let busy = matches!(app.stage, Stage::Checking(_) | Stage::Login(_));

    // scrolls once the settings card (with the Cloudflare section open) outgrows a small window
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
        header(ui, full);
        ui.add_space(full.height() * 0.1);
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
            ui.label(widgets::semi(format!("No {} yet. Open Settings to add one.", id.key_label()), 11.5, MUTED));
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
                    if button(ui, "Save", Btn::Primary, !app.key_input.trim().is_empty(), Some(90.0)).clicked() || enter {
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

/// For a Cloudflare check that never passes in the app: the User Agent and
/// `cf_clearance` cookie from the user's own browser (same PC, same IP).
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
        p.text(pos2(rect.left() + 32.0, rect.center().y), Align2::LEFT_CENTER, "Set Cloudflare cookie manually", theme::font_semi(13.0), if app.cf_open { TEXT } else { TEXT2 });
        let (tag, tag_color) = match &saved {
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
                        "Use this when the Cloudflare check never passes in the app, but {mirror} opens fine in a browser on this computer. \
                         Copy that browser's User Agent and its cf_clearance cookie here. They only work from the same IP address and on the same mirror."
                    ))
                    .size(11.5)
                    .color(TEXT2),
                )
                .wrap(),
            );
            let engine = cloudflare::edge_engine();
            if let Some(v) = engine {
                note(ui, format!("Best taken from Microsoft Edge: FastBet runs on Edge's engine (version {v}), so Cloudflare sees the same browser."), TEXT2);
            }
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
                format!("Get User Agent opens {mirror}/cdn-cgi/trace in your default browser. Open it in the browser you take the cookie from, and copy everything after uag="),
                MUTED,
            );
            let other = cloudflare::browser_of(&app.cf_agent_input).filter(|b| engine.is_some_and(|v| *b != ("Edge", v)));
            if let (Some((name, major)), Some(v)) = (other, engine) {
                note(ui, format!("That's {name} {major}, FastBet is Edge {v}. If Cloudflare refuses the cookie, copy both values from Microsoft Edge."), GOLD);
            }

            ui.add_space(2.0);
            widgets::caption(ui, "Cloudflare clearance cookie");
            widgets::text_box(ui, &mut app.cf_cookie_input, "Paste the cf_clearance value", None, true, false);
            note(ui, format!("In that browser, on {mirror}: press F12 → Application → Cookies → https://{mirror} and copy the value of cf_clearance."), MUTED);

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
                        Some(s) => ui.label(widgets::semi(format!("Saved for {}, not used on {mirror}", s.mirror), 11.5, GOLD)),
                        None => ui.label(""),
                    };
                    if saved.is_some() {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill_button(ui, "Remove", RED, false).on_hover_text("Forget the saved cookie and User Agent").clicked() {
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
                if button(ui, "Open settings", Btn::Ghost, true, None).clicked() {
                    open_settings = true;
                }
                if cloudflare && reach == 2 && button(ui, "Set Cloudflare cookie", Btn::Ghost, true, None).clicked() {
                    set_cookie = true;
                }
            });
        } else if stuck {
            ui.add_space(2.0);
            ui.add(egui::Label::new(widgets::semi("Stuck on Cloudflare? If the casino website opens in your browser, you can copy its Cloudflare cookie into the app.", 11.5, TEXT2)).wrap());
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
