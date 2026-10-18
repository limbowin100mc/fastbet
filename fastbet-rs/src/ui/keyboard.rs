//! Typing on Android. GameActivity's keyboard connection (an on-screen keyboard, or a PC's keyboard
//! in BlueStacks) edits a text buffer of its own and only reports the result, which winit doesn't
//! pass on. So while a text field has focus this watches that buffer: whatever the keyboard added
//! becomes typed text, whatever it took away becomes backspaces.
//!
//! The buffer starts out full of padding, so Backspace still has something to delete when the field
//! already holds text (the field's own text isn't known here); it is padded again when it runs low.

use std::time::Duration;

use egui::{Context, Event, Id, Key, Modifiers, RawInput};

/// Blank text the buffer starts with: as many backspaces as it holds work on the field's text.
const PAD: usize = 64;

#[derive(Default)]
pub struct Keyboard {
    /// The text field the buffer is mirrored into.
    field: Option<Id>,
    /// The buffer as last seen.
    last: String,
}

impl Keyboard {
    /// Turns what the keyboard did since the last frame into this frame's input.
    pub fn feed(&mut self, ctx: &Context, raw: &mut RawInput) {
        let focused = ctx.memory(|m| m.focused());
        if focused != self.field {
            self.field = focused;
            if focused.is_some() {
                self.reset();
            }
        }
        if self.field.is_none() {
            return;
        }
        // the keyboard's changes arrive without waking the app: look again shortly
        ctx.request_repaint_after(Duration::from_millis(60));
        let Some((now, composing)) = crate::android::keyboard_text() else { return };
        if now == self.last {
            return;
        }
        // Android emptied the buffer itself (a new keyboard connection): pad it again, deleting
        // nothing (a person can't clear the whole padding between two looks)
        if now.is_empty() && self.last.chars().count() > 8 {
            self.reset();
            return;
        }
        let common = self.last.chars().zip(now.chars()).take_while(|(a, b)| a == b).count();
        let removed = self.last.chars().count() - common;
        for _ in 0..removed {
            for pressed in [true, false] {
                raw.events.push(Event::Key { key: Key::Backspace, physical_key: None, pressed, repeat: false, modifiers: Modifiers::NONE });
            }
        }
        let added: String = now.chars().skip(common).collect();
        if !added.is_empty() {
            raw.events.push(Event::Text(added));
        }
        self.last = now;
        // between words, keep the padding topped up and the buffer short
        let len = self.last.chars().count();
        if !composing && (len < PAD / 2 || len > PAD * 4) {
            self.reset();
        }
    }

    fn reset(&mut self) {
        self.last = " ".repeat(PAD);
        crate::android::set_keyboard_text(&self.last);
    }
}
