//! Dark and light palettes, modelled on HWiNFO's two looks, and their mapping onto
//! egui's `Visuals`. No widget hard-codes a colour: everything goes through
//! [`ThemeTokens`].

use egui::{
    Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Theme, ThemePreference, Visuals,
    vec2,
};
use kwinfo_core::{Kind, Rgb};

/// The user's setting (Follow system, Dark or Light); it lives in the configuration.
pub use kwinfo_core::ThemeMode;

/// Minimum contrast of text against its background (WCAG AA).
pub const TEXT_CONTRAST: f64 = 4.5;
/// Minimum contrast of icons and other graphics against their background.
pub const GRAPHIC_CONTRAST: f64 = 3.0;

/// A resolved palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Palette {
    Dark,
    Light,
}

impl Palette {
    pub fn preference(self) -> ThemePreference {
        match self {
            Palette::Dark => ThemePreference::Dark,
            Palette::Light => ThemePreference::Light,
        }
    }

    pub fn egui_theme(self) -> Theme {
        match self {
            Palette::Dark => Theme::Dark,
            Palette::Light => Theme::Light,
        }
    }

    pub fn from_theme(theme: Theme) -> Self {
        match theme {
            Theme::Dark => Palette::Dark,
            Theme::Light => Palette::Light,
        }
    }
}

/// Default series colours per sensor kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GraphColors {
    pub temperature: Color32,
    pub voltage: Color32,
    pub power: Color32,
    pub fan: Color32,
    pub usage: Color32,
    pub clock: Color32,
    pub other: Color32,
}

/// Semantic colour tokens. See PLAN.md section 5.2 for the rationale behind the values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeTokens {
    pub palette: Palette,
    pub window_bg: Color32,
    pub table_bg: Color32,
    pub group_row: Color32,
    pub alt_row: Color32,
    pub hover_row: Color32,
    pub selection_fill: Color32,
    pub selection_outline: Color32,
    pub text_primary: Color32,
    pub text_secondary: Color32,
    pub grid: Color32,
    pub current_tint: Color32,
    pub accent: Color32,
    pub highlight: Color32,
    pub graph_bg: Color32,
    pub graph_grid: Color32,
    pub graph: GraphColors,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const DARK: ThemeTokens = ThemeTokens {
    palette: Palette::Dark,
    window_bg: rgb(0x1b1b1b),
    table_bg: rgb(0x202020),
    group_row: rgb(0x2a2a2a),
    alt_row: rgb(0x242424),
    hover_row: rgb(0x2f2f2f),
    selection_fill: rgb(0x1f3a5f),
    selection_outline: rgb(0x3b82f6),
    text_primary: rgb(0xe6e6e6),
    text_secondary: rgb(0xa0a0a0),
    grid: rgb(0x3a3a3a),
    current_tint: rgb(0x243040),
    accent: rgb(0x4da3ff),
    highlight: rgb(0xff7a1a),
    graph_bg: rgb(0x0d0d0d),
    graph_grid: rgb(0x333333),
    graph: GraphColors {
        temperature: rgb(0xb22222),
        voltage: rgb(0xd4a017),
        power: rgb(0x1e90ff),
        fan: rgb(0x22a06b),
        usage: rgb(0x2ea8a0),
        clock: rgb(0x8a7dd6),
        other: rgb(0x5b7fb8),
    },
};

pub const LIGHT: ThemeTokens = ThemeTokens {
    palette: Palette::Light,
    window_bg: rgb(0xf0f0f0),
    table_bg: rgb(0xffffff),
    group_row: rgb(0xf3f3f3),
    alt_row: rgb(0xf8f8f8),
    hover_row: rgb(0xeaf2fb),
    selection_fill: rgb(0xcce8ff),
    selection_outline: rgb(0x0a64c2),
    text_primary: rgb(0x1a1a1a),
    text_secondary: rgb(0x5c5c5c),
    grid: rgb(0xd0d0d0),
    current_tint: rgb(0xeef4fb),
    accent: rgb(0x0a64c2),
    highlight: rgb(0xff8c00),
    graph_bg: rgb(0xe9f3fb),
    graph_grid: rgb(0xa9d0ec),
    graph: GraphColors {
        temperature: rgb(0xe57373),
        voltage: rgb(0xc9a227),
        power: rgb(0x7986cb),
        fan: rgb(0x66bb6a),
        usage: rgb(0x8fb98f),
        clock: rgb(0x9b8fd8),
        other: rgb(0x90a4ae),
    },
};

impl ThemeTokens {
    pub fn for_palette(palette: Palette) -> &'static ThemeTokens {
        match palette {
            Palette::Dark => &DARK,
            Palette::Light => &LIGHT,
        }
    }

    /// The backgrounds a table row's text and icons can sit on: plain, striped, hovered,
    /// the Current column's tint and the selection.
    pub fn row_backgrounds(&self) -> [Color32; 5] {
        [
            self.table_bg,
            self.alt_row,
            self.hover_row,
            self.current_tint,
            self.selection_fill,
        ]
    }

    /// `color`, with its lightness moved only as far as needed to reach `min_contrast`
    /// against every row background: lighter in the dark palette, darker in the light
    /// one. Hue and saturation stay, so a custom colour is recognisably the same in both
    /// themes (PLAN.md 5.1).
    pub fn readable(&self, color: Color32, min_contrast: f64) -> Color32 {
        let backgrounds = self.row_backgrounds();
        let ok = |c: Color32| {
            backgrounds
                .iter()
                .all(|&bg| contrast(c, bg) >= min_contrast)
        };
        if ok(color) {
            return color;
        }
        let (h, s, l) = to_hsl(color);
        // Lightness only ever moves away from the backgrounds, so `ok` flips once along
        // the way and a bisection finds the smallest change.
        let (mut near, mut far) = match self.palette {
            Palette::Dark => (l, 1.0),
            Palette::Light => (l, 0.0),
        };
        if !ok(from_hsl(h, s, far)) {
            return from_hsl(h, s, far);
        }
        for _ in 0..24 {
            let mid = (near + far) / 2.0;
            if ok(from_hsl(h, s, mid)) {
                far = mid;
            } else {
                near = mid;
            }
        }
        from_hsl(h, s, far)
    }

    /// A user's custom row colour, made readable as text in this palette.
    pub fn custom_text(&self, rgb: Rgb) -> Color32 {
        self.readable(Color32::from_rgb(rgb.r, rgb.g, rgb.b), TEXT_CONTRAST)
    }

    /// The icon colour of a kind of reading: its graph colour, readable as a graphic.
    pub fn icon_color(&self, kind: Kind) -> Color32 {
        self.readable(self.graph_color(kind), GRAPHIC_CONTRAST)
    }

    pub fn graph_color(&self, kind: Kind) -> Color32 {
        match kind {
            Kind::Temperature => self.graph.temperature,
            Kind::Voltage | Kind::Current => self.graph.voltage,
            Kind::Power | Kind::Energy => self.graph.power,
            Kind::Fan | Kind::Pwm => self.graph.fan,
            Kind::Usage => self.graph.usage,
            Kind::Clock => self.graph.clock,
            Kind::Throughput | Kind::Capacity | Kind::Count | Kind::Boolean | Kind::Other => {
                self.graph.other
            }
        }
    }

    /// Maps the tokens onto egui's visuals, starting from egui's own dark or light set so
    /// that anything we do not cover still fits.
    pub fn visuals(&self) -> Visuals {
        let mut v = match self.palette {
            Palette::Dark => Visuals::dark(),
            Palette::Light => Visuals::light(),
        };
        let thin = |color: Color32| Stroke::new(1.0, color);
        let radius = CornerRadius::same(3);

        v.override_text_color = Some(self.text_primary);
        v.window_fill = self.window_bg;
        v.panel_fill = self.window_bg;
        v.faint_bg_color = self.alt_row;
        v.extreme_bg_color = self.table_bg;
        v.code_bg_color = self.group_row;
        v.text_edit_bg_color = Some(self.table_bg);
        v.window_stroke = thin(self.grid);
        v.window_corner_radius = CornerRadius::same(4);
        v.menu_corner_radius = CornerRadius::same(4);
        v.hyperlink_color = self.accent;
        v.selection.bg_fill = self.selection_fill;
        v.selection.stroke = thin(self.selection_outline);
        v.striped = true;

        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.window_bg;
        w.noninteractive.weak_bg_fill = self.window_bg;
        w.noninteractive.bg_stroke = thin(self.grid);
        w.noninteractive.fg_stroke = thin(self.text_primary);
        w.noninteractive.corner_radius = radius;

        w.inactive.bg_fill = self.group_row;
        w.inactive.weak_bg_fill = self.group_row;
        w.inactive.bg_stroke = thin(self.grid);
        w.inactive.fg_stroke = thin(self.text_primary);
        w.inactive.corner_radius = radius;

        w.hovered.bg_fill = self.hover_row;
        w.hovered.weak_bg_fill = self.hover_row;
        w.hovered.bg_stroke = thin(self.accent);
        w.hovered.fg_stroke = Stroke::new(1.5, self.text_primary);
        w.hovered.corner_radius = radius;

        w.active.bg_fill = self.selection_fill;
        w.active.weak_bg_fill = self.selection_fill;
        w.active.bg_stroke = thin(self.selection_outline);
        w.active.fg_stroke = Stroke::new(2.0, self.text_primary);
        w.active.corner_radius = radius;

        w.open.bg_fill = self.group_row;
        w.open.weak_bg_fill = self.group_row;
        w.open.bg_stroke = thin(self.grid);
        w.open.fg_stroke = thin(self.text_primary);
        w.open.corner_radius = radius;

        v
    }
}

/// Registers both palettes with egui and tightens the spacing to table density.
/// Switching afterwards is a matter of `ctx.set_theme(palette.preference())`.
pub fn install(ctx: &egui::Context) {
    ctx.set_visuals_of(Theme::Dark, DARK.visuals());
    ctx.set_visuals_of(Theme::Light, LIGHT.visuals());
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |style| {
            // Rows react to clicks anywhere, so labels must not swallow them for text
            // selection; values are copied through the context menu instead.
            style.interaction.selectable_labels = false;
            style.spacing.item_spacing = vec2(6.0, 3.0);
            style.spacing.button_padding = vec2(8.0, 3.0);
            style.text_styles.insert(
                TextStyle::Small,
                FontId::new(11.0, FontFamily::Proportional),
            );
            style
                .text_styles
                .insert(TextStyle::Body, FontId::new(13.0, FontFamily::Proportional));
            style.text_styles.insert(
                TextStyle::Button,
                FontId::new(13.0, FontFamily::Proportional),
            );
            style.text_styles.insert(
                TextStyle::Heading,
                FontId::new(17.0, FontFamily::Proportional),
            );
            style.text_styles.insert(
                TextStyle::Monospace,
                FontId::new(12.5, FontFamily::Monospace),
            );
        });
    }
}

/// WCAG relative luminance of an sRGB colour.
pub fn luminance(c: Color32) -> f64 {
    let channel = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.039_28 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
}

/// WCAG contrast ratio between two colours, from 1 to 21.
pub fn contrast(a: Color32, b: Color32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Hue (0–1), saturation and lightness of an opaque colour.
fn to_hsl(c: Color32) -> (f64, f64, f64) {
    let [r, g, b] = [c.r(), c.g(), c.b()].map(|v| f64::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d == 0.0 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, s, l)
}

fn from_hsl(h: f64, s: f64, l: f64) -> Color32 {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let h6 = h * 6.0;
    let x = c * (1.0 - (h6.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h6 as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let byte = |v: f64| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color32::from_rgb(byte(r), byte(g), byte(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_meets_contrast_in_both_palettes() {
        for tokens in [&DARK, &LIGHT] {
            for bg in [
                tokens.window_bg,
                tokens.table_bg,
                tokens.group_row,
                tokens.alt_row,
                tokens.current_tint,
            ] {
                assert!(
                    contrast(tokens.text_primary, bg) >= 4.5,
                    "{:?} primary text on {bg:?}",
                    tokens.palette
                );
                assert!(
                    contrast(tokens.text_secondary, bg) >= 3.0,
                    "{:?} secondary text on {bg:?}",
                    tokens.palette
                );
            }
        }
    }

    #[test]
    fn hsl_round_trips() {
        for c in [
            rgb(0xff8c00),
            rgb(0x0a64c2),
            rgb(0x808080),
            rgb(0x000000),
            rgb(0xffffff),
            rgb(0xb22222),
            rgb(0x2ea8a0),
        ] {
            let (h, s, l) = to_hsl(c);
            assert_eq!(from_hsl(h, s, l), c);
        }
    }

    #[test]
    fn custom_colours_stay_readable_in_both_palettes() {
        // A spread of hues and lightnesses, including the extremes.
        let mut samples = vec![rgb(0x000000), rgb(0xffffff), rgb(0x0000ff), rgb(0xffff00)];
        for h in 0..12 {
            for l in [0.1, 0.3, 0.5, 0.7, 0.9] {
                samples.push(from_hsl(f64::from(h) / 12.0, 0.85, l));
            }
        }
        for tokens in [&DARK, &LIGHT] {
            for &c in &samples {
                let text = tokens.custom_text(Rgb::new(c.r(), c.g(), c.b()));
                for bg in tokens.row_backgrounds() {
                    assert!(
                        contrast(text, bg) >= TEXT_CONTRAST - 0.01,
                        "{:?}: {c:?} became {text:?}, {:.2}:1 on {bg:?}",
                        tokens.palette,
                        contrast(text, bg)
                    );
                }
            }
        }
        // A colour that is already readable is left alone.
        assert_eq!(DARK.custom_text(Rgb::new(0xff, 0x8c, 0x00)), rgb(0xff8c00));
        assert_eq!(LIGHT.custom_text(Rgb::new(0x0a, 0x64, 0xc2)), rgb(0x0a64c2));
        // Orange is too light for white: it gets darker but stays orange.
        let orange = LIGHT.custom_text(Rgb::new(0xff, 0x8c, 0x00));
        assert!(luminance(orange) < luminance(rgb(0xff8c00)));
        let (h, _, _) = to_hsl(orange);
        assert!((h - to_hsl(rgb(0xff8c00)).0).abs() < 0.02, "hue kept");
    }

    #[test]
    fn kind_icons_reach_graphic_contrast() {
        let kinds = [
            Kind::Temperature,
            Kind::Voltage,
            Kind::Power,
            Kind::Fan,
            Kind::Usage,
            Kind::Clock,
            Kind::Other,
        ];
        for tokens in [&DARK, &LIGHT] {
            for kind in kinds {
                let color = tokens.icon_color(kind);
                for bg in tokens.row_backgrounds() {
                    assert!(
                        contrast(color, bg) >= GRAPHIC_CONTRAST - 0.01,
                        "{:?} {kind:?} icon on {bg:?}",
                        tokens.palette
                    );
                }
            }
        }
    }

    #[test]
    fn visuals_carry_the_palette() {
        let dark = DARK.visuals();
        assert!(dark.dark_mode);
        assert_eq!(dark.panel_fill, DARK.window_bg);
        let light = LIGHT.visuals();
        assert!(!light.dark_mode);
        assert_eq!(light.extreme_bg_color, LIGHT.table_bg);
    }
}
