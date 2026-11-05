//! The banner over the Originals and Slots lobbies, the way Antebot heads its pages: the FASTBET
//! wordmark with the page's name under it, on a band of the page's colour, and cards on its right
//! for the casino's challenges and rewards.

use egui::text::{LayoutJob, TextFormat};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, mix, BG, BG1, G1, GOLD, LINE, TEXT, TEXT2};
use super::{challenges, widgets, App};

pub const HEIGHT: f32 = 164.0;
const CARD: egui::Vec2 = vec2(216.0, 92.0);
const CARD_GAP: f32 = 14.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Originals,
    Slots,
}

impl Page {
    fn name(self) -> &'static str {
        match self {
            Page::Originals => "Originals",
            Page::Slots => "Slots",
        }
    }

    /// The band's colour: deep enough for the wordmark's red and yellow to stand out on it.
    fn tint(self) -> Color32 {
        match self {
            Page::Originals => Color32::from_rgb(0x0b, 0x74, 0x80),
            Page::Slots => Color32::from_rgb(0xa8, 0x74, 0x0a),
        }
    }
}

#[derive(Clone, Copy)]
enum Icon {
    Trophy,
    Crown,
    Gift,
}

#[derive(Clone, Copy)]
enum Go {
    Challenges,
    Rewards,
}

struct Card {
    tag: &'static str,
    title: String,
    sub: String,
    icon: Icon,
    color: Color32,
    go: Go,
}

/// Paints the banner into `rect` (the full width of the page, [`HEIGHT`] tall).
pub fn show(app: &mut App, ui: &mut Ui, rect: Rect, page: Page) {
    let p = ui.painter_at(rect);
    let tint = page.tint();
    // dark under the wordmark, the page's colour towards the right
    p.rect_filled(rect, CornerRadius::ZERO, BG1);
    let fade = Rect::from_min_max(pos2(rect.left() + rect.width() * 0.28, rect.top()), rect.max);
    widgets::gradient_rect(&p, fade, 0, [BG1, mix(BG1, tint, 0.62), BG1, mix(BG1, tint, 0.34)]);
    widgets::glow(&p, pos2(rect.right() - 160.0, rect.top() + 10.0), 420.0, alpha(tint, 80));
    dots(&p, rect);
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, LINE));

    // FASTBET, and the page's name under it in italics
    let x = rect.left() + 56.0;
    let mark = Rect::from_min_size(pos2(x, rect.top() + 40.0), widgets::wordmark_size(42.0));
    widgets::paint_wordmark_in(&p, mark);
    let mut job = LayoutJob::default();
    job.append(page.name(), 0.0, TextFormat { font_id: theme::font_semi(32.0), color: TEXT, italics: true, ..Default::default() });
    let name = p.layout_job(job);
    let name_right = mark.left() + name.size().x;
    p.galley(pos2(mark.left() - 2.0, mark.bottom() + 2.0), name, TEXT);

    // the cards, right to left, as many as fit beside the title
    let cards = cards(app);
    let mut right = rect.right() - 28.0;
    let mut go = None;
    for (i, c) in cards.iter().enumerate().rev() {
        let left = right - CARD.x;
        if left < mark.right().max(name_right) + 48.0 {
            break;
        }
        let r = Rect::from_min_size(pos2(left, rect.center().y - CARD.y / 2.0), CARD);
        if card(ui, r, c, i) {
            go = Some(c.go);
        }
        right = left - CARD_GAP;
    }
    match go {
        Some(Go::Rewards) => app.go_rewards(),
        Some(Go::Challenges) => {
            if page == Page::Originals {
                app.go_slots();
            }
            app.challenges.window_open = true;
        }
        None => {}
    }
}

/// A faint dotted pattern over the band, fading out towards its left.
fn dots(p: &egui::Painter, rect: Rect) {
    let step = 16.0;
    let mut y = rect.top() + step / 2.0;
    while y < rect.bottom() {
        let mut x = rect.left() + step / 2.0;
        while x < rect.right() {
            let t = ((x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            p.circle_filled(pos2(x, y), 0.9, alpha(Color32::WHITE, (5.0 + 12.0 * t) as u8));
            x += step;
        }
        y += step;
    }
}

/// The casino's challenges and rewards, where it has them.
fn cards(app: &mut App) -> Vec<Card> {
    let casino = app.casino();
    let mut out = Vec::new();
    if casino.has_challenges() {
        challenges::refresh(app);
        let list = &app.challenges.list;
        let running = list.iter().filter(|c| !c.koth).count();
        let top = list.iter().map(|c| c.prize_usd).fold(0.0, f64::max);
        if let Some(k) = app.challenges.koth() {
            out.push(Card {
                tag: "LIVE",
                title: "King of the Hill".into(),
                sub: format!("{} · pool {}", k.game.name, challenges::dollars(k.prize_usd)),
                icon: Icon::Crown,
                color: GOLD,
                go: Go::Challenges,
            });
        }
        if running > 0 {
            out.push(Card {
                tag: "CHALLENGES",
                title: format!("{running} challenge{}", if running == 1 { "" } else { "s" }),
                sub: format!("Prizes up to {}", challenges::dollars(top)),
                icon: Icon::Trophy,
                color: GOLD,
                go: Go::Challenges,
            });
        }
    }
    if casino.has_rewards() {
        let ready = app.rewards.any_claimable();
        out.push(Card {
            tag: "REWARDS",
            title: if ready { "Ready to claim".into() } else { "Rewards".into() },
            sub: "Rakeback and bonuses".into(),
            icon: Icon::Gift,
            color: G1,
            go: Go::Rewards,
        });
    }
    out
}

/// A card: its tag, what it is, and its icon large on the right. True when clicked.
fn card(ui: &mut Ui, r: Rect, c: &Card, i: usize) -> bool {
    let resp = ui.interact(r, ui.id().with(("banner-card", i)), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let r = r.translate(vec2(0.0, -2.0 * t));
    let p = ui.painter();
    widgets::shadow(p, r, 12, 6.0, 16.0, 90);
    p.rect(r, CornerRadius::same(12), mix(alpha(BG, 200), alpha(BG1, 235), t), Stroke::new(1.0, alpha(mix(Color32::WHITE, c.color, t), (28.0 + 90.0 * t) as u8)), StrokeKind::Inside);

    // the icon, large, in a soft light of its colour
    let ic = pos2(r.right() - 44.0, r.center().y);
    widgets::glow(p, ic, 46.0, alpha(c.color, (40.0 + 30.0 * t) as u8));
    let ir = Rect::from_center_size(ic, vec2(42.0, 42.0));
    match c.icon {
        Icon::Trophy => widgets::paint_trophy(p, ir, c.color),
        Icon::Crown => widgets::paint_crown(p, ir, c.color),
        Icon::Gift => widgets::paint_gift(p, ir, c.color, t),
    }

    let x = r.left() + 14.0;
    let max_w = ic.x - 28.0 - x;
    let tag = p.layout_no_wrap(c.tag.into(), theme::font_bold(9.5), c.color);
    let tr = Rect::from_min_size(pos2(x, r.top() + 13.0), tag.size() + vec2(12.0, 5.0));
    p.rect_filled(tr, CornerRadius::same(5), alpha(c.color, 36));
    p.galley(tr.min + vec2(6.0, 2.5), tag, c.color);
    widgets::fit_text(p, pos2(x, r.top() + 50.0), Align2::LEFT_CENTER, &c.title, theme::font_bold(15.0), TEXT, max_w);
    widgets::fit_text(p, pos2(x, r.top() + 70.0), Align2::LEFT_CENTER, &c.sub, theme::font(11.5), TEXT2, max_w);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}
