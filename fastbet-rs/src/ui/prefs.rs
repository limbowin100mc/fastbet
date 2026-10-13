//! The Settings window (account menu → Settings): app-wide options, one category at a time
//! from the list on the left.

use egui::{pos2, vec2, Align, Align2, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui};

use super::theme::{self, alpha, BG1, DIM, G1, G2, GOLD, LINE, MUTED, TEXT, TEXT2};
use super::widgets::{self, Btn};
use super::App;
use crate::settings::{clean_seed_name, SeedChars, RANDOM_SEED_DEFAULT, RANDOM_SEED_LEN, SEED_NAME_MAX};

const WIDTH: f32 = 640.0;
const NAV_W: f32 = 180.0;
const BODY_H: f32 = 500.0;

/// A category in the list on the left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Seed,
}

impl Tab {
    const ALL: [Tab; 1] = [Tab::Seed];

    fn name(self) -> &'static str {
        match self {
            Tab::Seed => "Seed",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Seed => "seed",
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.prefs_open {
        app.seed_name_editing = false;
        return;
    }
    let mut close = false;
    let frame = Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, theme::LINE2))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(0))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 30, spread: 0, color: egui::Color32::from_black_alpha(160) });
    let modal = egui::Modal::new(egui::Id::new("settings-dialog")).frame(frame).backdrop_color(alpha(theme::BG, 170)).show(ctx, |ui| {
        ui.set_width(WIDTH);
        ui.spacing_mut().item_spacing.y = 0.0;
        Frame::new().inner_margin(Margin::symmetric(18, 14)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(widgets::bold("Settings", 14.5, TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::icon_button(ui, "×", MUTED, true, "").clicked() {
                        close = true;
                    }
                });
            });
        });
        ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, LINE));
        let (body, _) = ui.allocate_exact_size(vec2(WIDTH, BODY_H), Sense::hover());
        let nav = Rect::from_min_size(body.min, vec2(NAV_W, BODY_H));
        ui.painter().vline(nav.right(), body.y_range(), Stroke::new(1.0, LINE));
        ui.scope_builder(egui::UiBuilder::new().max_rect(nav.shrink(8.0)), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for tab in Tab::ALL {
                if nav_item(ui, tab.icon(), tab.name(), app.prefs_tab == tab).clicked() {
                    app.prefs_tab = tab;
                }
            }
        });
        let page = Rect::from_min_max(pos2(nav.right() + 1.0, body.top()), body.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(page), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                Frame::new().inner_margin(Margin::symmetric(22, 18)).show(ui, |ui| {
                    let w = page.width() - 44.0;
                    ui.set_width(w);
                    match app.prefs_tab {
                        Tab::Seed => seed(app, ui, w),
                    }
                });
            });
        });
    });
    if close || modal.should_close() {
        app.prefs_open = false;
        app.seed_name_editing = false;
    }
}

/// One category on the left: icon and name, green while it's the open one.
fn nav_item(ui: &mut Ui, icon_name: &str, text: &str, on: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
    let p = ui.painter();
    if on {
        p.rect_filled(rect, CornerRadius::same(8), alpha(G1, 20));
        let bar = Rect::from_min_max(pos2(rect.left(), rect.top() + 9.0), pos2(rect.left() + 3.0, rect.bottom() - 9.0));
        p.rect_filled(bar, CornerRadius::same(2), G1);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(8), alpha(egui::Color32::WHITE, 8));
    }
    let color = if on { TEXT } else if resp.hovered() { TEXT2 } else { MUTED };
    widgets::icon(p, Rect::from_center_size(pos2(rect.left() + 20.0, rect.center().y), vec2(14.0, 14.0)), icon_name, if on { G1 } else { color });
    p.text(pos2(rect.left() + 38.0, rect.center().y), Align2::LEFT_CENTER, text, theme::font_semi(12.5), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn heading(ui: &mut Ui, title: &str, sub: &str) {
    ui.label(widgets::bold(title, 17.0, TEXT));
    ui.add_space(4.0);
    ui.label(RichText::new(sub).size(12.0).color(MUTED));
    ui.add_space(16.0);
}

fn seed(app: &mut App, ui: &mut Ui, w: f32) {
    heading(ui, "Seed", "Define the client seed, or how random seeds are made.");
    let own = app.settings.client_seed().is_some();
    let in_use = |on: bool| if on { widgets::semi("In use", 11.5, G2) } else { widgets::semi("Not used", 11.5, DIM) };
    let help = "Every seed change (Change seed now, Auto seed changes, Ctrl+R) sends this name as your client seed instead of a                 random one. The server seed still changes each time, so the results do too. When an account's client seed isn't                 your name, FastBet changes the seed once to put it on. Leave the name blank to use the random format.";
    widgets::card(ui, w, "Client seed", Some(help), Some(in_use(own)), |ui| {
        let mut en = app.settings.own_seed;
        if widgets::toggle(ui, "Use my own seed name", &mut en, true) {
            app.settings.own_seed = en;
            app.mark_dirty();
        }
        if !en {
            app.seed_name_editing = false;
            return;
        }
        let name = &mut app.settings.seed_name;
        let count = format!("{}/{SEED_NAME_MAX}", name.chars().count());
        let r = labelled(ui, "Seed name", |ui| widgets::text_box(ui, name, "Leave blank to use the random format below", Some(&count), true, false));
        if r.changed() {
            *name = clean_seed_name(name);
            app.mark_dirty();
        }
        app.seed_name_editing = r.has_focus();
        seed_status(app, ui);
    });
    ui.add_space(12.0);
    let help = "How the random client seeds are made when you don't use your own name: how many characters, and which ones.";
    widgets::card(ui, w, "Random seed format", Some(help), Some(in_use(!own)), |ui| random_format(app, ui));
}

/// A caption over a field.
fn labelled(ui: &mut Ui, caption: &str, add: impl FnOnce(&mut Ui) -> egui::Response) -> egui::Response {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        widgets::caption(ui, caption);
        add(ui)
    })
    .inner
}

/// Length, characters and a sample of the random client seeds, Antebot's way.
fn random_format(app: &mut App, ui: &mut Ui) {
    let (lo, hi) = (*RANDOM_SEED_LEN.start(), *RANDOM_SEED_LEN.end());
    let default = app.settings.seed_chars == SeedChars::default() && app.settings.random_seed_len() == RANDOM_SEED_DEFAULT;
    let mut reset = false;
    widgets::row(ui, widgets::FIELD_H, |ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        widgets::caption(ui, "Length");
        let r = ui
            .scope(|ui| {
                ui.set_min_width(72.0);
                ui.set_max_width(72.0);
                widgets::text_box(ui, &mut app.settings.seed_length, &RANDOM_SEED_DEFAULT.to_string(), None, true, false)
            })
            .inner;
        if r.changed() {
            app.settings.seed_length = app.settings.seed_length.chars().filter(char::is_ascii_digit).take(3).collect();
            app.mark_dirty();
        }
        ui.label(RichText::new(format!("({lo}–{hi})")).size(11.5).color(DIM));
        if !default {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let link = ui.add(egui::Label::new(widgets::semi("Reset to default", 12.0, G1)).sense(Sense::click()));
                reset = link.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("24 characters, hex").clicked();
            });
        }
    });
    if reset {
        app.settings.seed_length = RANDOM_SEED_DEFAULT.to_string();
        app.settings.seed_chars = SeedChars::default();
        app.mark_dirty();
    }

    // hex on its own, or any mix of the other three; never none
    let mut c = app.settings.seed_chars;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 20.0;
        let mut hex = c.hex;
        if widgets::checkbox(ui, "Hex (0–9, a–f)", &mut hex, true) {
            c = SeedChars { hex, upper: !hex, lower: !hex, digits: !hex };
        }
        let mut upper = c.upper && !c.hex;
        if widgets::checkbox(ui, "A–Z", &mut upper, true) {
            c = SeedChars { hex: false, upper, ..c };
        }
        let mut lower = c.lower && !c.hex;
        if widgets::checkbox(ui, "a–z", &mut lower, true) {
            c = SeedChars { hex: false, lower, ..c };
        }
        let mut digits = c.digits && !c.hex;
        if widgets::checkbox(ui, "0–9", &mut digits, true) {
            c = SeedChars { hex: false, digits, ..c };
        }
    });
    if !(c.hex || c.upper || c.lower || c.digits) {
        c = SeedChars::default();
    }
    if c != app.settings.seed_chars {
        app.settings.seed_chars = c;
        app.mark_dirty();
    }

    let len = app.settings.random_seed_len();
    let typed_ok = app.settings.seed_length.trim().parse::<usize>().is_ok_and(|n| RANDOM_SEED_LEN.contains(&n));
    let key = (len, c.alphabet());
    if app.seed_preview.as_ref().is_none_or(|(k, _)| *k != key) {
        let sample = crate::engine::random_seed(key.0, &key.1);
        app.seed_preview = Some((key, sample));
    }
    let sample = app.seed_preview.as_ref().map(|(_, s)| s.as_str()).unwrap_or_default();
    if typed_ok {
        widgets::readout(ui, format!("Preview  {sample}"), TEXT2, G1);
    } else {
        widgets::readout(ui, format!("From {lo} to {hi} characters; using {len}.  Preview  {sample}"), TEXT2, GOLD);
    }
}

/// Whether the account is on the name yet, under the field.
fn seed_status(app: &mut App, ui: &mut Ui) {
    let Some(want) = app.settings.client_seed() else {
        widgets::readout(ui, "Blank, so seeds are random (format below). Names take letters, digits, - and _.", TEXT2, DIM);
        return;
    };
    let current = app.provable.client_seed.clone();
    if app.seed_name_editing {
        widgets::readout(ui, "Used once you're done typing.", TEXT2, DIM);
    } else if !app.ready() {
        widgets::readout(ui, "Used from the next seed change.", TEXT2, DIM);
    } else if app.seed.busy {
        widgets::readout(ui, "Changing the seed…", TEXT2, G1);
    } else if current.as_deref() == Some(want.as_str()) {
        let who = app.active_name().map(|n| format!(" on {n}")).unwrap_or_default();
        widgets::readout(ui, format!("In use{who}."), TEXT2, G1);
    } else {
        let msg = match current {
            Some(c) if c.chars().count() > 14 => format!("This account is still on {}…", c.chars().take(12).collect::<String>()),
            Some(c) => format!("This account is still on {c}."),
            None => "Used from the next seed change.".to_string(),
        };
        widgets::readout(ui, msg, TEXT2, GOLD);
        if widgets::button(ui, "Change seed now", Btn::Ghost, true, Some(ui.available_width())).clicked() {
            app.rotate_now();
        }
    }
}
