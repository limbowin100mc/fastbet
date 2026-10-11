//! The "Check for updates" dialog: result of the check, download progress,
//! and the hand-off to the installer.

use std::time::{Duration, Instant};

use egui::{vec2, Align, CornerRadius, Frame, Layout, Margin, Rect, RichText, Stroke, Ui};

use super::theme::{self, alpha, BG1, G1, GREEN, LINE, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, icon, pill_button, Btn};
use super::App;
use crate::update::{current, State, RELEASES_PAGE};

const WIDTH: f32 = 420.0;

enum Act {
    Check,
    Install,
    Cancel,
    Open(String),
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.update_open {
        return;
    }
    let state = app.updater.state();
    // the installer is up: give the message a moment, then close so it can replace the app
    if let State::Installing { .. } = state {
        let at = *app.update_close_at.get_or_insert_with(Instant::now);
        if at.elapsed() > Duration::from_millis(1500) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    if matches!(state, State::Checking | State::Downloading { .. }) {
        ctx.request_repaint_after(Duration::from_millis(50));
    }
    let betting = app.auto.running || app.slot.running;

    let mut act = None;
    let mut close = false;
    let frame = Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, theme::LINE2))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(0))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(160) });
    let modal = egui::Modal::new(egui::Id::new("update-dialog")).frame(frame).backdrop_color(alpha(theme::BG, 170)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(widgets::bold("Updates", 14.5, TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, "×", MUTED, true, "").clicked() {
                        close = true;
                    }
                });
            });
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
        Frame::new().inner_margin(Margin::symmetric(18, 16)).show(ui, |ui| {
            ui.set_width(WIDTH - 36.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            body(ui, &state, betting, &mut act);
        });
    });
    if close || modal.should_close() {
        match state {
            // nothing to stop any more; the app is about to close
            State::Installing { .. } => {}
            State::Downloading { .. } => {
                app.updater.cancel();
                app.update_open = false;
            }
            _ => {
                app.updater.dismiss();
                app.update_open = false;
            }
        }
    }
    match act {
        Some(Act::Check) => app.updater.check(),
        Some(Act::Install) => app.updater.install(),
        Some(Act::Cancel) => app.updater.cancel(),
        Some(Act::Open(url)) => ctx.open_url(egui::OpenUrl::new_tab(url)),
        None => {}
    }
}

fn body(ui: &mut Ui, state: &State, betting: bool, act: &mut Option<Act>) {
    match state {
        State::Idle | State::Checking => {
            headline(ui, "spin", MUTED, "Checking for updates…");
        }
        State::UpToDate => {
            headline(ui, "check", GREEN, "You're up to date");
            note(ui, &format!("FastBet v{} is the latest version.", current()));
        }
        State::Available { version, page, asset, in_app } => {
            headline(ui, "download", G1, &format!("FastBet v{version} is available"));
            note(ui, &format!("You have v{}.", current()));
            if *in_app && betting {
                ui.label(widgets::semi("Stop betting first: installing closes FastBet.", 11.5, theme::GOLD));
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                match asset {
                    Some(_) if *in_app => {
                        if button(ui, "Download & install", Btn::Primary, !betting, Some(170.0)).clicked() {
                            *act = Some(Act::Install);
                        }
                    }
                    Some(a) => {
                        if button(ui, "Download .deb", Btn::Primary, true, Some(150.0)).clicked() {
                            *act = Some(Act::Open(a.browser_download_url.clone()));
                        }
                    }
                    None => {}
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Release page ↗", MUTED, false).clicked() {
                        *act = Some(Act::Open(page.clone()));
                    }
                });
            });
            let how = match asset {
                Some(_) if *in_app && cfg!(windows) => "The installer opens and FastBet closes so it can be updated.",
                Some(_) if *in_app => "FastBet restarts on the new version.",
                Some(_) => "Open the downloaded package to install it.",
                None => "There is no download for this system; see the release page.",
            };
            note(ui, how);
        }
        State::Downloading { version, done, total } => {
            headline(ui, "download", G1, &format!("Downloading v{version}…"));
            progress(ui, if *total > 0 { *done as f32 / *total as f32 } else { 0.0 });
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} / {}", mb(*done), mb(*total))).size(11.5).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Cancel", MUTED, false).clicked() {
                        *act = Some(Act::Cancel);
                    }
                });
            });
        }
        State::Installing { version } => {
            headline(ui, "check", GREEN, &format!("Starting v{version}…"));
            note(ui, "FastBet closes in a moment.");
        }
        State::Failed(e) => {
            headline(ui, "cross", RED, "Couldn't update");
            ui.add(egui::Label::new(RichText::new(e).size(11.5).color(theme::mix(RED, TEXT2, 0.4))).wrap());
            ui.horizontal(|ui| {
                if button(ui, "Try again", Btn::Ghost, true, Some(100.0)).clicked() {
                    *act = Some(Act::Check);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if pill_button(ui, "Release page ↗", MUTED, false).clicked() {
                        *act = Some(Act::Open(RELEASES_PAGE.into()));
                    }
                });
            });
        }
    }
}

fn headline(ui: &mut Ui, icon_name: &str, color: egui::Color32, text: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
        let r = if icon_name == "check" || icon_name == "cross" { r } else { r.shrink(2.0) };
        icon(ui.painter(), r, icon_name, color);
        ui.label(widgets::semi(text, 13.5, TEXT));
    });
}

fn note(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(11.5).color(MUTED)).wrap());
}

fn progress(ui: &mut Ui, frac: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(4), theme::BG);
    let w = (r.width() * frac.clamp(0.0, 1.0)).max(8.0);
    p.rect_filled(Rect::from_min_size(r.min, vec2(w, r.height())), CornerRadius::same(4), G1);
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}
