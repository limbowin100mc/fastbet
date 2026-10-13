//! The window's own title bar (the system one is turned off so the account can sit in the
//! middle): the wordmark on the left, who is logged in and at which casino in the middle,
//! the window buttons on the right. Also the resize grips the system border gave before.

use egui::{pos2, vec2, Align, Color32, CursorIcon, Id, Layout, PointerButton, Rect, ResizeDirection, Sense, Stroke, Ui, ViewportCommand};

use super::theme::{self, DIM, LINE, MUTED, TEXT, TEXT2};
use super::widgets;
use super::{App, Screen};

pub const HEIGHT: f32 = 34.0;
const BUTTON_W: f32 = 46.0;
const CLOSE_HOVER: Color32 = Color32::from_rgb(0xe8, 0x11, 0x23);

pub fn show(app: &mut App, ui: &mut Ui) {
    let bar = ui.max_rect();
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ui.painter().hline(bar.x_range(), bar.bottom() - 0.5, Stroke::new(1.0, LINE));

    // the bar moves the window; the buttons, added after, sit on top of it
    let drag = ui.interact(bar, Id::new("titlebar-drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    let mut left = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_max(pos2(bar.left() + 14.0, bar.top()), bar.right_bottom())).layout(Layout::left_to_right(Align::Center)),
    );
    widgets::wordmark(&mut left, 14.0);

    if !matches!(app.screen, Screen::Accounts | Screen::Account) {
        who(app, ui, bar);
    }

    let mut x = bar.right() - BUTTON_W;
    if button(ui, Rect::from_min_size(pos2(x, bar.top()), vec2(BUTTON_W, bar.height())), Glyph::Close).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    x -= BUTTON_W;
    let glyph = if maximized { Glyph::Restore } else { Glyph::Maximize };
    if button(ui, Rect::from_min_size(pos2(x, bar.top()), vec2(BUTTON_W, bar.height())), glyph).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    x -= BUTTON_W;
    if button(ui, Rect::from_min_size(pos2(x, bar.top()), vec2(BUTTON_W, bar.height())), Glyph::Minimize).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// "name · [logo] Casino", centred on the window.
fn who(app: &mut App, ui: &Ui, bar: Rect) {
    let id = app.casino();
    let p = ui.painter();
    let name = app.active_name().unwrap_or_default();
    let name_g = p.layout_no_wrap(name.clone(), theme::font_semi(12.5), TEXT);
    let casino_g = p.layout_no_wrap(id.name().to_string(), theme::font_semi(12.5), MUTED);
    const LOGO: f32 = 14.0;
    const GAP: f32 = 7.0;
    // the dot between name and casino, with room on both sides
    const SEP: f32 = 18.0;
    let name_w = if name.is_empty() { 0.0 } else { name_g.size().x + SEP };
    let w = name_w + LOGO + GAP + casino_g.size().x;
    let cy = bar.center().y;
    let mut x = bar.center().x - w / 2.0;
    if !name.is_empty() {
        p.galley(pos2(x, cy - name_g.size().y / 2.0), name_g, TEXT);
        x += name_w;
        p.circle_filled(pos2(x - SEP / 2.0, cy), 1.5, DIM);
    }
    app.logos.paint(ui.painter(), Rect::from_min_size(pos2(x, cy - LOGO / 2.0), vec2(LOGO, LOGO)), id, TEXT2);
    x += LOGO + GAP;
    ui.painter().galley(pos2(x, cy - casino_g.size().y / 2.0), casino_g, MUTED);
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

fn button(ui: &Ui, rect: Rect, glyph: Glyph) -> egui::Response {
    let resp = ui.interact(rect, Id::new(("titlebar-button", glyph as u8)), Sense::click());
    let p = ui.painter();
    let close = matches!(glyph, Glyph::Close);
    if resp.hovered() {
        let fill = if close { CLOSE_HOVER } else { theme::BG3 };
        p.rect_filled(rect, 0.0, if resp.is_pointer_button_down_on() { fill.gamma_multiply(0.8) } else { fill });
    }
    let color = if close && resp.hovered() { Color32::WHITE } else { TEXT2 };
    let s = Stroke::new(1.0, color);
    // pixel centre, so 1px lines stay crisp
    let ppp = ui.ctx().pixels_per_point();
    let c = pos2((rect.center().x * ppp).floor() / ppp + 0.5 / ppp, (rect.center().y * ppp).floor() / ppp + 0.5 / ppp);
    const R: f32 = 5.0;
    match glyph {
        Glyph::Minimize => {
            p.hline(c.x - R..=c.x + R, c.y, s);
        }
        Glyph::Maximize => {
            p.rect_stroke(Rect::from_center_size(c, vec2(2.0 * R, 2.0 * R)), 1.0, s, egui::StrokeKind::Middle);
        }
        Glyph::Restore => {
            let back = Rect::from_min_max(pos2(c.x - R + 2.0, c.y - R), pos2(c.x + R, c.y + R - 2.0));
            p.line(vec![pos2(back.left(), back.top() + 0.5), pos2(back.right(), back.top() + 0.5)], s);
            p.line(vec![pos2(back.right(), back.top()), pos2(back.right(), back.bottom())], s);
            p.rect_stroke(Rect::from_min_max(pos2(c.x - R, c.y - R + 2.0), pos2(c.x + R - 2.0, c.y + R)), 1.0, s, egui::StrokeKind::Middle);
        }
        Glyph::Close => {
            p.line_segment([pos2(c.x - R, c.y - R), pos2(c.x + R, c.y + R)], s);
            p.line_segment([pos2(c.x - R, c.y + R), pos2(c.x + R, c.y - R)], s);
        }
    }
    resp
}

/// Grips along the window's edges and corners that resize it (not while maximized).
/// Added after everything else, so they win over what lies under them.
pub fn resize_grips(ui: &Ui) {
    let ctx = ui.ctx();
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)) {
        return;
    }
    let r = ctx.content_rect();
    const E: f32 = 4.0;
    const C: f32 = 10.0;
    use ResizeDirection as D;
    let grips = [
        // edges first: the corners, added later, win where they overlap
        (D::North, Rect::from_min_max(r.left_top(), pos2(r.right(), r.top() + E)), CursorIcon::ResizeVertical),
        (D::South, Rect::from_min_max(pos2(r.left(), r.bottom() - E), r.right_bottom()), CursorIcon::ResizeVertical),
        (D::West, Rect::from_min_max(r.left_top(), pos2(r.left() + E, r.bottom())), CursorIcon::ResizeHorizontal),
        (D::East, Rect::from_min_max(pos2(r.right() - E, r.top()), r.right_bottom()), CursorIcon::ResizeHorizontal),
        (D::NorthWest, Rect::from_min_size(r.left_top(), vec2(C, C)), CursorIcon::ResizeNwSe),
        (D::NorthEast, Rect::from_min_size(r.right_top() - vec2(C, 0.0), vec2(C, C)), CursorIcon::ResizeNeSw),
        (D::SouthWest, Rect::from_min_size(r.left_bottom() - vec2(0.0, C), vec2(C, C)), CursorIcon::ResizeNeSw),
        (D::SouthEast, Rect::from_min_size(r.right_bottom() - vec2(C, C), vec2(C, C)), CursorIcon::ResizeNwSe),
    ];
    for (i, (dir, rect, cursor)) in grips.into_iter().enumerate() {
        let resp = ui.interact(rect, Id::new(("resize-grip", i)), Sense::drag()).on_hover_cursor(cursor);
        if resp.drag_started_by(PointerButton::Primary) {
            ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
        }
    }
}

