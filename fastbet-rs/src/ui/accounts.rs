//! Account screens: pick an account, then log in (with the connection check).

use std::time::Duration;

use egui::{pos2, vec2, Align, Align2, CornerRadius, Frame, Layout, Margin, Rect, RichText, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, DIM, G1, G2, GREEN, LINE, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, icon, list_row, pill_button, Btn};
use super::{App, Stage};
use crate::casino::{CasinoId, ConnStatus};

fn header(ui: &mut Ui, full: Rect) {
    ui.add_space(full.height() * 0.11);
    ui.horizontal(|ui| {
        ui.add_space((ui.available_width() - 190.0) / 2.0);
        widgets::logo_mark(ui, 40.0);
        ui.add_space(8.0);
        widgets::wordmark(ui, 40.0);
    });
}

fn footer(app: &App, ui: &Ui, full: Rect) {
    let p = ui.painter();
    p.text(pos2(full.left() + 20.0, full.bottom() - 16.0), Align2::LEFT_CENTER, format!("v{}", env!("CARGO_PKG_VERSION")), theme::font(10.5), DIM);
    let (c, txt) = match app.status {
        ConnStatus::Ready => (GREEN, "connected"),
        ConnStatus::Offline => (RED, "offline"),
        _ => (MUTED, "idle"),
    };
    p.circle_filled(pos2(full.right() - 70.0, full.bottom() - 16.0), 3.0, c);
    p.text(pos2(full.right() - 62.0, full.bottom() - 16.0), Align2::LEFT_CENTER, txt, theme::font(10.5), DIM);
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
                    let (mark, color) = casino_mark(*id);
                    if list_row(ui, mark, color, id.name(), &value, true).clicked() {
                        picked = Some(*id);
                    }
                }
                list_row(ui, "?", DIM, "More casinos", "Coming soon", false);
            });
        if let Some(id) = picked {
            app.open_account(id);
        }
    });
    footer(app, ui, full);
}

pub fn casino_mark(id: CasinoId) -> (&'static str, egui::Color32) {
    match id {
        CasinoId::ChipsGg => ("C", G2),
    }
}

/// One account: avatar, name, Settings / Login, plus the settings card and the connection check.
pub fn account(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    let id = app.casino();
    let name = app.username_for(id);
    let busy = matches!(app.stage, Stage::Checking(_) | Stage::Login(_));

    ui.vertical_centered(|ui| {
        header(ui, full);
        ui.add_space(full.height() * 0.1);
        let (rect, _) = ui.allocate_exact_size(vec2(76.0, 76.0), egui::Sense::hover());
        let p = ui.painter();
        p.circle_filled(rect.center(), 38.0, BG2);
        p.circle_stroke(rect.center(), 38.0, Stroke::new(1.5, alpha(TEXT, 180)));
        let initials: String = name.as_deref().unwrap_or("?").chars().take(2).collect::<String>().to_uppercase();
        p.text(rect.center(), Align2::CENTER_CENTER, initials, theme::font_bold(26.0), TEXT);
        ui.add_space(8.0);
        ui.label(widgets::bold(name.clone().unwrap_or_else(|| "New account".into()), 16.0, TEXT));
        ui.label(widgets::semi(id.name(), 12.0, MUTED));
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
            ui.label(widgets::semi("No API key yet. Open Settings to add one.", 11.5, MUTED));
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
    });
    footer(app, ui, full);

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
                if pill_button(ui, "open API Keys page ↗", G2, true).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(id.api_key_url()));
                }
                ui.add_space(2.0);
                widgets::caption(ui, "API key");
                let masked = !app.key_input.is_empty() && !app.token_visible;
                let r = widgets::text_box(ui, &mut app.key_input, "paste your API key", None, true, masked);
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if button(ui, "Save", Btn::Primary, !app.key_input.trim().is_empty(), Some(90.0)).clicked() || enter {
                        save = true;
                    }
                    if pill_button(ui, if app.token_visible { "hide" } else { "show" }, MUTED, app.token_visible).clicked() {
                        app.token_visible = !app.token_visible;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill_button(ui, "log in in a window instead", MUTED, false).clicked() {
                            window = true;
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
    let mut open_settings = false;
    let close = modal_frame(ui, "Account connection check", |ui| {
        step(ui, reach, &format!("Reaching {}", app.casino().name()));
        step(ui, if reach == 1 { cred } else { 0 }, "Checking account credentials");
        if cred == 2 || reach == 2 {
            ui.add_space(2.0);
            ui.label(widgets::semi(app.status.long(&app.status_detail), 11.5, RED));
            ui.horizontal(|ui| {
                if button(ui, "Open settings", Btn::Ghost, true, None).clicked() {
                    open_settings = true;
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
