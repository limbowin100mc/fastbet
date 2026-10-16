//! The VIP pill beside the balance (casinos with VIP levels): a ring filling up towards the next
//! rank, the rank, and the dollars still to wager for it; its dropdown has the XP to the next
//! rank and level.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Frame, Margin, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, G1, LINE, LINE2, MUTED, TEXT, TEXT2};
use super::widgets;
use super::App;
use crate::casino::VipStatus;
use crate::money::{fmt_compact, fmt_int};

/// "≈ $3.6k" for `xp`, at the account's rate.
pub(super) fn usd_text(v: &VipStatus, xp: f64) -> Option<String> {
    let about = if v.in_dollars { "" } else { "≈ " };
    v.usd_for(xp).map(|usd| format!("{about}${}", fmt_compact(usd)))
}

/// "10,550 XP", or nothing where the casino counts dollars (the dollars say it).
pub(super) fn xp_text(v: &VipStatus, xp: f64) -> Option<String> {
    (!v.in_dollars).then(|| format!("{} XP", fmt_int(xp.ceil() as i128)))
}

/// How the dollar figures are worked out, for their tooltips.
pub(super) fn rate_tip(v: &VipStatus) -> String {
    if v.in_dollars {
        return "The dollars you wager count towards the next rank, one for one.".into();
    }
    match v.xp_per_usd {
        Some(r) => format!("At your own rate of {r:.1} XP per $1 wagered over the last weeks. Slots earn more XP per dollar than originals."),
        None => "Dollar amounts show once a few weeks of play are on record.".into(),
    }
}

/// A ring around `c` filling clockwise from the top as far as `t` (0–1).
pub(super) fn ring(p: &egui::Painter, c: Pos2, r: f32, width: f32, t: f32) {
    p.circle_stroke(c, r, Stroke::new(width, LINE2));
    if t <= 0.0 {
        return;
    }
    let n = (64.0 * t).ceil().max(2.0) as usize;
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * t.min(1.0) * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::line(pts, Stroke::new(width, G1)));
}

/// The pill. Toggles the dropdown.
pub fn pill(app: &mut App, ui: &mut Ui) {
    let Some(v) = app.vip.clone() else { return };
    let rest = match v.xp_to_rank() {
        Some(xp) => usd_text(&v, xp).unwrap_or_else(|| format!("{} XP", fmt_compact(xp))),
        None => "Top rank".into(),
    };
    let (rank_font, rest_font) = (theme::font_bold(12.5), theme::font_semi(11.5));
    let measure = |t: &str, f: egui::FontId| ui.painter().layout_no_wrap(t.to_string(), f, TEXT).size().x;
    let (rank_w, rest_w) = (measure(&v.rank, rank_font.clone()), measure(&rest, rest_font.clone()));
    let w = 36.0 + rank_w + 8.0 + rest_w + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let p = ui.painter();
    let hovered = resp.hovered() || app.vip_open;
    p.rect(rect, CornerRadius::same(16), if hovered { BG2 } else { BG1 }, Stroke::new(1.0, if app.vip_open { alpha(G1, 140) } else { LINE }), StrokeKind::Inside);
    ring(p, pos2(rect.left() + 20.0, rect.center().y), 8.0, 2.4, v.rank_progress() as f32);
    let x = rect.left() + 36.0;
    p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, &v.rank, rank_font, TEXT);
    p.text(pos2(x + rank_w + 8.0, rect.center().y), Align2::LEFT_CENTER, rest, rest_font, MUTED);
    let c = pos2(rect.right() - 15.0, rect.center().y + if app.vip_open { -1.0 } else { 1.0 });
    let dir = if app.vip_open { -1.0 } else { 1.0 };
    p.line_segment([c + vec2(-4.0, -2.0 * dir), c + vec2(0.0, 2.0 * dir)], Stroke::new(1.6, MUTED));
    p.line_segment([c + vec2(0.0, 2.0 * dir), c + vec2(4.0, -2.0 * dir)], Stroke::new(1.6, MUTED));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let resp = match (&v.next_rank, app.vip_open) {
        (Some((next, _)), false) => resp.on_hover_text(format!("{:.0}% of the way to {next}", v.rank_progress() * 100.0)),
        _ => resp,
    };
    if resp.clicked() {
        app.vip_open = !app.vip_open;
    }
    if app.vip_open {
        dropdown(app, ui, rect, &v);
    }
}

fn dropdown(app: &mut App, ui: &mut Ui, anchor: Rect, v: &VipStatus) {
    let width = 290.0;
    let mut open_rewards = false;
    let area = egui::Area::new(egui::Id::new("vip-dropdown"))
        .fixed_pos(pos2(anchor.center().x - width / 2.0, anchor.bottom() + 8.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            Frame::new()
                .fill(BG1)
                .stroke(Stroke::new(1.0, LINE2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(12))
                .shadow(egui::epaint::Shadow { offset: [0, 10], blur: 30, spread: 0, color: Color32::from_black_alpha(170) })
                .show(ui, |ui| {
                    let w = width - 24.0;
                    ui.set_width(w);
                    ui.spacing_mut().item_spacing.y = 6.0;
                    // the ring with how far, the rank and the next one
                    let (hr, _) = ui.allocate_exact_size(vec2(w, 64.0), Sense::hover());
                    let p = ui.painter();
                    let c = pos2(hr.left() + 30.0, hr.center().y);
                    ring(p, c, 25.0, 4.0, v.rank_progress() as f32);
                    p.text(c, Align2::CENTER_CENTER, format!("{:.0}%", v.rank_progress() * 100.0), theme::font_bold(13.0), TEXT);
                    let x = hr.left() + 70.0;
                    p.text(pos2(x, hr.center().y - 9.0), Align2::LEFT_CENTER, &v.rank, theme::font_bold(15.0), TEXT);
                    let next = match &v.next_rank {
                        Some((r, _)) => format!("Next: {r}"),
                        None => "Top rank".into(),
                    };
                    p.text(pos2(x, hr.center().y + 10.0), Align2::LEFT_CENTER, next, theme::font(12.0), MUTED);
                    widgets::hsep(ui);
                    let tip = rate_tip(v);
                    if let (Some((rank, _)), Some(xp)) = (&v.next_rank, v.xp_to_rank()) {
                        row(ui, rank, xp_text(v, xp), usd_text(v, xp)).on_hover_text(&tip);
                    }
                    if let Some(level) = v.level {
                        let xp = v.xp_to_level();
                        row(ui, &format!("Level {}", level + 1), xp_text(v, xp), usd_text(v, xp)).on_hover_text(&tip);
                    }
                    if app.casino().has_rewards() {
                        widgets::hsep(ui);
                        if widgets::menu_item(ui, "gift", "Rewards").clicked() {
                            open_rewards = true;
                        }
                    }
                });
        });
    let rect = area.response.rect;
    let outside = ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p) && !anchor.contains(p)));
    if outside || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.vip_open = false;
    }
    if open_rewards {
        app.vip_open = false;
        app.go_rewards();
    }
}

/// "Collector I      460,550 XP   ≈ $154k": what is still to go for a goal.
fn row(ui: &mut Ui, label: &str, xp: Option<String>, usd: Option<String>) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 24.0), Sense::hover());
    let p = ui.painter();
    p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, label, theme::font(12.0), MUTED);
    let usd_w = 64.0;
    if let Some(xp) = xp {
        p.text(pos2(r.right() - usd_w - 8.0, r.center().y), Align2::RIGHT_CENTER, xp, theme::font_semi(12.0), TEXT2);
    }
    p.text(pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, usd.unwrap_or_else(|| "–".into()), theme::font_bold(12.5), TEXT);
    resp
}
