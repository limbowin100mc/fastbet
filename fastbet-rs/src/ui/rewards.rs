//! The Rewards page (casinos with rewards): the VIP level, then a card per reward (rakeback,
//! the daily, weekly and monthly bonuses …) with its worth and a Claim button, or when it comes.

use std::collections::HashSet;
use std::time::{Duration, Instant, SystemTime};

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, BG1, BG2, DIM, G1, GOLD, LINE, LINE2, MUTED, RED, TEXT, TEXT2};
use super::widgets::{self, button, Btn};
use super::{vip, App};
use crate::casino::{Reward, RewardKind};
use crate::engine::Command;
use crate::money::fmt_usd;

/// The page's column is at most this wide, centred.
const MAX_W: f32 = 1040.0;
const CARD_H: f32 = 196.0;
const GAP: f32 = 14.0;

#[derive(Default)]
pub struct State {
    pub list: Vec<Reward>,
    pub loaded: bool,
    pub error: Option<String>,
    pub asked_at: Option<Instant>,
    /// Rewards with a claim in flight.
    pub claiming: HashSet<String>,
}

impl State {
    /// Something is waiting to be claimed (the rail button shows a dot).
    pub fn any_claimable(&self) -> bool {
        self.list.iter().any(|r| r.claimable)
    }
}

/// Asks for the rewards when the last answer is older than `every`.
pub fn refresh(app: &mut App, every: Duration) {
    if !app.casino().has_rewards() || !app.ready() {
        return;
    }
    if app.rewards.asked_at.is_none_or(|t| t.elapsed() >= every) {
        app.rewards.asked_at = Some(Instant::now());
        app.send(Command::Rewards);
    }
}

/// "2d 15h", "4h 12m", "12m 05s": the time left until `t`.
pub fn until(t: SystemTime) -> String {
    let s = t.duration_since(SystemTime::now()).map(|d| d.as_secs()).unwrap_or(0);
    let (d, h, m) = (s / 86_400, (s / 3600) % 24, (s / 60) % 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m {:02}s", s % 60)
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    // a countdown that ran out means something new to claim
    let due = app.rewards.list.iter().any(|r| r.next_at.is_some_and(|t| t <= SystemTime::now()));
    refresh(app, if due { Duration::from_secs(5) } else { Duration::from_secs(60) });
    ui.ctx().request_repaint_after(Duration::from_secs(1));

    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let avail = ui.available_width();
        let w = (avail - 80.0).min(MAX_W);
        ui.horizontal(|ui| {
            ui.add_space(((avail - w) / 2.0).max(40.0));
            ui.vertical(|ui| {
                ui.set_width(w);
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add_space(40.0);
                ui.label(widgets::bold("Rewards", 28.0, TEXT));
                ui.add_space(10.0);
                // where claims take the coin picked in the wallet, say which; elsewhere the site or the earned coins decide
                let sub = if matches!(app.casino(), crate::casino::CasinoId::ChipsGg | crate::casino::CasinoId::Shuffle) {
                    format!("Rakeback and VIP bonuses, paid in {}.", app.active_currency().to_uppercase())
                } else {
                    format!("Rakeback and VIP bonuses on {}.", app.casino().name())
                };
                ui.label(egui::RichText::new(sub).size(13.5).color(MUTED));
                ui.add_space(24.0);

                if let Some(v) = app.vip.clone() {
                    hero(ui, w, &v);
                    ui.add_space(GAP);
                }
                if let Some(e) = app.rewards.error.clone() {
                    ui.horizontal(|ui| {
                        ui.label(widgets::semi(format!("Couldn't load the rewards: {e}"), 12.0, RED));
                        if button(ui, "Retry", Btn::Ghost, app.ready(), Some(72.0)).clicked() {
                            app.rewards.asked_at = None;
                        }
                    });
                    ui.add_space(12.0);
                }
                if app.rewards.loaded && app.rewards.list.is_empty() {
                    ui.label(widgets::semi("Nothing to claim right now", 12.0, DIM));
                } else if app.rewards.loaded {
                    cards(app, ui, w);
                } else {
                    ui.label(widgets::semi(if app.ready() { "Loading…" } else { "Connecting…" }, 12.0, DIM));
                }
                ui.add_space(32.0);
            });
        });
    });
}

/// The VIP level: a big ring filling towards the next rank, then how far the next rank and the
/// next level are.
fn hero(ui: &mut Ui, w: f32, v: &crate::casino::VipStatus) {
    let (r, resp) = ui.allocate_exact_size(vec2(w, 148.0), Sense::hover());
    let p = ui.painter().with_clip_rect(r);
    p.rect(r, CornerRadius::same(14), BG1, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let c = pos2(r.left() + 82.0, r.center().y);
    widgets::glow(&p, c, 150.0, alpha(G1, 22));
    vip::ring(&p, c, 50.0, 7.0, v.rank_progress() as f32);
    p.text(c - vec2(0.0, 6.0), Align2::CENTER_CENTER, format!("{:.0}%", v.rank_progress() * 100.0), theme::font_bold(22.0), TEXT);
    let next = v.next_rank.as_ref().map(|(n, _)| n.as_str()).unwrap_or("Top rank");
    widgets::fit_text(&p, c + vec2(0.0, 16.0), Align2::CENTER_CENTER, next, theme::font_semi(10.5), MUTED, 80.0);

    let x = r.left() + 164.0;
    let title = p.text(pos2(x, r.top() + 38.0), Align2::LEFT_CENTER, &v.rank, theme::font_bold(22.0), TEXT);
    if let Some(level) = v.level {
        p.text(pos2(title.right() + 12.0, r.top() + 40.0), Align2::LEFT_CENTER, format!("Level {level}"), theme::font_semi(13.0), MUTED);
    }
    let goals: Vec<(String, f64, f64)> = v
        .next_rank
        .iter()
        .map(|(n, _)| (n.clone(), v.xp_to_rank().unwrap_or(0.0), v.rank_progress()))
        .chain(v.level.map(|l| (format!("Level {}", l + 1), v.xp_to_level(), v.progress())))
        .collect();
    let right = r.right() - 24.0;
    let bar_l = x + 110.0;
    let bar_r = right - 190.0;
    for (i, (name, xp, t)) in goals.iter().enumerate() {
        let y = r.top() + 82.0 + i as f32 * 32.0;
        p.text(pos2(x, y), Align2::LEFT_CENTER, name, theme::font_semi(12.5), TEXT2);
        if bar_r - bar_l > 60.0 {
            let bar = Rect::from_min_max(pos2(bar_l, y - 3.0), pos2(bar_r, y + 3.0));
            p.rect_filled(bar, CornerRadius::same(3), LINE2);
            if *t > 0.0 {
                p.rect_filled(Rect::from_min_size(bar.min, vec2((bar.width() * *t as f32).max(6.0), 6.0)), CornerRadius::same(3), G1);
            }
        }
        let usd = vip::usd_text(v, *xp).unwrap_or_else(|| "–".into());
        let usd_r = p.text(pos2(right, y), Align2::RIGHT_CENTER, usd, theme::font_bold(13.0), TEXT);
        if let Some(xp) = vip::xp_text(v, *xp) {
            p.text(pos2(usd_r.left() - 12.0, y), Align2::RIGHT_CENTER, xp, theme::font_semi(12.0), MUTED);
        }
    }
    resp.on_hover_text(vip::rate_tip(v));
}

fn cards(app: &mut App, ui: &mut Ui, w: f32) {
    let list = app.rewards.list.clone();
    let cols = (((w + GAP) / (210.0 + GAP)).floor() as usize).clamp(1, 4);
    let card_w = (w - GAP * (cols - 1) as f32) / cols as f32;
    let rows = list.len().div_ceil(cols);
    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (CARD_H + GAP) - GAP), Sense::hover());
    let mut claim: Option<String> = None;
    for (i, r) in list.iter().enumerate() {
        let at = pos2(area.left() + (i % cols) as f32 * (card_w + GAP), area.top() + (i / cols) as f32 * (CARD_H + GAP));
        if card(app, ui, Rect::from_min_size(at, vec2(card_w, CARD_H)), r) {
            match &r.link {
                // the casino takes this one on its own site
                Some(url) => ui.ctx().open_url(egui::OpenUrl::new_tab(url)),
                None => claim = Some(r.id.clone()),
            }
        }
    }
    if let Some(id) = claim {
        let currency = app.active_currency();
        app.rewards.claiming.insert(id.clone());
        app.send(Command::ClaimReward { id, currency });
    }
}

fn icon_of(kind: RewardKind) -> &'static str {
    match kind {
        RewardKind::Rakeback => "coins",
        RewardKind::Daily => "sun",
        RewardKind::Weekly | RewardKind::Monthly => "calendar",
        RewardKind::LevelUp => "arrow-up",
        RewardKind::Reload => "refresh",
        RewardKind::Bonus => "gift",
        RewardKind::Challenge => "trophy",
    }
}

/// One reward. Returns true when Claim was pressed.
fn card(app: &App, ui: &mut Ui, rect: Rect, r: &Reward) -> bool {
    let resp = ui.interact(rect, ui.id().with(("reward-card", &r.id)), Sense::hover());
    let p = ui.painter().with_clip_rect(rect);
    p.rect(rect, CornerRadius::same(14), BG1, Stroke::new(1.0, if r.claimable { alpha(G1, 100) } else { LINE }), StrokeKind::Inside);
    if r.claimable {
        widgets::glow(&p, rect.left_top() + vec2(36.0, 36.0), 130.0, alpha(G1, 20));
    }
    let inner = rect.shrink2(vec2(18.0, 16.0));
    let ic = inner.left_top() + vec2(18.0, 18.0);
    p.circle_filled(ic, 18.0, if r.claimable { alpha(G1, 30) } else { BG2 });
    p.circle_stroke(ic, 18.0, Stroke::new(1.0, if r.claimable { alpha(G1, 90) } else { LINE2 }));
    widgets::icon(&p, Rect::from_center_size(ic, vec2(16.0, 16.0)), icon_of(r.kind), if r.claimable { G1 } else { TEXT2 });
    let worth = r.usd.filter(|v| *v > 0.0);
    // top right: just "Ready"; when it expires goes under the amount
    let ready_w = if r.claimable {
        p.text(pos2(inner.right(), ic.y), Align2::RIGHT_CENTER, "Ready", theme::font_semi(10.5), G1).width() + 10.0
    } else {
        0.0
    };
    widgets::fit_text(&p, pos2(inner.left() + 46.0, ic.y), Align2::LEFT_CENTER, &r.title, theme::font_semi(13.5), TEXT, inner.width() - 46.0 - ready_w);
    // the big line: what it pays, else that it's ready, else how long until it is
    // dust reads as such, not as $0.0000
    let money = |v: f64| if v < 0.0001 { "< $0.0001".to_string() } else { fmt_usd(v) };
    let value = match (worth, r.claimable, r.next_at) {
        (Some(v), true, _) => money(v),
        (None, true, _) => "Ready".into(),
        (_, false, Some(t)) => until(t),
        (Some(v), false, None) => money(v),
        _ if r.kind == RewardKind::Rakeback => fmt_usd(0.0),
        _ => "–".into(),
    };
    p.text(pos2(inner.left(), inner.top() + 66.0), Align2::LEFT_CENTER, value, theme::font_bold(28.0), if r.claimable { TEXT } else { MUTED });
    widgets::fit_text(&p, pos2(inner.left(), inner.top() + 96.0), Align2::LEFT_CENTER, &r.detail, theme::font(11.5), MUTED, inner.width());
    if let Some(t) = r.expires.filter(|_| r.claimable) {
        p.text(pos2(inner.left(), inner.top() + 114.0), Align2::LEFT_CENTER, format!("Expires in {}", until(t)), theme::font_semi(11.0), GOLD);
    }

    let claiming = app.rewards.claiming.contains(&r.id);
    let on_site = format!("Claim on {}", app.casino().name());
    let label = if claiming {
        "Claiming…"
    } else if r.link.is_some() && r.claimable {
        on_site.as_str()
    } else {
        "Claim"
    };
    let br = Rect::from_min_size(pos2(inner.left(), inner.bottom() - widgets::FIELD_H), vec2(inner.width(), widgets::FIELD_H));
    let mut bui = ui.new_child(egui::UiBuilder::new().max_rect(br).id_salt(("reward", &r.id)));
    let enabled = r.claimable && !claiming && (app.ready() || r.link.is_some());
    let kind = if r.claimable { Btn::Primary } else { Btn::Ghost };
    let clicked = button(&mut bui, label, kind, enabled, Some(br.width())).clicked();
    if resp.hovered() && !r.detail.is_empty() {
        resp.on_hover_text(&r.detail);
    }
    clicked
}

/// Colour of the dot on the rail button while something waits to be claimed.
pub const DOT: Color32 = G1;
