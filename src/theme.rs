//! Fonts, palette and widget styling.
//!
//! Text uses the native Windows faces (Segoe UI, Cascadia Mono) when present and
//! falls back to egui's bundled fonts. Icons come from the Lucide icon font.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, Style,
    TextStyle, Theme, Visuals, epaint::text::FontTweak,
};
pub use lucide_icons::Icon;

pub const SEMIBOLD: &str = "semibold";

/// Colors that are not part of egui's `Visuals`.
#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub surface: Color32,
    pub raised: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub muted: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub good: Color32,
    pub caution: Color32,
    pub bad: Color32,
}

const DARK: Palette = Palette {
    bg: Color32::from_rgb(0x12, 0x13, 0x15),
    surface: Color32::from_rgb(0x19, 0x1A, 0x1D),
    raised: Color32::from_rgb(0x23, 0x25, 0x29),
    border: Color32::from_rgb(0x27, 0x29, 0x2D),
    border_strong: Color32::from_rgb(0x36, 0x39, 0x3E),
    text: Color32::from_rgb(0xE6, 0xE7, 0xE9),
    muted: Color32::from_rgb(0x8E, 0x92, 0x99),
    faint: Color32::from_rgb(0x5A, 0x5E, 0x65),
    accent: Color32::from_rgb(0x34, 0xBE, 0x82),
    on_accent: Color32::from_rgb(0x08, 0x1A, 0x11),
    good: Color32::from_rgb(0x34, 0xBE, 0x82),
    caution: Color32::from_rgb(0xE0, 0xA6, 0x3C),
    bad: Color32::from_rgb(0xE5, 0x48, 0x4D),
};

const LIGHT: Palette = Palette {
    bg: Color32::from_rgb(0xF5, 0xF5, 0xF6),
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    raised: Color32::from_rgb(0xEB, 0xEC, 0xEE),
    border: Color32::from_rgb(0xE1, 0xE2, 0xE5),
    border_strong: Color32::from_rgb(0xC9, 0xCB, 0xD0),
    text: Color32::from_rgb(0x17, 0x18, 0x1A),
    muted: Color32::from_rgb(0x66, 0x6B, 0x73),
    faint: Color32::from_rgb(0xA0, 0xA4, 0xAB),
    accent: Color32::from_rgb(0x1A, 0x8A, 0x5A),
    on_accent: Color32::WHITE,
    good: Color32::from_rgb(0x1A, 0x8A, 0x5A),
    caution: Color32::from_rgb(0xB7, 0x79, 0x0F),
    bad: Color32::from_rgb(0xCD, 0x2B, 0x31),
};

pub fn palette(ui: &egui::Ui) -> Palette {
    if ui.visuals().dark_mode { DARK } else { LIGHT }
}

/// Icon glyph followed by a label, for buttons and tabs.
pub fn with_icon(icon: Icon, label: &str) -> String {
    format!("{}  {}", char::from(icon), label)
}

pub fn icon(icon: Icon) -> String {
    char::from(icon).to_string()
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(fonts());
    for (theme, p) in [(Theme::Dark, DARK), (Theme::Light, LIGHT)] {
        ctx.style_mut_of(theme, |style| apply(style, &p, theme == Theme::Dark));
    }
}

fn system_font(file: &str) -> Option<FontData> {
    std::fs::read(format!("C:\\Windows\\Fonts\\{file}")).ok().map(FontData::from_owned)
}

fn fonts() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, data: FontData, family: FontFamily, front: bool| {
        defs.font_data.insert(name.to_owned(), Arc::new(data));
        let list = defs.families.entry(family).or_default();
        if front { list.insert(0, name.to_owned()) } else { list.push(name.to_owned()) }
    };

    if let Some(f) = system_font("segoeui.ttf") {
        add("segoe", f, FontFamily::Proportional, true);
    }
    if let Some(f) = system_font("CascadiaMono.ttf").or_else(|| system_font("consola.ttf")) {
        add("mono", f, FontFamily::Monospace, true);
    }
    let semibold = FontFamily::Name(SEMIBOLD.into());
    match system_font("seguisb.ttf") {
        Some(f) => add("segoe-semibold", f, semibold.clone(), true),
        None => {
            let fallback = defs.families[&FontFamily::Proportional].clone();
            defs.families.insert(semibold.clone(), fallback);
        }
    }

    // Lucide glyphs live in the Private Use Area, so the icon font can sit at the
    // end of every chain and be mixed freely with text.
    let lucide = FontData::from_static(lucide_icons::LUCIDE_FONT_BYTES)
        .tweak(FontTweak { scale: 1.05, y_offset_factor: 0.08, ..Default::default() });
    defs.font_data.insert("lucide".into(), Arc::new(lucide));
    for family in [FontFamily::Proportional, FontFamily::Monospace, semibold] {
        defs.families.entry(family).or_default().push("lucide".into());
    }
    defs
}

fn apply(style: &mut Style, p: &Palette, dark: bool) {
    use FontFamily::{Monospace, Proportional};
    style.text_styles = [
        (TextStyle::Small, FontId::new(11.5, Proportional)),
        (TextStyle::Body, FontId::new(13.5, Proportional)),
        (TextStyle::Button, FontId::new(13.5, Proportional)),
        (TextStyle::Heading, FontId::new(20.0, FontFamily::Name(SEMIBOLD.into()))),
        (TextStyle::Monospace, FontId::new(13.0, Monospace)),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = egui::vec2(8.0, 8.0);
    s.button_padding = egui::vec2(10.0, 5.0);
    s.interact_size.y = 28.0;
    s.menu_margin = Margin::same(6);
    s.window_margin = Margin::same(12);
    s.combo_height = 320.0;

    let r = CornerRadius::same(6);
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.surface;
    v.window_stroke = Stroke::new(1.0, p.border_strong);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.extreme_bg_color = p.surface;
    v.text_edit_bg_color = Some(p.surface);
    v.faint_bg_color = if dark { Color32::from_rgb(0x16, 0x17, 0x1A) } else { Color32::from_rgb(0xF9, 0xF9, 0xFA) };
    v.code_bg_color = p.raised;
    v.hyperlink_color = p.accent;
    v.warn_fg_color = p.caution;
    v.error_fg_color = p.bad;
    v.selection.bg_fill = p.accent.gamma_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0, p.accent);
    let shadow = Color32::from_black_alpha(if dark { 90 } else { 28 });
    v.window_shadow.color = shadow;
    v.popup_shadow.color = shadow;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.weak_bg_fill = p.bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.surface;
    w.inactive.weak_bg_fill = p.surface;
    w.inactive.bg_stroke = Stroke::new(1.0, p.border);
    w.inactive.fg_stroke = Stroke::new(1.0, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = p.raised;
    w.hovered.weak_bg_fill = p.raised;
    w.hovered.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.hovered.fg_stroke = Stroke::new(1.0, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.raised;
    w.active.weak_bg_fill = p.raised;
    w.active.bg_stroke = Stroke::new(1.0, p.accent);
    w.active.fg_stroke = Stroke::new(1.0, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open.bg_fill = p.raised;
    w.open.weak_bg_fill = p.raised;
    w.open.bg_stroke = Stroke::new(1.0, p.border_strong);
    w.open.fg_stroke = Stroke::new(1.0, p.text);
    w.open.corner_radius = r;

    style.visuals = v;
}

/// Flat bordered container used for cards and cells.
pub fn card(ui: &egui::Ui) -> egui::Frame {
    let p = palette(ui);
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
}

/// Segmented control. Returns `true` when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, value: &mut T, options: &[(T, String)]) -> bool {
    let p = palette(ui);
    let mut changed = false;
    egui::Frame::new()
        .fill(p.surface)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Inside a right-to-left parent the row is laid out mirrored, so feed it reversed.
            let rtl = ui.layout().main_dir() == egui::Direction::RightToLeft;
            ui.horizontal(|ui| {
                let ordered: Vec<_> = if rtl { options.iter().rev().collect() } else { options.iter().collect() };
                for (opt, label) in ordered {
                    let selected = *value == *opt;
                    let text = egui::RichText::new(label).color(if selected { p.text } else { p.muted });
                    let btn = egui::Button::new(text)
                        .fill(if selected { p.raised } else { Color32::TRANSPARENT })
                        .stroke(Stroke::NONE)
                        .corner_radius(CornerRadius::same(5))
                        .min_size(egui::vec2(0.0, 24.0));
                    if ui.add(btn).clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// Groups digits: 1234567 -> "1,234,567".
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}
