//! The FastBet account screen: shown instead of the app until a FastBet account with a running plan
//! is logged in (see crate::member).

use egui::{pos2, vec2, Align2, CornerRadius, Frame, Key, Margin, Rect, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, DIM, GREEN, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, pill_button, Btn};
use super::App;
use crate::member::{self, Gate};

/// The two plans, as the website sells them.
const PLANS: [(&str, &str); 2] = [("Weekly", "7 days"), ("Monthly", "30 days")];

pub fn show(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    let gate = app.member.gate();
    let busy = app.member.busy();
    let mut open: Option<String> = None;

    egui::ScrollArea::vertical().id_salt("gate").auto_shrink([false, false]).show(ui, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(full.height() * 0.07 + 4.0);
            widgets::wordmark(ui, 32.0);
            ui.add_space((full.height() * 0.07).max(18.0));
            let w = 380.0_f32.min(full.width() - 40.0);
            match &gate {
                Gate::Checking => checking(ui, w),
                Gate::LoggedOut { error } => login_card(app, ui, w, error.as_deref(), false, &mut open),
                Gate::LoggingIn => login_card(app, ui, w, None, true, &mut open),
                Gate::NoPlan { username, ended } => no_plan(app, ui, w, username, ended.as_deref(), busy, &mut open),
                Gate::Offline { error } => offline(app, ui, w, error, busy),
                // the app shows itself; this screen isn't drawn
                Gate::Active { .. } => {}
            }
            ui.add_space(24.0);
        });
    });
    ui.painter().text(pos2(full.left() + 20.0, full.bottom() - 16.0), Align2::LEFT_CENTER, format!("v{}", env!("CARGO_PKG_VERSION")), theme::font(10.5), DIM);

    if let Some(url) = open {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
}

fn card(ui: &mut Ui, w: f32, add: impl FnOnce(&mut Ui)) {
    Frame::new()
        .fill(BG1)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(22))
        .show(ui, |ui| {
            ui.set_width(w - 44.0);
            ui.spacing_mut().item_spacing.y = 6.0;
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), add);
        });
}

fn title(ui: &mut Ui, text: &str, sub: &str) {
    ui.label(widgets::bold(text, 18.0, TEXT));
    ui.add_space(2.0);
    ui.label(widgets::semi(sub, 12.0, MUTED));
    ui.add_space(12.0);
}

fn checking(ui: &mut Ui, w: f32) {
    card(ui, w, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Spinner::new().size(16.0).color(TEXT2));
            ui.add_space(6.0);
            ui.label(widgets::semi("Checking your FastBet plan…", 13.0, TEXT2));
        });
    });
}

fn login_card(app: &mut App, ui: &mut Ui, w: f32, error: Option<&str>, busy: bool, open: &mut Option<String>) {
    let mut submit = false;
    card(ui, w, |ui| {
        title(ui, "Log in to FastBet", "Your FastBet account opens the app while it has a Weekly or Monthly plan.");
        widgets::caption(ui, "Email or username");
        let r1 = widgets::text_box(ui, &mut app.gate_login, "you@example.com or your username", None, !busy, false);
        ui.add_space(6.0);
        widgets::caption(ui, "Password");
        let r2 = widgets::text_box(ui, &mut app.gate_password, "", None, !busy, !app.gate_show_pw);
        widgets::checkbox(ui, "Show password", &mut app.gate_show_pw, !busy);
        let enter = ui.input(|i| i.key_pressed(Key::Enter)) && (r1.lost_focus() || r2.lost_focus());
        if let Some(e) = error {
            ui.add_space(4.0);
            ui.label(widgets::semi(e, 12.0, RED));
        }
        ui.add_space(10.0);
        let ready = !busy && !app.gate_login.trim().is_empty() && !app.gate_password.is_empty();
        let text = if busy { "Logging in…" } else { "Log in" };
        if button(ui, text, Btn::Primary, ready, Some(ui.available_width())).clicked() || (enter && ready) {
            submit = true;
        }
    });
    ui.add_space(14.0);
    links(ui, &[("Create an account", "/signup"), ("Forgot password?", "/forgot-password")], open);
    if submit {
        app.member.login(app.gate_login.clone(), app.gate_password.clone());
        app.gate_password.clear();
    }
}

#[allow(clippy::too_many_arguments)]
fn no_plan(app: &mut App, ui: &mut Ui, w: f32, username: &str, ended: Option<&str>, busy: bool, open: &mut Option<String>) {
    let mut check = false;
    card(ui, w, |ui| {
        let head = match ended {
            Some(p) => format!("Your {} plan ran out", member::plan_label(p)),
            None => "Get a plan to start".to_string(),
        };
        title(ui, &head, &format!("Logged in as {username}. FastBet opens with a Weekly or Monthly plan."));

        // the two plans side by side
        let gap = 8.0;
        let tile_w = (ui.available_width() - gap) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (name, days) in PLANS {
                let (r, _) = ui.allocate_exact_size(vec2(tile_w, 62.0), egui::Sense::hover());
                let p = ui.painter();
                p.rect(r, CornerRadius::same(10), BG2, Stroke::new(1.0, LINE2), StrokeKind::Inside);
                p.text(pos2(r.left() + 12.0, r.top() + 18.0), Align2::LEFT_CENTER, name.to_uppercase(), theme::font_semi(10.0), MUTED);
                p.text(pos2(r.left() + 12.0, r.top() + 41.0), Align2::LEFT_CENTER, days, theme::font_bold(17.0), TEXT);
            }
        });
        ui.add_space(12.0);
        if button(ui, "Get a plan  ›", Btn::Primary, true, Some(ui.available_width())).clicked() {
            *open = Some(member::page("/plans"));
        }
        ui.add_space(4.0);
        let again = if busy { "Checking…" } else { "I've paid · check again" };
        if button(ui, again, Btn::Ghost, !busy, Some(ui.available_width())).clicked() {
            check = true;
        }
        ui.add_space(6.0);
        ui.label(widgets::semi("Pay on the website: FastBet opens here by itself once the payment is confirmed.", 11.0, DIM));
    });
    ui.add_space(14.0);
    if pill_button(ui, "Log out", MUTED, false).clicked() {
        app.member.logout();
    }
    if check {
        app.member.check();
    }
}

fn offline(app: &mut App, ui: &mut Ui, w: f32, error: &str, busy: bool) {
    let mut retry = false;
    card(ui, w, |ui| {
        title(ui, "Can't reach FastBet", "Your plan couldn't be checked. Check your connection and try again.");
        ui.label(widgets::semi(error, 11.0, DIM));
        ui.add_space(12.0);
        let text = if busy { "Checking…" } else { "Try again" };
        if button(ui, text, Btn::Primary, !busy, Some(ui.available_width())).clicked() {
            retry = true;
        }
    });
    ui.add_space(14.0);
    if pill_button(ui, "Log out", MUTED, false).clicked() {
        app.member.logout();
    }
    if retry {
        app.member.check();
    }
}

/// Small links to the website under the card.
fn links(ui: &mut Ui, items: &[(&str, &str)], open: &mut Option<String>) {
    let font = theme::font_semi(11.5);
    let widths: Vec<f32> = items.iter().map(|(t, _)| ui.painter().layout_no_wrap(t.to_string(), font.clone(), MUTED).size().x).collect();
    let gap = 22.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * (items.len() as f32 - 1.0);
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), egui::Sense::hover());
    let mut x = row.center().x - total / 2.0;
    for ((text, path), w) in items.iter().zip(widths) {
        let r = Rect::from_min_size(pos2(x, row.top()), vec2(w, row.height()));
        let resp = ui.interact(r, ui.id().with(("gate-link", *path)), egui::Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let color = if resp.hovered() { TEXT } else { MUTED };
        ui.painter().text(r.left_center(), Align2::LEFT_CENTER, *text, font.clone(), color);
        if resp.clicked() {
            *open = Some(member::page(path));
        }
        x += w + gap;
    }
}

/// The FastBet account in the account menu: who, and the plan's time left.
pub fn menu_header(m: &member::Member, ui: &mut Ui) {
    let Gate::Active { username, plan, offline } = m.gate() else { return };
    let left = member::time_left(m.seconds_left());
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), egui::Sense::hover());
    let p = ui.painter();
    p.text(pos2(r.left() + 10.0, r.top() + 12.0), Align2::LEFT_CENTER, "FASTBET", theme::font_semi(9.5), DIM);
    widgets::fit_text(p, pos2(r.left() + 10.0, r.top() + 28.0), Align2::LEFT_CENTER, &username, theme::font_semi(12.0), TEXT2, r.width() * 0.5);
    let tag = format!("{} · {left}{}", member::plan_label(&plan), if offline { " · offline" } else { "" });
    p.text(pos2(r.right() - 10.0, r.top() + 28.0), Align2::RIGHT_CENTER, tag, theme::font_semi(11.0), alpha(GREEN, 230));
}
