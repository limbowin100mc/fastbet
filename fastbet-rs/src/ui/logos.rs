//! Casino logos: small white SVGs bundled into the binary, rasterised at the
//! exact pixel size they are drawn at, so they stay sharp at any UI scale.

use std::collections::HashMap;

use egui::{pos2, Color32, ColorImage, Painter, Rect, TextureHandle, TextureOptions};
use resvg::{tiny_skia, usvg};

use crate::casino::CasinoId;

fn svg(id: CasinoId) -> &'static [u8] {
    match id {
        CasinoId::ChipsGg => include_bytes!("../../assets/casinos/chipsgg.svg"),
        CasinoId::Stake => include_bytes!("../../assets/casinos/stake.svg"),
        CasinoId::Shuffle => include_bytes!("../../assets/casinos/shuffle.svg"),
        CasinoId::Goated => include_bytes!("../../assets/casinos/goated.svg"),
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

/// Renders the SVG into a `w`×`h` image, aspect kept, centred.
fn rasterise(data: &[u8], w: u32, h: u32) -> Option<ColorImage> {
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
}
