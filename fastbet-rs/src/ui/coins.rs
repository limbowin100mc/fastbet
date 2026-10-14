//! Currency icons on screen: each casino's own coin and fiat art (see [`crate::currency`]), rasterised
//! at the pixel size they are drawn at. Codes without art get a coloured disc with their first letter.

use egui::{pos2, vec2, Align2, Color32, ColorImage, Id, Painter, Pos2, Rect, TextureHandle, TextureOptions};

use super::theme::{self, G1};
use crate::casino::CasinoId;
use crate::currency::{self, Art};

/// Paints `code`'s icon, as `casino` draws it, in a `size`-point square centred on `center`.
pub fn paint(p: &Painter, center: Pos2, size: f32, casino: CasinoId, code: &str) {
    let px = (size * p.ctx().pixels_per_point()).round() as u32;
    if px == 0 || code.is_empty() {
        return;
    }
    let code = code.to_ascii_lowercase();
    let id = Id::new(("currency-icon", casino, &code, px));
    let tex = p.ctx().data_mut(|d| d.get_temp::<Option<TextureHandle>>(id)).unwrap_or_else(|| {
        let tex = render(casino, &code, px).map(|img| p.ctx().load_texture(format!("currency-{}-{code}-{px}", casino.key()), img, TextureOptions::LINEAR));
        p.ctx().data_mut(|d| d.insert_temp(id, tex.clone()));
        tex
    });
    match tex {
        Some(t) => {
            let rect = Rect::from_center_size(center, vec2(size, size));
            p.image(t.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        }
        None => letter(p, center, size / 2.0, &code),
    }
}

fn render(casino: CasinoId, code: &str, px: u32) -> Option<ColorImage> {
    match currency::art(casino, code)? {
        Art::Svg(svg) => super::logos::rasterise(svg.as_bytes(), px, px),
        Art::Picture(bytes) => round_picture(bytes, px),
    }
}

/// A picture scaled to `px` and cut to a circle, the way the sites show them.
fn round_picture(bytes: &[u8], px: u32) -> Option<ColorImage> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let img = image::imageops::resize(&img, px, px, image::imageops::FilterType::Triangle);
    let r = px as f32 / 2.0;
    let mut rgba = img.into_raw();
    for (i, p) in rgba.chunks_exact_mut(4).enumerate() {
        let (x, y) = ((i as u32 % px) as f32 + 0.5, (i as u32 / px) as f32 + 0.5);
        let d = ((x - r).powi(2) + (y - r).powi(2)).sqrt();
        p[3] = (p[3] as f32 * (r - d + 0.5).clamp(0.0, 1.0)) as u8;
    }
    Some(ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba))
}

/// Brand-ish colour for a coin without art, grey for unknown ones.
fn letter_color(sym: &str) -> Color32 {
    match sym {
        "ton" => Color32::from_rgb(0x00, 0x98, 0xea),
        "matic" => Color32::from_rgb(0x82, 0x47, 0xe5),
        "chips" => G1,
        _ => Color32::from_rgb(0x7c, 0x86, 0x9c),
    }
}

fn letter(p: &Painter, center: Pos2, radius: f32, sym: &str) {
    p.circle_filled(center, radius, letter_color(sym));
    let letter: String = sym.chars().next().map(|ch| ch.to_ascii_uppercase().to_string()).unwrap_or_default();
    p.text(center, Align2::CENTER_CENTER, letter, theme::font_bold(radius * 1.15), Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_renders() {
        let codes = ["btc", "eth", "ltc", "usdt", "sol", "doge", "bch", "xrp", "trx", "eos", "bnb", "usdc", "ape", "busd", "cro", "dai", "link"];
        let fiats = ["usd", "eur", "jpy", "inr", "cad", "cny", "brl", "rub", "try", "ars", "xof", "xaf", "gbp", "chf"];
        let shuffle = ["shfl", "ton", "bonk", "wif", "pump", "shib", "trump", "cash", "matic", "avax", "btc", "usdt", "eur", "pln", "vnd", "dkk", "idr", "krw", "nzd"];
        let all = codes.iter().chain(&fiats).chain(&["sand", "shib", "uni", "pol", "trump"]).map(|c| (CasinoId::Stake, c));
        for (casino, code) in all.chain(shuffle.iter().map(|c| (CasinoId::Shuffle, c))) {
            let img = render(casino, code, 32).unwrap_or_else(|| panic!("{} has no {code} icon", casino.name()));
            let opaque = img.pixels.iter().filter(|p| p.a() > 200).count();
            assert!(opaque > 32 * 32 / 4, "{} {code} icon is mostly empty ({opaque} solid pixels)", casino.name());
        }
        assert!(render(CasinoId::Stake, "nope", 32).is_none());
    }
}
