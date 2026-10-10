//! Right pane: session profit with its chart, then the numbers in grouped cards.

use egui::{pos2, vec2, Align, Align2, Color32, CornerRadius, Frame, Layout, Margin, Mesh, Rect, Sense, Shape, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG2, DIM, G1, GOLD, GREEN, LINE, MUTED, RED, TEXT};
use super::widgets::{self, semi, value_chips};
use super::App;
use crate::money::{fmt_int, fmt_ms, fmt_multi};

struct Cell {
    cap: &'static str,
    value: String,
    color: Color32,
    sub: String,
}

fn cell(cap: &'static str, value: String, color: Color32, sub: impl Into<String>) -> Cell {
    Cell { cap, value, color, sub: sub.into() }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 12 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        let slots = app.screen == super::Screen::Slot;
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            ui.label(widgets::bold(if slots { "Slot session" } else { "Session" }, 14.0, TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::glyph_button(ui, "refresh", "Reset statistics").clicked() {
                    if slots {
                        app.slot_stats = super::stats::Stats::fresh(&app.slot_stats.cur.clone(), app.slot_stats.decimals);
                    } else {
                        app.reset_stats();
                    }
                }
            });
        });

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
            let full = ui.available_width() - 2.0;
            let (per_s, per_m) = if slots { app.slot_stats.speed() } else { app.stats.speed() };
            let cur = if slots {
                app.slot_stats.cur.clone()
            } else if app.stats.cur.is_empty() {
                app.active_currency()
            } else {
                app.stats.cur.clone()
            };
            let has = slots || app.info(&cur).is_some();
            let prim = |v: i128, sign: bool| if has { app.prim(v, &cur, sign) } else { "0".to_string() };
            let sec = |v: i128, sign: bool| if has { app.sec(v, &cur, sign) } else { String::new() };
            let s = if slots { &app.slot_stats } else { &app.stats };

            // profit, the number that matters most, with the curve under it
            let edge = s.edge_pct().map(|e| format!("{}{e:.2}%", if e >= 0.0 { "+" } else { "" }));
            let sub = [edge, Some(sec(s.profit, true)).filter(|x| !x.is_empty())].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
            profit_card(ui, full, &prim(s.profit, true), theme::pnl(s.profit), &sub, s.bets, &s.chart_points());

            let (streak, streak_c) = if s.streak > 0 {
                (format!("W{}", s.streak), GREEN)
            } else if s.streak < 0 {
                (format!("L{}", -s.streak), RED)
            } else {
                ("–".to_string(), TEXT)
            };
            let rtp = s.rtp_pct();
            grid(
                ui,
                full,
                vec![
                    [
                        cell("Bet speed", format!("{per_s:.1}/s"), if per_s > 0.0 { G1 } else { TEXT }, format!("{}/m · {}", fmt_int(per_m as i128), fmt_ms(s.latency))),
                        cell("Streak", streak, streak_c, format!("best W{} · L{}", s.best_win_streak(), s.worst_loss_streak())),
                    ],
                    [
                        cell("Wins", fmt_int(s.wins as i128), GREEN, s.win_pct().map(|p| format!("{p:.2}%")).unwrap_or("–".into())),
                        cell("Losses", fmt_int(s.losses as i128), RED, if s.hits > 0 { format!("{} hit{}", s.hits, if s.hits > 1 { "s" } else { "" }) } else { String::new() }),
                    ],
                    [
                        cell("Wagered", prim(s.wagered, false), TEXT, sec(s.wagered, false)),
                        cell("RTP", rtp.map(|r| format!("{r:.2}%")).unwrap_or("–".into()), if rtp.is_none() { TEXT } else if rtp.unwrap_or(0.0) >= 100.0 { GREEN } else { RED }, format!("{} bets", fmt_int(s.bets as i128))),
                    ],
                ],
            );
            grid(
                ui,
                full,
                vec![
                    [
                        cell("Highest profit", prim(s.peak, true), theme::pnl(s.peak), ""),
                        cell("Lowest", prim(s.low, false), theme::pnl(s.low), ""),
                    ],
                    [
                        cell("Highest win", prim(s.hi_win, true), if s.hi_win > 0 { GREEN } else { TEXT }, ""),
                        cell("Highest loss", prim(-s.hi_loss, false), if s.hi_loss > 0 { RED } else { TEXT }, ""),
                    ],
                    [
                        cell("Max drawdown", prim(-s.max_dd, false), if s.max_dd > 0 { RED } else { TEXT }, ""),
                        cell("Highest bet", prim(s.hi_bet, false), TEXT, ""),
                    ],
                ],
            );
            if s.retries > 0 {
                grid(ui, full, vec![[cell("Busy retries", fmt_int(s.retries as i128), GOLD, "server said wait"), cell("", String::new(), TEXT, "")]]);
            }
            chips_card(ui, full, "Top multipliers", &s.top_multi.iter().map(|m| fmt_multi(*m)).collect::<Vec<_>>(), GOLD);
            chips_card(ui, full, "Best win streaks", &s.win_streaks_now().iter().map(|v| v.to_string()).collect::<Vec<_>>(), GREEN);
            chips_card(ui, full, "Worst loss streaks", &s.loss_streaks_now().iter().map(|v| v.to_string()).collect::<Vec<_>>(), RED);
        });
    });
}

fn card_frame() -> Frame {
    Frame::new().fill(BG2).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(12))
}

fn profit_card(ui: &mut Ui, width: f32, value: &str, color: Color32, sub: &str, bets: u64, pts: &[f64]) {
    card_frame().inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 10 }).show(ui, |ui| {
        ui.set_width(width - 30.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.label(semi("Profit", 11.5, MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(semi(format!("{} bets", fmt_int(bets as i128)), 11.0, DIM));
            });
        });
        let (vr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::hover());
        widgets::fit_text(ui.painter(), vr.left_center(), Align2::LEFT_CENTER, value, theme::font_bold(24.0), color, vr.width());
        if !sub.is_empty() {
            ui.label(semi(sub, 11.0, DIM));
        }
        ui.add_space(8.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
        draw_chart(ui, rect, if bets == 0 { &[] } else { pts });
    });
}

/// Two-column card of captioned values, split by hairlines.
fn grid(ui: &mut Ui, width: f32, rows: Vec<[Cell; 2]>) {
    let heights: Vec<f32> = rows.iter().map(|r| if r.iter().any(|c| !c.sub.is_empty()) { 62.0 } else { 48.0 }).collect();
    let (rect, _) = ui.allocate_exact_size(vec2(width, heights.iter().sum()), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(12), BG2, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let half = rect.width() / 2.0;
    let mut y = rect.top();
    for (i, (row, h)) in rows.iter().zip(&heights).enumerate() {
        if i > 0 {
            p.hline((rect.left() + 12.0)..=(rect.right() - 12.0), y, Stroke::new(1.0, LINE));
        }
        for (j, c) in row.iter().enumerate() {
            if c.cap.is_empty() {
                continue;
            }
            let x = rect.left() + half * j as f32 + 14.0;
            let max_w = half - 22.0;
            p.text(pos2(x, y + 12.0), Align2::LEFT_TOP, c.cap, theme::font_semi(11.0), MUTED);
            widgets::fit_text(p, pos2(x, y + 28.0), Align2::LEFT_TOP, &c.value, theme::font_semi(15.0), c.color, max_w);
            if !c.sub.is_empty() {
                widgets::fit_text(p, pos2(x, y + 47.0), Align2::LEFT_TOP, &c.sub, theme::font(10.5), DIM, max_w);
            }
        }
        if row[1].cap.is_empty() {
            y += h;
            continue;
        }
        p.vline(rect.center().x, (y + 10.0)..=(y + h - 10.0), Stroke::new(1.0, LINE));
        y += h;
    }
}

fn chips_card(ui: &mut Ui, width: f32, cap: &str, values: &[String], color: Color32) {
    card_frame().inner_margin(Margin { left: 14, right: 14, top: 11, bottom: 12 }).show(ui, |ui| {
        ui.set_width(width - 30.0);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(semi(cap, 11.0, MUTED));
        value_chips(ui, width - 30.0, values, color);
    });
}

/// Profit curve: line with a gradient fill, green above zero and red below.
fn draw_chart(ui: &Ui, rect: Rect, pts: &[f64]) {
    let p = ui.painter_at(rect.expand(4.0));
    let (mut min, mut max) = (0.0f64, 0.0f64);
    for v in pts {
        min = min.min(*v);
        max = max.max(*v);
    }
    if max == min {
        max += 1.0;
        min -= 1.0;
    }
    let pad = 4.0;
    let w = rect.width();
    let h = rect.height();
    let n = pts.len().max(2) as f32 - 1.0;
    let xf = |i: usize| rect.left() + (i as f32) * w / n;
    let yf = |v: f64| rect.top() + pad + (((max - v) / (max - min)) as f32) * (h - 2.0 * pad);
    let zy = yf(0.0);

    let mut x = rect.left();
    while x < rect.right() {
        p.line_segment([pos2(x, zy), pos2((x + 3.0).min(rect.right()), zy)], Stroke::new(1.0, alpha(Color32::WHITE, 40)));
        x += 7.0;
    }
    if pts.len() < 2 {
        p.text(rect.center(), Align2::CENTER_CENTER, "No bets yet", theme::font(11.5), DIM);
        return;
    }
    let points: Vec<egui::Pos2> = pts.iter().enumerate().map(|(i, v)| pos2(xf(i), yf(*v))).collect();

    let mut mesh = Mesh::default();
    let top = yf(max);
    let bottom = yf(min);
    for seg in points.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let mut push_quad = |a: egui::Pos2, b: egui::Pos2, color: Color32, extreme: f32| {
            let strength = |pt: egui::Pos2| {
                let span = (zy - extreme).abs().max(1.0);
                ((zy - pt.y).abs() / span).clamp(0.0, 1.0)
            };
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(a, alpha(color, (80.0 * strength(a)) as u8));
            mesh.colored_vertex(b, alpha(color, (80.0 * strength(b)) as u8));
            mesh.colored_vertex(pos2(b.x, zy), alpha(color, 0));
            mesh.colored_vertex(pos2(a.x, zy), alpha(color, 0));
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        };
        let above = |pt: egui::Pos2| pt.y <= zy;
        if above(a) == above(b) {
            let (color, extreme) = if above(a) { (GREEN, top) } else { (RED, bottom) };
            push_quad(a, b, color, extreme);
        } else {
            let t = (zy - a.y) / (b.y - a.y);
            let m = pos2(a.x + (b.x - a.x) * t, zy);
            let (ca, ea) = if above(a) { (GREEN, top) } else { (RED, bottom) };
            let (cb, eb) = if above(b) { (GREEN, top) } else { (RED, bottom) };
            push_quad(a, m, ca, ea);
            push_quad(m, b, cb, eb);
        }
    }
    p.add(Shape::mesh(mesh));

    for seg in points.windows(2) {
        let (pa, pb) = (seg[0], seg[1]);
        let c = if (pa.y + pb.y) / 2.0 <= zy { GREEN } else { RED };
        p.line_segment([pa, pb], Stroke::new(1.6, c));
    }
    let last = *points.last().unwrap();
    let lc = if pts[pts.len() - 1] >= 0.0 { GREEN } else { RED };
    p.circle_filled(last, 5.0, alpha(lc, 60));
    p.circle_filled(last, 2.6, lc);
}
