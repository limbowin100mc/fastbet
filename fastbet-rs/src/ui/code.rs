//! Code Mode's panel, in place of the stage: the script editor (highlighted, with line numbers),
//! examples, a check, and beside it the console the script logs to and a reference of what a
//! script can use.

use std::collections::VecDeque;
use std::time::SystemTime;

use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Label, Layout, Margin, Rect, RichText, Sense, Stroke, StrokeKind, TextBuffer, Ui, UiBuilder};

use super::stats::clock;
use super::theme::{self, alpha, BG, BG1, BG3, DIM, G1, G2, GOLD, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, select_opt, Btn};
use super::App;
use crate::engine::ScriptLine;
use crate::script::{Script, Setup, EXAMPLES};

const LOG_MAX: usize = 400;

/// What the panel keeps between frames.
#[derive(Default)]
pub struct State {
    /// What scripts logged, oldest first.
    pub log: VecDeque<ScriptLine>,
    /// The last check or run: what it found, or the error.
    pub status: Option<Result<String, String>>,
    /// The side column shows the reference, not the console.
    pub reference: bool,
    /// The script from before an example replaced it.
    pub undo: Option<String>,
    /// A Code Mode run is going (the console notes its end).
    pub running: bool,
}

impl State {
    pub fn push(&mut self, lines: Vec<ScriptLine>) {
        for l in lines {
            if l.error {
                self.status = Some(Err(l.text.clone()));
            }
            self.log.push_back(l);
        }
        while self.log.len() > LOG_MAX {
            self.log.pop_front();
        }
    }

    /// A run ended by itself or by Stop: `why` as the engine put it.
    pub fn ended(&mut self, why: &str) {
        let why = if why.is_empty() { "Stopped" } else { why };
        self.note(format!("Run ended: {why}"), false);
        self.status = Some(Ok(why.to_string()));
    }

    fn note(&mut self, text: impl Into<String>, error: bool) {
        self.push(vec![ScriptLine { at: SystemTime::now(), text: text.into(), error }]);
    }
}

/// The script's first run, as the Check button and a run start it.
pub fn setup(app: &App, game: &str) -> Setup {
    let unit = if app.fiat_mode() { app.fiat().to_uppercase() } else { app.active_currency().to_uppercase() };
    Setup {
        code: app.settings.script.clone(),
        bet: crate::money::num(&app.settings.amount).unwrap_or(0.0).max(0.0),
        target: app.target().unwrap_or(2.0),
        game: game.to_string(),
        unit,
        currency: app.active_currency(),
    }
}

/// The panel, filling `rect` (the stage, already drawn).
pub fn panel(app: &mut App, ui: &mut Ui, rect: Rect, game: &str) {
    let running = app.auto.running;
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 48.0));
    toolbar(app, ui, bar, game, running);
    let body = Rect::from_min_max(pos2(rect.left(), bar.bottom()), rect.max);
    ui.painter().hline(rect.x_range().shrink(1.0), bar.bottom(), Stroke::new(1.0, LINE));
    let side_w = (body.width() * 0.34).clamp(230.0, 340.0);
    let editor_rect = Rect::from_min_max(body.min, pos2(body.right() - side_w, body.bottom())).shrink(1.0);
    let side_rect = Rect::from_min_max(pos2(body.right() - side_w, body.top()), body.max);
    ui.painter().vline(side_rect.left(), body.y_range().shrink(1.0), Stroke::new(1.0, LINE));
    let mut eui = ui.new_child(UiBuilder::new().max_rect(editor_rect).layout(Layout::top_down(Align::Min)));
    eui.set_clip_rect(editor_rect);
    editor(app, &mut eui, editor_rect, running);
    let mut sui = ui.new_child(UiBuilder::new().max_rect(side_rect.shrink2(vec2(12.0, 10.0))).layout(Layout::top_down(Align::Min)));
    sui.set_clip_rect(side_rect);
    side(app, &mut sui);
}

fn toolbar(app: &mut App, ui: &mut Ui, bar: Rect, game: &str, running: bool) {
    let p = ui.painter();
    let ir = Rect::from_center_size(pos2(bar.left() + 24.0, bar.center().y), vec2(15.0, 15.0));
    widgets::icon(p, ir, "code", G2);
    let g = p.layout_no_wrap("strategy.js".into(), theme::font_semi(13.0), TEXT);
    let title_w = g.size().x;
    p.galley(pos2(ir.right() + 10.0, bar.center().y - g.size().y / 2.0), g, TEXT);

    // examples, and undo after one replaced the script
    let ex = Rect::from_min_size(pos2(ir.right() + 10.0 + title_w + 16.0, bar.center().y - widgets::FIELD_H / 2.0), vec2(136.0, widgets::FIELD_H));
    let mut xui = ui.new_child(UiBuilder::new().max_rect(ex));
    let names: Vec<String> = EXAMPLES.iter().map(|(n, _)| n.to_string()).collect();
    if let Some(i) = select_opt(&mut xui, "code-examples", &names, None, "Examples", !running) {
        if app.settings.script != EXAMPLES[i].1 {
            app.code.undo = Some(std::mem::replace(&mut app.settings.script, EXAMPLES[i].1.to_string()));
            app.code.status = None;
            app.mark_dirty();
        }
    }
    if app.code.undo.is_some() && !running {
        let ur = Rect::from_min_size(pos2(ex.right() + 6.0, ex.top()), vec2(64.0, widgets::FIELD_H));
        let mut uui = ui.new_child(UiBuilder::new().max_rect(ur));
        if widgets::button(&mut uui, "Undo", Btn::Ghost, true, Some(64.0)).on_hover_text("Bring back the script from before the example").clicked() {
            if let Some(old) = app.code.undo.take() {
                app.settings.script = old;
                app.code.status = None;
                app.mark_dirty();
            }
        }
    }

    // right: check, and what the last check or run said
    let cr = Rect::from_min_size(pos2(bar.right() - 12.0 - 82.0, bar.center().y - widgets::FIELD_H / 2.0), vec2(82.0, widgets::FIELD_H));
    let mut cui = ui.new_child(UiBuilder::new().max_rect(cr));
    if widgets::button(&mut cui, "Check", Btn::Ghost, !running, Some(82.0)).on_hover_text("Run the script's start once, without betting").clicked() {
        check(app, game);
    }
    let (text, color) = match (&app.code.status, running) {
        (_, true) => ("Running".to_string(), G2),
        (Some(Ok(_)), _) => ("Ready".to_string(), G1),
        (Some(Err(_)), _) => ("Error".to_string(), RED),
        (None, _) => ("Not checked".to_string(), MUTED),
    };
    let p = ui.painter();
    let g = p.layout_no_wrap(text, theme::font_semi(11.5), color);
    let pill = Rect::from_min_size(pos2(cr.left() - 10.0 - g.size().x - 30.0, bar.center().y - 12.0), vec2(g.size().x + 30.0, 24.0));
    p.rect(pill, CornerRadius::same(12), alpha(color, 18), Stroke::new(1.0, alpha(color, 70)), StrokeKind::Inside);
    p.circle_filled(pos2(pill.left() + 13.0, pill.center().y), 3.5, color);
    p.galley(pos2(pill.left() + 22.0, pill.center().y - g.size().y / 2.0), g, color);
    if let Some(Err(e)) = &app.code.status {
        ui.interact(pill, ui.id().with("code-status"), Sense::hover()).on_hover_text(e.clone());
    }
}

fn check(app: &mut App, game: &str) {
    let s = setup(app, game);
    let unit = s.unit.clone();
    match Script::check(s) {
        Ok(d) => {
            let msg = format!("Checked: first bet {} {unit}, target {:.2}×", trim(d.bet), d.target);
            app.code.note(msg.clone(), false);
            for l in d.logs {
                app.code.note(l, false);
            }
            app.code.status = Some(Ok(msg));
        }
        Err(e) => app.code.note(e, true),
    }
    app.code.reference = false;
}

fn trim(v: f64) -> String {
    let s = format!("{v:.8}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The editor: line numbers in a gutter, the highlighted code beside them.
fn editor(app: &mut App, ui: &mut Ui, rect: Rect, running: bool) {
    let font = theme::font_mono(12.5);
    let row_h = ui.painter().layout_no_wrap("0".into(), font.clone(), TEXT).size().y;
    let lines = app.settings.script.split('\n').count().max(1);
    let digits = lines.to_string().len().max(2) as f32;
    let char_w = ui.painter().layout_no_wrap("0".into(), font.clone(), TEXT).size().x;
    let gutter_w = digits * char_w + 26.0;
    let top = 12.0;
    ui.painter().rect_filled(Rect::from_min_size(rect.min, vec2(gutter_w, rect.height())), CornerRadius { nw: 0, ne: 0, sw: 13, se: 0 }, alpha(BG, 160));
    let mut changed = false;
    egui::ScrollArea::both().id_salt("code-editor").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let h = (lines as f32 * row_h + top * 2.0).max(rect.height());
            let (gr, _) = ui.allocate_exact_size(vec2(gutter_w, h), Sense::hover());
            let p = ui.painter();
            for i in 0..lines {
                let y = gr.top() + top + row_h * (i as f32 + 0.5);
                p.text(pos2(gr.right() - 12.0, y), Align2::RIGHT_CENTER, format!("{}", i + 1), font.clone(), alpha(DIM, 200));
            }
            p.vline(gr.right() - 0.5, gr.y_range(), Stroke::new(1.0, alpha(LINE, 200)));
            let f = font.clone();
            let mut layouter = |ui: &Ui, buf: &dyn TextBuffer, _wrap: f32| {
                let mut job = highlight(buf.as_str(), f.clone());
                job.wrap.max_width = f32::INFINITY;
                ui.painter().layout_job(job)
            };
            // the editor draws no frame, so its padding is spacing around it
            ui.add_space(12.0);
            let w = (rect.width() - gutter_w - 28.0).max(100.0);
            ui.vertical(|ui| {
                ui.add_space(top);
                let out = egui::TextEdit::multiline(&mut app.settings.script)
                    .id_salt("code-text")
                    .frame(Frame::NONE)
                    .font(font.clone())
                    .code_editor()
                    .lock_focus(true)
                    .interactive(!running)
                    .margin(Margin::ZERO)
                    .min_size(vec2(w, h - top))
                    .desired_width(w)
                    .layouter(&mut layouter)
                    .show(ui);
                changed = out.response.changed();
            });
        });
    });
    if changed {
        app.code.status = None;
        app.mark_dirty();
    }
}

/// Colours for the script's words, the way code editors show them.
fn highlight(code: &str, font: FontId) -> LayoutJob {
    const KEYWORDS: [&str; 26] = [
        "var", "let", "const", "function", "return", "if", "else", "for", "while", "do", "break", "continue", "true", "false", "null", "undefined", "new", "typeof",
        "of", "in", "this", "switch", "case", "default", "try", "catch",
    ];
    const API: [&str; 18] = [
        "betSize", "target", "baseBet", "profit", "wagered", "bets", "wins", "losses", "winStreak", "lossStreak", "game", "unit", "currency", "onBet", "stop", "changeSeed", "log",
        "console",
    ];
    let keyword = Color32::from_rgb(0xc0, 0x8b, 0xf6);
    let string = Color32::from_rgb(0xe9, 0xc4, 0x6a);
    let number = Color32::from_rgb(0xff, 0x9e, 0x64);
    let comment = Color32::from_rgb(0x5c, 0x67, 0x7d);
    let api = G2;
    let plain = TEXT2;
    let mut job = LayoutJob::default();
    let fmt = |c: Color32, italics: bool| TextFormat { font_id: font.clone(), color: c, italics, ..Default::default() };
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0;
    let mut plain_buf = String::new();
    let flush = |job: &mut LayoutJob, buf: &mut String| {
        if !buf.is_empty() {
            job.append(buf, 0.0, fmt(plain, false));
            buf.clear();
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let start = i;
        let (color, italics) = if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            (comment, true)
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            (comment, true)
        } else if c == '"' || c == '\'' || c == '`' {
            i += 1;
            while i < chars.len() && chars[i] != c && chars[i] != '\n' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            (string, false)
        } else if c.is_ascii_digit() || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            (number, false)
        } else if c.is_alphabetic() || c == '_' || c == '$' {
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '$') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            // a property (bet.win) reads plain, a bare name the script knows stands out
            let after_dot = start > 0 && chars[start - 1] == '.';
            if KEYWORDS.contains(&word.as_str()) {
                (keyword, false)
            } else if API.contains(&word.as_str()) && !after_dot {
                (api, false)
            } else {
                plain_buf.push_str(&word);
                continue;
            }
        } else {
            plain_buf.push(c);
            i += 1;
            continue;
        };
        flush(&mut job, &mut plain_buf);
        let text: String = chars[start..i].iter().collect();
        job.append(&text, 0.0, fmt(color, italics));
    }
    flush(&mut job, &mut plain_buf);
    job
}

/// Console or reference, under small tabs.
fn side(app: &mut App, ui: &mut Ui) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (i, name) in ["Console", "Reference"].iter().enumerate() {
            let on = app.code.reference == (i == 1);
            let g = ui.painter().layout_no_wrap(name.to_string(), theme::font_semi(12.0), TEXT);
            let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 20.0, 28.0), Sense::click());
            let p = ui.painter();
            if on {
                p.rect(r, CornerRadius::same(7), BG3, Stroke::new(1.0, LINE2), StrokeKind::Inside);
            } else if resp.hovered() {
                p.rect_filled(r, CornerRadius::same(7), alpha(Color32::WHITE, 6));
            }
            let color = if on { TEXT } else if resp.hovered() { TEXT2 } else { MUTED };
            p.galley(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, color);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.code.reference = i == 1;
            }
        }
        if !app.code.reference && !app.code.log.is_empty() {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::glyph_button(ui, "trash", "Clear the console").clicked() {
                    app.code.log.clear();
                }
            });
        }
    });
    ui.add_space(8.0);
    if app.code.reference {
        reference(app, ui, w);
    } else {
        console(app, ui, w);
    }
}

fn console(app: &App, ui: &mut Ui, w: f32) {
    if app.code.log.is_empty() {
        ui.add_space(6.0);
        ui.label(RichText::new("Nothing logged yet.").size(12.0).color(TEXT2));
        ui.label(RichText::new("log(…) in the script writes here, and so do errors and Check.").size(11.5).color(MUTED));
        return;
    }
    egui::ScrollArea::vertical().id_salt("code-console").auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 5.0;
        for l in &app.code.log {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                ui.label(widgets::mono(clock(l.at), 10.5, DIM));
                let color = if l.error { RED } else { TEXT2 };
                ui.add(Label::new(widgets::mono(l.text.clone(), 11.5, color)).wrap());
            });
        }
    });
}

fn reference(app: &App, ui: &mut Ui, w: f32) {
    let unit = if app.fiat_mode() { app.fiat().to_uppercase() } else { app.active_currency().to_uppercase() };
    let groups: [(&str, Vec<(&str, String)>); 4] = [
        ("Set these", vec![("betSize", format!("The next bet, in {unit}")), ("target", "Limbo's target for the next bet".into())]),
        (
            "onBet(bet)",
            vec![
                ("function onBet(bet)", "Called after every bet".into()),
                ("bet.win", "true when it paid at least the stake".into()),
                ("bet.amount · bet.payout · bet.profit", format!("In {unit}")),
                ("bet.multiplier", "What it landed on".into()),
                ("bet.target", "The target it had".into()),
            ],
        ),
        (
            "Read these",
            vec![
                ("baseBet", "The bet size in the panel".into()),
                ("profit · wagered", format!("This run so far, in {unit}")),
                ("bets · wins · losses", "Counted this run".into()),
                ("winStreak · lossStreak", "The streak going on".into()),
                ("game · unit · currency", "Where the run plays".into()),
            ],
        ),
        (
            "Functions",
            vec![
                ("stop(reason)", "Ends the run".into()),
                ("changeSeed()", "New seed before the next bet".into()),
                ("log(…)", "Writes to the console".into()),
            ],
        ),
    ];
    egui::ScrollArea::vertical().id_salt("code-reference").auto_shrink([false, false]).show(ui, |ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 3.0;
        for (title, items) in groups {
            ui.add_space(4.0);
            ui.label(widgets::semi(title.to_uppercase(), 10.0, MUTED));
            ui.add_space(2.0);
            for (name, about) in items {
                Frame::new().fill(BG1).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(7)).inner_margin(Margin::symmetric(9, 6)).show(ui, |ui| {
                    ui.set_width(w - 20.0);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(widgets::mono(name, 11.5, G2));
                    ui.add(Label::new(RichText::new(about).size(11.0).color(TEXT2)).wrap());
                });
            }
            ui.add_space(6.0);
        }
        ui.label(RichText::new("Amounts are in the unit the bet card shows. Stop profit / loss, max bet and the other stops still apply.").size(10.5).color(GOLD.gamma_multiply(0.8)));
    });
}
