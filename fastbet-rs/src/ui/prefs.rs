//! The Settings window (account menu → Settings): app-wide options, one category at a time
//! from the list on the left.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use egui::{pos2, vec2, Align, Align2, CornerRadius, Frame, Layout, Margin, Rect, RichText, Sense, Stroke, Ui};

use super::theme::{self, alpha, BG1, DIM, G1, G2, GOLD, LINE, MUTED, TEXT, TEXT2};
use super::widgets::{self, Btn};
use super::{sound, App};
use crate::settings::{clean_seed_name, SeedChars, RANDOM_SEED_DEFAULT, RANDOM_SEED_LEN, SEED_NAME_MAX};
use crate::storage::{self, fmt_bytes, Part, Report};

const WIDTH: f32 = 640.0;
const NAV_W: f32 = 180.0;
const BODY_H: f32 = 500.0;

/// A category in the list on the left.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Seed,
    Sound,
    Storage,
}

impl Tab {
    const ALL: [Tab; 3] = [Tab::Seed, Tab::Sound, Tab::Storage];

    fn name(self) -> &'static str {
        match self {
            Tab::Seed => "Seed",
            Tab::Sound => "Sound",
            Tab::Storage => "Storage",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Seed => "seed",
            Tab::Sound => "sound",
            Tab::Storage => "stack",
        }
    }
}

/// Settings → Sound: the file picker while it's open, and what the last check of the picked file found.
#[derive(Default)]
pub struct SoundView {
    pick: Option<mpsc::Receiver<Option<PathBuf>>>,
    /// The file checked, and whether it plays (with its length, when the file says).
    checked: Option<(PathBuf, Result<Option<Duration>, String>)>,
}

/// Settings → Storage: the last sizes measured, a measure or clear running, and what it did.
#[derive(Default)]
pub struct StorageView {
    report: Option<Report>,
    job: Option<mpsc::Receiver<Report>>,
    /// The parts the running job clears, and their sizes before.
    clearing: Vec<(Part, u64)>,
    note: Option<String>,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    take_picked_sound(app);
    if !app.prefs_open {
        app.seed_name_editing = false;
        // checked again when the page opens: the file may have moved meanwhile
        app.sound_view.checked = None;
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
                        Tab::Sound => sound_page(app, ui, w),
                        Tab::Storage => storage_page(app, ui, w),
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

/// Measures (and first clears `parts`) on a worker thread; the page picks the result up.
fn storage_job(app: &mut App, parts: Vec<Part>) {
    let before = app.storage.report.clone().unwrap_or_default();
    app.storage.clearing = parts.iter().map(|p| (*p, before.get(*p).bytes)).collect();
    let (tx, rx) = mpsc::channel();
    let repaint = app.repaint.clone();
    std::thread::spawn(move || {
        for p in parts {
            storage::clear(p);
        }
        let _ = tx.send(storage::measure());
        repaint();
    });
    app.storage.job = Some(rx);
}

/// Bets, hits and stats: in memory only, emptied here.
fn clear_bets(app: &mut App) {
    super::clear_history(app);
    app.hits.clear();
    app.hits_seen = 0;
    app.card = None;
    app.spins.clear();
    app.spins_total = 0;
    app.reset_stats();
    app.slot_stats = super::stats::Stats::fresh(&app.slot_stats.cur.clone(), app.slot_stats.decimals);
}

fn storage_page(app: &mut App, ui: &mut Ui, w: f32) {
    heading(ui, "Storage", "What FastBet keeps on this PC. Clearing never touches your logins or settings.");
    // the sizes: measured when the page opens, and again after every clear
    if let Some(report) = app.storage.job.as_ref().and_then(|rx| rx.try_recv().ok()) {
        app.storage.job = None;
        if !app.storage.clearing.is_empty() {
            let (before, left): (u64, u64) = app.storage.clearing.iter().fold((0, 0), |(b, l), (p, was)| (b + was, l + report.get(*p).bytes));
            let freed = before.saturating_sub(left);
            app.storage.note = Some(if left > 1024 * 1024 {
                format!("Freed {}. {} is in use by an open window or connection and stays until FastBet restarts.", fmt_bytes(freed), fmt_bytes(left))
            } else {
                format!("Freed {}.", fmt_bytes(freed))
            });
            app.storage.clearing.clear();
        }
        app.storage.report = Some(report);
    }
    if app.storage.report.is_none() && app.storage.job.is_none() {
        storage_job(app, vec![]);
    }
    let busy = app.storage.job.is_some();
    if busy {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(150));
    }
    let size = |part: Part| match (&app.storage.report, busy) {
        (_, true) if app.storage.clearing.iter().any(|(p, _)| *p == part) => "Clearing…".to_string(),
        (Some(r), _) => fmt_bytes(r.get(part).bytes),
        (None, _) => "Measuring…".to_string(),
    };
    let in_memory = app.history.entries.iter().filter(|e| matches!(e, super::stats::HistEntry::Bet { .. })).count() + app.spins.len();
    let rows: Vec<(Option<Part>, &str, &str, String)> = vec![
        (
            None,
            "Bets and history",
            "Never saved to disk. Bets, hits and stats live in memory and are gone when FastBet closes.",
            format!("{} in memory", crate::money::fmt_int(in_memory as i128)),
        ),
        (Some(Part::Pictures), "Game pictures", "Slot and originals artwork. It downloads again when it's shown.", size(Part::Pictures)),
        (
            Some(Part::Browser),
            "Browser data",
            "Login windows and Cloudflare checks. Your saved logins stay; a login window opens signed out and Cloudflare checks run again.",
            size(Part::Browser),
        ),
        (Some(Part::Updates), "Downloaded updates", "Installers of updates that are already installed.", size(Part::Updates)),
        (Some(Part::Backups), "Old settings backups", "Copies kept when the settings couldn't be read. They hold old logins.", size(Part::Backups)),
    ];
    let mut clear: Option<Option<Part>> = None;
    widgets::card(ui, w, "On this PC", None, None, |ui| {
        let inner = ui.available_width();
        for (i, (part, title, about, value)) in rows.iter().enumerate() {
            if i > 0 {
                ui.add_space(10.0);
                widgets::hsep(ui);
                ui.add_space(10.0);
            }
            let empty = part.is_some_and(|p| app.storage.report.as_ref().is_some_and(|r| r.get(p).files == 0));
            // the name, its size and Clear on one line, what it is under them
            ui.allocate_ui_with_layout(vec2(inner, 34.0), Layout::left_to_right(Align::Center), |ui| {
                ui.label(widgets::semi(*title, 13.0, TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button(ui, "Clear", Btn::Ghost, !busy && !empty, Some(72.0)).clicked() {
                        clear = Some(*part);
                    }
                    ui.add_space(4.0);
                    ui.label(widgets::semi(value.clone(), 12.5, if empty { DIM } else { TEXT2 }));
                });
            });
            ui.add_space(2.0);
            ui.label(RichText::new(*about).size(11.5).color(MUTED));
        }
    });
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        ui.set_width(w);
        if let Some(note) = &app.storage.note {
            ui.label(RichText::new(note).size(12.0).color(G2));
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if widgets::button(ui, "Clear all", Btn::Ghost, !busy, Some(100.0)).clicked() {
                clear_bets(app);
                storage_job(app, Part::ALL.to_vec());
            }
        });
    });
    match clear {
        Some(None) => {
            clear_bets(app);
            app.storage.note = Some("Bets, hits and stats cleared.".into());
        }
        Some(Some(part)) => storage_job(app, vec![part]),
        None => {}
    }
    if let Some(dir) = storage::settings_dir() {
        ui.add_space(18.0);
        ui.label(RichText::new(format!("Settings and logins are kept in {}", dir.display())).size(11.5).color(DIM));
        ui.add_space(4.0);
        if ui.add(egui::Label::new(RichText::new("Open folder").size(11.5).color(TEXT2).underline()).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            open_folder(&dir);
        }
    }
}

fn open_folder(dir: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

/// Opens the system's file picker on a worker thread; `take_picked_sound` picks up the answer.
fn pick_sound(app: &mut App) {
    let (tx, rx) = mpsc::channel();
    let repaint = app.repaint.clone();
    let dir = app.settings.hit_sound_file().and_then(|p| p.parent().map(Path::to_path_buf));
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Choose the hit sound")
            .add_filter("Sound or video", sound::EXTENSIONS)
            .add_filter("All files", &["*"]);
        if let Some(dir) = dir {
            dialog = dialog.set_directory(dir);
        }
        let _ = tx.send(dialog.pick_file());
        repaint();
    });
    app.sound_view.pick = Some(rx);
}

/// A file chosen in the picker becomes the hit sound (and turns it on).
fn take_picked_sound(app: &mut App) {
    let Some(rx) = &app.sound_view.pick else { return };
    let picked = match rx.try_recv() {
        Ok(picked) => picked,
        Err(mpsc::TryRecvError::Empty) => return,
        Err(mpsc::TryRecvError::Disconnected) => None,
    };
    app.sound_view.pick = None;
    if let Some(path) = picked {
        app.sound.stop();
        app.settings.hit_sound_file = path.to_string_lossy().into_owned();
        app.settings.hit_sound = true;
        app.mark_dirty();
    }
}

fn sound_page(app: &mut App, ui: &mut Ui, w: f32) {
    heading(ui, "Sound", "What plays when a bet hits, or when Stop on next win ends a run.");
    let file = app.settings.hit_sound_file();
    if let Some(path) = &file {
        if app.sound_view.checked.as_ref().is_none_or(|(p, _)| p != path) {
            app.sound_view.checked = Some((path.clone(), sound::check(path)));
        }
    }
    let checked = file.as_ref().and(app.sound_view.checked.as_ref()).map(|(_, r)| r.clone());
    let playing = app.sound.playing();
    if playing {
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
    let picking = app.sound_view.pick.is_some();
    let help = "Plays when a bet reaches Stop on hit (on slots: Stop on multiplier) and when Stop on next win ends a run. \
                Pick a sound file, or a video to play its sound. FastBet plays it from where it is on your PC, so leave \
                the file there.";
    let state = if app.settings.hit_sound { widgets::semi("On", 11.5, G2) } else { widgets::semi("Off", 11.5, DIM) };
    widgets::card(ui, w, "Hit sound", Some(help), Some(state), |ui| {
        let mut on = app.settings.hit_sound;
        if widgets::toggle(ui, "Sound on hit", &mut on, true) {
            app.settings.hit_sound = on;
            app.mark_dirty();
        }
        let mut reset = false;
        widgets::row(ui, 18.0, |ui| {
            widgets::caption(ui, "Sound");
            if file.is_some() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let link = ui.add(egui::Label::new(widgets::semi("Use the ding", 12.0, G1)).sense(Sense::click()));
                    reset = link.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                });
            }
        });
        let mut play = false;
        let mut choose = false;
        widgets::row(ui, widgets::FIELD_H, |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                choose = widgets::button(ui, if picking { "Choosing…" } else { "Choose file" }, Btn::Ghost, !picking, Some(108.0)).clicked();
                play = widgets::button(ui, if playing { "Stop" } else { "Play" }, Btn::Ghost, true, Some(72.0)).clicked();
                let name = match &file {
                    Some(p) => p.file_name().map_or_else(|| p.to_string_lossy(), |n| n.to_string_lossy()).into_owned(),
                    None => "Ding".to_string(),
                };
                let len = checked.clone().and_then(Result::ok).flatten();
                let path = file.as_ref().map(|p| p.display().to_string());
                sound_name(ui, &name, len, path.as_deref());
            });
        });
        if let Some(Err(why)) = &checked {
            widgets::readout(ui, format!("Can't play this file: {why}. Hits play the ding instead."), TEXT2, GOLD);
        }
        if reset {
            app.sound.stop();
            app.settings.hit_sound_file.clear();
            app.mark_dirty();
        } else if choose {
            pick_sound(app);
        } else if play {
            if playing {
                app.sound.stop();
            } else {
                app.sound.play(file.clone());
            }
        }
    });
}

/// The sound's name in a recessed box like the fields, its length on the right.
fn sound_name(ui: &mut Ui, name: &str, len: Option<Duration>, path: Option<&str>) {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), widgets::FIELD_H), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(theme::RADIUS), theme::BG, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
    let mut right = rect.right() - 12.0;
    if let Some(d) = len {
        let secs = (d.as_secs_f32().round() as u64).max(1);
        let g = p.layout_no_wrap(format!("{}:{:02}", secs / 60, secs % 60), theme::font_semi(11.5), DIM);
        right -= g.size().x;
        p.galley(pos2(right, rect.center().y - g.size().y / 2.0), g, DIM);
        right -= 10.0;
    }
    let color = if path.is_some() { TEXT } else { TEXT2 };
    widgets::fit_text(p, pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, name, theme::font_semi(13.0), color, right - rect.left() - 12.0);
    if let Some(path) = path {
        resp.on_hover_text(path);
    }
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
