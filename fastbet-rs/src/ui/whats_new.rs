//! "What's new": once after an update, the release notes of every version since the one that last
//! showed them, from the website (`/api/app/notes`).

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use egui::{vec2, Align, CornerRadius, Frame, Layout, Margin, RichText, Stroke};
use serde::Deserialize;

use super::theme::{self, alpha, BG1, DIM, G1, LINE, MUTED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::App;

const WIDTH: f32 = 460.0;

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub version: String,
    pub title: String,
    #[serde(default)]
    pub lines: Vec<Line>,
}

/// One line of the notes: "item" (a bullet), "heading" or "text".
#[derive(Clone, Debug, Deserialize)]
pub struct Line {
    pub kind: String,
    pub text: String,
}

#[derive(Deserialize)]
struct Reply {
    releases: Vec<Release>,
}

#[derive(Default)]
pub struct WhatsNew {
    rx: Option<mpsc::Receiver<Result<Vec<Release>, String>>>,
    releases: Vec<Release>,
    pub open: bool,
    /// The version the notes are for; once they show, the settings remember it (not for the dev hook).
    pub version: String,
    remember: bool,
}

impl WhatsNew {
    /// Asks the website for the notes after `from` up to `to` (`from` empty: just `to`'s).
    pub fn fetch(from: &str, to: &str, remember: bool, repaint: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (tx, rx) = mpsc::channel();
        let url = crate::member::page(&format!("/api/app/notes?from={from}&to={to}"));
        std::thread::spawn(move || {
            let res = reqwest::blocking::Client::builder()
                .user_agent(format!("FastBet/{}", env!("CARGO_PKG_VERSION")))
                .timeout(Duration::from_secs(20))
                .build()
                .and_then(|c| c.get(url).send())
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.json::<Reply>())
                .map(|r| r.releases)
                .map_err(|e| e.to_string());
            let _ = tx.send(res);
            repaint();
        });
        Self { rx: Some(rx), version: to.to_string(), remember, ..Default::default() }
    }

    /// Every frame: true the moment the notes arrive and are to be remembered as seen. Without
    /// notes for this version yet (or the website out of reach) nothing opens, and the next
    /// start asks again.
    pub fn poll(&mut self) -> bool {
        let Some(res) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) else { return false };
        self.rx = None;
        match res {
            Ok(list) if list.iter().any(|r| r.version == self.version) => {
                self.releases = list;
                self.open = true;
                self.remember
            }
            Ok(_) => false,
            Err(e) => {
                tracing::warn!("What's new: {e}");
                false
            }
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.whats_new.open {
        return;
    }
    let mut close = false;
    let frame = Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, theme::LINE2))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(0))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(160) });
    let max_h = (ctx.content_rect().height() * 0.62).max(220.0);
    let releases = app.whats_new.releases.clone();
    let modal = egui::Modal::new(egui::Id::new("whats-new")).frame(frame).backdrop_color(alpha(theme::BG, 170)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(widgets::bold("What's new", 14.5, TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, "×", MUTED, true, "").clicked() {
                        close = true;
                    }
                });
            });
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
        egui::ScrollArea::vertical().max_height(max_h).auto_shrink([false, true]).show(ui, |ui| {
            Frame::new().inner_margin(Margin::symmetric(18, 16)).show(ui, |ui| {
                ui.set_width(WIDTH - 36.0);
                for (i, r) in releases.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(14.0);
                    }
                    release(ui, r);
                }
            });
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
        Frame::new().inner_margin(Margin::symmetric(18, 12)).show(ui, |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Got it", Btn::Primary, true, Some(110.0)).clicked() {
                    close = true;
                }
            });
        });
    });
    if close || modal.should_close() {
        app.whats_new.open = false;
    }
}

/// "v3.22.0  Tabs", then its notes.
fn release(ui: &mut egui::Ui, r: &Release) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(RichText::new(format!("v{}", r.version)).font(theme::font_bold(12.0)).color(G1));
        ui.label(widgets::semi(&r.title, 13.5, TEXT));
    });
    ui.add_space(6.0);
    ui.spacing_mut().item_spacing.y = 5.0;
    for l in &r.lines {
        match l.kind.as_str() {
            "item" => {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let (dot, _) = ui.allocate_exact_size(vec2(16.0, 17.0), egui::Sense::hover());
                    ui.painter().circle_filled(egui::pos2(dot.left() + 5.0, dot.center().y), 2.0, DIM);
                    ui.add(egui::Label::new(RichText::new(&l.text).size(12.5).color(TEXT2)).wrap());
                });
            }
            "heading" => {
                ui.add_space(4.0);
                ui.label(widgets::semi(&l.text, 12.5, TEXT));
            }
            _ => {
                ui.add(egui::Label::new(RichText::new(&l.text).size(12.5).color(MUTED)).wrap());
            }
        }
    }
}
