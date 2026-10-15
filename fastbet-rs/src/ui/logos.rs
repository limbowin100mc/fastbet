//! The FastBet wordmark and the casino logos: SVGs bundled into the binary,
//! rasterised at the exact pixel size they are drawn at, so they stay sharp at any UI scale.

use std::collections::HashMap;
use std::sync::OnceLock;

use egui::{pos2, Color32, ColorImage, Id, Painter, Rect, TextureHandle, TextureOptions};
use resvg::{tiny_skia, usvg};

use crate::casino::CasinoId;

fn svg(id: CasinoId) -> &'static [u8] {
    match id {
        CasinoId::ChipsGg => include_bytes!("../../assets/casinos/chipsgg.svg"),
        CasinoId::Stake => include_bytes!("../../assets/casinos/stake.svg"),
        CasinoId::Shuffle => include_bytes!("../../assets/casinos/shuffle.svg"),
        CasinoId::Goated => include_bytes!("../../assets/casinos/goated.svg"),
        CasinoId::Gambulls => include_bytes!("../../assets/casinos/gambulls.svg"),
        CasinoId::Metaspins => include_bytes!("../../assets/casinos/metaspins.svg"),
    }
}

#[derive(Default)]
pub struct Logos {
    /// Per casino and pixel size; `None` if the SVG didn't render.
    textures: HashMap<(CasinoId, u32, u32), Option<TextureHandle>>,
}

impl Logos {
    /// Paints the casino's logo fitted and centred in `rect`.
    pub fn paint(&mut self, p: &Painter, rect: Rect, id: CasinoId, tint: Color32) {
        let ppp = p.ctx().pixels_per_point();
        let (w, h) = ((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
        if w == 0 || h == 0 {
            return;
        }
        let tex = self.textures.entry((id, w, h)).or_insert_with(|| {
            let img = rasterise(svg(id), w, h)?;
            Some(p.ctx().load_texture(format!("logo-{}-{w}x{h}", id.key()), img, TextureOptions::LINEAR))
        });
        if let Some(t) = tex {
            p.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
        }
    }
}

/// "FASTBET" in a red-to-yellow gradient with a soft glow, letters outlined (no font needed).
const WORDMARK: &[u8] = include_bytes!("../../assets/wordmark.svg");
/// Room wordmark.svg leaves around its letters for the glow, as a fraction of their height.
pub const WORDMARK_GLOW: f32 = 0.3;

/// Width over height of wordmark.svg, glow margin included.
pub fn wordmark_aspect() -> f32 {
    static ASPECT: OnceLock<f32> = OnceLock::new();
    *ASPECT.get_or_init(|| usvg::Tree::from_data(WORDMARK, &usvg::Options::default()).map_or(1.0, |t| t.size().width() / t.size().height()))
}

/// Paints wordmark.svg filling `rect` (glow margin included).
pub fn paint_wordmark(p: &Painter, rect: Rect) {
    let ppp = p.ctx().pixels_per_point();
    let (w, h) = ((rect.width() * ppp).round() as u32, (rect.height() * ppp).round() as u32);
    if w == 0 || h == 0 {
        return;
    }
    let id = Id::new(("wordmark", w, h));
    let tex = p.ctx().data_mut(|d| d.get_temp::<Option<TextureHandle>>(id)).unwrap_or_else(|| {
        let tex = rasterise(WORDMARK, w, h).map(|img| p.ctx().load_texture(format!("wordmark-{w}x{h}"), img, TextureOptions::LINEAR));
        p.ctx().data_mut(|d| d.insert_temp(id, tex.clone()));
        tex
    });
    if let Some(t) = tex {
        p.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// Renders the SVG into a `w`×`h` image, aspect kept, centred.
pub(super) fn rasterise(data: &[u8], w: u32, h: u32) -> Option<ColorImage> {
    let tree = usvg::Tree::from_data(data, &usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = (w as f32 / size.width()).min(h as f32 / size.height());
    let dx = (w as f32 - size.width() * scale) / 2.0;
    let dy = (h as f32 - size.height() * scale) / 2.0;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_logo_renders() {
        for id in CasinoId::ALL {
            let img = rasterise(svg(*id), 40, 40).unwrap_or_else(|| panic!("{} logo does not parse", id.name()));
            assert!(img.pixels.iter().any(|p| p.a() > 0), "{} logo is empty", id.name());
        }
    }

    #[test]
    fn wordmark_renders() {
        let img = rasterise(WORDMARK, 200, 58).expect("wordmark does not parse");
        assert!(img.pixels.iter().any(|p| p.a() > 0), "wordmark is empty");
        assert!((wordmark_aspect() - 3884.0 / 1126.0).abs() < 0.01);
    }
}
