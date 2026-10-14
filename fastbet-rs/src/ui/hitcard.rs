//! The hit card: a picture of a hit to keep or share (the game, what it paid, the win), drawn
//! at 1200×675 with resvg so the download is sharp; the app shows the same picture.
//!
//! Layers, back to front: the background (gradients, glows, speed streaks), the FastBet
//! wordmark's letters huge and faint, the game's artwork in a rounded frame, then the text
//! in Saira, the wordmark's own typeface (bundled, SIL Open Font License, assets/fonts).

use resvg::tiny_skia::{self, FillRule, FilterQuality, Mask, Paint, PathBuilder, Pattern, Pixmap, PixmapPaint, SpreadMode, Transform};
use resvg::usvg;

use crate::money::fmt_multi;

pub const W: u32 = 1200;
pub const H: u32 = 675;
const RADIUS: f32 = 36.0;
/// The multiplier's font size when it fits.
const MULT_SIZE: f32 = 156.0;
/// Room the wordmark SVG leaves around its letters for the glow, as a fraction of their height.
const WORDMARK_GLOW: f32 = 0.18;

const SAIRA_BLACK_ITALIC: &[u8] = include_bytes!("../../assets/fonts/Saira-BlackItalic.ttf");
const SAIRA_BOLD: &[u8] = include_bytes!("../../assets/fonts/Saira-Bold.ttf");
const SAIRA_SEMIBOLD: &[u8] = include_bytes!("../../assets/fonts/Saira-SemiBold.ttf");
const WORDMARK: &[u8] = include_bytes!("../../assets/wordmark.svg");

/// What the card shows.
#[derive(Clone, Debug)]
pub struct Card {
    pub game: String,
    /// Who made it, and where: "Hacksaw Gaming · Shuffle.com", "Originals · Stake.com".
    pub byline: String,
    /// What the hit paid, as a multiple of the stake.
    pub multiplier: f64,
    pub win: String,
    pub bet: String,
    /// "14 Oct 2026 · 16:30"
    pub when: String,
    /// Played for fun money.
    pub demo: bool,
    /// A hit (the stop-on-hit line, or a big win): the pill says HIT; any other win says WIN.
    pub hit: bool,
}

/// Draws the card. `art` is the game's artwork (PNG / JPEG / WebP bytes), if there is any.
pub fn render(card: &Card, art: Option<&[u8]>) -> Result<Pixmap, String> {
    let mut pix = Pixmap::new(W, H).ok_or("no pixmap")?;
    let opt = options();

    draw_svg(&mut pix, &background(), &opt)?;
    watermark(&mut pix)?;

    let art = art.and_then(decode);
    let frame = match &art {
        // portrait art (Stake, Shuffle) stands tall; chips.gg's is 4:3
        Some(a) if a.width() > a.height() => Frame { x: 64.0, y: 196.0, w: 372.0, h: 279.0 },
        _ => Frame { x: 72.0, y: 136.0, w: 304.0, h: 403.0 },
    };
    draw_svg(&mut pix, &art_backdrop(&frame, card), &opt)?;
    if let Some(a) = &art {
        paint_art(&mut pix, a, &frame);
    }
    draw_svg(&mut pix, &foreground(card, &frame, &opt)?, &opt)?;
    small_wordmark(&mut pix)?;

    // rounded corners
    let mut mask = Mask::new(W, H).ok_or("no mask")?;
    if let Some(path) = rounded_rect(0.0, 0.0, W as f32, H as f32, RADIUS) {
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
    }
    pix.apply_mask(&mask);
    Ok(pix)
}

/// The card as PNG bytes.
pub fn png(pix: &Pixmap) -> Result<Vec<u8>, String> {
    pix.encode_png().map_err(|e| e.to_string())
}

/// The card for egui.
pub fn color_image(pix: &Pixmap) -> egui::ColorImage {
    egui::ColorImage::from_rgba_premultiplied([pix.width() as usize, pix.height() as usize], pix.data())
}

struct Frame {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn options() -> usvg::Options<'static> {
    let mut opt = usvg::Options::default();
    let db = opt.fontdb_mut();
    for f in [SAIRA_BLACK_ITALIC, SAIRA_BOLD, SAIRA_SEMIBOLD] {
        db.load_font_data(f.to_vec());
    }
    opt.font_family = "Saira".into();
    opt
}

fn draw_svg(pix: &mut Pixmap, svg: &str, opt: &usvg::Options) -> Result<(), String> {
    let tree = usvg::Tree::from_str(svg, opt).map_err(|e| e.to_string())?;
    resvg::render(&tree, Transform::identity(), &mut pix.as_mut());
    Ok(())
}

/// Deep navy, a warm glow behind the multiplier, a cool one in the far corner, speed streaks.
fn background() -> String {
    let mut streaks = String::new();
    // thin slanted streaks, brighter toward the glow
    for (i, (y, len, op)) in [(92.0, 520.0, 0.30), (168.0, 760.0, 0.18), (238.0, 430.0, 0.40), (318.0, 900.0, 0.14), (388.0, 610.0, 0.26), (470.0, 820.0, 0.12), (548.0, 500.0, 0.22), (612.0, 700.0, 0.10)]
        .iter()
        .enumerate()
    {
        let x = 380.0 + (i as f32 * 97.0) % 360.0;
        streaks.push_str(&format!(r#"<rect x="{x}" y="{y}" width="{len}" height="2" rx="1" fill="url(#streak)" opacity="{op}"/>"#));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}">
<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="#101830"/><stop offset="0.55" stop-color="#0a0f1e"/><stop offset="1" stop-color="#05070d"/>
  </linearGradient>
  <radialGradient id="warm" cx="0.70" cy="0.56" r="0.62">
    <stop offset="0" stop-color="#ff6a1a" stop-opacity="0.42"/>
    <stop offset="0.35" stop-color="#ff3b2a" stop-opacity="0.20"/>
    <stop offset="1" stop-color="#ff1f3d" stop-opacity="0"/>
  </radialGradient>
  <radialGradient id="cool" cx="0.02" cy="0.0" r="0.7">
    <stop offset="0" stop-color="#6d5cff" stop-opacity="0.30"/><stop offset="1" stop-color="#6d5cff" stop-opacity="0"/>
  </radialGradient>
  <radialGradient id="ember" cx="1.0" cy="1.0" r="0.55">
    <stop offset="0" stop-color="#ffd21f" stop-opacity="0.18"/><stop offset="1" stop-color="#ffd21f" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="streak" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0" stop-color="#ffd21f" stop-opacity="0"/><stop offset="0.6" stop-color="#ff8a1f" stop-opacity="1"/><stop offset="1" stop-color="#ff1f3d" stop-opacity="0"/>
  </linearGradient>
  <linearGradient id="edge" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="#ffffff" stop-opacity="0.16"/><stop offset="0.5" stop-color="#ffffff" stop-opacity="0.04"/><stop offset="1" stop-color="#ff8a1f" stop-opacity="0.35"/>
  </linearGradient>
</defs>
<rect width="{W}" height="{H}" fill="url(#bg)"/>
<rect width="{W}" height="{H}" fill="url(#cool)"/>
<rect width="{W}" height="{H}" fill="url(#warm)"/>
<rect width="{W}" height="{H}" fill="url(#ember)"/>
<g transform="skewX(-24)">{streaks}</g>
<rect x="1.5" y="1.5" width="{w}" height="{h}" rx="{r}" fill="none" stroke="url(#edge)" stroke-width="3"/>
</svg>"##,
        w = W as f32 - 3.0,
        h = H as f32 - 3.0,
        r = RADIUS - 1.5,
    )
}

/// The wordmark's letters, huge and faint, in two staggered rows across the card.
fn watermark(pix: &mut Pixmap) -> Result<(), String> {
    let tree = usvg::Tree::from_data(WORDMARK, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let letters_h = 230.0;
    let scale = letters_h * (1.0 + 2.0 * WORDMARK_GLOW) / size.height();
    let (w, h) = ((size.width() * scale).ceil() as u32, (size.height() * scale).ceil() as u32);
    let mut layer = Pixmap::new(w, h).ok_or("no pixmap")?;
    resvg::render(&tree, Transform::from_scale(scale, scale), &mut layer.as_mut());
    let paint = PixmapPaint { opacity: 0.09, quality: FilterQuality::Bicubic, ..Default::default() };
    let tilt = Transform::from_rotate(-7.0);
    for (x, y) in [(-260.0, 10.0), (240.0, 330.0)] {
        pix.draw_pixmap(0, 0, layer.as_ref(), &paint, tilt.pre_translate(x, y), None);
    }
    Ok(())
}

/// The crisp wordmark in the top right corner.
fn small_wordmark(pix: &mut Pixmap) -> Result<(), String> {
    let tree = usvg::Tree::from_data(WORDMARK, &usvg::Options::default()).map_err(|e| e.to_string())?;
    let size = tree.size();
    let letters_h = 34.0;
    let scale = letters_h * (1.0 + 2.0 * WORDMARK_GLOW) / size.height();
    let w = size.width() * scale;
    let glow = letters_h * WORDMARK_GLOW;
    resvg::render(&tree, Transform::from_row(scale, 0.0, 0.0, scale, W as f32 - 64.0 - w + glow, 52.0 - glow), &mut pix.as_mut());
    Ok(())
}

/// A glow under the artwork, and the placeholder when there's no artwork: a dark tile with the
/// game's initial in the wordmark's colours, like the app icon.
fn art_backdrop(f: &Frame, card: &Card) -> String {
    let initial: String = card.game.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">
<defs>
  <filter id="soft" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="26"/></filter>
  <filter id="halo" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="18"/></filter>
  <linearGradient id="ph" x1="0" y1="1" x2="1" y2="0">
    <stop offset="0" stop-color="#ff1f3d"/><stop offset="0.5" stop-color="#ff6a1a"/><stop offset="1" stop-color="#ffd21f"/>
  </linearGradient>
  <linearGradient id="tile" x1="0" y1="0" x2="0" y2="1">
    <stop offset="0" stop-color="#18213a"/><stop offset="1" stop-color="#0a0e1b"/>
  </linearGradient>
</defs>
<rect x="{gx}" y="{gy}" width="{gw}" height="{gh}" rx="30" fill="#ff5a1f" opacity="0.45" filter="url(#soft)"/>
<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="26" fill="url(#tile)" stroke="#ffffff" stroke-opacity="0.08" stroke-width="2"/>
<text x="{cx}" y="{cy}" text-anchor="middle" font-family="Saira" font-weight="900" font-style="italic" font-size="{fs}" fill="#ff6a1a" opacity="0.6" filter="url(#halo)">{initial}</text>
<text x="{cx}" y="{cy}" text-anchor="middle" font-family="Saira" font-weight="900" font-style="italic" font-size="{fs}" fill="url(#ph)">{initial}</text>
</svg>"##,
        x = f.x,
        y = f.y,
        w = f.w,
        h = f.h,
        gx = f.x + 18.0,
        gy = f.y + 26.0,
        gw = f.w - 36.0,
        gh = f.h - 10.0,
        cx = f.x + f.w / 2.0,
        cy = f.y + f.h / 2.0 + f.h * 0.16,
        fs = f.h * 0.5,
        initial = xml(&initial),
    )
}

fn decode(bytes: &[u8]) -> Option<Pixmap> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    // tiny-skia wants premultiplied alpha
    for px in data.chunks_exact_mut(4) {
        let a = px[3] as u16;
        for c in &mut px[..3] {
            *c = ((*c as u16 * a + 127) / 255) as u8;
        }
    }
    Pixmap::from_vec(data, tiny_skia::IntSize::from_wh(w, h)?)
}

/// The artwork cropped to fill the frame (never stretched), in its rounded corners.
fn paint_art(pix: &mut Pixmap, art: &Pixmap, f: &Frame) {
    let (aw, ah) = (art.width() as f32, art.height() as f32);
    let scale = (f.w / aw).max(f.h / ah);
    let tx = f.x + (f.w - aw * scale) / 2.0;
    let ty = f.y + (f.h - ah * scale) / 2.0;
    let shader = Pattern::new(art.as_ref(), SpreadMode::Pad, FilterQuality::Bicubic, 1.0, Transform::from_row(scale, 0.0, 0.0, scale, tx, ty));
    let paint = Paint { shader, anti_alias: true, ..Default::default() };
    if let Some(path) = rounded_rect(f.x, f.y, f.w, f.h, 26.0) {
        pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

/// Every text of the card. Long names and amounts shrink to fit their room.
fn foreground(card: &Card, f: &Frame, opt: &usvg::Options) -> Result<String, String> {
    let x0 = f.x + f.w + 64.0;
    let room = W as f32 - 64.0 - x0;
    let mult = fmt_multi(card.multiplier).replace('×', "x");
    // first pass at the full sizes, then shrink what doesn't fit
    let mut sizes = Sizes { name: 56.0, mult: MULT_SIZE, win: 50.0, bet: 34.0 };
    let probe = text_layer(card, &mult, x0, f, &sizes);
    let tree = usvg::Tree::from_str(&probe, opt).map_err(|e| e.to_string())?;
    let width = |id: &str| tree.node_by_id(id).map(|n| n.abs_bounding_box().width()).unwrap_or(0.0);
    let fit = |size: f32, w: f32, max: f32| if w > max && w > 0.0 { size * max / w } else { size };
    sizes.name = fit(sizes.name, width("name"), room);
    sizes.mult = fit(sizes.mult, width("mult"), room + 20.0);
    let win_room = room * 0.58;
    sizes.win = fit(sizes.win, width("win"), win_room);
    sizes.bet = fit(sizes.bet, width("bet"), room - win_room - 36.0);
    Ok(text_layer(card, &mult, x0, f, &sizes))
}

struct Sizes {
    name: f32,
    mult: f32,
    win: f32,
    bet: f32,
}

fn text_layer(card: &Card, mult: &str, x0: f32, f: &Frame, s: &Sizes) -> String {
    let top = f.y.min(140.0);
    let room = W as f32 - 64.0 - x0;
    let bet_x = x0 + room * 0.58 + 36.0;
    let demo = if card.demo {
        format!(
            r##"<rect x="{x}" y="{y}" width="92" height="36" rx="18" fill="none" stroke="#8a94aa" stroke-opacity="0.6" stroke-width="2"/>
<text x="{tx}" y="{ty}" text-anchor="middle" font-family="Saira" font-weight="700" font-size="19" letter-spacing="3" fill="#c7cfdf">DEMO</text>"##,
            x = x0 + 110.0,
            y = top,
            tx = x0 + 156.0,
            ty = top + 25.0,
        )
    } else {
        String::new()
    };
    let when_x = x0 + if card.demo { 222.0 } else { 116.0 };
    // the multiplier's baseline stays put when a long one shrinks, and the row under it too
    let base = top + 176.0 + MULT_SIZE * 0.78;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}">
<defs>
  <linearGradient id="hot" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0" stop-color="#ff1f3d"/><stop offset="0.5" stop-color="#ff6a1a"/><stop offset="1" stop-color="#ffd21f"/>
  </linearGradient>
  <linearGradient id="pill" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="#ffe14d"/><stop offset="1" stop-color="#ff9a1f"/>
  </linearGradient>
  <filter id="glow" x="-20%" y="-60%" width="140%" height="220%"><feGaussianBlur stdDeviation="22"/></filter>
  <linearGradient id="rule" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0" stop-color="#ffffff" stop-opacity="0.22"/><stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
  </linearGradient>
</defs>
<rect x="{x0}" y="{top}" width="96" height="36" rx="18" fill="url(#pill)"/>
<text x="{px}" y="{py}" text-anchor="middle" font-family="Saira" font-weight="900" font-style="italic" font-size="22" letter-spacing="2" fill="#1a0f00">{pill}</text>
{demo}
<text x="{when_x}" y="{py}" font-family="Saira" font-weight="600" font-size="21" fill="#8a94aa">{when}</text>
<text id="name" x="{x0}" y="{name_y}" font-family="Saira" font-weight="700" font-size="{name_s}" fill="#ffffff">{name}</text>
<text x="{x0}" y="{by_y}" font-family="Saira" font-weight="600" font-size="23" fill="#8a94aa">{byline}</text>
<text x="{mx}" y="{mult_y}" font-family="Saira" font-weight="900" font-style="italic" font-size="{mult_s}" fill="#ff6a1a" opacity="0.55" filter="url(#glow)">{mult}</text>
<text id="mult" x="{mx}" y="{mult_y}" font-family="Saira" font-weight="900" font-style="italic" font-size="{mult_s}" fill="url(#hot)">{mult}</text>
<rect x="{x0}" y="{rule_y}" width="{room}" height="2" fill="url(#rule)"/>
<text x="{x0}" y="{label_y}" font-family="Saira" font-weight="700" font-size="17" letter-spacing="4" fill="#8a94aa">WIN</text>
<text id="win" x="{x0}" y="{value_y}" font-family="Saira" font-weight="700" font-size="{win_s}" fill="#ffffff">{win}</text>
<text x="{bet_x}" y="{label_y}" font-family="Saira" font-weight="700" font-size="17" letter-spacing="4" fill="#8a94aa">BET</text>
<text id="bet" x="{bet_x}" y="{value_y}" font-family="Saira" font-weight="600" font-size="{bet_s}" fill="#c7cfdf">{bet}</text>
</svg>"##,
        pill = if card.hit { "HIT" } else { "WIN" },
        px = x0 + 48.0,
        py = top + 26.0,
        when = xml(&card.when),
        name_y = top + 104.0,
        name_s = s.name,
        name = xml(&card.game),
        by_y = top + 142.0,
        byline = xml(&card.byline),
        // the italic leans right: start a touch left so the column reads straight
        mx = x0 - 6.0,
        mult_y = base,
        mult_s = s.mult,
        mult = xml(mult),
        rule_y = base + 34.0,
        label_y = base + 76.0,
        value_y = base + 128.0,
        win_s = s.win,
        win = xml(&card.win),
        bet_s = s.bet,
        bet = xml(&card.bet),
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0);
    // control point offset for a quarter circle
    let k = r * 0.552_284_8;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(game: &str, mult: f64, demo: bool) -> Card {
        Card {
            game: game.into(),
            byline: "Hacksaw Gaming · Shuffle.com".into(),
            multiplier: mult,
            win: "$642.20".into(),
            bet: "$0.20".into(),
            when: "14 Oct 2026 · 16:30".into(),
            demo,
            hit: true,
        }
    }

    #[test]
    fn card_renders_text_and_art() {
        let pix = render(&sample("Le Vampire", 3211.0, false), None).unwrap();
        assert_eq!((pix.width(), pix.height()), (W, H));
        // the corners are cut round, the middle is painted
        assert_eq!(pix.pixel(0, 0).map(|p| p.alpha()), Some(0));
        assert_eq!(pix.pixel(W / 2, H / 2).map(|p| p.alpha()), Some(255));
        // a long name still fits its room
        let opt = options();
        let f = Frame { x: 72.0, y: 136.0, w: 304.0, h: 403.0 };
        let svg = foreground(&sample("Wanted Dead or a Wild: The Extremely Long Edition Deluxe", 12.5, true), &f, &opt).unwrap();
        let tree = usvg::Tree::from_str(&svg, &opt).unwrap();
        let name = tree.node_by_id("name").unwrap().abs_bounding_box();
        assert!(name.width() > 100.0, "the name's text was laid out");
        assert!(name.right() <= W as f32 - 60.0, "the name fits: right edge at {}", name.right());
    }

    /// `FASTBET_CARD_OUT=<dir> [FASTBET_CARD_ART=<image file>] cargo test card_preview -- --ignored`
    #[test]
    #[ignore]
    fn card_preview() {
        let dir = std::env::var("FASTBET_CARD_OUT").expect("FASTBET_CARD_OUT");
        let art = std::env::var("FASTBET_CARD_ART").ok().and_then(|p| std::fs::read(p).ok());
        for (i, c) in [sample("Le Vampire", 3211.0, false), sample("Peter & Sons: Barbarossa Beyond the Edge", 100.0, true), sample("Limbo", 1_000_000.0, false), sample("Dice", 2.0, false)].iter().enumerate() {
            let pix = render(c, if i < 3 { art.as_deref() } else { None }).unwrap();
            std::fs::write(format!("{dir}/card{i}.png"), png(&pix).unwrap()).unwrap();
        }
    }
}
