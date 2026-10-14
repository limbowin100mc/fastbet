//! Hits: the bets and spins that reached the stop-on-hit line. The history tables list them
//! under their Hits tab; clicking one (or a gold row under Bets) opens its card, drawn by
//! [`hitcard`](super::hitcard), to download as a PNG or copy.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::SystemTime;

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Id, Layout, Rect, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions, Ui, UiBuilder};

use super::stats::{clock, date_time, file_stamp};
use super::theme::{self, alpha, BG, BG1, DIM, GOLD, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::{hitcard, images, App};
use crate::engine::slots::SpinRecord;
use crate::engine::BetRecord;
use crate::money::{fmt_int, fmt_multi};
use crate::slots::{fmt_money, PlayMode};

/// Hits kept for the session, newest first.
const KEEP: usize = 500;
/// A win paying at least this many times the stake is a hit, whatever the stop-on-hit line.
pub const BIG_WIN: f64 = 10.0;
const TABS_H: f32 = 40.0;
const ROW_H: f32 = 34.0;

/// Which history row a hit came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKey {
    /// A Limbo bet or an original's round, by its number in the history.
    Bet(u64),
    /// A slot spin, by its number in the slot session.
    Spin(u64),
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub key: HitKey,
    pub at: SystemTime,
    pub game: String,
    pub byline: String,
    /// What it paid, as a multiple of the stake.
    pub multiplier: f64,
    pub win: String,
    pub bet: String,
    /// The game's artwork URL (empty when there's none).
    pub art: String,
    pub demo: bool,
    /// A hit (in the Hits list); otherwise a win whose card was asked for.
    pub hit: bool,
}

impl Hit {
    fn card(&self) -> hitcard::Card {
        hitcard::Card {
            game: self.game.clone(),
            byline: self.byline.clone(),
            multiplier: self.multiplier,
            win: self.win.clone(),
            bet: self.bet.clone(),
            when: date_time(self.at),
            demo: self.demo,
            hit: self.hit,
        }
    }
}

fn push(app: &mut App, hit: Hit) {
    app.hits.insert(0, hit);
    app.hits.truncate(KEEP);
}

/// What a bet paid, as a multiple of the stake: Limbo's target when it won (the roll when a
/// hit line under the target was reached on a loss), an original's round multiplier.
fn paid(rec: &BetRecord) -> f64 {
    if rec.game.is_none() && !rec.win {
        rec.outcome
    } else {
        rec.target
    }
}

/// A bet that reached the stop-on-hit line, or a win of at least [`BIG_WIN`].
pub fn is_hit_bet(rec: &BetRecord) -> bool {
    rec.hit || (rec.win && paid(rec) >= BIG_WIN)
}

/// A spin that reached the stop line, or paid at least [`BIG_WIN`].
pub fn is_hit_spin(rec: &SpinRecord) -> bool {
    rec.hit || (!rec.held && rec.multiplier >= BIG_WIN)
}

/// A Limbo bet or an original's round that hit; `n` is its number in the history.
pub fn record_bet(app: &mut App, n: u64, rec: &BetRecord) {
    let hit = bet_hit(app, n, rec);
    push(app, hit);
}

/// The card's facts about bet `n`.
fn bet_hit(app: &App, n: u64, rec: &BetRecord) -> Hit {
    let casino = app.casino();
    let (game, slug) = match rec.game {
        Some(g) => (casino.original_name(g).to_string(), g.slug()),
        None => ("Limbo".to_string(), "limbo"),
    };
    let art = app.orig.catalog.iter().find(|c| c.id == slug).map(|c| c.image.clone()).unwrap_or_default();
    Hit {
        key: HitKey::Bet(n),
        at: rec.at,
        game,
        byline: format!("Originals · {}", casino.name()),
        multiplier: paid(rec),
        win: app.prim(rec.payout, &rec.currency, false),
        bet: app.prim(rec.amount, &rec.currency, false),
        art,
        demo: false,
        hit: is_hit_bet(rec),
    }
}

/// A slot spin that hit; `n` is its number in the slot session.
pub fn record_spin(app: &mut App, n: u64, rec: &SpinRecord) {
    if let Some(hit) = spin_hit(app, n, rec) {
        push(app, hit);
    }
}

/// The card's facts about spin `n` of the open game.
fn spin_hit(app: &App, n: u64, rec: &SpinRecord) -> Option<Hit> {
    let game = app.slot.game.as_ref()?;
    Some(Hit {
        key: HitKey::Spin(n),
        at: rec.at,
        game: game.name.clone(),
        byline: format!("{} · {}", game.maker(), app.casino().name()),
        multiplier: rec.multiplier,
        win: fmt_money(rec.win, &rec.currency, rec.decimals),
        bet: fmt_money(rec.cost, &rec.currency, rec.decimals),
        art: game.image.clone(),
        demo: app.slot.mode == Some(PlayMode::Demo),
        hit: is_hit_spin(rec),
    })
}

/// "Bets · Hits" at the top of a history table. True while Hits is picked.
pub fn tabs(app: &mut App, ui: &mut Ui, width: f32) -> bool {
    let (row, _) = ui.allocate_exact_size(vec2(width, TABS_H), Sense::hover());
    let unseen = app.hits.len().saturating_sub(app.hits_seen);
    let mut x = row.left() + 12.0;
    for (i, label) in ["Bets", "Hits"].iter().enumerate() {
        let picked = (i == 1) == app.hits_tab;
        let g = ui.painter().layout_no_wrap(label.to_string(), theme::font_semi(12.5), TEXT);
        let count = (i == 1 && !app.hits.is_empty()).then(|| fmt_int(app.hits.len() as i128));
        let chip = count.as_ref().map(|c| ui.painter().layout_no_wrap(c.clone(), theme::font_bold(10.5), TEXT));
        let chip_w = chip.as_ref().map_or(0.0, |c| c.size().x + 12.0);
        let r = Rect::from_min_size(pos2(x, row.top()), vec2(g.size().x + if chip_w > 0.0 { chip_w + 7.0 } else { 0.0 }, row.height()));
        let resp = ui.interact(r.expand2(vec2(6.0, 0.0)), ui.id().with(("history-tab", i)), Sense::click());
        let color = if picked {
            TEXT
        } else if resp.hovered() {
            TEXT2
        } else {
            MUTED
        };
        let p = ui.painter();
        p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, *label, theme::font_semi(12.5), color);
        if let Some(c) = chip {
            let cr = Rect::from_min_size(pos2(r.left() + g.size().x + 7.0, r.center().y - 9.0), vec2(chip_w, 18.0));
            let (fill, text) = if unseen > 0 && !app.hits_tab { (GOLD, Color32::from_rgb(0x1a, 0x10, 0x00)) } else { (alpha(Color32::WHITE, 18), TEXT2) };
            p.rect_filled(cr, CornerRadius::same(9), fill);
            p.galley(pos2(cr.center().x - c.size().x / 2.0, cr.center().y - c.size().y / 2.0), c, text);
        }
        if picked {
            let line = Rect::from_min_max(pos2(r.left(), row.bottom() - 2.0), pos2(r.right(), row.bottom()));
            p.rect_filled(line, CornerRadius::same(1), if i == 1 { GOLD } else { theme::G1 });
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            app.hits_tab = i == 1;
        }
        x = r.right() + 24.0;
    }
    ui.painter().hline(row.x_range().expand(8.0), row.bottom(), Stroke::new(1.0, LINE));
    if app.hits_tab {
        app.hits_seen = app.hits.len();
    }
    app.hits_tab
}

struct Col {
    name: &'static str,
    weight: f32,
    right: bool,
}

const COLS: &[Col] = &[
    Col { name: "Time", weight: 0.9, right: false },
    Col { name: "Game", weight: 2.0, right: false },
    Col { name: "Multiplier", weight: 1.1, right: true },
    Col { name: "Bet", weight: 1.1, right: true },
    Col { name: "Win", weight: 1.2, right: true },
    Col { name: "", weight: 1.0, right: true },
];

/// The hits, newest first; a click opens the card.
pub fn table(app: &mut App, ui: &mut Ui, width: f32) {
    let sum: f32 = COLS.iter().map(|c| c.weight).sum();
    let widths: Vec<f32> = COLS.iter().map(|c| (width - 8.0) * c.weight / sum).collect();
    let cell_pos = |cell: Rect, right: bool| if right { (pos2(cell.right() - 12.0, cell.center().y), Align2::RIGHT_CENTER) } else { (pos2(cell.left() + 12.0, cell.center().y), Align2::LEFT_CENTER) };

    let (hr, _) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
    let p = ui.painter();
    let mut x = hr.left() + 8.0;
    for (c, w) in COLS.iter().zip(&widths) {
        let cell = Rect::from_min_size(pos2(x, hr.top()), vec2(*w, hr.height()));
        let (pos, al) = cell_pos(cell, c.right);
        p.text(pos, al, c.name.to_uppercase(), theme::font_semi(10.5), DIM);
        x += w;
    }
    p.hline(hr.x_range().expand(8.0), hr.bottom(), Stroke::new(1.0, LINE));

    if app.hits.is_empty() {
        let (r, _) = ui.allocate_exact_size(vec2(width, 90.0), Sense::hover());
        ui.painter().text(pos2(r.center().x, r.center().y - 8.0), Align2::CENTER_CENTER, "No hits yet", theme::font_semi(13.0), MUTED);
        ui.painter().text(pos2(r.center().x, r.center().y + 12.0), Align2::CENTER_CENTER, "Every hit lands here, with a card to save and share", theme::font(12.0), DIM);
        return;
    }
    let mut open_i = None;
    egui::ScrollArea::vertical().id_salt("hits-table").auto_shrink([false, false]).show_rows(ui, ROW_H, app.hits.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let h = &app.hits[i];
            let (rr, resp) = ui.allocate_exact_size(vec2(width, ROW_H), Sense::click());
            let hovered = resp.hovered();
            let p = ui.painter();
            if hovered {
                p.rect_filled(rr, CornerRadius::same(6), alpha(GOLD, 22));
            } else if i % 2 == 1 {
                p.rect_filled(rr, CornerRadius::same(6), alpha(Color32::WHITE, 3));
            }
            let bar = Rect::from_min_size(pos2(rr.left() + 2.0, rr.center().y - 7.0), vec2(3.0, 14.0));
            p.rect_filled(bar, CornerRadius::same(2), alpha(GOLD, 220));
            let game = if h.demo { format!("{}  · demo", h.game) } else { h.game.clone() };
            let cells: [(String, Color32, bool); 6] = [
                (clock(h.at), MUTED, false),
                (game, TEXT2, false),
                (fmt_multi(h.multiplier), GOLD, true),
                (h.bet.clone(), TEXT2, false),
                (h.win.clone(), TEXT, true),
                ("View card ›".into(), if hovered { GOLD } else { MUTED }, true),
            ];
            let mut x = rr.left() + 8.0;
            for ((text, color, strong), (col, w)) in cells.iter().zip(COLS.iter().zip(&widths)) {
                let cell = Rect::from_min_size(pos2(x, rr.top()), vec2(*w, ROW_H));
                let (pos, al) = cell_pos(cell, col.right);
                let font = if *strong { theme::font_semi(12.5) } else { theme::font(12.5) };
                widgets::fit_text(p, pos, al, text, font, *color, w - 16.0);
                x += w;
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                open_i = Some(i);
            }
        }
    });
    if let Some(i) = open_i {
        let hit = app.hits[i].clone();
        open(app, hit);
    }
}

/// The open card: rendering on a worker thread, then shown with its buttons.
pub struct CardView {
    hit: Hit,
    rx: Option<mpsc::Receiver<Result<(egui::ColorImage, Vec<u8>), String>>>,
    tex: Option<TextureHandle>,
    image: Option<egui::ColorImage>,
    png: Option<Vec<u8>>,
    error: Option<String>,
    note: Option<String>,
    saved: Option<PathBuf>,
}

/// Opens the card of bet `n` (a history number): a hit's, or any bet's still in the history.
pub fn open_bet(app: &mut App, n: u64) {
    let hit = app.hits.iter().find(|h| h.key == HitKey::Bet(n)).cloned().or_else(|| {
        app.history.entries.iter().find_map(|e| match e {
            super::stats::HistEntry::Bet { n: m, rec } if *m == n => Some(bet_hit(app, n, rec)),
            _ => None,
        })
    });
    if let Some(hit) = hit {
        open(app, hit);
    }
}

/// Opens the card of spin `n` of the slot session: a hit's, or any spin's still in the list.
pub fn open_spin(app: &mut App, n: u64) {
    let hit = app.hits.iter().find(|h| h.key == HitKey::Spin(n)).cloned().or_else(|| {
        let i = app.spins_total.checked_sub(n)? as usize;
        app.spins.get(i).and_then(|rec| spin_hit(app, n, rec))
    });
    if let Some(hit) = hit {
        open(app, hit);
    }
}

pub fn open(app: &mut App, hit: Hit) {
    let (tx, rx) = mpsc::channel();
    let card = hit.card();
    let art = hit.art.clone();
    let repaint = app.repaint.clone();
    std::thread::spawn(move || {
        let bytes = if art.is_empty() { None } else { images::bytes(&big(&art)).or_else(|| images::bytes(&art)) };
        let res = hitcard::render(&card, bytes.as_deref()).and_then(|pix| Ok((hitcard::color_image(&pix), hitcard::png(&pix)?)));
        let _ = tx.send(res);
        repaint();
    });
    app.card = Some(CardView { hit, rx: Some(rx), tex: None, image: None, png: None, error: None, note: None, saved: None });
}

/// The artwork at twice the size where the CDN resizes on request (imgix: Stake, Shuffle).
fn big(url: &str) -> String {
    let Some((base, query)) = url.split_once('?').filter(|_| url.contains(".imgix.net/")) else { return url.to_string() };
    let query: Vec<String> = query
        .split('&')
        .map(|kv| match kv.split_once('=') {
            Some((k @ ("w" | "h"), v)) => v.parse::<u32>().map(|n| format!("{k}={}", n * 2)).unwrap_or_else(|_| kv.to_string()),
            _ => kv.to_string(),
        })
        .collect();
    format!("{base}?{}", query.join("&"))
}

/// The card window, while one is open.
pub fn show(app: &mut App, ctx: &egui::Context) {
    let Some(view) = app.card.as_mut() else { return };
    if let Some(res) = view.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
        view.rx = None;
        match res {
            Ok((img, png)) => {
                view.tex = Some(ctx.load_texture("hit-card", img.clone(), TextureOptions::LINEAR));
                view.image = Some(img);
                view.png = Some(png);
            }
            Err(e) => view.error = Some(e),
        }
    }
    let screen = ctx.content_rect();
    let w = (screen.width() - 160.0).min((screen.height() - 200.0) * hitcard::W as f32 / hitcard::H as f32).clamp(420.0, 960.0);
    let h = w * hitcard::H as f32 / hitcard::W as f32;
    let mut close = false;
    let modal = egui::Modal::new(Id::new("hit-card")).frame(Frame::new()).backdrop_color(alpha(BG, 238)).show(ctx, |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 0.0;
        let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
        let p = ui.painter();
        match (&view.tex, &view.error) {
            (Some(t), _) => {
                // a soft shadow under the card
                p.rect_filled(r.translate(vec2(0.0, 10.0)).shrink(6.0), CornerRadius::same(28), alpha(Color32::BLACK, 120));
                p.image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            (None, Some(e)) => {
                p.rect(r, CornerRadius::same(24), BG1, Stroke::new(1.0, LINE2), StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, format!("Couldn't draw the card: {e}"), theme::font(13.0), RED);
            }
            _ => {
                p.rect(r, CornerRadius::same(24), BG1, Stroke::new(1.0, LINE2), StrokeKind::Inside);
                p.text(r.center(), Align2::CENTER_CENTER, "Drawing your card…", theme::font_semi(14.0), MUTED);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        ui.add_space(18.0);
        let (bar, _) = ui.allocate_exact_size(vec2(w, 38.0), Sense::hover());
        let ready = view.png.is_some();
        let mut left = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::left_to_right(Align::Center)));
        left.spacing_mut().item_spacing.x = 10.0;
        if button(&mut left, "Download PNG", Btn::Primary, ready, Some(170.0)).clicked() {
            match save(view) {
                Ok(path) => {
                    view.note = Some(format!("Saved to {}", path.parent().map(|d| d.display().to_string()).unwrap_or_default()));
                    view.saved = Some(path);
                }
                Err(e) => view.note = Some(e),
            }
        }
        if button(&mut left, "Copy image", Btn::Ghost, view.image.is_some(), Some(130.0)).clicked() {
            if let Some(img) = &view.image {
                left.ctx().copy_image(img.clone());
                view.note = Some("Copied: paste it anywhere".into());
            }
        }
        if let Some(path) = view.saved.clone() {
            if button(&mut left, "Show in folder", Btn::Ghost, true, None).clicked() {
                reveal(&path);
            }
        }
        let mut right = ui.new_child(UiBuilder::new().max_rect(bar).layout(Layout::right_to_left(Align::Center)));
        right.spacing_mut().item_spacing.x = 12.0;
        if button(&mut right, "Close", Btn::Ghost, true, Some(90.0)).clicked() {
            close = true;
        }
        if let Some(note) = &view.note {
            right.label(widgets::semi(note.clone(), 12.0, TEXT2));
        }
    });
    if close || modal.should_close() {
        app.card = None;
    }
}

/// Saves the card into Downloads (or the home folder) and returns its path.
fn save(view: &CardView) -> Result<PathBuf, String> {
    let png = view.png.as_ref().ok_or("The card isn't ready yet")?;
    let dirs = directories::UserDirs::new().ok_or("No home folder")?;
    let dir = dirs.download_dir().unwrap_or(dirs.home_dir()).to_path_buf();
    let path = dir.join(file_name(&view.hit));
    std::fs::write(&path, png).map_err(|e| format!("Couldn't save the card: {e}"))?;
    Ok(path)
}

/// "FastBet Le Vampire 3211.00x 2026-10-14 16-30-05.png", without the characters file names can't have.
fn file_name(hit: &Hit) -> String {
    let mult = fmt_multi(hit.multiplier).replace('×', "x").replace(',', "");
    format!("FastBet {} {mult} {}.png", hit.game, file_stamp(hit.at))
        .chars()
        .map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect()
}

/// Opens the folder with the file picked, where the system can.
fn reveal(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    if let Some(dir) = path.parent() {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imgix_art_is_fetched_twice_as_big() {
        assert_eq!(big("https://shuffle-com.imgix.net/k?w=264&h=346&fit=crop&fm=jpg"), "https://shuffle-com.imgix.net/k?w=528&h=692&fit=crop&fm=jpg");
        assert_eq!(big("https://cdn.hub88.io/x.jpg"), "https://cdn.hub88.io/x.jpg");
    }

    #[test]
    fn card_files_get_names_windows_takes() {
        let hit = Hit {
            key: HitKey::Spin(1),
            at: SystemTime::now(),
            game: "Peter & Sons: Barbarossa / Beyond?".into(),
            byline: String::new(),
            multiplier: 3211.0,
            win: String::new(),
            bet: String::new(),
            art: String::new(),
            demo: false,
            hit: true,
        };
        let name = file_name(&hit);
        assert!(name.starts_with("FastBet Peter & Sons- Barbarossa - Beyond- 3211"), "{name}");
        assert!(name.ends_with(".png") && !name.contains(['/', ':', '?', '×']), "{name}");
    }
}
