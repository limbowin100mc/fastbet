//! Lobby: a short header and one card per game.

use egui::{pos2, vec2, Align2, Color32, CornerRadius, Mesh, Painter, Rect, Sense, Shape, Stroke, StrokeKind, Ui};

use super::theme::{self, alpha, mix, G1, MUTED, TEXT, TEXT2};
use super::widgets;
use super::App;
use crate::money;
use crate::slots::Provider;

const CARD_W: f32 = 264.0;
const CARD_H: f32 = 336.0;
const GAP: f32 = 24.0;

enum Art {
    Limbo,
    Slots,
}

struct Game {
    key: &'static str,
    name: &'static str,
    tag: &'static str,
    badge: String,
    line: String,
    colors: [Color32; 4],
    art: Art,
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let full = ui.max_rect();
    widgets::dot_grid(ui, full);
    let p = ui.painter_at(full);
    widgets::glow(&p, pos2(full.left() + 160.0, full.top() - 60.0), 560.0, alpha(G1, 22));

    let x0 = full.left() + 40.0;
    let mut y = full.top() + 40.0;
    p.text(pos2(x0, y), Align2::LEFT_TOP, "Lobby", theme::font_bold(28.0), TEXT);
    y += 42.0;
    p.text(pos2(x0, y), Align2::LEFT_TOP, format!("Instant bets on {}. No animations, no delays.", app.casino().name()), theme::font(13.5), MUTED);
    y += 52.0;

    let id = app.casino();
    let providers: Vec<&str> = Provider::SUPPORTED.iter().map(|p| Provider::display_producer(p)).collect();
    let mut games = vec![
        Game {
            key: "limbo",
            name: "Limbo",
            tag: "Originals",
            badge: format!("{}% edge", trim_pct(100.0 - id.win_chance(1.0).min(100.0))),
            line: format!("Targets up to {}×", money::group_thousands(&format!("{}", id.max_target() as i64))),
            colors: [
                Color32::from_rgb(0x2f, 0xe8, 0xa8),
                Color32::from_rgb(0x15, 0xc4, 0x8a),
                Color32::from_rgb(0x0b, 0x86, 0x6a),
                Color32::from_rgb(0x07, 0x52, 0x4c),
            ],
            art: Art::Limbo,
        },
    ];
    if id.has_slots() {
        games.push(Game {
            key: "slots",
            name: "Slots",
            tag: "Autoplay",
            badge: format!("{} providers", providers.len()),
            line: providers.join(" · "),
            colors: [
                Color32::from_rgb(0xa8, 0x6b, 0xff),
                Color32::from_rgb(0x7f, 0x45, 0xf0),
                Color32::from_rgb(0x4d, 0x22, 0xb0),
                Color32::from_rgb(0x2a, 0x12, 0x78),
            ],
            art: Art::Slots,
        });
    }

    p.text(pos2(x0, y), Align2::LEFT_CENTER, "Games", theme::font_semi(14.0), TEXT2);
    let gw = p.layout_no_wrap("Games".into(), theme::font_semi(14.0), TEXT2).size().x;
    p.text(pos2(x0 + gw + 8.0, y), Align2::LEFT_CENTER, games.len().to_string(), theme::font_semi(12.0), theme::DIM);
    y += 22.0;

    let avail = full.right() - 40.0 - x0;
    let cols = ((avail + GAP) / (CARD_W + GAP)).floor().max(1.0) as usize;
    for (i, g) in games.iter().enumerate() {
        let col = (i % cols) as f32;
        let row = (i / cols) as f32;
        let rect = Rect::from_min_size(pos2(x0 + col * (CARD_W + GAP), y + row * (CARD_H + GAP)), vec2(CARD_W, CARD_H));
        if card(ui, rect, g).clicked() {
            match g.art {
                Art::Limbo => app.go_limbo(),
                Art::Slots => app.go_slots(),
            }
        }
    }
}

fn trim_pct(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn card(ui: &mut Ui, rect: Rect, g: &Game) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with(("game-card", g.key)), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id.with("lift"), resp.hovered(), 0.14);
    let r = rect.translate(vec2(0.0, -4.0 * t));
    let p = ui.painter();
    widgets::shadow(p, r, 18, 10.0 + 8.0 * t, 24.0 + 16.0 * t, (110.0 + 50.0 * t) as u8);
    widgets::gradient_rect(p, r, 18, g.colors);

    // light from the top-right; the falloff ends before the rounded corner so the clip never shows
    let lit = ui.painter_at(r);
    widgets::glow(&lit, pos2(r.right() - 80.0, r.top() + 100.0), 128.0, alpha(Color32::WHITE, (30.0 + 20.0 * t) as u8));

    let art = Rect::from_min_max(pos2(r.left() + 22.0, r.top() + 62.0), pos2(r.right() - 22.0, r.top() + r.height() * 0.64));
    match g.art {
        Art::Limbo => limbo_art(p, art),
        Art::Slots => slots_art(ui, art),
    }
    let p = ui.painter();

    // tag and badge
    let tag_g = p.layout_no_wrap(g.tag.to_uppercase(), theme::font_bold(10.0), Color32::WHITE);
    let tr = Rect::from_min_size(r.left_top() + vec2(16.0, 16.0), tag_g.size() + vec2(16.0, 9.0));
    p.rect_filled(tr, CornerRadius::same(7), alpha(Color32::WHITE, 40));
    p.galley(tr.min + vec2(8.0, 4.5), tag_g, Color32::WHITE);
    let badge_g = p.layout_no_wrap(g.badge.clone(), theme::font_semi(11.0), Color32::WHITE);
    let br = Rect::from_min_size(pos2(r.right() - 16.0 - badge_g.size().x - 16.0, r.top() + 16.0), badge_g.size() + vec2(16.0, 8.0));
    p.rect_filled(br, CornerRadius::same(7), alpha(Color32::BLACK, 70));
    p.galley(br.min + vec2(8.0, 4.0), badge_g, Color32::WHITE);

    // name and one line of detail
    p.text(pos2(r.left() + 20.0, r.bottom() - 46.0), Align2::LEFT_BOTTOM, g.name, theme::font_bold(30.0), Color32::WHITE);
    widgets::fit_text(p, pos2(r.left() + 21.0, r.bottom() - 22.0), Align2::LEFT_BOTTOM, &g.line, theme::font_semi(12.5), alpha(Color32::WHITE, 200), r.width() - 42.0);

    // hairline edge, brighter on hover
    p.rect_stroke(r, CornerRadius::same(18), Stroke::new(1.0, alpha(Color32::WHITE, (30.0 + 50.0 * t) as u8)), StrokeKind::Inside);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A multiplier curve climbing to its result, with faint gridlines.
fn limbo_art(p: &Painter, r: Rect) {
    let white = Color32::WHITE;
    for (i, lbl) in ["10×", "5×", "2×"].iter().enumerate() {
        let y = r.top() + 18.0 + i as f32 * (r.height() - 30.0) / 3.0;
        let mut x = r.left();
        while x < r.right() {
            p.line_segment([pos2(x, y), pos2((x + 4.0).min(r.right()), y)], Stroke::new(1.0, alpha(white, 34)));
            x += 9.0;
        }
        p.text(pos2(r.left(), y - 3.0), Align2::LEFT_BOTTOM, *lbl, theme::font_semi(10.0), alpha(white, 130));
    }
    let base = r.bottom();
    let n = 40;
    let k = 3.4f32;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let x = i as f32 / n as f32;
            let y = ((k * x).exp() - 1.0) / (k.exp() - 1.0);
            pos2(r.left() + 8.0 + x * (r.width() - 40.0), base - 6.0 - y * (r.height() - 40.0))
        })
        .collect();
    // area under the curve, fading towards the baseline
    let mut mesh = Mesh::default();
    for w in pts.windows(2) {
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], alpha(white, 70));
        mesh.colored_vertex(w[1], alpha(white, 70));
        mesh.colored_vertex(pos2(w[1].x, base), alpha(white, 0));
        mesh.colored_vertex(pos2(w[0].x, base), alpha(white, 0));
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    p.add(Shape::mesh(mesh));
    p.add(Shape::line(pts.clone(), Stroke::new(3.0, white)));
    let end = *pts.last().unwrap();
    p.circle_filled(end, 11.0, alpha(white, 50));
    p.circle_filled(end, 5.5, white);
    let chip = Rect::from_min_size(pos2(end.x - 86.0, end.y - 12.0), vec2(70.0, 24.0));
    p.rect_filled(chip, CornerRadius::same(12), alpha(Color32::BLACK, 70));
    p.text(chip.center(), Align2::CENTER_CENTER, "24.80×", theme::font_bold(12.5), white);
}

/// Three reels showing sevens on a gold payline; neighbours peek in, clipped.
fn slots_art(ui: &Ui, r: Rect) {
    let reel_w = 56.0;
    let reel_h = 92.0;
    let gap = 12.0;
    let total = reel_w * 3.0 + gap * 2.0;
    let x0 = r.center().x - total / 2.0;
    let cy = r.center().y + 6.0;
    let red = Color32::from_rgb(0xe8, 0x33, 0x4f);
    let cream = Color32::from_rgb(0xfb, 0xf5, 0xe8);
    let p = ui.painter();
    let frame = Rect::from_min_max(pos2(x0 - 10.0, cy - reel_h / 2.0 - 10.0), pos2(x0 + total + 10.0, cy + reel_h / 2.0 + 10.0));
    p.rect_filled(frame, CornerRadius::same(14), alpha(Color32::BLACK, 60));
    for i in 0..3 {
        let rr = Rect::from_min_size(pos2(x0 + i as f32 * (reel_w + gap), cy - reel_h / 2.0), vec2(reel_w, reel_h));
        widgets::gradient_rect(p, rr, 9, [cream, cream, mix(cream, Color32::from_rgb(0xc9, 0xb8, 0x9a), 0.55), mix(cream, Color32::from_rgb(0xc9, 0xb8, 0x9a), 0.55)]);
        let clip = ui.painter_at(rr.shrink(1.0));
        for (dy, a) in [(-60.0, 60u8), (0.0, 255), (60.0, 60)] {
            clip.text(pos2(rr.center().x, rr.center().y + dy), Align2::CENTER_CENTER, "7", theme::font_bold(40.0), alpha(red, a));
        }
    }
    // payline
    let y = cy;
    p.line_segment([pos2(frame.left() - 4.0, y), pos2(frame.right() + 4.0, y)], Stroke::new(2.0, alpha(theme::GOLD, 210)));
    for (x, d) in [(frame.left() - 4.0, 1.0f32), (frame.right() + 4.0, -1.0)] {
        let tri = vec![pos2(x, y - 6.0), pos2(x + 8.0 * d, y), pos2(x, y + 6.0)];
        p.add(Shape::convex_polygon(tri, theme::GOLD, Stroke::NONE));
    }
}
