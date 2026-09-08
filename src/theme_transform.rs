//! Deterministic, UI-independent color transforms for complete themes.
//!
//! A transform always complements the encoded sRGB channels first (when
//! requested), then rotates hue in HSL space. Keeping this order in one type
//! prevents the application chrome and syntax theme from applying settings in
//! subtly different ways.
//!
//! These operations deliberately do not repair semantic contrast. Inversion
//! and especially hue rotation can change relative luminance, so callers must
//! re-check foreground/background pairs against their accessibility target
//! after transforming a palette.

#[cfg(test)]
use eframe::egui::Color32;
use syntect::highlighting::{Color as SyntectColor, Theme as SyntectTheme};

use crate::sublime_theme::{ImportedTheme, Rgba, SemanticPalette, is_dark_background};

/// User-configurable operations applied uniformly to every theme color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeTransform {
    pub invert: bool,
    pub hue_shift_degrees: f32,
}

impl Default for ThemeTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl ThemeTransform {
    pub const IDENTITY: Self = Self {
        invert: false,
        hue_shift_degrees: 0.0,
    };

    pub const fn new(invert: bool, hue_shift_degrees: f32) -> Self {
        Self {
            invert,
            hue_shift_degrees,
        }
    }

    /// The hue rotation normalized to the half-open range `[0, 360)`.
    ///
    /// Non-finite values are treated as zero. This keeps a corrupt or
    /// half-entered settings value from turning every output channel into 0.
    pub fn normalized_hue_shift(self) -> f32 {
        if self.hue_shift_degrees.is_finite() {
            self.hue_shift_degrees.rem_euclid(360.0)
        } else {
            0.0
        }
    }

    #[cfg(test)]
    pub fn is_identity(self) -> bool {
        !self.invert && self.normalized_hue_shift() == 0.0
    }

    /// Transform an unpremultiplied sRGB color while preserving alpha.
    pub fn apply_rgba(self, color: Rgba) -> Rgba {
        let color = if self.invert {
            Rgba::from_rgba(255 - color.r, 255 - color.g, 255 - color.b, color.a)
        } else {
            color
        };
        rotate_hue(color, self.normalized_hue_shift())
    }

    /// Transform an egui color without changing its alpha channel.
    #[cfg(test)]
    pub fn apply_color32(self, color: Color32) -> Color32 {
        let [red, green, blue, alpha] = color.to_srgba_unmultiplied();
        let transformed = self.apply_rgba(Rgba::from_rgba(red, green, blue, alpha));
        Color32::from_rgba_unmultiplied(transformed.r, transformed.g, transformed.b, transformed.a)
    }

    /// Transform every semantic role used by application chrome and editors.
    pub fn apply_semantic_palette(self, palette: SemanticPalette) -> SemanticPalette {
        SemanticPalette {
            background: self.apply_rgba(palette.background),
            surface: self.apply_rgba(palette.surface),
            elevated_surface: self.apply_rgba(palette.elevated_surface),
            foreground: self.apply_rgba(palette.foreground),
            muted: self.apply_rgba(palette.muted),
            border: self.apply_rgba(palette.border),
            accent: self.apply_rgba(palette.accent),
            error: self.apply_rgba(palette.error),
            warning: self.apply_rgba(palette.warning),
            info: self.apply_rgba(palette.info),
            success: self.apply_rgba(palette.success),
            editor_background: self.apply_rgba(palette.editor_background),
            current_line: self.apply_rgba(palette.current_line),
            selection: self.apply_rgba(palette.selection),
            selection_foreground: self.apply_rgba(palette.selection_foreground),
            caret: self.apply_rgba(palette.caret),
            gutter_background: self.apply_rgba(palette.gutter_background),
            gutter_foreground: self.apply_rgba(palette.gutter_foreground),
            plain: self.apply_rgba(palette.plain),
            comment: self.apply_rgba(palette.comment),
            operator: self.apply_rgba(palette.operator),
            number: self.apply_rgba(palette.number),
            emphasis: self.apply_rgba(palette.emphasis),
            link: self.apply_rgba(palette.link),
            string: self.apply_rgba(palette.string),
            label: self.apply_rgba(palette.label),
            heading: self.apply_rgba(palette.heading),
            keyword: self.apply_rgba(palette.keyword),
            interpolated: self.apply_rgba(palette.interpolated),
            error_background: self.apply_rgba(palette.error_background),
        }
    }

    /// Transform all structured colors in a Syntect theme in place.
    ///
    /// `popup_css` and `phantom_css` are intentionally retained verbatim:
    /// Syntect exposes them as opaque CSS strings rather than parsed colors.
    pub fn apply_syntect_theme(self, theme: &mut SyntectTheme) {
        let settings = &mut theme.settings;
        for value in [
            &mut settings.foreground,
            &mut settings.background,
            &mut settings.caret,
            &mut settings.line_highlight,
            &mut settings.misspelling,
            &mut settings.minimap_border,
            &mut settings.accent,
            &mut settings.bracket_contents_foreground,
            &mut settings.brackets_foreground,
            &mut settings.brackets_background,
            &mut settings.tags_foreground,
            &mut settings.highlight,
            &mut settings.find_highlight,
            &mut settings.find_highlight_foreground,
            &mut settings.gutter,
            &mut settings.gutter_foreground,
            &mut settings.selection,
            &mut settings.selection_foreground,
            &mut settings.selection_border,
            &mut settings.inactive_selection,
            &mut settings.inactive_selection_foreground,
            &mut settings.guide,
            &mut settings.active_guide,
            &mut settings.stack_guide,
            &mut settings.shadow,
        ]
        .into_iter()
        .flatten()
        {
            *value = self.apply_syntect_color(*value);
        }

        for item in &mut theme.scopes {
            if let Some(color) = &mut item.style.foreground {
                *color = self.apply_syntect_color(*color);
            }
            if let Some(color) = &mut item.style.background {
                *color = self.apply_syntect_color(*color);
            }
        }
    }

    /// Transform both halves of an imported theme and update its inferred
    /// light/dark classification from the resulting semantic background.
    pub fn apply_imported_theme(self, theme: &mut ImportedTheme) {
        theme.palette = self.apply_semantic_palette(theme.palette);
        self.apply_syntect_theme(&mut theme.syntect_theme);
        theme.dark_mode = is_dark_background(theme.palette.background);
    }

    fn apply_syntect_color(self, color: SyntectColor) -> SyntectColor {
        let transformed = self.apply_rgba(Rgba::from(color));
        transformed.into()
    }
}

fn rotate_hue(color: Rgba, degrees: f32) -> Rgba {
    if degrees == 0.0 {
        return color;
    }

    let red = f32::from(color.r) / 255.0;
    let green = f32::from(color.g) / 255.0;
    let blue = f32::from(color.b) / 255.0;
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let chroma = maximum - minimum;

    // Hue is undefined for greys. Returning the original channels also avoids
    // introducing a colored rounding artefact into neutral UI surfaces.
    if chroma <= f32::EPSILON {
        return color;
    }

    let hue_sector = if maximum == red {
        ((green - blue) / chroma).rem_euclid(6.0)
    } else if maximum == green {
        (blue - red) / chroma + 2.0
    } else {
        (red - green) / chroma + 4.0
    };
    let hue = (hue_sector * 60.0 + degrees).rem_euclid(360.0);
    let lightness = (maximum + minimum) / 2.0;
    let saturation = chroma / (1.0 - (2.0 * lightness - 1.0).abs());
    let rotated_chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let intermediate = rotated_chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let (red, green, blue) = match (hue / 60.0).floor() as u8 {
        0 => (rotated_chroma, intermediate, 0.0),
        1 => (intermediate, rotated_chroma, 0.0),
        2 => (0.0, rotated_chroma, intermediate),
        3 => (0.0, intermediate, rotated_chroma),
        4 => (intermediate, 0.0, rotated_chroma),
        _ => (rotated_chroma, 0.0, intermediate),
    };
    let offset = lightness - rotated_chroma / 2.0;

    Rgba::from_rgba(
        unit_to_channel(red + offset),
        unit_to_channel(green + offset),
        unit_to_channel(blue + offset),
        color.a,
    )
}

fn unit_to_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use syntect::highlighting::{FontStyle, StyleModifier, ThemeItem, ThemeSettings};

    use super::*;
    use crate::sublime_theme::ThemeFormat;

    fn repeated_palette(color: Rgba) -> SemanticPalette {
        SemanticPalette {
            background: color,
            surface: color,
            elevated_surface: color,
            foreground: color,
            muted: color,
            border: color,
            accent: color,
            error: color,
            warning: color,
            info: color,
            success: color,
            editor_background: color,
            current_line: color,
            selection: color,
            selection_foreground: color,
            caret: color,
            gutter_background: color,
            gutter_foreground: color,
            plain: color,
            comment: color,
            operator: color,
            number: color,
            emphasis: color,
            link: color,
            string: color,
            label: color,
            heading: color,
            keyword: color,
            interpolated: color,
            error_background: color,
        }
    }

    #[test]
    fn identity_and_full_turns_are_bit_exact() {
        let color = Rgba::from_rgba(17, 93, 211, 67);
        assert_eq!(ThemeTransform::IDENTITY.apply_rgba(color), color);
        assert_eq!(ThemeTransform::new(false, 360.0).apply_rgba(color), color);
        assert_eq!(ThemeTransform::new(false, -720.0).apply_rgba(color), color);
        assert!(ThemeTransform::new(false, f32::NAN).is_identity());
        assert_eq!(
            ThemeTransform::new(false, f32::INFINITY).apply_rgba(color),
            color
        );
    }

    #[test]
    fn inversion_is_exact_self_inverse_and_preserves_alpha() {
        let original = Rgba::from_rgba(12, 140, 251, 39);
        let invert = ThemeTransform::new(true, 0.0);
        let transformed = invert.apply_rgba(original);
        assert_eq!(transformed, Rgba::from_rgba(243, 115, 4, 39));
        assert_eq!(invert.apply_rgba(transformed), original);
    }

    #[test]
    fn hue_rotation_wraps_in_both_directions() {
        let red = Rgba::from_rgba(255, 0, 0, 91);
        assert_eq!(
            ThemeTransform::new(false, 120.0).apply_rgba(red),
            Rgba::from_rgba(0, 255, 0, 91)
        );
        assert_eq!(
            ThemeTransform::new(false, -120.0).apply_rgba(red),
            Rgba::from_rgba(0, 0, 255, 91)
        );
        assert_eq!(
            ThemeTransform::new(false, 600.0).apply_rgba(red),
            ThemeTransform::new(false, -120.0).apply_rgba(red)
        );
    }

    #[test]
    fn achromatic_colors_never_gain_a_tint() {
        for channel in [0, 1, 127, 254, 255] {
            let grey = Rgba::from_rgba(channel, channel, channel, 123);
            assert_eq!(ThemeTransform::new(false, 73.5).apply_rgba(grey), grey);
        }
    }

    #[test]
    fn combined_transform_is_explicitly_invert_then_hue_shift() {
        let original = Rgba::from_rgba(31, 92, 207, 204);
        let combined = ThemeTransform::new(true, 47.0).apply_rgba(original);
        let inverted = ThemeTransform::new(true, 0.0).apply_rgba(original);
        let reference = ThemeTransform::new(false, 47.0).apply_rgba(inverted);
        assert_eq!(combined, reference);
    }

    #[test]
    fn opposite_transform_round_trips_with_only_channel_rounding() {
        let original = Rgba::from_rgba(19, 147, 221, 88);
        let transformed = ThemeTransform::new(true, 37.0).apply_rgba(original);
        let round_trip = ThemeTransform::new(true, -37.0).apply_rgba(transformed);
        for (actual, expected) in [
            (round_trip.r, original.r),
            (round_trip.g, original.g),
            (round_trip.b, original.b),
            (round_trip.a, original.a),
        ] {
            assert!(u8::abs_diff(actual, expected) <= 1);
        }
    }

    #[test]
    fn color32_mapping_preserves_unmultiplied_alpha() {
        let original = Color32::from_rgba_unmultiplied(90, 20, 150, 33);
        let transform = ThemeTransform::new(true, 90.0);
        let transformed = transform.apply_color32(original);
        assert_eq!(transformed.a(), 33);

        // Color32 is premultiplied internally, so compare against its own
        // unmultiplied representation to account for low-alpha quantization.
        let [red, green, blue, alpha] = original.to_srgba_unmultiplied();
        let expected = transform.apply_rgba(Rgba::from_rgba(red, green, blue, alpha));
        assert_eq!(
            transformed,
            Color32::from_rgba_unmultiplied(expected.r, expected.g, expected.b, expected.a)
        );
    }

    #[test]
    fn semantic_palette_maps_every_role() {
        let source = Rgba::from_rgba(10, 40, 90, 70);
        let expected = ThemeTransform::new(true, 120.0).apply_rgba(source);
        assert_eq!(
            ThemeTransform::new(true, 120.0).apply_semantic_palette(repeated_palette(source)),
            repeated_palette(expected)
        );
    }

    #[test]
    fn syntect_mapping_covers_settings_and_scopes_without_touching_metadata() {
        let source = SyntectColor {
            r: 12,
            g: 34,
            b: 56,
            a: 78,
        };
        let mut theme = SyntectTheme {
            name: Some("Example".to_owned()),
            author: Some("Author".to_owned()),
            settings: ThemeSettings {
                foreground: Some(source),
                background: Some(source),
                popup_css: Some("color: #123456".to_owned()),
                ..ThemeSettings::default()
            },
            scopes: vec![ThemeItem {
                style: StyleModifier {
                    foreground: Some(source),
                    background: Some(source),
                    font_style: Some(FontStyle::BOLD),
                },
                ..ThemeItem::default()
            }],
        };

        ThemeTransform::new(true, 0.0).apply_syntect_theme(&mut theme);

        let expected = SyntectColor {
            r: 243,
            g: 221,
            b: 199,
            a: 78,
        };
        assert_eq!(theme.name.as_deref(), Some("Example"));
        assert_eq!(theme.settings.foreground, Some(expected));
        assert_eq!(theme.settings.background, Some(expected));
        assert_eq!(theme.settings.popup_css.as_deref(), Some("color: #123456"));
        assert_eq!(theme.scopes[0].style.foreground, Some(expected));
        assert_eq!(theme.scopes[0].style.background, Some(expected));
        assert_eq!(theme.scopes[0].style.font_style, Some(FontStyle::BOLD));
    }

    #[test]
    fn imported_theme_keeps_both_color_models_synchronized() {
        let dark = Rgba::rgb(8, 12, 16);
        let syntect_dark: SyntectColor = dark.into();
        let mut theme = ImportedTheme {
            name: Some("Dark".to_owned()),
            author: None,
            format: ThemeFormat::SublimeColorScheme,
            dark_mode: true,
            palette: repeated_palette(dark),
            syntect_theme: SyntectTheme {
                settings: ThemeSettings {
                    background: Some(syntect_dark),
                    ..ThemeSettings::default()
                },
                ..SyntectTheme::default()
            },
        };

        ThemeTransform::new(true, 0.0).apply_imported_theme(&mut theme);

        assert!(!theme.dark_mode);
        assert_eq!(theme.palette.background, Rgba::rgb(247, 243, 239));
        assert_eq!(
            theme.syntect_theme.settings.background,
            Some(SyntectColor {
                r: 247,
                g: 243,
                b: 239,
                a: 255,
            })
        );
    }

    #[test]
    fn identity_preserves_the_complete_resolved_theme_and_appearance() {
        // #aaaaaa falls between the two thresholds that previously disagreed,
        // so this catches an identity transform reclassifying a light import.
        let background = Rgba::rgb(0xaa, 0xaa, 0xaa);
        let syntect_background: SyntectColor = background.into();
        let original = ImportedTheme {
            name: Some("Midtone".to_owned()),
            author: Some("Theme Author".to_owned()),
            format: ThemeFormat::SublimeColorScheme,
            dark_mode: false,
            palette: repeated_palette(background),
            syntect_theme: SyntectTheme {
                name: Some("Midtone".to_owned()),
                author: Some("Theme Author".to_owned()),
                settings: ThemeSettings {
                    background: Some(syntect_background),
                    foreground: Some(SyntectColor {
                        r: 1,
                        g: 2,
                        b: 3,
                        a: 4,
                    }),
                    ..ThemeSettings::default()
                },
                ..SyntectTheme::default()
            },
        };
        let mut transformed = original.clone();

        ThemeTransform::IDENTITY.apply_imported_theme(&mut transformed);

        assert_eq!(transformed, original);
        assert!(!transformed.dark_mode);
    }
}
