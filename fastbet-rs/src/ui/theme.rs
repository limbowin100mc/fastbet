//! Colours, fonts and egui visuals.

use std::sync::{Arc, OnceLock};

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, TextStyle, Vec2};

// Deep navy base, one green accent, salmon red, amber for pause. Few borders, no noise.
pub const BG: Color32 = Color32::from_rgb(0x07, 0x0b, 0x14);
pub const BG1: Color32 = Color32::from_rgb(0x0b, 0x10, 0x1e);
pub const BG2: Color32 = Color32::from_rgb(0x10, 0x17, 0x27);
pub const BG3: Color32 = Color32::from_rgb(0x17, 0x20, 0x36);
pub const STAGE: Color32 = Color32::from_rgb(0x04, 0x06, 0x0c);
pub const LINE: Color32 = Color32::from_rgb(0x1a, 0x23, 0x36);
pub const LINE2: Color32 = Color32::from_rgb(0x27, 0x32, 0x4b);
pub const TEXT: Color32 = Color32::from_rgb(0xea, 0xee, 0xf7);
pub const TEXT2: Color32 = Color32::from_rgb(0xc7, 0xcf, 0xdf);
pub const MUTED: Color32 = Color32::from_rgb(0x8a, 0x94, 0xaa);
pub const DIM: Color32 = Color32::from_rgb(0x59, 0x63, 0x78);
pub const G1: Color32 = Color32::from_rgb(0x1e, 0xd3, 0x80);
pub const G2: Color32 = Color32::from_rgb(0x2b, 0xe4, 0xa6);
pub const GREEN: Color32 = Color32::from_rgb(0x1e, 0xd3, 0x80);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x5c, 0x7a);
pub const GOLD: Color32 = Color32::from_rgb(0xff, 0xd1, 0x4a);
pub const BLUE: Color32 = Color32::from_rgb(0x5b, 0x8d, 0xef);
pub const INK: Color32 = Color32::from_rgb(0x03, 0x12, 0x0b);

pub const RADIUS: u8 = 8;
pub const RADIUS_SM: u8 = 6;

/// `c` with alpha `a` (0..=255), not premultiplied.
pub fn alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}

pub fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

pub fn pnl(v: i128) -> Color32 {
    if v > 0 {
        GREEN
    } else if v < 0 {
        RED
    } else {
        TEXT
    }
}

struct Families {
    semi: FontFamily,
    bold: FontFamily,
    mono: FontFamily,
}

static FAMILIES: OnceLock<Families> = OnceLock::new();

fn families() -> &'static Families {
    FAMILIES.get_or_init(|| Families {
        semi: FontFamily::Proportional,
        bold: FontFamily::Proportional,
        mono: FontFamily::Monospace,
    })
}

pub fn semi() -> FontFamily {
    families().semi.clone()
}
pub fn bold() -> FontFamily {
    families().bold.clone()
}
pub fn mono() -> FontFamily {
    families().mono.clone()
}

pub fn font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn font_semi(size: f32) -> FontId {
    FontId::new(size, semi())
}
pub fn font_bold(size: f32) -> FontId {
    FontId::new(size, bold())
}
pub fn font_mono(size: f32) -> FontId {
    FontId::new(size, mono())
}

/// Loads system fonts when available (Segoe UI on Windows) and falls back to egui's defaults.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let base = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let base_mono = fonts.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();

    let candidates: &[(&str, &[&str])] = &[
        ("ui", &["C:/Windows/Fonts/segoeui.ttf", "/System/Library/Fonts/SFNS.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"]),
        ("ui-semi", &["C:/Windows/Fonts/seguisb.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"]),
        ("ui-bold", &["C:/Windows/Fonts/segoeuib.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf"]),
        ("ui-mono", &["C:/Windows/Fonts/consola.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf"]),
    ];
    let mut loaded = std::collections::HashSet::new();
    for (name, paths) in candidates {
        for p in *paths {
            if let Ok(bytes) = std::fs::read(p) {
                fonts.font_data.insert((*name).to_string(), Arc::new(FontData::from_owned(bytes)));
                loaded.insert(*name);
                break;
            }
        }
    }

    let mut fam = Families { semi: FontFamily::Proportional, bold: FontFamily::Proportional, mono: FontFamily::Monospace };
    if loaded.contains("ui") {
        let mut list = vec!["ui".to_string()];
        list.extend(base.iter().cloned());
        fonts.families.insert(FontFamily::Proportional, list);
    }
    if loaded.contains("ui-semi") {
        let mut list = vec!["ui-semi".to_string()];
        list.extend(base.iter().cloned());
        fonts.families.insert(FontFamily::Name("semi".into()), list);
        fam.semi = FontFamily::Name("semi".into());
    }
    if loaded.contains("ui-bold") {
        let mut list = vec!["ui-bold".to_string()];
        list.extend(base.iter().cloned());
        fonts.families.insert(FontFamily::Name("bold".into()), list);
        fam.bold = FontFamily::Name("bold".into());
    }
    if loaded.contains("ui-mono") {
        let mut list = vec!["ui-mono".to_string()];
        list.extend(base_mono.iter().cloned());
        fonts.families.insert(FontFamily::Monospace, list);
    }
    let _ = FAMILIES.set(fam);
    ctx.set_fonts(fonts);
}

pub fn install_visuals(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG1;
    v.window_fill = BG1;
    v.extreme_bg_color = BG;
    v.faint_bg_color = BG2;
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(RADIUS);
    v.selection.bg_fill = alpha(G1, 70);
    v.selection.stroke = Stroke::new(1.0, G1);
    v.hyperlink_color = G2;
    v.text_cursor.stroke = Stroke::new(1.5, G1);

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = BG2;
    w.noninteractive.weak_bg_fill = BG2;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, MUTED);
    w.noninteractive.corner_radius = CornerRadius::same(RADIUS_SM);

    w.inactive.bg_fill = BG3;
    w.inactive.weak_bg_fill = BG2;
    w.inactive.bg_stroke = Stroke::new(1.0, LINE);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT2);
    w.inactive.corner_radius = CornerRadius::same(RADIUS_SM);

    w.hovered.bg_fill = mix(BG3, LINE2, 0.5);
    w.hovered.weak_bg_fill = BG3;
    w.hovered.bg_stroke = Stroke::new(1.0, LINE2);
    w.hovered.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.corner_radius = CornerRadius::same(RADIUS_SM);
    w.hovered.expansion = 0.0;

    w.active.bg_fill = LINE2;
    w.active.weak_bg_fill = LINE2;
    w.active.bg_stroke = Stroke::new(1.0, G1);
    w.active.fg_stroke = Stroke::new(1.0, TEXT);
    w.active.corner_radius = CornerRadius::same(RADIUS_SM);
    w.active.expansion = 0.0;

    w.open.bg_fill = BG3;
    w.open.weak_bg_fill = BG3;
    w.open.bg_stroke = Stroke::new(1.0, LINE2);
    w.open.fg_stroke = Stroke::new(1.0, TEXT);
    w.open.corner_radius = CornerRadius::same(RADIUS_SM);

    ctx.set_visuals(v);

    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 5.0);
        s.spacing.interact_size = Vec2::new(40.0, 24.0);
        s.spacing.window_margin = Margin::same(12);
        s.spacing.menu_margin = Margin::same(8);
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.scroll.floating = true;
        s.spacing.scroll.bar_inner_margin = 2.0;
        s.text_styles.insert(TextStyle::Body, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Button, FontId::proportional(13.0));
        s.text_styles.insert(TextStyle::Small, FontId::proportional(11.0));
        s.text_styles.insert(TextStyle::Heading, FontId::new(18.0, semi()));
        s.text_styles.insert(TextStyle::Monospace, FontId::monospace(12.5));
        s.visuals.popup_shadow.spread = 2;
        s.visuals.popup_shadow.blur = 14;
        s.visuals.popup_shadow.color = Color32::from_black_alpha(140);
        s.visuals.window_shadow.blur = 24;
        s.visuals.window_shadow.color = Color32::from_black_alpha(150);
    });
}
