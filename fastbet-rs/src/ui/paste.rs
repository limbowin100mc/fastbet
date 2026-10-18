//! Phone text fields: hold a field for half a second and a Paste bubble comes up; tapping it
//! types what Android's clipboard holds into the field, at the cursor (as Ctrl+V does on a PC).

use std::time::Duration;

use egui::{pos2, vec2, Align2, Area, Context, CornerRadius, Event, Id, Order, Pos2, Rect, RawInput, Sense, Stroke, StrokeKind};

use super::theme::*;

/// How long a touch stays down before it counts as holding.
const HOLD_SECS: f64 = 0.5;

#[derive(Default)]
pub struct Paste {
    /// The bubble is up for this field, where the finger was.
    open: Option<(Id, Pos2)>,
    /// Where it was drawn last frame (a press outside closes it).
    rect: Option<Rect>,
    /// This press already opened the bubble.
    done_for_press: bool,
    /// Clipboard text waiting to go into the next frame's input.
    pending: Option<String>,
    /// Why nothing was pasted, shown where the bubble was until this time.
    note: Option<(String, Pos2, f64)>,
}

impl Paste {
    /// Shows the bubble, opening it on a hold over a focused field.
    pub fn show(&mut self, ctx: &Context) {
        let (down, start, origin, dragging, pressed, now, at) = ctx.input(|i| {
            (i.pointer.primary_down(), i.pointer.press_start_time(), i.pointer.press_origin(), i.pointer.is_decidedly_dragging(), i.pointer.any_pressed(), i.time, i.pointer.interact_pos())
        });
        if !down {
            self.done_for_press = false;
        }
        // a press outside the bubble closes it
        if pressed && self.open.is_some() && !self.rect.zip(at).is_some_and(|(r, p)| r.contains(p)) {
            self.open = None;
        }
        if down && !dragging && !self.done_for_press {
            if let (Some(t0), Some(pos)) = (start, origin) {
                if now - t0 >= HOLD_SECS {
                    // text fields take focus on the press itself
                    if let Some(id) = ctx.memory(|m| m.focused()) {
                        self.open = Some((id, pos));
                    }
                    self.done_for_press = true;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(60));
                }
            }
        }
        self.show_note(ctx, now);
        let Some((id, at)) = self.open else {
            self.rect = None;
            return;
        };
        let size = vec2(96.0, 40.0);
        // above the finger, inside the screen
        let screen = ctx.content_rect();
        let x = (at.x - size.x / 2.0).clamp(screen.left() + 8.0, (screen.right() - size.x - 8.0).max(screen.left()));
        let y = (at.y - size.y - 36.0).max(screen.top() + 8.0);
        let resp = Area::new(Id::new("paste-bubble"))
            .order(Order::Foreground)
            .fixed_pos(pos2(x, y))
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
                let p = ui.painter();
                p.rect(rect, CornerRadius::same(20), if resp.is_pointer_button_down_on() { BG3 } else { BG2 }, Stroke::new(1.0, LINE2), StrokeKind::Inside);
                p.text(rect.center(), Align2::CENTER_CENTER, "Paste", font_semi(15.0), TEXT);
                resp
            })
            .inner;
        self.rect = Some(resp.rect);
        if resp.clicked() {
            self.open = None;
            // the tap on the bubble took the focus away: give it back so the text lands there
            ctx.memory_mut(|m| m.request_focus(id));
            match crate::android::clipboard_text() {
                Ok(text) => self.pending = Some(text),
                Err(e) => {
                    tracing::warn!("paste: {e}");
                    self.note = Some((e, pos2(x, y), now + 2.5));
                }
            }
            ctx.request_repaint();
        }
    }

    /// A short note where the bubble was (why nothing was pasted).
    fn show_note(&mut self, ctx: &Context, now: f64) {
        let Some((text, at, until)) = self.note.clone() else { return };
        if now > until {
            self.note = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(200));
        Area::new(Id::new("paste-note")).order(Order::Foreground).fixed_pos(at).show(ctx, |ui| {
            egui::Frame::new()
                .fill(BG2)
                .stroke(Stroke::new(1.0, LINE2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::symmetric(14, 9))
                .show(ui, |ui| ui.label(egui::RichText::new(text).font(font(14.0)).color(TEXT2)));
        });
    }

    /// Hands the clipboard text to the focused field as a paste.
    pub fn feed(&mut self, raw: &mut RawInput) {
        if let Some(text) = self.pending.take() {
            raw.events.push(Event::Paste(text));
        }
    }
}
