//! The Session pane: session profit with its chart, the statistics as one list, and the
//! ranked lists (multipliers, streaks) in tabs.
//! It sits on the right, or floats wherever it's dragged to, and minimizes to a small card
//! with just the profit.

use egui::{
    pos2, vec2, Align, Align2, Color32, CornerRadius, CursorIcon, Frame, Id, Layout, Margin, Mesh, Order, Pos2, Rect, Sense, Shape, Stroke,
    StrokeKind, Ui, UiBuilder, Vec2,
};

use super::theme::{self, alpha, BG1, BG2, DIM, G1, GOLD, GREEN, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, semi};
use super::{titlebar, App, Screen};
use crate::money::{fmt_int, fmt_ms, fmt_multi, fmt_time};

/// Width of the floating pane.
const FLOAT_W: f32 = 292.0;
/// Width of the minimized card.
const MINI_W: f32 = 232.0;
/// Dropped with the pointer this close to the window's right edge, the pane docks again.
const DOCK_SNAP: f32 = 48.0;
/// Height of the page's top bar: a pane minimized from the side waits under it.
const TOP_BAR: f32 = 56.0;
/// A floating pane keeps this far inside the window.
const EDGE: f32 = 8.0;
const DRAG: &str = "session-drag";
const FLOAT: &str = "session-float";
/// Height of a row in the ranked lists.
const ROW_H: f32 = 26.0;
/// Rows a ranked list shows before it scrolls.
const LIST_ROWS: f32 = 5.0;

/// The pane docked: it fills the right side panel.
pub fn docked(app: &mut App, ui: &mut Ui) {
    let pane = ui.max_rect();
    Frame::new().inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 12 }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 10.0;
        header(app, ui, Some(pane));
        body(app, ui, None);
    });
}

/// The pane off the side (floating, or minimized): a card over the page.
pub fn floating(app: &mut App, ctx: &egui::Context) {
    let bounds = bounds(ctx);
    let s = app.settings.session.clone();
    let id = Id::new(FLOAT);
    let size = ctx.memory(|m| m.area_rect(id)).map_or(vec2(if s.minimized { MINI_W } else { FLOAT_W }, 80.0), |r| r.size());
    let pos = if s.docked {
        // minimized from the side: top right of the page, under its top bar
        pos2(bounds.right() - MINI_W - 12.0, bounds.top() + TOP_BAR + 12.0)
    } else {
        clamp(pos2(s.pos[0], s.pos[1]), size, bounds.shrink(EDGE))
    };
    egui::Area::new(id).order(Order::Middle).fixed_pos(pos).movable(false).constrain(false).show(ctx, |ui| {
        // the whole card moves it; what's inside and takes a press (buttons, scrolling) wins
        let card = Rect::from_min_size(pos, size);
        drag(app, ui, card, card);
        let frame = Frame::new()
            .fill(BG1)
            .stroke(Stroke::new(1.0, LINE2))
            .corner_radius(CornerRadius::same(14))
            .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(140) });
        if s.minimized {
            frame.inner_margin(Margin { left: 10, right: 6, top: 6, bottom: 10 }).show(ui, |ui| mini(app, ui));
        } else {
            frame.inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 12 }).show(ui, |ui| {
                ui.set_width(FLOAT_W - 24.0);
                ui.spacing_mut().item_spacing.y = 10.0;
                header(app, ui, None);
                // the card stays inside the window: less its margins and border, and the header
                body(app, ui, Some(bounds.height() - 2.0 * EDGE - 24.0 - 38.0));
            });
        }
    });
    // dragged toward the right edge: where it would dock (over the card, which sits there too)
    if ctx.dragged_id() == Some(Id::new(DRAG)) && moving(ctx) && !app.settings.session.minimized && near_dock(ctx) {
        let target = Rect::from_min_max(pos2(bounds.right() - 280.0, bounds.top() + TOP_BAR), bounds.max).shrink(8.0);
        let p = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("session-dock-target")));
        p.rect(target, CornerRadius::same(12), alpha(G1, 28), Stroke::new(1.5, alpha(G1, 170)), StrokeKind::Inside);
    }
}

/// Where the pane may float: the window under the title bar.
fn bounds(ctx: &egui::Context) -> Rect {
    let r = ctx.content_rect();
    Rect::from_min_max(pos2(r.left(), r.top() + titlebar::HEIGHT), r.max)
}

/// `pos` moved so a `size` pane at it stays inside `bounds` (top-left first, when it can't).
fn clamp(pos: Pos2, size: Vec2, bounds: Rect) -> Pos2 {
    pos2(pos.x.min(bounds.right() - size.x).max(bounds.left()), pos.y.min(bounds.bottom() - size.y).max(bounds.top()))
}

fn near_dock(ctx: &egui::Context) -> bool {
    let right = ctx.content_rect().right();
    ctx.input(|i| i.pointer.latest_pos()).is_some_and(|p| p.x >= right - DOCK_SNAP)
}

/// Moves the pane while `handle` is dragged. Pulled off the side, it floats from where `pane` was;
/// dropped at the right edge, it docks again.
fn drag(app: &mut App, ui: &Ui, handle: Rect, pane: Rect) {
    let resp = ui.interact(handle, Id::new(DRAG), Sense::drag());
    let ctx = ui.ctx();
    // a drag-only widget is "dragged" from the press on: only a real move counts, so the
    // buttons on top still get their clicks
    let moving = moving(ctx);
    // where the pane was when the move began; the pointer's offset from its press point moves it
    let start_id = Id::new(DRAG).with("start");
    let s = &mut app.settings.session;
    if resp.dragged() && moving {
        let start = ctx.data_mut(|d| {
            *d.get_temp_mut_or_insert_with(start_id, || {
                if s.docked {
                    // pulled off the side: it floats from where it was
                    s.docked = false;
                    [pane.left(), pane.top()]
                } else {
                    s.pos
                }
            })
        });
        let moved = ctx.input(|i| i.pointer.total_drag_delta()).unwrap_or_default();
        let p = clamp(pos2(start[0], start[1]) + moved, pane.size(), bounds(ctx).shrink(EDGE));
        s.pos = [p.x, p.y];
    }
    if resp.drag_stopped() {
        ctx.data_mut(|d| d.remove::<[f32; 2]>(start_id));
        if moving {
            if !s.minimized && near_dock(ctx) {
                s.docked = true;
            }
            app.dirty = true;
        }
    }
    let grabbing = resp.dragged() && moving;
    resp.on_hover_cursor(if grabbing { CursorIcon::Grabbing } else { CursorIcon::Grab });
}

/// The pointer is held down and has gone too far (or too long) for a click.
fn moving(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.pointer.is_decidedly_dragging())
}

/// "Session" with its grip, then the buttons: minimize, back to the side (when floating), reset.
fn header(app: &mut App, ui: &mut Ui, docked_pane: Option<Rect>) {
    let slots = app.screen == Screen::Slot;
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    if let Some(pane) = docked_pane {
        drag(app, ui, row, pane);
    }
    let p = ui.painter();
    widgets::icon(p, Rect::from_center_size(pos2(row.left() + 4.0, row.center().y), vec2(8.0, 12.0)), "grip", DIM);
    let title = p.text(pos2(row.left() + 16.0, row.center().y), Align2::LEFT_CENTER, if slots { "Slot session" } else { "Session" }, theme::font_bold(14.0), TEXT);
    // how long it has run: up to now while betting, else to the last bet
    let (s, running) = if slots { (&app.slot_stats, app.slot.running) } else { (&app.stats, app.auto.running) };
    if let (Some(start), Some(last)) = (s.started, s.last_at) {
        let end = if running { std::time::Instant::now() } else { last };
        p.text(pos2(title.right() + 10.0, row.center().y + 0.5), Align2::LEFT_CENTER, fmt_time(end.duration_since(start).as_millis()), theme::font_mono(11.0), DIM);
    }

    let mut buttons = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)));
    buttons.spacing_mut().item_spacing.x = 2.0;
    if widgets::glyph_button(&mut buttons, "minimize", "Minimize: just the profit").clicked() {
        app.settings.session.minimized = true;
        app.dirty = true;
    }
    if !app.settings.session.docked && widgets::glyph_button(&mut buttons, "dock", "Back to the side").clicked() {
        app.settings.session.docked = true;
        app.dirty = true;
    }
    if widgets::glyph_button(&mut buttons, "refresh", "Reset statistics").clicked() {
        if slots {
            app.slot_stats = super::stats::Stats::fresh(&app.slot_stats.cur.clone(), app.slot_stats.decimals);
        } else {
            app.reset_stats();
        }
    }
}

/// The minimized card: the profit, and a button that opens the whole pane again.
fn mini(app: &mut App, ui: &mut Ui) {
    ui.set_width(MINI_W - 16.0);
    ui.spacing_mut().item_spacing.y = 2.0;
    let (slots, cur, has) = currency(app);
    let s = if slots { &app.slot_stats } else { &app.stats };
    let value = if has { app.prim(s.profit, &cur, true) } else { "0".to_string() };
    let (color, bets) = (theme::pnl(s.profit), s.bets);

    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let p = ui.painter().clone();
    widgets::icon(&p, Rect::from_center_size(pos2(row.left() + 4.0, row.center().y), vec2(8.0, 12.0)), "grip", DIM);
    p.text(pos2(row.left() + 16.0, row.center().y), Align2::LEFT_CENTER, if slots { "Slot profit" } else { "Profit" }, theme::font_semi(11.5), MUTED);
    let mut buttons = ui.new_child(UiBuilder::new().max_rect(row).layout(Layout::right_to_left(Align::Center)));
    if widgets::glyph_button(&mut buttons, "expand", "Show the whole session").clicked() {
        app.settings.session.minimized = false;
        app.dirty = true;
    }

    let (vr, _) = ui.allocate_exact_size(vec2(ui.available_width() - 4.0, 24.0), Sense::hover());
    let bets = format!("{} bets", fmt_int(bets as i128));
    let bets_w = p.layout_no_wrap(bets.clone(), theme::font_semi(11.0), DIM).size().x;
    widgets::fit_text(&p, vr.left_center(), Align2::LEFT_CENTER, &value, theme::font_bold(20.0), color, vr.width() - bets_w - 10.0);
    p.text(vr.right_center(), Align2::RIGHT_CENTER, bets, theme::font_semi(11.0), DIM);
}

/// Whose numbers the pane shows (the slot's, on a slot's page), in which currency, and
/// whether that currency is known yet.
fn currency(app: &App) -> (bool, String, bool) {
    let slots = app.screen == Screen::Slot;
    let cur = if slots {
        app.slot_stats.cur.clone()
    } else if app.stats.cur.is_empty() {
        app.active_currency()
    } else {
        app.stats.cur.clone()
    };
    let has = slots || app.info(&cur).is_some();
    (slots, cur, has)
}

/// The profit card, the statistics and the ranked lists, scrolling. `max_h` caps their height
/// (floating); without it they fill what's left (docked).
fn body(app: &mut App, ui: &mut Ui, max_h: Option<f32>) {
    let scroll = match max_h {
        // an Area lays out in last frame's size: the minimum lets it grow back to what the numbers need
        Some(h) => egui::ScrollArea::vertical().auto_shrink([false, true]).min_scrolled_height(h).max_height(h),
        None => egui::ScrollArea::vertical().auto_shrink([false, false]),
    };
    scroll.show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
        let full = ui.available_width() - 2.0;
        let (slots, cur, has) = currency(app);
        let (per_s, per_m) = if slots { app.slot_stats.speed() } else { app.stats.speed() };
        let prim = |v: i128, sign: bool| if has { app.prim(v, &cur, sign) } else { "0".to_string() };
        let sec = |v: i128, sign: bool| if has { app.sec(v, &cur, sign) } else { String::new() };
        let s = if slots { &app.slot_stats } else { &app.stats };

        // profit, the number that matters most, with the curve under it
        let edge = s.edge_pct().map(|e| format!("{}{e:.2}%", if e >= 0.0 { "+" } else { "" }));
        let sub = [Some(sec(s.profit, true)).filter(|x| !x.is_empty()), edge].into_iter().flatten().collect::<Vec<_>>().join("  ·  ");
        profit_card(ui, full, &prim(s.profit, true), theme::pnl(s.profit), &sub, s.bets, &s.chart_points());

        let (streak, streak_c) = if s.streak > 0 {
            (format!("W{}", s.streak), GREEN)
        } else if s.streak < 0 {
            (format!("L{}", -s.streak), RED)
        } else {
            ("–".to_string(), TEXT)
        };
        let dash = |v: i128, sign: bool| if v == 0 { "–".to_string() } else { prim(v, sign) };
        let mut activity = vec![
            row("Speed", if s.bets > 0 { format!("{per_s:.1}/s") } else { "–".into() }, TEXT, if s.bets > 0 { format!("{}/m", fmt_int(per_m as i128)) } else { String::new() }),
            row("Latency", if s.latency > 0.0 { fmt_ms(s.latency) } else { "–".into() }, TEXT, ""),
            row("Wagered", prim(s.wagered, false), TEXT, sec(s.wagered, false)),
            row("Wins / losses", format!("{} / {}", fmt_int(s.wins as i128), fmt_int(s.losses as i128)), TEXT, s.win_pct().map(|p| format!("{p:.1}%")).unwrap_or_default()),
            row("RTP", s.rtp_pct().map(|r| format!("{r:.2}%")).unwrap_or("–".into()), TEXT, ""),
            row("Streak", streak, streak_c, if s.bets > 0 { format!("best W{} · L{}", s.best_win_streak(), s.worst_loss_streak()) } else { String::new() }),
        ];
        if s.hits > 0 {
            activity.push(row("Hits", fmt_int(s.hits as i128), TEXT, ""));
        }
        let mut extremes = vec![
            row("Highest profit", dash(s.peak, true), TEXT, ""),
            row("Lowest profit", dash(s.low, false), TEXT, ""),
            row("Biggest win", dash(s.hi_win, true), TEXT, ""),
            row("Biggest loss", dash(-s.hi_loss, false), TEXT, ""),
            row("Max drawdown", dash(-s.max_dd, false), TEXT, ""),
            row("Highest bet", dash(s.hi_bet, false), TEXT, ""),
        ];
        if s.retries > 0 {
            extremes.push(row("Busy retries", fmt_int(s.retries as i128), GOLD, ""));
        }
        stats_card(ui, full, &[activity, extremes]);

        let multis: Vec<Ranked> = s.top_multi.iter().map(|m| Ranked { value: fmt_multi(m.multi), amount: prim(m.profit, true), tip: Some("Open the card".into()) }).collect();
        let multi_ns: Vec<u64> = s.top_multi.iter().map(|m| m.n).collect();
        let streak_rows = |list: Vec<super::stats::Streak>, mark: &str| -> Vec<Ranked> {
            list.iter().map(|st| Ranked { value: format!("{mark}{}", st.len), amount: prim(st.profit, true), tip: None }).collect()
        };
        let lists = [
            ("Multipliers", multis, TEXT, true),
            ("Win streaks", streak_rows(s.win_streaks_now(), "W"), GREEN, false),
            ("Loss streaks", streak_rows(s.loss_streaks_now(), "L"), RED, false),
        ];
        let clicked = lists_card(ui, full, &lists);
        if let Some(n) = clicked.and_then(|i| multi_ns.get(i).copied()) {
            if slots {
                super::hits::open_spin(app, n);
            } else {
                super::hits::open_bet(app, n);
            }
        }
    });
}

fn card_frame() -> Frame {
    Frame::new().fill(BG2).stroke(Stroke::new(1.0, LINE)).corner_radius(CornerRadius::same(12))
}

fn profit_card(ui: &mut Ui, width: f32, value: &str, color: Color32, sub: &str, bets: u64, pts: &[f64]) {
    card_frame().inner_margin(Margin { left: 14, right: 14, top: 12, bottom: 12 }).show(ui, |ui| {
        ui.set_width(width - 30.0);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.label(semi("Profit", 11.5, MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(semi(format!("{} bets", fmt_int(bets as i128)), 11.0, DIM));
            });
        });
        let (vr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::hover());
        widgets::fit_text(ui.painter(), vr.left_center(), Align2::LEFT_CENTER, value, theme::font_bold(26.0), color, vr.width());
        let (sr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::hover());
        if !sub.is_empty() {
            widgets::fit_text(ui.painter(), sr.left_center(), Align2::LEFT_CENTER, sub, theme::font(11.0), DIM, sr.width());
        }
        ui.add_space(10.0);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 104.0), Sense::hover());
        draw_chart(ui, rect, if bets == 0 { &[] } else { pts });
    });
}

/// One line of the statistics: a grey label, the value on the right, and a dim note before it.
struct Row {
    label: &'static str,
    value: String,
    color: Color32,
    note: String,
}

fn row(label: &'static str, value: String, color: Color32, note: impl Into<String>) -> Row {
    Row { label, value, color, note: note.into() }
}

/// Height of a line of the statistics.
const STAT_H: f32 = 25.0;

/// The statistics in one card, label on the left and value on the right, in groups split by a hairline.
fn stats_card(ui: &mut Ui, width: f32, groups: &[Vec<Row>]) {
    let (pad, gap) = (8.0, 13.0);
    let lines: usize = groups.iter().map(Vec::len).sum();
    let height = pad * 2.0 + lines as f32 * STAT_H + gap * (groups.len().saturating_sub(1)) as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(12), BG2, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let (left, right) = (rect.left() + 14.0, rect.right() - 14.0);
    let mut y = rect.top() + pad;
    for (gi, group) in groups.iter().enumerate() {
        if gi > 0 {
            p.hline(left..=right, y + gap / 2.0, Stroke::new(1.0, LINE));
            y += gap;
        }
        for r in group {
            let cy = y + STAT_H / 2.0;
            let label = p.text(pos2(left, cy), Align2::LEFT_CENTER, r.label, theme::font(11.5), MUTED);
            let room = right - label.right() - 12.0;
            let value = p.layout_no_wrap(r.value.clone(), theme::font_semi(12.5), r.color);
            let value_w = value.size().x.min(room);
            widgets::fit_text(p, pos2(right, cy), Align2::RIGHT_CENTER, &r.value, theme::font_semi(12.5), r.color, room);
            if !r.note.is_empty() {
                let note_room = room - value_w - 8.0;
                if note_room > 24.0 {
                    widgets::fit_text(p, pos2(right - value_w - 8.0, cy), Align2::RIGHT_CENTER, &r.note, theme::font(11.0), DIM, note_room);
                }
            }
            y += STAT_H;
        }
    }
}

/// A row of a ranked list: the value in the list's colour, what it won or lost, and a hover note.
struct Ranked {
    value: String,
    amount: String,
    tip: Option<String>,
}

/// The ranked lists in one card, a tab each: (name, rows best first, value colour, rows clickable).
/// Past `LIST_ROWS` a list scrolls on its own, with the edge that has more to show fading out.
/// Returns the row clicked in a clickable list.
fn lists_card(ui: &mut Ui, width: f32, lists: &[(&str, Vec<Ranked>, Color32, bool)]) -> Option<usize> {
    let tab_id = ui.id().with("session-list-tab");
    let mut tab: usize = ui.data(|d| d.get_temp(tab_id)).unwrap_or(0).min(lists.len() - 1);
    let mut clicked = None;
    card_frame().inner_margin(Margin { left: 12, right: 10, top: 4, bottom: 8 }).show(ui, |ui| {
        ui.set_width(width - 24.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        // the tabs, underlined like the history's
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        // a third of the card each, so they fit however narrow the pane is
        let third = bar.width() / lists.len() as f32;
        for (i, (name, rows, _, _)) in lists.iter().enumerate() {
            let font = theme::font_semi(11.5);
            let g = ui.painter().layout_no_wrap(name.to_string(), font.clone(), TEXT);
            let cx = bar.left() + third * (i as f32 + 0.5);
            let r = Rect::from_center_size(pos2(cx, bar.center().y), vec2(g.size().x.min(third - 8.0), bar.height()));
            let resp = ui.interact(r.expand2(vec2(5.0, 0.0)), ui.id().with(("session-tab", i)), Sense::click());
            let color = if i == tab {
                TEXT
            } else if resp.hovered() {
                TEXT2
            } else {
                MUTED
            };
            let p = ui.painter();
            widgets::fit_text(p, r.center(), Align2::CENTER_CENTER, name, font, color, third - 8.0);
            if i == tab {
                p.rect_filled(Rect::from_min_max(pos2(r.left(), bar.bottom() - 2.0), pos2(r.right(), bar.bottom())), CornerRadius::same(1), G1);
            } else if !rows.is_empty() {
                p.circle_filled(pos2(r.right() + 5.0, r.center().y - 4.0), 2.0, alpha(TEXT2, 120));
            }
            if resp.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                tab = i;
            }
        }
        ui.painter().hline(bar.x_range(), bar.bottom(), Stroke::new(1.0, LINE));
        ui.add_space(4.0);

        let (name, rows, color, clickable) = &lists[tab];
        if rows.is_empty() {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, "None yet", theme::font(11.5), DIM);
            return;
        }
        let out = egui::ScrollArea::vertical().id_salt(*name).max_height(ROW_H * LIST_ROWS).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (i, r) in rows.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), if *clickable { Sense::click() } else { Sense::hover() });
                let p = ui.painter();
                if *clickable && resp.hovered() {
                    p.rect_filled(rect.shrink2(vec2(0.0, 1.0)), CornerRadius::same(6), alpha(Color32::WHITE, 8));
                }
                p.text(pos2(rect.left() + 9.0, rect.center().y), Align2::CENTER_CENTER, (i + 1).to_string(), theme::font_semi(10.5), DIM);
                let value = p.layout_no_wrap(r.value.clone(), theme::font_semi(12.5), *color);
                let value_w = value.size().x;
                p.galley(pos2(rect.left() + 26.0, rect.center().y - value.size().y / 2.0), value, *color);
                // clear of the scroll bar, which floats over the right edge
                let right = rect.right() - 10.0;
                let room = right - (rect.left() + 26.0 + value_w + 12.0);
                widgets::fit_text(p, pos2(right, rect.center().y), Align2::RIGHT_CENTER, &r.amount, theme::font(11.5), MUTED, room);
                let resp = if *clickable { resp.on_hover_cursor(CursorIcon::PointingHand) } else { resp };
                if *clickable && resp.clicked() {
                    clicked = Some(i);
                }
                if let Some(tip) = &r.tip {
                    resp.on_hover_text(tip);
                }
            }
        });
        let (view, top, content) = (out.inner_rect, out.state.offset.y, out.content_size.y);
        let p = ui.painter();
        if top > 0.5 {
            fade(p, Rect::from_min_size(view.min, vec2(view.width() - 8.0, 14.0)), BG2, true);
        }
        if top + view.height() < content - 0.5 {
            fade(p, Rect::from_min_max(pos2(view.left(), view.bottom() - 14.0), pos2(view.right() - 8.0, view.bottom())), BG2, false);
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, tab));
    // only the multipliers' rows open something
    clicked.filter(|_| lists[tab].3)
}

/// `color` fading to clear across `rect`: from its top edge when `from_top`, else from its bottom.
fn fade(p: &egui::Painter, rect: Rect, color: Color32, from_top: bool) {
    let (top, bottom) = if from_top { (color, Color32::TRANSPARENT) } else { (Color32::TRANSPARENT, color) };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    p.add(Shape::mesh(mesh));
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
