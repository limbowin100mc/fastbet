//! Reusable widgets: pills, section cards, fields, segmented controls,
//! buttons, stat chips, gradients and glows, the dot-grid background and the logo.

use egui::emath::GuiRounding as _;
use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Frame, Layout, Margin, Painter, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, TextEdit, TextureHandle, Ui, UiBuilder, Vec2,
};

use super::logos;
use super::theme::{self, alpha, mix, BG, BG1, BG2, BG3, DIM, G1, G2, GOLD, INK, LINE, LINE2, MUTED, RADIUS, RADIUS_SM, RED, TEXT, TEXT2};

/// Height of text fields and the buttons that sit next to them.
pub const FIELD_H: f32 = 34.0;

pub fn semi(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text.into()).size(size).color(color).family(theme::semi())
}

pub fn bold(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text.into()).size(size).color(color).family(theme::bold())
}

pub fn mono(text: impl Into<String>, size: f32, color: Color32) -> RichText {
    RichText::new(text.into()).size(size).color(color).family(theme::mono())
}

/// Colour of the captions over fields: quieter than values, louder than notes.
const LABEL: Color32 = Color32::from_rgb(0x9a, 0xa0, 0xab);

/// Small caption like "Bet size".
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(semi(text, 11.5, LABEL));
}

/// A caption with a "?" after it that explains the field when hovered.
pub fn caption_help(ui: &mut Ui, text: &str, help: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        caption(ui, text);
        help_icon(ui, help);
    });
}

/// A small "?" that shows `text` when hovered.
pub fn help_icon(ui: &mut Ui, text: &str) -> Response {
    help_icon_ui(ui, |ui| {
        ui.label(RichText::new(text).size(12.0).color(TEXT2));
    })
}

/// A small "?" that shows what `add` draws when hovered.
pub fn help_icon_ui(ui: &mut Ui, add: impl FnOnce(&mut Ui)) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
    help_glyph(ui.painter(), rect.center(), resp.hovered());
    help_tip(resp, add)
}

/// The "?" in a circle.
fn help_glyph(p: &Painter, c: Pos2, hovered: bool) {
    p.circle(c, 7.5, if hovered { BG3 } else { BG2 }, Stroke::new(1.0, if hovered { G1 } else { LINE2 }));
    p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "?", theme::font_bold(10.0), if hovered { TEXT } else { MUTED });
}

fn help_tip(resp: Response, add: impl FnOnce(&mut Ui)) -> Response {
    resp.on_hover_cursor(egui::CursorIcon::Help).on_hover_ui(|ui| {
        ui.set_max_width(280.0);
        ui.spacing_mut().item_spacing.y = 6.0;
        add(ui);
    })
}

/// A one-line readout of what the settings add up to ("2 tiles · pays 1.28× if all are gems"),
/// in `color` on a recessed strip with an `accent` edge.
pub fn readout(ui: &mut Ui, text: impl Into<String>, color: Color32, accent: Color32) {
    let w = ui.available_width();
    let galley = ui.painter().layout(text.into(), theme::font_semi(11.5), color, w - 26.0);
    let (rect, _) = ui.allocate_exact_size(vec2(w, galley.size().y + 16.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(RADIUS), BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let edge = Rect::from_min_max(pos2(rect.left() + 1.0, rect.top() + 7.0), pos2(rect.left() + 3.0, rect.bottom() - 7.0));
    p.rect_filled(edge, CornerRadius::same(1), accent);
    p.galley(pos2(rect.left() + 14.0, rect.center().y - galley.size().y / 2.0), galley, color);
}

/// A row of exactly `height` with its contents centred on it, so a caption lines up with the
/// taller control beside it.
pub fn row(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, height), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_height(height);
        add(ui);
    });
}

/// Dropdowns (egui combo boxes) drawn like the text fields next to them: same height, fill and
/// border. Applies to the rest of `ui`.
pub fn field_combos(ui: &mut Ui) {
    ui.spacing_mut().interact_size.y = FIELD_H;
    ui.spacing_mut().button_padding = vec2(12.0, 8.0);
    let w = &mut ui.visuals_mut().widgets;
    for (state, stroke) in [(&mut w.inactive, LINE), (&mut w.hovered, LINE2), (&mut w.active, G1), (&mut w.open, LINE2)] {
        state.weak_bg_fill = BG;
        state.bg_fill = BG;
        state.bg_stroke = Stroke::new(1.0, stroke);
        state.corner_radius = CornerRadius::same(RADIUS);
        state.expansion = 0.0;
    }
}

/// Quiet text inside a card, for games with nothing to set.
pub fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED));
}

/// Paints `text` no wider than `max_w`, shrinking the font down to 75% before it gives up and clips.
pub fn fit_text(p: &Painter, pos: Pos2, align: Align2, text: &str, font: FontId, color: Color32, max_w: f32) {
    let mut font = font;
    let min = font.size * 0.75;
    loop {
        let g = p.layout_no_wrap(text.to_string(), font.clone(), color);
        if g.size().x <= max_w || font.size <= min {
            let r = align.anchor_size(pos, g.size());
            p.galley(r.min, g, color);
            return;
        }
        font.size -= 0.5;
    }
}

// ----------------------------------------------------------------- backgrounds

/// Faint dot grid, like a drafting board.
pub fn dot_grid(ui: &Ui, rect: Rect) {
    let p = ui.painter_at(rect);
    p.rect_filled(rect, CornerRadius::ZERO, BG);
    let step = 28.0;
    let c = alpha(Color32::WHITE, 7);
    let mut y = rect.top() + step / 2.0;
    while y < rect.bottom() {
        let mut x = rect.left() + step / 2.0;
        while x < rect.right() {
            p.circle_filled(pos2(x, y), 0.9, c);
            x += step;
        }
        y += step;
    }
}

/// 2×2 texture sampled bilinearly: a smooth four-corner gradient with egui's anti-aliased rounded edges.
fn gradient_texture(ctx: &egui::Context, corners: [Color32; 4]) -> TextureHandle {
    let id = egui::Id::new(("fb-gradient", corners.map(|c| c.to_array())));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let t = ctx.load_texture("gradient", egui::ColorImage::new([2, 2], corners.to_vec()), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Rounded rectangle with a gradient between its corners: top-left, top-right, bottom-left, bottom-right.
pub fn gradient_rect(p: &Painter, rect: Rect, radius: u8, corners: [Color32; 4]) {
    let tex = gradient_texture(p.ctx(), corners);
    // texel centres sit at 0.25 / 0.75, so this maps the corners exactly onto the rect
    let uv = Rect::from_min_max(pos2(0.25, 0.25), pos2(0.75, 0.75));
    p.add(egui::epaint::RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE).with_texture(tex.id(), uv));
}

fn glow_texture(ctx: &egui::Context) -> TextureHandle {
    let id = egui::Id::new("fb-glow");
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let n = 96;
    let mut px = Vec::with_capacity(n * n);
    for y in 0..n {
        for x in 0..n {
            let dx = (x as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let dy = (y as f32 + 0.5) / n as f32 * 2.0 - 1.0;
            let d = (dx * dx + dy * dy).sqrt().min(1.0);
            let k = 1.0 - d;
            px.push(Color32::from_white_alpha((k * k * (3.0 - 2.0 * k) * 255.0) as u8));
        }
    }
    let t = ctx.load_texture("glow", egui::ColorImage::new([n, n], px), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// Soft radial light. `color` carries the strength in its alpha.
pub fn glow(p: &Painter, center: Pos2, radius: f32, color: Color32) {
    let tex = glow_texture(p.ctx());
    let rect = Rect::from_center_size(center, Vec2::splat(radius * 2.0));
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    p.add(egui::epaint::RectShape::filled(rect, CornerRadius::ZERO, color).with_texture(tex.id(), uv));
}

/// A small label on a game tile ("Edge: 1%"), its `align` corner at `at`. Returns where it went.
pub fn tile_badge(p: &Painter, at: Pos2, align: Align2, text: &str, fill: Color32, fg: Color32) -> Rect {
    let g = p.layout_no_wrap(text.to_string(), theme::font_semi(10.0), fg);
    let r = align.anchor_size(at, g.size() + vec2(12.0, 6.0));
    p.rect_filled(r, CornerRadius::same(5), fill);
    p.galley(r.min + vec2(6.0, 3.0), g, fg);
    r
}

/// Drop shadow under a rounded rect.
pub fn shadow(p: &Painter, rect: Rect, radius: u8, offset_y: f32, blur: f32, strength: u8) {
    let s = egui::epaint::Shadow { offset: [0, offset_y as i8], blur: blur as u8, spread: 0, color: Color32::from_black_alpha(strength) };
    p.add(s.as_shape(rect, CornerRadius::same(radius)));
}

/// Wordmark: "FASTBET" in a red-to-yellow gradient, letters `height` points tall.
/// Its glow spills past the space it takes.
pub fn wordmark(ui: &mut Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(wordmark_size(height), Sense::hover());
    paint_wordmark_in(ui.painter(), rect);
}

/// The space [`wordmark`] takes, glow left out.
pub fn wordmark_size(height: f32) -> Vec2 {
    let glow = height * logos::WORDMARK_GLOW;
    vec2((height + 2.0 * glow) * logos::wordmark_aspect() - 2.0 * glow, height)
}

/// The wordmark's letters filling `rect` (one of [`wordmark_size`]), its glow around it.
pub fn paint_wordmark_in(p: &Painter, rect: Rect) {
    let glow = rect.height() * logos::WORDMARK_GLOW;
    logos::paint_wordmark(p, rect.expand(glow).round_to_pixels(p.ctx().pixels_per_point()));
}

// ----------------------------------------------------------------- pills

pub fn pill(ui: &mut Ui, text: impl Into<String>, color: Color32) -> Response {
    Frame::new()
        .fill(alpha(color, 22))
        .stroke(Stroke::new(1.0, alpha(color, 60)))
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| ui.label(semi(text, 11.0, color)))
        .inner
}

pub fn pill_button(ui: &mut Ui, text: impl Into<String>, color: Color32, on: bool) -> Response {
    let text: String = text.into();
    let galley = ui.painter().layout_no_wrap(text.clone(), theme::font_semi(10.5), color);
    let size = galley.size() + vec2(18.0, 7.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let (fill, stroke, fg) = if on {
            (alpha(color, if resp.hovered() { 60 } else { 36 }), alpha(color, 150), color)
        } else {
            (if resp.hovered() { BG3 } else { BG2 }, LINE2, MUTED)
        };
        p.rect(rect, CornerRadius::same(20), fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
        p.text(rect.center(), Align2::CENTER_CENTER, text, theme::font_semi(10.5), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Connection status: a coloured dot and a word, on a faint tinted pill.
pub fn status_dot(ui: &mut Ui, color: Color32, text: &str, pulse: bool) -> Response {
    let font = theme::font_semi(11.5);
    let tw = ui.painter().layout_no_wrap(text.to_string(), font.clone(), color).size().x;
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + 32.0, 26.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(13), alpha(color, 20));
    let c = pos2(rect.left() + 13.0, rect.center().y);
    if pulse {
        let t = ui.input(|i| i.time) as f32;
        let k = (t * 3.0).sin() * 0.5 + 0.5;
        p.circle_filled(c, 3.5 + 3.0 * k, alpha(color, (60.0 * (1.0 - k)) as u8));
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(40));
    } else {
        p.circle_filled(c, 6.0, alpha(color, 40));
    }
    p.circle_filled(c, 3.5, color);
    p.text(pos2(rect.left() + 23.0, rect.center().y), Align2::LEFT_CENTER, text, font, color);
    resp
}

/// Header chip: tiny icon and a value, e.g. the session timer.
pub fn meta_chip(ui: &mut Ui, icon_name: &str, text: &str, color: Color32) -> Response {
    let font = theme::font_semi(12.0);
    let tw = ui.painter().layout_no_wrap(text.to_string(), font.clone(), color).size().x;
    let has_icon = !icon_name.is_empty();
    let w = tw + if has_icon { 38.0 } else { 20.0 };
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, CornerRadius::same(RADIUS), BG1);
    let mut x = rect.left() + 10.0;
    if has_icon {
        let ir = Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(12.0, 12.0));
        icon(p, ir, icon_name, MUTED);
        x += 18.0;
    }
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, text, font, color);
    resp
}

/// Keyboard key hint, e.g. "Space".
pub fn keycap(p: &Painter, rect: Rect, text: &str, color: Color32) {
    p.rect(rect, CornerRadius::same(4), alpha(color, 26), Stroke::new(1.0, alpha(color, 70)), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, theme::font_semi(10.0), color);
}

// ----------------------------------------------------------------- buttons

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Primary,
    Danger,
    Warn,
    Ghost,
}

/// Solid button. `width` of None sizes to the text.
pub fn button(ui: &mut Ui, text: &str, kind: Btn, enabled: bool, width: Option<f32>) -> Response {
    button_key(ui, text, None, kind, enabled, width)
}

/// Button with an optional keyboard hint drawn as a keycap after the label.
pub fn button_key(ui: &mut Ui, text: &str, key: Option<&str>, kind: Btn, enabled: bool, width: Option<f32>) -> Response {
    button_sized(ui, text, key, kind, enabled, width, FIELD_H)
}

/// Height of [`action_button`].
pub const ACTION_H: f32 = 44.0;

/// The page's main action (Bet, Start, Cash out): a taller button with a larger label.
pub fn action_button(ui: &mut Ui, text: &str, key: Option<&str>, kind: Btn, enabled: bool, width: f32) -> Response {
    button_sized(ui, text, key, kind, enabled, Some(width), ACTION_H)
}

fn button_sized(ui: &mut Ui, text: &str, key: Option<&str>, kind: Btn, enabled: bool, width: Option<f32>, height: f32) -> Response {
    let font = theme::font_bold(if height > FIELD_H { 14.5 } else { 13.0 });
    let measure = |t: &str, f: FontId| ui.painter().layout_no_wrap(t.to_string(), f, TEXT).size().x;
    let text_w = measure(text, font.clone());
    let key_w = key.map(|k| measure(k, theme::font_semi(10.0)) + 12.0).unwrap_or(0.0);
    let content_w = text_w + if key.is_some() { key_w + 8.0 } else { 0.0 };
    let size = vec2(width.unwrap_or(content_w + 32.0).max(content_w + 20.0), height);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let hovered = resp.hovered() && enabled;
    let pressed = resp.is_pointer_button_down_on() && enabled;
    let (fill, fg, stroke) = match kind {
        Btn::Primary => (G1, INK, Stroke::NONE),
        Btn::Danger => (RED, Color32::from_rgb(0x2a, 0x08, 0x10), Stroke::NONE),
        Btn::Warn => (GOLD, Color32::from_rgb(0x2a, 0x20, 0x00), Stroke::NONE),
        Btn::Ghost => (BG2, TEXT, Stroke::new(1.0, if hovered { LINE2 } else { LINE })),
    };
    let fill = match (kind, hovered, pressed) {
        (_, _, true) => mix(fill, Color32::BLACK, 0.08),
        (Btn::Ghost, true, _) => BG3,
        (_, true, _) => mix(fill, Color32::WHITE, 0.12),
        _ => fill,
    };
    let (fill, fg) = if enabled { (fill, fg) } else { (BG3, DIM) };
    let p = ui.painter();
    if enabled && kind != Btn::Ghost {
        shadow(p, rect, RADIUS, 2.0, 10.0, 60);
    }
    p.rect(rect, CornerRadius::same(RADIUS), fill, stroke, StrokeKind::Inside);
    if enabled && kind != Btn::Ghost {
        // top highlight gives the solid buttons a little depth
        p.hline(rect.x_range().shrink(6.0), rect.top() + 1.0, Stroke::new(1.0, alpha(Color32::WHITE, 50)));
    }
    let x = rect.center().x - content_w / 2.0;
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, text, font, fg);
    if let Some(k) = key {
        let kr = Rect::from_min_size(pos2(x + content_w - key_w, rect.center().y - 9.0), vec2(key_w, 18.0));
        keycap(p, kr, k, fg);
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Small button next to fields (½, 2×, quick targets).
pub fn chip_button_sized(ui: &mut Ui, text: &str, on: bool, enabled: bool, size: Vec2) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    let (fill, stroke, fg) = if on {
        (alpha(G1, 26), Stroke::new(1.0, alpha(G1, 120)), G1)
    } else if resp.hovered() && enabled {
        (BG3, Stroke::NONE, TEXT)
    } else {
        (BG2, Stroke::NONE, if enabled { TEXT2 } else { DIM })
    };
    p.rect(rect, CornerRadius::same(RADIUS_SM), fill, stroke, StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, text, theme::font_semi(11.5), fg);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// Flat glyph button with hover background.
pub fn icon_button(ui: &mut Ui, text: &str, color: Color32, enabled: bool, tip: &str) -> Response {
    let size = vec2(26.0, 26.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    if resp.hovered() && enabled {
        p.rect_filled(rect, CornerRadius::same(RADIUS_SM), alpha(Color32::WHITE, 14));
    }
    p.text(rect.center(), Align2::CENTER_CENTER, text, theme::font(14.0), if enabled { color } else { DIM });
    let resp = if enabled { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
    if tip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tip)
    }
}

/// Square button with a painted icon (see [`icon`]).
/// Round button with an icon, lit while `on` (its menu is open).
pub fn round_icon_button(ui: &mut Ui, icon_name: &str, on: bool, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() || on, 0.1);
    let p = ui.painter();
    p.circle_filled(rect.center(), 17.0, mix(BG1, BG2, t));
    p.circle_stroke(rect.center(), 17.0, Stroke::new(1.0, if on { alpha(G1, 170) } else { mix(LINE, LINE2, t) }));
    icon(p, Rect::from_center_size(rect.center(), vec2(15.0, 15.0)), icon_name, mix(MUTED, TEXT, t));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if on {
        resp
    } else {
        resp.on_hover_text(tip)
    }
}

pub fn glyph_button(ui: &mut Ui, icon_name: &str, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered();
    if hovered {
        p.rect_filled(rect, CornerRadius::same(RADIUS_SM), alpha(Color32::WHITE, 12));
    }
    icon(p, Rect::from_center_size(rect.center(), vec2(13.0, 13.0)), icon_name, if hovered { TEXT } else { MUTED });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip)
}

/// Left rail navigation button: icon only, accent tint and edge marker when active. `draw`
/// paints the icon in the colour given, and gets the hover (0–1, eased) to animate it.
pub fn rail_button(ui: &mut Ui, on: bool, tip: &str, draw: impl FnOnce(&egui::Painter, Rect, Color32, f32)) -> Response {
    let rail_left = ui.max_rect().left();
    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.1);
    let p = ui.painter();
    if on {
        p.rect_filled(rect, CornerRadius::same(10), alpha(G1, 26));
        let bar = Rect::from_min_max(pos2(rail_left, rect.top() + 10.0), pos2(rail_left + 3.0, rect.bottom() - 10.0));
        p.rect_filled(bar, CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, G1);
    } else if t > 0.0 {
        p.rect_filled(rect, CornerRadius::same(10), alpha(Color32::WHITE, (10.0 * t) as u8));
    }
    let color = if on { G1 } else { mix(MUTED, TEXT, t) };
    draw(p, rect.shrink(11.0), color, t);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(tip)
}

// ----------------------------------------------------------------- sections

/// Collapsible settings card: a header row with title, optional status dot and
/// chevron; the body sits inside the same card under a hairline.
pub fn section(ui: &mut Ui, width: f32, title: &str, active: bool, open: &mut bool, add: impl FnOnce(&mut Ui)) {
    fold(ui, width, title, "", active, open, add);
}

/// The surface every settings card sits on: a step lighter than the page, no outline.
fn card_frame(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui, f32)) {
    ui.set_max_width(width);
    Frame::new()
        .fill(BG1)
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::ZERO)
        .show(ui, |ui| {
            let inner_w = width - 2.0;
            ui.set_width(inner_w);
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui, inner_w);
        });
}

/// A card's fields, under a hairline.
fn card_body(ui: &mut Ui, inner_w: f32, add: impl FnOnce(&mut Ui)) {
    let (sep, _) = ui.allocate_exact_size(vec2(inner_w, 1.0), Sense::hover());
    ui.painter().hline(sep.x_range(), sep.center().y, Stroke::new(1.0, LINE));
    Frame::new().inner_margin(Margin { left: 14, right: 14, top: 14, bottom: 14 }).show(ui, |ui| {
        let w = inner_w - 28.0;
        ui.set_max_width(w);
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 10.0;
        add(ui);
    });
}

/// A settings card that is always open: a title (with a "?" that explains it, and a value on the
/// right) over its fields.
pub fn card(ui: &mut Ui, width: f32, title: &str, help: Option<&str>, right: Option<RichText>, add: impl FnOnce(&mut Ui)) {
    card_frame(ui, width, |ui, inner_w| {
        let (rect, _) = ui.allocate_exact_size(vec2(inner_w, 42.0), Sense::hover());
        let font = theme::font_semi(13.0);
        let tw = ui.painter().layout_no_wrap(title.to_string(), font.clone(), TEXT).size().x;
        ui.painter().text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, title, font, TEXT);
        if let Some(h) = help {
            let r = Rect::from_center_size(pos2(rect.left() + 14.0 + tw + 14.0, rect.center().y), vec2(16.0, 16.0));
            let resp = ui.interact(r, ui.id().with(("card-help", title)), Sense::hover());
            help_glyph(ui.painter(), r.center(), resp.hovered());
            help_tip(resp, |ui| {
                ui.label(RichText::new(h).size(12.0).color(TEXT2));
            });
        }
        if let Some(t) = right {
            let mut rui = ui.new_child(UiBuilder::new().max_rect(rect.shrink2(vec2(14.0, 0.0))).layout(Layout::right_to_left(Align::Center)));
            rui.label(t);
        }
        card_body(ui, inner_w, add);
    });
}

thread_local! {
    /// Set while a [`fold_list`] draws its rows: whether the next fold is its first.
    static FOLD_LIST: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Settings that fold away, as the rows of one card with hairlines between them. The folds
/// `add` draws (it gets the card's inner width) become its rows.
pub fn fold_list(ui: &mut Ui, width: f32, add: impl FnOnce(&mut Ui, f32)) {
    card_frame(ui, width, |ui, inner_w| {
        let outer = FOLD_LIST.replace(Some(true));
        add(ui, inner_w);
        FOLD_LIST.set(outer);
    });
}

/// A settings card that folds away. Its header says what is set (`summary`, green while
/// `active`), so a closed card still shows its state. Inside a [`fold_list`] it is one of the
/// list's rows instead of a card of its own.
pub fn fold(ui: &mut Ui, width: f32, title: &str, summary: &str, active: bool, open: &mut bool, add: impl FnOnce(&mut Ui)) {
    match FOLD_LIST.get() {
        Some(first) => {
            FOLD_LIST.set(Some(false));
            fold_row(ui, width, Some(!first), title, summary, active, open, add);
        }
        None => card_frame(ui, width, |ui, inner_w| fold_row(ui, inner_w, None, title, summary, active, open, add)),
    }
}

/// A fold's header row and, while open, its fields. `divided`: None on a card of its own, in a
/// list whether a hairline goes above it (every row after the first).
#[allow(clippy::too_many_arguments)]
fn fold_row(ui: &mut Ui, inner_w: f32, divided: Option<bool>, title: &str, summary: &str, active: bool, open: &mut bool, add: impl FnOnce(&mut Ui)) {
    let in_list = divided.is_some();
    if divided == Some(true) {
        let (sep, _) = ui.allocate_exact_size(vec2(inner_w, 1.0), Sense::hover());
        ui.painter().hline(sep.x_range().shrink(14.0), sep.center().y, Stroke::new(1.0, LINE));
    }
    let (rect, resp) = ui.allocate_exact_size(vec2(inner_w, 42.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *open = !*open;
    }
    let t = ui.ctx().animate_bool_with_time(resp.id.with("open"), *open, 0.12);
    let p = ui.painter();
    if resp.hovered() {
        // a list's rows light up inside its rounded edges
        let (r, cr) = if in_list {
            (rect.shrink2(vec2(6.0, 3.0)), CornerRadius::same(RADIUS))
        } else if *open {
            (rect.shrink(1.0), CornerRadius { nw: 11, ne: 11, sw: 0, se: 0 })
        } else {
            (rect.shrink(1.0), CornerRadius::same(11))
        };
        p.rect_filled(r, cr, alpha(Color32::WHITE, 5));
    }
    let font = theme::font_semi(13.0);
    let title_w = p.layout_no_wrap(title.to_string(), font.clone(), TEXT).size().x;
    p.text(pos2(rect.left() + 14.0, rect.center().y), Align2::LEFT_CENTER, title, font, if *open || resp.hovered() { TEXT } else { TEXT2 });
    // chevron flips between down (closed) and up (open)
    let c = pos2(rect.right() - 18.0, rect.center().y);
    let dy = 2.2 - 4.4 * t;
    let pts = [c + vec2(-4.5, -dy), c + vec2(0.0, dy), c + vec2(4.5, -dy)];
    p.add(egui::Shape::line(pts.to_vec(), Stroke::new(1.6, if resp.hovered() { TEXT2 } else { MUTED })));
    if summary.is_empty() {
        if active {
            let c = pos2(rect.left() + 14.0 + title_w + 10.0, rect.center().y);
            p.circle_filled(c, 5.5, alpha(G1, 40));
            p.circle_filled(c, 3.0, G1);
        }
    } else {
        // the state, right-aligned before the chevron, cut short rather than overlap the title
        let room = rect.width() - title_w - 14.0 - 40.0 - 16.0;
        let color = if active { G2 } else { DIM };
        fit_text(p, pos2(rect.right() - 34.0, rect.center().y), Align2::RIGHT_CENTER, summary, theme::font_semi(11.5), color, room.max(40.0));
    }
    if *open && in_list {
        // a row's fields hang under its header; the next row's hairline closes them
        Frame::new().inner_margin(Margin { left: 14, right: 14, top: 2, bottom: 14 }).show(ui, |ui| {
            let w = inner_w - 28.0;
            ui.set_max_width(w);
            ui.set_width(w);
            ui.spacing_mut().item_spacing.y = 10.0;
            add(ui);
        });
    } else if *open {
        card_body(ui, inner_w, add);
    }
}

// ----------------------------------------------------------------- fields

pub fn field(ui: &mut Ui, caption_text: &str, value: &mut String, hint: &str, suffix: Option<&str>, enabled: bool) -> Response {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        caption(ui, caption_text);
        text_box(ui, value, hint, suffix, enabled, false)
    })
    .inner
}

/// A whole-number [`field`] in `range`. What is typed stays as typed while the box has focus
/// (so "13" can be typed past "1"), and the box shows the value again once left. ↑ / ↓ step
/// it, held down they repeat; with `keys_anywhere` they also work while no other box takes
/// typing. Returns true when the value changed.
pub fn int_field(ui: &mut Ui, caption_text: &str, value: &mut u32, range: (u32, u32), enabled: bool, keys_anywhere: bool) -> bool {
    let id = ui.make_persistent_id(("int-field", caption_text));
    let before = *value;
    let mut text = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_else(|| value.to_string());
    let r = field(ui, caption_text, &mut text, &range.0.to_string(), Some(&format!("{}–{}", range.0, range.1)), enabled);
    if r.changed() {
        if let Ok(n) = text.trim().parse::<u32>() {
            *value = n.clamp(range.0, range.1);
        }
    }
    let typing_elsewhere = !r.has_focus() && ui.ctx().egui_wants_keyboard_input();
    if enabled && (r.has_focus() || (keys_anywhere && !typing_elsewhere)) {
        let step = ui.input(|i| i.num_presses(egui::Key::ArrowUp) as i64 - i.num_presses(egui::Key::ArrowDown) as i64);
        if step != 0 {
            *value = (*value as i64 + step).clamp(range.0 as i64, range.1 as i64) as u32;
            text = value.to_string();
        }
    }
    if r.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, text));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    *value != before
}

/// Single-line input, recessed into the card, with an optional unit on the right.
pub fn text_box(ui: &mut Ui, value: &mut String, hint: &str, suffix: Option<&str>, enabled: bool, password: bool) -> Response {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, FIELD_H), Sense::hover());
    let suffix_galley = suffix.map(|s| ui.painter().layout_no_wrap(s.to_string(), theme::font_semi(11.0), MUTED));
    let suffix_w = suffix_galley.as_ref().map(|g| g.size().x + 10.0).unwrap_or(0.0);
    let hovered = enabled && ui.rect_contains_pointer(rect);
    ui.painter().rect(
        rect,
        CornerRadius::same(RADIUS),
        if enabled { BG } else { alpha(BG, 120) },
        Stroke::new(1.0, if hovered { LINE2 } else { LINE }),
        StrokeKind::Inside,
    );
    let inner = Rect::from_min_max(pos2(rect.left() + 12.0, rect.top()), pos2(rect.right() - 12.0 - suffix_w, rect.bottom()));
    let edit = TextEdit::singleline(value)
        .frame(Frame::NONE)
        .hint_text(RichText::new(hint).color(DIM))
        .font(theme::font_semi(13.5))
        .text_color(if enabled { TEXT } else { MUTED })
        .password(password)
        .margin(Margin::ZERO)
        .desired_width(inner.width());
    let r = ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)), |ui| ui.add_enabled(enabled, edit)).inner;
    if let Some(g) = suffix_galley {
        let pos = pos2(rect.right() - 12.0 - g.size().x, rect.center().y - g.size().y / 2.0);
        ui.painter().galley(pos, g, MUTED);
    }
    if r.has_focus() {
        let p = ui.painter();
        p.rect_stroke(rect.expand(2.0), CornerRadius::same(RADIUS + 2), Stroke::new(2.0, alpha(G1, 40)), StrokeKind::Inside);
        p.rect_stroke(rect, CornerRadius::same(RADIUS), Stroke::new(1.0, alpha(G1, 170)), StrokeKind::Inside);
    }
    r
}

/// Segmented control filling the available width. Returns the newly selected index when it changed.
pub fn segmented(ui: &mut Ui, items: &[&str], selected: usize, enabled: bool) -> Option<usize> {
    let mut changed = None;
    let h = 32.0;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(RADIUS), BG, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let n = items.len().max(1) as f32;
    let seg_w = (w - 6.0) / n;
    // the highlight slides to the selected segment
    let x = ui.ctx().animate_value_with_time(resp.id.with("seg-x"), selected as f32, 0.12);
    let sel = Rect::from_min_size(pos2(rect.left() + 3.0 + seg_w * x, rect.top() + 3.0), vec2(seg_w, h - 6.0));
    p.rect(sel, CornerRadius::same(RADIUS_SM), if enabled { BG3 } else { BG2 }, Stroke::new(1.0, LINE2), StrokeKind::Inside);
    for (i, item) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 3.0 + seg_w * i as f32, rect.top() + 3.0), vec2(seg_w, h - 6.0));
        let id = resp.id.with(("seg", i));
        let sr = ui.interact(r, id, if enabled { Sense::click() } else { Sense::hover() });
        let on = i == selected;
        let color = if !enabled {
            DIM
        } else if on {
            TEXT
        } else if sr.hovered() {
            TEXT2
        } else {
            MUTED
        };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *item, theme::font_semi(12.0), color);
        if sr.clicked() && enabled && !on {
            changed = Some(i);
        }
        if enabled && !on {
            sr.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }
    changed
}

/// A dropdown drawn like the text fields. It opens a list under itself with a tick by the
/// picked entry (and a search box on top when the list is long). Returns the index clicked.
pub fn select(ui: &mut Ui, id_salt: &str, items: &[String], selected: usize, enabled: bool) -> Option<usize> {
    select_opt(ui, id_salt, items, Some(selected), "–", enabled)
}

/// [`select`] that may have nothing picked: it then shows `placeholder`.
pub fn select_opt(ui: &mut Ui, id_salt: &str, items: &[String], selected: Option<usize>, placeholder: &str, enabled: bool) -> Option<usize> {
    const ROW: f32 = 30.0;
    let id = ui.make_persistent_id(id_salt);
    let (open_id, search_id, fresh_id) = (id.with("open"), id.with("search"), id.with("fresh"));
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, FIELD_H), if enabled { Sense::click() } else { Sense::hover() });
    let mut open = enabled && ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    if resp.clicked() {
        open = !open;
        ui.data_mut(|d| {
            d.insert_temp(search_id, String::new());
            d.insert_temp(fresh_id, open);
        });
    }
    let hovered = enabled && resp.hovered();
    let p = ui.painter();
    let stroke = if open { alpha(G1, 170) } else if hovered { LINE2 } else { LINE };
    p.rect(rect, CornerRadius::same(RADIUS), if enabled { BG } else { alpha(BG, 120) }, Stroke::new(1.0, stroke), StrokeKind::Inside);
    let (text, color) = match selected.and_then(|i| items.get(i)) {
        Some(t) => (t.as_str(), if enabled { TEXT } else { MUTED }),
        None => (placeholder, if enabled { TEXT2 } else { DIM }),
    };
    fit_text(p, pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, text, theme::font_semi(13.0), color, rect.width() - 44.0);
    let c = pos2(rect.right() - 16.0, rect.center().y);
    let dir = if open { -1.0 } else { 1.0 };
    let color = if enabled { MUTED } else { DIM };
    p.line_segment([c + vec2(-4.0, -2.0 * dir), c + vec2(0.0, 2.0 * dir)], Stroke::new(1.6, color));
    p.line_segment([c + vec2(0.0, 2.0 * dir), c + vec2(4.0, -2.0 * dir)], Stroke::new(1.6, color));
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    if !open {
        ui.data_mut(|d| d.insert_temp(open_id, false));
        return None;
    }

    let mut picked = None;
    let mut search: String = ui.data(|d| d.get_temp(search_id)).unwrap_or_default();
    let fresh = ui.data_mut(|d| d.remove_temp::<bool>(fresh_id)).unwrap_or(false);
    let searchable = items.len() > 10;
    // as wide as the field, or as its longest entry
    let widest = items.iter().map(|t| ui.painter().layout_no_wrap(t.clone(), theme::font_semi(12.5), TEXT).size().x).fold(0.0, f32::max);
    let w = w.max(widest + 64.0).min(360.0);
    let area = egui::Area::new(id.with("list")).fixed_pos(pos2(rect.left(), rect.bottom() + 4.0)).order(egui::Order::Foreground).show(ui.ctx(), |ui| {
        Frame::new()
            .fill(BG1)
            .stroke(Stroke::new(1.0, LINE2))
            .corner_radius(CornerRadius::same(RADIUS))
            .inner_margin(Margin::same(6))
            .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(160) })
            .show(ui, |ui| {
                ui.set_width(w - 12.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                if searchable {
                    let sr = text_box(ui, &mut search, "Search…", None, true, false);
                    if fresh {
                        sr.request_focus();
                    }
                    if sr.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        let q = search.trim().to_lowercase();
                        picked = (0..items.len()).find(|&i| items[i].to_lowercase().contains(&q));
                    }
                }
                let q = search.trim().to_lowercase();
                let shown: Vec<usize> = (0..items.len()).filter(|&i| q.is_empty() || items[i].to_lowercase().contains(&q)).collect();
                egui::ScrollArea::vertical().max_height(ROW * 8.5).auto_shrink([false, true]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let rw = ui.available_width();
                    for &i in &shown {
                        let (r, rr) = ui.allocate_exact_size(vec2(rw, ROW), Sense::click());
                        if fresh && Some(i) == selected {
                            ui.scroll_to_rect(r, Some(Align::Center));
                        }
                        let on = Some(i) == selected;
                        let p = ui.painter();
                        if rr.hovered() {
                            p.rect_filled(r, CornerRadius::same(RADIUS_SM), BG3);
                        }
                        fit_text(p, pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, &items[i], theme::font_semi(12.5), if on || rr.hovered() { TEXT } else { TEXT2 }, rw - 40.0);
                        if on {
                            let t = pos2(r.right() - 18.0, r.center().y);
                            p.line_segment([t + vec2(-5.0, 0.0), t + vec2(-1.5, 3.5)], Stroke::new(1.8, G1));
                            p.line_segment([t + vec2(-1.5, 3.5), t + vec2(5.0, -4.0)], Stroke::new(1.8, G1));
                        }
                        if rr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            picked = Some(i);
                        }
                    }
                    if shown.is_empty() {
                        let (r, _) = ui.allocate_exact_size(vec2(rw, ROW), Sense::hover());
                        ui.painter().text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, "No match", theme::font_semi(12.5), MUTED);
                    }
                });
            });
    });
    // a pick, a click outside or Escape closes it
    let list = area.response.rect;
    let outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !list.contains(p) && !rect.contains(p)));
    if picked.is_some() || outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        open = false;
    }
    ui.data_mut(|d| {
        d.insert_temp(open_id, open);
        d.insert_temp(search_id, search);
    });
    picked
}

/// Page tabs like "Manual Bet | Automatic". Returns the new index on change.
pub fn tab_bar(ui: &mut Ui, items: &[&str], selected: usize, enabled: bool) -> Option<usize> {
    let mut changed = None;
    let font = theme::font_semi(12.5);
    let seg_w = items.iter().map(|t| ui.painter().layout_no_wrap(t.to_string(), font.clone(), TEXT).size().x + 32.0).fold(0.0, f32::max);
    let total = seg_w * items.len() as f32 + 8.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(total, FIELD_H + 2.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(10), BG1, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let x = ui.ctx().animate_value_with_time(resp.id.with("tab-x"), selected as f32, 0.14);
    let sel = Rect::from_min_size(pos2(rect.left() + 4.0 + seg_w * x, rect.top() + 4.0), vec2(seg_w, rect.height() - 8.0));
    shadow(p, sel, 7, 1.0, 6.0, 70);
    p.rect(sel, CornerRadius::same(7), BG3, Stroke::new(1.0, LINE2), StrokeKind::Inside);
    for (i, item) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(rect.left() + 4.0 + seg_w * i as f32, rect.top() + 4.0), vec2(seg_w, rect.height() - 8.0));
        let tr = ui.interact(r, resp.id.with(("tab", i)), if enabled { Sense::click() } else { Sense::hover() });
        let on = i == selected;
        let color = if on {
            TEXT
        } else if !enabled {
            DIM
        } else if tr.hovered() {
            TEXT2
        } else {
            MUTED
        };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *item, font.clone(), color);
        if tr.clicked() && enabled && !on {
            changed = Some(i);
        }
        if enabled && !on {
            tr.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }
    changed
}

fn icon_tab_w(ui: &Ui, text: &str, compact: bool) -> f32 {
    if compact {
        return 13.0 + 28.0;
    }
    ui.painter().layout_no_wrap(text.to_string(), theme::font_semi(12.5), TEXT).size().x + 14.0 + 7.0 + 28.0
}

/// How wide [`icon_tabs`] is for `items`.
pub fn icon_tabs_width(ui: &Ui, items: &[(&str, &str)], compact: bool) -> f32 {
    items.iter().map(|(_, t)| icon_tab_w(ui, t, compact)).sum::<f32>() + 8.0
}

/// Page tabs with an icon before each label (`(icon, label)`), each as wide as its label; the
/// highlight slides to the selected one. `compact`: icons only, the label on hover. Returns the
/// new index on change.
pub fn icon_tabs(ui: &mut Ui, items: &[(&str, &str)], selected: usize, enabled: bool, compact: bool) -> Option<usize> {
    let mut changed = None;
    let font = theme::font_semi(12.5);
    let widths: Vec<f32> = items.iter().map(|(_, t)| icon_tab_w(ui, t, compact)).collect();
    let (rect, resp) = ui.allocate_exact_size(vec2(icon_tabs_width(ui, items, compact), FIELD_H + 2.0), Sense::hover());
    let lefts: Vec<f32> = widths.iter().scan(rect.left() + 4.0, |x, w| {
        let l = *x;
        *x += w;
        Some(l)
    }).collect();
    let sel = selected.min(items.len().saturating_sub(1));
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(10), BG1, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let x = ui.ctx().animate_value_with_time(resp.id.with("tab-x"), lefts[sel], 0.14);
    let w = ui.ctx().animate_value_with_time(resp.id.with("tab-w"), widths[sel], 0.14);
    let hl = Rect::from_min_size(pos2(x, rect.top() + 4.0), vec2(w, rect.height() - 8.0));
    shadow(p, hl, 7, 1.0, 6.0, 70);
    p.rect(hl, CornerRadius::same(7), BG3, Stroke::new(1.0, LINE2), StrokeKind::Inside);
    for (i, (icon_name, text)) in items.iter().enumerate() {
        let r = Rect::from_min_size(pos2(lefts[i], rect.top() + 4.0), vec2(widths[i], rect.height() - 8.0));
        let tr = ui.interact(r, resp.id.with(("itab", i)), if enabled { Sense::click() } else { Sense::hover() });
        let on = i == sel;
        let color = if on {
            TEXT
        } else if !enabled {
            DIM
        } else if tr.hovered() {
            TEXT2
        } else {
            MUTED
        };
        let p = ui.painter();
        let ir = Rect::from_center_size(pos2(r.left() + 14.0 + 6.5, r.center().y), vec2(13.0, 13.0));
        icon(p, ir, icon_name, if on { G2 } else { color });
        let tr = if compact {
            tr.on_hover_text(*text)
        } else {
            p.text(pos2(ir.right() + 7.0, r.center().y), Align2::LEFT_CENTER, *text, font.clone(), color);
            tr
        };
        if tr.clicked() && enabled && !on {
            changed = Some(i);
        }
        if enabled && !on {
            tr.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }
    changed
}

/// A full-width button outlined with a dashed line, for adding things ("+ Add condition").
pub fn dashed_button(ui: &mut Ui, text: &str, height: f32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), height), if enabled { Sense::click() } else { Sense::hover() });
    let hovered = enabled && resp.hovered();
    let p = ui.painter();
    if hovered {
        p.rect_filled(rect, CornerRadius::same(RADIUS), alpha(G1, 14));
    }
    // the outline: straight edges and quarter circles at the corners
    let rad = RADIUS as f32;
    let r = rect.shrink(0.5);
    let mut pts = vec![];
    for (c, a0) in [
        (pos2(r.right() - rad, r.top() + rad), -90.0f32),
        (pos2(r.right() - rad, r.bottom() - rad), 0.0),
        (pos2(r.left() + rad, r.bottom() - rad), 90.0),
        (pos2(r.left() + rad, r.top() + rad), 180.0),
    ] {
        for k in 0..=6 {
            let a = (a0 + 15.0 * k as f32).to_radians();
            pts.push(pos2(c.x + rad * a.cos(), c.y + rad * a.sin()));
        }
    }
    pts.push(pts[0]);
    let stroke = Stroke::new(1.0, if hovered { alpha(G1, 200) } else { LINE2 });
    p.extend(egui::Shape::dashed_line(&pts, stroke, 5.0, 4.0));
    let color = if !enabled {
        DIM
    } else if hovered {
        G2
    } else {
        TEXT2
    };
    let g = p.layout_no_wrap(text.to_string(), theme::font_semi(12.5), color);
    let total = 12.0 + 7.0 + g.size().x;
    let x = rect.center().x - total / 2.0;
    icon(p, Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(11.0, 11.0)), "plus", color);
    p.galley(pos2(x + 19.0, rect.center().y - g.size().y / 2.0), g, color);
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A small on / off switch. Returns true when clicked (and enabled).
pub fn switch(ui: &mut Ui, on: bool, enabled: bool) -> bool {
    let size = vec2(30.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    let t = ui.ctx().animate_bool(resp.id, on);
    let p = ui.painter();
    let fill = mix(BG3, G1, t * 0.9);
    p.rect(rect, CornerRadius::same(8), if enabled { fill } else { alpha(fill, 120) }, Stroke::new(1.0, mix(LINE2, G1, t)), StrokeKind::Inside);
    let knob = pos2(rect.left() + 8.0 + t * (size.x - 16.0), rect.center().y);
    p.circle_filled(knob, 5.5, mix(MUTED, INK, t));
    if enabled {
        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    resp.clicked() && enabled
}

/// On/off switch with a label to its left.
pub fn toggle(ui: &mut Ui, text: &str, on: &mut bool, enabled: bool) -> bool {
    toggle_help(ui, text, None, on, enabled)
}

/// On/off switch with a label to its left, and a "?" after the label that explains it.
pub fn toggle_help(ui: &mut Ui, text: &str, help: Option<&str>, on: &mut bool, enabled: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(RichText::new(text).size(12.5).color(if enabled { TEXT2 } else { DIM }));
        if let Some(h) = help {
            help_icon(ui, h);
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            let size = vec2(34.0, 18.0);
            let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
            if resp.clicked() && enabled {
                *on = !*on;
                changed = true;
            }
            let t = ui.ctx().animate_bool(resp.id, *on);
            let p = ui.painter();
            let fill = mix(BG3, G1, t * 0.9);
            p.rect(rect, CornerRadius::same(9), fill, Stroke::new(1.0, mix(LINE2, G1, t)), StrokeKind::Inside);
            let knob = pos2(rect.left() + 9.0 + t * (size.x - 18.0), rect.center().y);
            p.circle_filled(knob, 6.5, mix(MUTED, INK, t));
            if enabled {
                resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
        });
    });
    changed
}

/// A checkbox and its label (both click it), laid out in the current row. Returns whether it flipped.
pub fn checkbox(ui: &mut Ui, text: &str, on: &mut bool, enabled: bool) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        check_label(ui, text, on, enabled)
    })
    .inner
}

fn check_label(ui: &mut Ui, text: &str, on: &mut bool, enabled: bool) -> bool {
    let mut changed = false;
    let (rect, resp) = ui.allocate_exact_size(vec2(18.0, 18.0), if enabled { Sense::click() } else { Sense::hover() });
    if resp.clicked() && enabled {
        *on = !*on;
        changed = true;
    }
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(5), if *on { G1 } else { BG }, Stroke::new(1.0, if *on { G1 } else { LINE2 }), StrokeKind::Inside);
    if *on {
        let o = rect.left_top();
        p.add(egui::Shape::line(vec![o + vec2(4.5, 9.0), o + vec2(7.8, 12.3), o + vec2(13.5, 5.5)], Stroke::new(1.9, INK)));
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    let label = ui.add(egui::Label::new(RichText::new(text).size(12.5).color(if enabled { TEXT2 } else { DIM })).sense(Sense::click()));
    if label.clicked() && enabled {
        *on = !*on;
        changed = true;
    }
    changed
}

/// Checkbox row with a number field on the right, like "Change after bets [70]".
pub fn check_field(ui: &mut Ui, text: &str, on: &mut bool, value: &mut String, suffix: Option<&str>, enabled: bool) -> bool {
    check_field_help(ui, text, None, on, value, suffix, enabled)
}

/// [`check_field`] with a "?" after the label that explains it.
pub fn check_field_help(ui: &mut Ui, text: &str, help: Option<&str>, on: &mut bool, value: &mut String, suffix: Option<&str>, enabled: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 9.0;
        changed = check_label(ui, text, on, enabled);
        if let Some(h) = help {
            ui.add_space(-3.0);
            help_icon(ui, h);
        }
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.scope(|ui| {
                ui.set_max_width(88.0);
                ui.set_min_width(88.0);
                let r = text_box(ui, value, "0", suffix, enabled && *on, false);
                changed |= r.changed();
            });
        });
    });
    changed
}

// ----------------------------------------------------------------- stats

pub fn hsep(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, LINE));
}

/// Letter in a tinted rounded square, for list rows without a logo.
pub fn letter_mark(p: &egui::Painter, r: Rect, letter: &str, color: Color32) {
    p.rect_filled(r, CornerRadius::same(6), alpha(color, 40));
    p.text(r.center(), Align2::CENTER_CENTER, letter, theme::font_bold(12.0), color);
}

/// Clickable list row: an icon on the left (`mark` draws it into a 22×22 square), name, value on the right, chevron.
pub fn list_row(ui: &mut Ui, mark: impl FnOnce(&egui::Painter, Rect), name: &str, value: &str, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    if resp.hovered() && enabled {
        p.rect_filled(rect, CornerRadius::ZERO, alpha(Color32::WHITE, 6));
    }
    mark(p, Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(22.0, 22.0)));
    p.text(pos2(rect.left() + 46.0, rect.center().y), Align2::LEFT_CENTER, name, theme::font(12.5), MUTED);
    p.text(pos2(rect.left() + 160.0, rect.center().y), Align2::LEFT_CENTER, value, theme::font_semi(12.5), if enabled { TEXT } else { DIM });
    let c = pos2(rect.right() - 18.0, rect.center().y);
    p.line_segment([c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0)], Stroke::new(1.6, MUTED));
    p.line_segment([c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)], Stroke::new(1.6, MUTED));
    p.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, LINE));
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// One entry of a popup menu: icon and label, highlighted on hover.
pub fn menu_item(ui: &mut Ui, icon_name: &str, text: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    let p = ui.painter();
    let hot = resp.hovered();
    if hot {
        p.rect_filled(rect, CornerRadius::same(8), alpha(Color32::WHITE, 8));
    }
    let color = if hot { TEXT } else { TEXT2 };
    icon(p, Rect::from_center_size(pos2(rect.left() + 18.0, rect.center().y), vec2(14.0, 14.0)), icon_name, color);
    p.text(pos2(rect.left() + 36.0, rect.center().y), Align2::LEFT_CENTER, text, theme::font_semi(12.5), color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Originals: a die tipped onto its corner, five pips up. `roll` (0–1, the hover) tips it further.
pub fn paint_die(p: &egui::Painter, r: Rect, color: Color32, roll: f32) {
    let c = r.center();
    let side = r.width() * 0.8;
    let angle = -0.26 - 0.45 * roll;
    let body = Rect::from_center_size(c, vec2(side, side));
    p.add(egui::epaint::RectShape::new(body, CornerRadius::same((side * 0.24) as u8), alpha(color, 40), Stroke::new(1.6, color), StrokeKind::Middle).with_angle(angle));
    let turn = egui::emath::Rot2::from_angle(angle);
    let k = side * 0.26;
    for (x, y) in [(0.0, 0.0), (-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
        p.circle_filled(c + turn * vec2(x * k, y * k), side * 0.095, color);
    }
}

/// Slots: a slot machine, three reels in its window and the lever at its side. `pull` (0–1, the
/// hover) pulls the lever down.
pub fn paint_slot_machine(p: &egui::Painter, r: Rect, color: Color32, pull: f32) {
    let s = Stroke::new(1.6, color);
    let at = |fx: f32, fy: f32| pos2(r.left() + r.width() * fx, r.top() + r.height() * fy);
    p.rect(Rect::from_min_max(at(0.0, 0.1), at(0.74, 1.0)), CornerRadius::same(3), alpha(color, 40), s, StrokeKind::Middle);
    // the window, a symbol on each reel
    p.rect(Rect::from_min_max(at(0.13, 0.3), at(0.61, 0.64)), CornerRadius::same(1), alpha(color, 30), Stroke::new(1.2, color), StrokeKind::Middle);
    for fx in [0.21, 0.37, 0.53] {
        p.circle_filled(at(fx, 0.47), r.width() * 0.06, color);
    }
    // the payout tray
    p.line_segment([at(0.22, 0.82), at(0.52, 0.82)], s);
    // the lever: out of the side, up to its knob
    let knob = 0.12 + 0.38 * pull;
    p.add(egui::Shape::line(vec![at(0.74, 0.72), at(0.92, 0.72), at(0.92, knob)], s));
    p.circle_filled(at(0.92, knob), r.width() * 0.11, color);
}

/// Rewards: a gift box with its ribbon and bow. `open` (0–1, the hover) lifts the lid.
pub fn paint_gift(p: &egui::Painter, r: Rect, color: Color32, open: f32) {
    let s = Stroke::new(1.6, color);
    let at = |fx: f32, fy: f32| pos2(r.left() + r.width() * fx, r.top() + r.height() * fy);
    let lift = 0.1 * open;
    p.rect(Rect::from_min_max(at(0.1, 0.46), at(0.9, 1.0)), CornerRadius::same(2), alpha(color, 40), s, StrokeKind::Middle);
    p.rect(Rect::from_min_max(at(0.0, 0.26 - lift), at(1.0, 0.46 - lift)), CornerRadius::same(2), alpha(color, 40), s, StrokeKind::Middle);
    p.line_segment([at(0.5, 0.46), at(0.5, 1.0)], s);
    p.line_segment([at(0.5, 0.26 - lift), at(0.5, 0.46 - lift)], s);
    // the bow: two loops over the lid
    let knot = at(0.5, 0.26 - lift);
    for dx in [-1.0, 1.0] {
        p.add(egui::Shape::line(vec![knot, at(0.5 + 0.3 * dx, 0.02 - lift), at(0.5 + 0.34 * dx, 0.18 - lift), knot], s));
    }
}

/// King of the Hill: a three-pointed crown.
pub fn paint_crown(p: &egui::Painter, r: Rect, color: Color32) {
    let at = |fx: f32, fy: f32| pos2(r.left() + r.width() * fx, r.top() + r.height() * fy);
    let pts = vec![at(0.06, 0.82), at(0.0, 0.26), at(0.3, 0.52), at(0.5, 0.12), at(0.7, 0.52), at(1.0, 0.26), at(0.94, 0.82)];
    p.add(egui::Shape::convex_polygon(vec![at(0.06, 0.82), at(0.04, 0.6), at(0.96, 0.6), at(0.94, 0.82)], alpha(color, 60), Stroke::NONE));
    p.add(egui::Shape::closed_line(pts, Stroke::new(1.6, color)));
    for fx in [0.0, 0.5, 1.0] {
        p.circle_filled(at(fx, if fx == 0.5 { 0.1 } else { 0.24 }), r.width() * 0.07, color);
    }
}

/// Challenges: a trophy cup on its stand.
pub fn paint_trophy(p: &egui::Painter, r: Rect, color: Color32) {
    let s = Stroke::new(1.6, color);
    let at = |fx: f32, fy: f32| pos2(r.left() + r.width() * fx, r.top() + r.height() * fy);
    let cup = vec![at(0.22, 0.06), at(0.78, 0.06), at(0.74, 0.4), at(0.6, 0.58), at(0.4, 0.58), at(0.26, 0.4)];
    p.add(egui::Shape::convex_polygon(cup.clone(), alpha(color, 40), Stroke::NONE));
    p.add(egui::Shape::closed_line(cup, s));
    // the handles
    p.add(egui::Shape::line(vec![at(0.22, 0.14), at(0.04, 0.14), at(0.08, 0.34), at(0.27, 0.42)], s));
    p.add(egui::Shape::line(vec![at(0.78, 0.14), at(0.96, 0.14), at(0.92, 0.34), at(0.73, 0.42)], s));
    p.line_segment([at(0.5, 0.58), at(0.5, 0.8)], s);
    p.rect(Rect::from_min_max(at(0.26, 0.8), at(0.74, 0.96)), CornerRadius::same(1), alpha(color, 40), s, StrokeKind::Middle);
}

/// Paints a tiny icon by name into `r`.
pub fn icon(p: &egui::Painter, r: Rect, name: &str, color: Color32) {
    let s = Stroke::new(1.6, color);
    let at = |fx: f32, fy: f32| pos2(r.left() + r.width() * fx, r.top() + r.height() * fy);
    match name {
        "dice" => paint_die(p, r, color, 0.0),
        "slots" => paint_slot_machine(p, r, color, 0.0),
        "gift" => paint_gift(p, r, color, 0.0),
        "crown" => paint_crown(p, r, color),
        "trophy" => paint_trophy(p, r, color),
        "coins" => {
            // two coins, the front one over the back one
            for (fx, fy, fr) in [(0.64, 0.36, 0.3), (0.4, 0.62, 0.34)] {
                let c = at(fx, fy);
                let rad = r.width() * fr;
                p.circle_filled(c, rad, BG2);
                p.circle_stroke(c, rad, s);
                p.circle_stroke(c, rad * 0.5, Stroke::new(1.2, color));
            }
        }
        "sun" => {
            let c = r.center();
            p.circle_stroke(c, r.width() * 0.22, s);
            for i in 0..8 {
                let a = std::f32::consts::TAU * i as f32 / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * r.width() * 0.36, c + d * r.width() * 0.5], s);
            }
        }
        "calendar" => {
            p.rect(Rect::from_min_max(at(0.06, 0.16), at(0.94, 0.96)), CornerRadius::same(2), Color32::TRANSPARENT, s, StrokeKind::Middle);
            p.line_segment([at(0.06, 0.38), at(0.94, 0.38)], s);
            for fx in [0.3, 0.7] {
                p.line_segment([at(fx, 0.04), at(fx, 0.24)], s);
            }
            for (fx, fy) in [(0.3, 0.58), (0.5, 0.58), (0.7, 0.58), (0.3, 0.78), (0.5, 0.78)] {
                p.circle_filled(at(fx, fy), r.width() * 0.05, color);
            }
        }
        "arrow-up" => {
            p.line_segment([at(0.5, 0.92), at(0.5, 0.1)], s);
            p.add(egui::Shape::line(vec![at(0.18, 0.42), at(0.5, 0.1), at(0.82, 0.42)], s));
        }
        "bolt" => {
            let pts = [(13.2, 2.0), (4.0, 13.6), (10.6, 13.6), (9.8, 22.0), (20.0, 9.9), (13.3, 9.9)];
            let sc = r.height() / 22.0;
            let o = r.center() - vec2(12.0 * sc, 12.0 * sc);
            let mut mesh = egui::Mesh::default();
            for (x, y) in pts {
                mesh.colored_vertex(o + vec2(x * sc, y * sc), color);
            }
            for (a, b, c) in [(2, 3, 4), (2, 4, 5), (2, 5, 0), (2, 0, 1)] {
                mesh.add_triangle(a, b, c);
            }
            p.add(egui::Shape::mesh(mesh));
        }
        "reels" => {
            // three reel columns with a symbol dot each
            let w = r.width() / 3.6;
            for i in 0..3 {
                let x = r.left() + i as f32 * (w + 1.5);
                let rr = Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height()));
                p.rect_stroke(rr, CornerRadius::same(1), s, StrokeKind::Middle);
                p.circle_filled(pos2(rr.center().x, r.top() + r.height() * (0.3 + 0.2 * i as f32)), 1.4, color);
            }
        }
        "chart" => {
            p.add(egui::Shape::line(vec![at(0.0, 1.0), at(0.33, 0.45), at(0.66, 0.7), at(1.0, 0.0)], s));
        }
        "gear" => {
            // a cog: eight teeth around a ring, with the axle hole
            let c = r.center();
            let (body, tip) = (r.width() * 0.34, r.width() * 0.5);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment([c + d * (body - 1.0), c + d * tip], Stroke::new(s.width * 1.9, color));
            }
            p.circle_stroke(c, body, s);
            p.circle_stroke(c, r.width() * 0.13, s);
        }
        "user" => {
            p.circle_stroke(pos2(r.center().x, r.top() + r.height() * 0.32), r.width() * 0.22, s);
            p.line_segment([pos2(r.left() + 1.0, r.bottom()), pos2(r.center().x - r.width() * 0.3, r.top() + r.height() * 0.62)], s);
            p.line_segment([pos2(r.right() - 1.0, r.bottom()), pos2(r.center().x + r.width() * 0.3, r.top() + r.height() * 0.62)], s);
            p.line_segment([pos2(r.center().x - r.width() * 0.3, r.top() + r.height() * 0.62), pos2(r.center().x + r.width() * 0.3, r.top() + r.height() * 0.62)], s);
        }
        "check" => {
            p.circle_filled(r.center(), r.width() / 2.0, color);
            let c = r.center();
            p.line_segment([c + vec2(-4.0, 0.0), c + vec2(-1.0, 3.0)], Stroke::new(2.0, INK));
            p.line_segment([c + vec2(-1.0, 3.0), c + vec2(4.5, -3.0)], Stroke::new(2.0, INK));
        }
        "cross" => {
            p.circle_filled(r.center(), r.width() / 2.0, color);
            let c = r.center();
            p.line_segment([c + vec2(-3.5, -3.5), c + vec2(3.5, 3.5)], Stroke::new(2.0, INK));
            p.line_segment([c + vec2(3.5, -3.5), c + vec2(-3.5, 3.5)], Stroke::new(2.0, INK));
        }
        "x" => {
            p.line_segment([at(0.15, 0.15), at(0.85, 0.85)], s);
            p.line_segment([at(0.85, 0.15), at(0.15, 0.85)], s);
        }
        "spin" => {
            let t = p.ctx().input(|i| i.time) as f32;
            let c = r.center();
            let rad = r.width() / 2.0 - 1.0;
            let n = 24;
            let mut pts = Vec::with_capacity(n);
            for i in 0..n {
                let a = t * 4.0 + i as f32 / n as f32 * std::f32::consts::TAU * 0.75;
                pts.push(pos2(c.x + a.cos() * rad, c.y + a.sin() * rad));
            }
            p.add(egui::Shape::line(pts, Stroke::new(2.0, color)));
            p.ctx().request_repaint_after(std::time::Duration::from_millis(16));
        }
        "clock" => {
            p.circle_stroke(r.center(), r.width() / 2.0, s);
            p.line_segment([r.center(), r.center() + vec2(0.0, -r.height() * 0.3)], s);
            p.line_segment([r.center(), r.center() + vec2(r.width() * 0.22, 0.0)], s);
        }
        "stack" => {
            // bets placed: a small stack of chips
            for (i, fy) in [0.78f32, 0.5, 0.22].iter().enumerate() {
                let rr = Rect::from_center_size(at(0.5, *fy), vec2(r.width(), r.height() * 0.26));
                if i == 0 {
                    p.rect_stroke(rr, CornerRadius::same(3), s, StrokeKind::Middle);
                } else {
                    p.rect_stroke(rr.shrink2(vec2(r.width() * 0.04 * i as f32, 0.0)), CornerRadius::same(3), s, StrokeKind::Middle);
                }
            }
        }
        "layers" => {
            p.add(egui::Shape::line(vec![at(0.5, 0.05), at(1.0, 0.32), at(0.5, 0.6), at(0.0, 0.32), at(0.5, 0.05)], s));
            p.add(egui::Shape::line(vec![at(0.0, 0.62), at(0.5, 0.9), at(1.0, 0.62)], s));
        }
        "refresh" => {
            let c = r.center();
            let rad = r.width() / 2.0;
            let mut pts = Vec::new();
            for i in 0..20 {
                let a = -0.6 + i as f32 / 19.0 * std::f32::consts::TAU * 0.78;
                pts.push(pos2(c.x + a.cos() * rad, c.y + a.sin() * rad));
            }
            p.add(egui::Shape::line(pts, s));
            let tip = pos2(c.x + (-0.6f32).cos() * rad, c.y + (-0.6f32).sin() * rad);
            p.line_segment([tip, tip + vec2(-3.5, -1.0)], s);
            p.line_segment([tip, tip + vec2(0.0, 3.5)], s);
        }
        "download" => {
            // arrow down into a tray
            p.line_segment([at(0.5, 0.04), at(0.5, 0.66)], s);
            p.add(egui::Shape::line(vec![at(0.24, 0.42), at(0.5, 0.68), at(0.76, 0.42)], s));
            p.add(egui::Shape::line(vec![at(0.06, 0.66), at(0.06, 0.94), at(0.94, 0.94), at(0.94, 0.66)], s));
        }
        "swap" => {
            // two arrows passing each other: right on top, left below
            p.line_segment([at(0.08, 0.3), at(0.92, 0.3)], s);
            p.add(egui::Shape::line(vec![at(0.7, 0.08), at(0.92, 0.3), at(0.7, 0.52)], s));
            p.line_segment([at(0.92, 0.7), at(0.08, 0.7)], s);
            p.add(egui::Shape::line(vec![at(0.3, 0.48), at(0.08, 0.7), at(0.3, 0.92)], s));
        }
        "globe" => {
            let (c, rad) = (r.center(), r.width() / 2.0);
            p.circle_stroke(c, rad, s);
            p.add(egui::Shape::ellipse_stroke(c, vec2(rad * 0.42, rad), s));
            p.line_segment([c - vec2(rad, 0.0), c + vec2(rad, 0.0)], s);
        }
        "trash" => {
            p.line_segment([at(0.05, 0.2), at(0.95, 0.2)], s);
            p.add(egui::Shape::line(vec![at(0.35, 0.2), at(0.38, 0.04), at(0.62, 0.04), at(0.65, 0.2)], s));
            p.add(egui::Shape::line(vec![at(0.16, 0.2), at(0.22, 0.98), at(0.78, 0.98), at(0.84, 0.2)], s));
        }
        "grip" => {
            // something to take hold of: two columns of three dots
            for fx in [0.3, 0.7] {
                for fy in [0.18, 0.5, 0.82] {
                    p.circle_filled(at(fx, fy), 1.3, color);
                }
            }
        }
        "minimize" => {
            p.line_segment([at(0.12, 0.55), at(0.88, 0.55)], s);
        }
        "expand" => {
            // arrows out to two corners
            p.line_segment([at(0.14, 0.86), at(0.86, 0.14)], s);
            p.add(egui::Shape::line(vec![at(0.5, 0.1), at(0.9, 0.1), at(0.9, 0.5)], s));
            p.add(egui::Shape::line(vec![at(0.1, 0.5), at(0.1, 0.9), at(0.5, 0.9)], s));
        }
        "dock" => {
            // a window with its right column filled: back to the side
            let side = Rect::from_min_max(at(0.62, 0.0), at(1.0, 1.0));
            p.rect_filled(side, CornerRadius { nw: 0, sw: 0, ne: 2, se: 2 }, alpha(color, 110));
            p.rect_stroke(r, CornerRadius::same(2), s, StrokeKind::Middle);
            p.line_segment([at(0.62, 0.0), at(0.62, 1.0)], s);
        }
        "seed" => {
            // a sprout: a stem with a leaf to each side
            let leaf = |a: Pos2, b: Pos2| {
                let d = b - a;
                let (m, n) = (a + d * 0.5, vec2(-d.y, d.x) * 0.36);
                let mut pts = Vec::with_capacity(18);
                for (c, back) in [(m + n, false), (m - n, true)] {
                    for i in 0..=8 {
                        let t = if back { 1.0 - i as f32 / 8.0 } else { i as f32 / 8.0 };
                        pts.push(a.lerp(c, t).lerp(c.lerp(b, t), t));
                    }
                }
                p.add(egui::Shape::closed_line(pts, s));
            };
            p.line_segment([at(0.5, 1.0), at(0.5, 0.46)], s);
            leaf(at(0.5, 0.62), at(0.04, 0.22));
            leaf(at(0.5, 0.46), at(0.96, 0.02));
        }
        "sound" => {
            // a speaker and two waves
            let cone = [at(0.0, 0.36), at(0.22, 0.36), at(0.5, 0.1), at(0.5, 0.9), at(0.22, 0.64), at(0.0, 0.64)];
            p.add(egui::Shape::closed_line(cone.to_vec(), s));
            for k in [0.2, 0.38] {
                let c = at(0.54, 0.5);
                let rad = r.width() * k;
                let pts = (0..=10).map(|i| {
                    let a = (-50.0 + 10.0 * i as f32).to_radians();
                    pos2(c.x + rad * a.cos(), c.y + rad * a.sin())
                });
                p.add(egui::Shape::line(pts.collect(), s));
            }
        }
        "cursor" => {
            // a pointer arrow, its tail to the bottom right
            let tip = [at(0.16, 0.04), at(0.16, 0.8), at(0.36, 0.62), at(0.52, 0.96), at(0.66, 0.9), at(0.5, 0.56), at(0.78, 0.56)];
            p.add(egui::Shape::closed_line(tip.to_vec(), s));
        }
        "loop" => {
            // two arrows chasing each other round a circle
            let c = r.center();
            let rad = r.width() * 0.42;
            for start in [0.15f32, std::f32::consts::PI + 0.15] {
                let pts: Vec<Pos2> = (0..=10).map(|i| start + i as f32 / 10.0 * 2.5).map(|a| pos2(c.x + rad * a.cos(), c.y + rad * a.sin())).collect();
                let end = *pts.last().unwrap_or(&c);
                let a = start + 2.5;
                let dir = vec2(-a.sin(), a.cos());
                let side = vec2(a.cos(), a.sin());
                p.add(egui::Shape::line(pts, s));
                let w = r.width() * 0.16;
                p.add(egui::Shape::line(vec![end - dir * w + side * w, end, end - dir * w - side * w], s));
            }
        }
        "branch" => {
            // a flow: one node splitting into two
            let (a, b, c) = (at(0.5, 0.14), at(0.18, 0.86), at(0.82, 0.86));
            let mid = at(0.5, 0.5);
            p.add(egui::Shape::line(vec![a, mid, pos2(b.x, mid.y + r.height() * 0.08), b], s));
            p.add(egui::Shape::line(vec![mid, pos2(c.x, mid.y + r.height() * 0.08), c], s));
            for q in [a, b, c] {
                p.circle_filled(q, r.width() * 0.13, color);
            }
        }
        "code" => {
            p.add(egui::Shape::line(vec![at(0.3, 0.2), at(0.02, 0.5), at(0.3, 0.8)], s));
            p.add(egui::Shape::line(vec![at(0.7, 0.2), at(0.98, 0.5), at(0.7, 0.8)], s));
            p.line_segment([at(0.58, 0.08), at(0.42, 0.92)], s);
        }
        "up" => {
            p.add(egui::Shape::line(vec![at(0.15, 0.68), at(0.5, 0.32), at(0.85, 0.68)], s));
        }
        "down" => {
            p.add(egui::Shape::line(vec![at(0.15, 0.32), at(0.5, 0.68), at(0.85, 0.32)], s));
        }
        "copy" => {
            p.rect_stroke(Rect::from_min_max(at(0.32, 0.32), at(0.96, 0.96)), CornerRadius::same(2), s, StrokeKind::Middle);
            p.add(egui::Shape::line(vec![at(0.68, 0.18), at(0.68, 0.04), at(0.04, 0.04), at(0.04, 0.68), at(0.18, 0.68)], s));
        }
        "bell" => {
            // a bell: dome, flared rim, clapper
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                    pos2(r.center().x + r.width() * 0.3 * a.cos(), r.top() + r.height() * 0.42 + r.height() * 0.3 * a.sin())
                })
                .collect();
            p.add(egui::Shape::line(pts, s));
            p.add(egui::Shape::line(vec![at(0.2, 0.42), at(0.16, 0.72), at(0.04, 0.8), at(0.96, 0.8), at(0.84, 0.72), at(0.8, 0.42)], s));
            p.circle_filled(at(0.5, 0.92), r.width() * 0.09, color);
        }
        "plus" => {
            p.line_segment([at(0.5, 0.08), at(0.5, 0.92)], s);
            p.line_segment([at(0.08, 0.5), at(0.92, 0.5)], s);
        }
        // a box with an arrow out of its corner: opens on a website
        "external" => {
            p.add(egui::Shape::line(vec![at(0.42, 0.14), at(0.14, 0.14), at(0.14, 0.86), at(0.86, 0.86), at(0.86, 0.58)], s));
            p.line_segment([at(0.48, 0.52), at(0.9, 0.1)], s);
            p.add(egui::Shape::line(vec![at(0.6, 0.1), at(0.9, 0.1), at(0.9, 0.4)], s));
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One frame of an `int_field` for 1–36 cards, fed `events`.
    fn frame(ctx: &egui::Context, value: &mut u32, events: Vec<egui::Event>) {
        let input = egui::RawInput { events, screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(320.0, 200.0))), ..Default::default() };
        let _ = ctx.run_ui(input, |ui| {
            int_field(ui, "Cards", value, (1, 36), true, true);
        });
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }
    }

    fn text(s: &str) -> egui::Event {
        egui::Event::Text(s.into())
    }

    #[test]
    fn int_field_keeps_what_is_typed() {
        let ctx = egui::Context::default();
        let mut v = 10;
        frame(&ctx, &mut v, vec![]);
        frame(&ctx, &mut v, vec![key(egui::Key::Tab)]);
        frame(&ctx, &mut v, vec![key(egui::Key::End), key(egui::Key::Backspace), key(egui::Key::Backspace)]);
        assert_eq!(v, 10, "an empty box keeps the value");
        frame(&ctx, &mut v, vec![text("1")]);
        assert_eq!(v, 1);
        frame(&ctx, &mut v, vec![text("3")]);
        assert_eq!(v, 13, "typing goes on past the first digit");
        frame(&ctx, &mut v, vec![key(egui::Key::ArrowUp), key(egui::Key::ArrowUp), key(egui::Key::ArrowDown)]);
        assert_eq!(v, 14);
        frame(&ctx, &mut v, vec![key(egui::Key::End), text("0")]);
        assert_eq!(v, 36, "too many is the most");
    }

    #[test]
    fn int_field_arrows_without_focus() {
        let ctx = egui::Context::default();
        let mut v = 2;
        frame(&ctx, &mut v, vec![]);
        frame(&ctx, &mut v, vec![key(egui::Key::ArrowDown)]);
        frame(&ctx, &mut v, vec![key(egui::Key::ArrowDown)]);
        assert_eq!(v, 1, "never below the least");
        frame(&ctx, &mut v, vec![key(egui::Key::ArrowUp); 3]);
        assert_eq!(v, 4);
    }
}
