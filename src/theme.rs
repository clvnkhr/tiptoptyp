//! Central visual tokens and component styles for tiptoptyp.
//!
//! Keep behavior and document-domain constants out of this module. Values here
//! describe rendered geometry, typography, color, or UI motion and are shared
//! by the main and child viewports.

use std::{cell::Cell, sync::Arc, time::Duration};

use eframe::egui::{self, Align, Color32, FontFamily, FontId, Layout, Rect, RichText, Vec2};

use crate::sublime_theme::{Rgba, SemanticPalette};

#[derive(Clone, Copy)]
struct ImportedPalette {
    dark_mode: bool,
    colors: SemanticPalette,
}

thread_local! {
    static IMPORTED_PALETTE: Cell<Option<ImportedPalette>> = const { Cell::new(None) };
}

/// Install or clear the current Sublime-derived visual palette.
///
/// egui runs application UI on one thread, so thread-local state keeps tests
/// isolated while allowing the existing small color-token API to remain the
/// single boundary used by panels, popups, and both syntax highlighters.
pub fn set_imported_palette(imported: Option<(bool, SemanticPalette)>) {
    IMPORTED_PALETTE.set(imported.map(|(dark_mode, colors)| ImportedPalette { dark_mode, colors }));
}

fn imported_palette() -> Option<ImportedPalette> {
    IMPORTED_PALETTE.get()
}

fn color(color: Rgba) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r, color.g, color.b, color.a)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpaceScale {
    pub hairline: f32,
    pub tight: f32,
    pub small: f32,
    pub control: f32,
    pub content: f32,
    pub card: f32,
}

pub const SPACE: SpaceScale = SpaceScale {
    hairline: 1.0,
    tight: 2.0,
    small: 4.0,
    control: 6.0,
    content: 8.0,
    card: 10.0,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RadiusScale {
    pub row: u8,
    pub chip: u8,
    pub card: u8,
}

pub const RADIUS: RadiusScale = RadiusScale {
    row: 2,
    chip: 4,
    card: 8,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeScale {
    pub supporting: f32,
    pub annotation: f32,
    pub content: f32,
}

pub const TYPE: TypeScale = TypeScale {
    supporting: 12.0,
    annotation: 12.5,
    content: 15.0,
};

/// Primary editor text, shared by Typst and generic-file highlighters.
pub fn editor_font() -> FontId {
    FontId::new(
        TYPE.content,
        FontFamily::Name(Arc::from(EDITOR_REGULAR_FAMILY)),
    )
}

const EDITOR_REGULAR_FAMILY: &str = "tiptoptyp-editor-regular";
const EDITOR_STRONG_FAMILY: &str = "tiptoptyp-editor-strong";

/// Select the editor's regular or strong font role.
///
/// On macOS the named roles resolve to the regular and bold faces of the
/// system-provided Menlo collection. Other platforms retain egui's bundled
/// metric-compatible monospace fallback when no paired face is available.
pub fn editor_font_with_weight(bold: bool) -> FontId {
    let family = if bold {
        FontFamily::Name(Arc::from(EDITOR_STRONG_FAMILY))
    } else {
        FontFamily::Name(Arc::from(EDITOR_REGULAR_FAMILY))
    };
    FontId::new(TYPE.content, family)
}

/// Register the named strong editor role while retaining every bundled glyph
/// fallback. This is called once during application construction.
pub fn configure_editor_fonts(context: &egui::Context) {
    let mut definitions = egui::FontDefinitions::default();
    let fallback = definitions
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let mut regular = fallback.clone();
    let mut strong = fallback;

    // Use the installed system font at runtime; no third-party font bytes are
    // copied into the repository or application bundle.
    #[cfg(target_os = "macos")]
    if let Ok(bytes) = std::fs::read("/System/Library/Fonts/Menlo.ttc") {
        const REGULAR_FACE: &str = "tiptoptyp-menlo-regular";
        const BOLD_FACE: &str = "tiptoptyp-menlo-bold";
        let mut regular_data = egui::FontData::from_owned(bytes.clone());
        regular_data.index = 0;
        let mut bold_data = egui::FontData::from_owned(bytes);
        bold_data.index = 1;
        definitions
            .font_data
            .insert(REGULAR_FACE.to_owned(), Arc::new(regular_data));
        definitions
            .font_data
            .insert(BOLD_FACE.to_owned(), Arc::new(bold_data));
        regular.insert(0, REGULAR_FACE.to_owned());
        strong.insert(0, BOLD_FACE.to_owned());
    }

    definitions
        .families
        .insert(FontFamily::Name(Arc::from(EDITOR_REGULAR_FAMILY)), regular);
    definitions
        .families
        .insert(FontFamily::Name(Arc::from(EDITOR_STRONG_FAMILY)), strong);
    context.set_fonts(definitions);
}

/// Compact monospace metadata drawn alongside editor content.
pub fn annotation_font() -> FontId {
    FontId::new(TYPE.annotation, FontFamily::Monospace)
}

/// Supporting interface copy such as compact tree annotations.
pub fn supporting_font() -> FontId {
    FontId::new(TYPE.supporting, FontFamily::Proportional)
}

const GENERIC_SYNTAX_THEME_DARK: &str = "base16-ocean.dark";
const GENERIC_SYNTAX_THEME_LIGHT: &str = "InspiredGitHub";

/// The bundled Syntect theme that complements the current interface contrast.
pub fn generic_syntax_theme_name(dark_mode: bool) -> &'static str {
    if dark_mode {
        GENERIC_SYNTAX_THEME_DARK
    } else {
        GENERIC_SYNTAX_THEME_LIGHT
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeMetrics {
    pub main_size: Vec2,
    pub main_min_size: Vec2,
    pub toolbar_height: f32,
    pub status_height: f32,
    pub panel_header_height: f32,
    pub settings_width: f32,
    pub settings_height: f32,
    pub settings_min_size: Vec2,
    pub typst_overrides_width: f32,
    pub typst_overrides_height: f32,
    pub typst_overrides_min_size: Vec2,
    pub problems_default_height: f32,
    pub problems_min_height: f32,
    pub problems_max_height: f32,
    pub explorer_default_width: f32,
    pub explorer_min_width: f32,
    pub split_editor_fraction: f32,
    pub split_preview_fraction: f32,
    pub split_preview_reserve: f32,
    pub split_editor_minimum: f32,
    pub split_pane_hard_minimum: f32,
}

/// The width contract for the two panes in Split mode.
///
/// Keeping this calculation outside the egui callback makes the most fragile
/// part of the main layout deterministic and directly testable. In
/// particular, very narrow windows must not let the editor's minimum consume
/// the preview pane or produce an invalid panel range.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitPaneLayout {
    pub editor_width: f32,
    pub editor_minimum: f32,
    pub editor_maximum: f32,
    pub preview_width: f32,
}

/// Calculate a valid split layout for the current content width.
pub fn split_pane_layout(available_width: f32) -> SplitPaneLayout {
    let available_width = available_width.max(0.0);
    let hard_minimum = METRICS.chrome.split_pane_hard_minimum;
    let pane_minimum = (available_width / 2.0).min(hard_minimum);
    let preview_reserve = METRICS
        .chrome
        .split_preview_reserve
        .min((available_width * METRICS.chrome.split_preview_fraction).max(hard_minimum));
    let preview_reserve = preview_reserve.min((available_width - pane_minimum).max(0.0));
    let editor_maximum = (available_width - preview_reserve).max(pane_minimum);
    let editor_minimum = METRICS
        .chrome
        .split_editor_minimum
        .min(editor_maximum)
        .min(available_width);
    let editor_width = (available_width * METRICS.chrome.split_editor_fraction)
        .clamp(editor_minimum, editor_maximum);

    SplitPaneLayout {
        editor_width,
        editor_minimum,
        editor_maximum,
        preview_width: (available_width - editor_width).max(0.0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpacingMetrics {
    pub global_item: Vec2,
    pub global_button_padding: Vec2,
    pub dense_button_padding_x: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarMetrics {
    pub compact_breakpoint: f32,
    pub traffic_lights_fallback_width: f32,
    pub traffic_lights_gap: f32,
    pub title_character_width: f32,
    pub title_padding: f32,
    pub title_height: f32,
    pub compact_title_min: f32,
    pub compact_title_max: f32,
    pub title_min: f32,
    pub title_max: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopupMetrics {
    pub card_inner_margin: i8,
    pub menu_outer_margin: i8,
    pub menu_horizontal_chrome: f32,
    pub menu_vertical_chrome: f32,
    pub viewport_edge: f32,
    pub rename_window_inset: f32,
    pub rename_max_width: f32,
    pub rename_input_height: f32,
    pub modal_window_inset: f32,
    pub modal_max_width: f32,
    pub modal_height_inset: f32,
    pub modal_message_min_height: f32,
    pub modal_message_max_height: f32,
    pub tooltip_width: f32,
    pub tooltip_text_padding: f32,
    pub tooltip_min_width: f32,
    pub tooltip_max_width: f32,
    pub tooltip_title_height: f32,
    pub tooltip_min_height: f32,
    pub tooltip_max_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextMetrics {
    pub approximate_character_ratio: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorMetrics {
    pub find_field_width: f32,
    pub wrapped_minimum_width: f32,
    pub source_character_width: f32,
    pub diagnostic_character_width: f32,
    pub unwrapped_width_padding: f32,
    pub unwrapped_minimum_width: f32,
    pub attention_start_radius: f32,
    pub attention_radius_growth: f32,
    pub attention_ring_start_radius: f32,
    pub attention_ring_growth: f32,
    pub attention_ring_width: f32,
    pub attention_ring_opacity: f32,
    pub attention_segments: u32,
    pub diagnostic_background_opacity: f32,
    pub annotation_gap: f32,
    pub tooltip_gap: f32,
    pub line_number_right_gap: f32,
    pub line_number_separator_gap: f32,
    pub line_number_separator_width: f32,
    pub diagnostic_marker_width: f32,
    pub diagnostic_marker_radius: f32,
    pub gutter_disabled_width: i8,
    pub gutter_digit_width: u32,
    pub gutter_base_width: u32,
    pub gutter_max_width: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewMetrics {
    pub page_margin: f32,
    pub page_gap: f32,
    pub shadow_offset_y: f32,
    pub shadow_expand: f32,
    pub shadow_radius: f32,
    pub page_radius: f32,
    pub page_border_width: f32,
    pub transition_vertical_fraction: f32,
    pub transition_min_top_space: f32,
    pub dark_transform_rgb_percent: [u16; 3],
    pub header_pages_min_width: f32,
    pub header_zoom_min_width: f32,
    pub header_percent_min_width: f32,
    pub zoom_step: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxMetrics {
    pub link_underline_width: f32,
    pub override_sample_background_alpha: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ExplorerMetrics {
    pub header_refresh_width: f32,
    pub header_row_height: f32,
    pub section_header_height: f32,
    pub section_gap: f32,
    pub row_height: f32,
    pub detail_breakpoint: f32,
    pub detail_width: f32,
    pub outline_indent: f32,
    pub outline_max_depth: usize,
    pub tree_icon_size: Vec2,
    pub tree_icon_stroke: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconMetrics {
    pub button_size: Vec2,
    pub button_icon_shrink: f32,
    pub static_size: f32,
    pub static_shrink: f32,
    pub stroke_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusChipMetrics {
    pub vertical_margin: i8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuMetrics {
    pub row_height: f32,
    pub file_size: Vec2,
    pub edit_size: Vec2,
    pub workspace_size: Vec2,
    pub editor_size: Vec2,
    pub status_log_size: Vec2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsMetrics {
    pub appearance_label_width: f32,
    pub theme_picker_max_height: f32,
    pub tool_gap: f32,
    pub source_preview_trigger_width: f32,
    pub tool_path_row_height: f32,
    pub tool_custom_label_reserve: f32,
    pub tool_custom_min_width: f32,
    pub tool_custom_max_width: f32,
    pub tool_path_min_width: f32,
    pub tool_path_estimated_font_size: f32,
    pub override_role_width: f32,
    pub override_color_width: f32,
    pub override_decoration_width: f32,
    pub override_sample_width: f32,
    pub override_reset_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProblemsMetrics {
    pub detail_indent: f32,
    pub hover_opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionTokens {
    pub editor_attention: Duration,
    pub hover_reset_gap: Duration,
    pub hover_poll: Duration,
    pub animation_frame: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemeMetrics {
    pub chrome: ChromeMetrics,
    pub spacing: SpacingMetrics,
    pub toolbar: ToolbarMetrics,
    pub popup: PopupMetrics,
    pub text: TextMetrics,
    pub editor: EditorMetrics,
    pub preview: PreviewMetrics,
    pub syntax: SyntaxMetrics,
    pub explorer: ExplorerMetrics,
    pub icon: IconMetrics,
    pub status_chip: StatusChipMetrics,
    pub menu: MenuMetrics,
    pub settings: SettingsMetrics,
    pub problems: ProblemsMetrics,
    pub motion: MotionTokens,
}

pub const METRICS: ThemeMetrics = ThemeMetrics {
    chrome: ChromeMetrics {
        main_size: Vec2::new(1400.0, 900.0),
        main_min_size: Vec2::new(220.0, 160.0),
        toolbar_height: 30.0,
        status_height: 24.0,
        panel_header_height: 28.0,
        settings_width: 620.0,
        settings_height: 560.0,
        settings_min_size: Vec2::new(360.0, 260.0),
        typst_overrides_width: 920.0,
        typst_overrides_height: 680.0,
        typst_overrides_min_size: Vec2::new(420.0, 300.0),
        problems_default_height: 140.0,
        problems_min_height: 70.0,
        problems_max_height: 320.0,
        explorer_default_width: 230.0,
        explorer_min_width: 0.0,
        split_editor_fraction: 0.52,
        split_preview_fraction: 0.48,
        split_preview_reserve: 96.0,
        split_editor_minimum: 100.0,
        split_pane_hard_minimum: 40.0,
    },
    spacing: SpacingMetrics {
        global_item: Vec2::new(SPACE.control, SPACE.small),
        global_button_padding: Vec2::new(7.0, 3.0),
        dense_button_padding_x: 3.0,
    },
    toolbar: ToolbarMetrics {
        compact_breakpoint: 620.0,
        traffic_lights_fallback_width: 64.0,
        traffic_lights_gap: 4.0,
        title_character_width: 8.0,
        title_padding: 8.0,
        title_height: 20.0,
        compact_title_min: 28.0,
        compact_title_max: 76.0,
        title_min: 48.0,
        title_max: 190.0,
    },
    popup: PopupMetrics {
        card_inner_margin: 9,
        menu_outer_margin: 7,
        menu_horizontal_chrome: 20.0,
        menu_vertical_chrome: 14.0,
        viewport_edge: 4.0,
        rename_window_inset: 52.0,
        rename_max_width: 340.0,
        rename_input_height: 24.0,
        modal_window_inset: 52.0,
        modal_max_width: 420.0,
        modal_height_inset: 116.0,
        modal_message_min_height: 24.0,
        modal_message_max_height: 180.0,
        tooltip_width: 360.0,
        tooltip_text_padding: 18.0,
        tooltip_min_width: 96.0,
        tooltip_max_width: 320.0,
        tooltip_title_height: 26.0,
        tooltip_min_height: 68.0,
        tooltip_max_height: 256.0,
    },
    text: TextMetrics {
        approximate_character_ratio: 0.56,
    },
    editor: EditorMetrics {
        find_field_width: 180.0,
        wrapped_minimum_width: 24.0,
        source_character_width: 8.5,
        diagnostic_character_width: 7.2,
        unwrapped_width_padding: 100.0,
        unwrapped_minimum_width: 640.0,
        attention_start_radius: 18.0,
        attention_radius_growth: 46.0,
        attention_ring_start_radius: 7.0,
        attention_ring_growth: 34.0,
        attention_ring_width: 1.2,
        attention_ring_opacity: 0.72,
        attention_segments: 28,
        diagnostic_background_opacity: 0.15,
        annotation_gap: 8.0,
        tooltip_gap: 6.0,
        line_number_right_gap: 9.0,
        line_number_separator_gap: 5.0,
        line_number_separator_width: 1.0,
        diagnostic_marker_width: 3.0,
        diagnostic_marker_radius: 1.5,
        gutter_disabled_width: 4,
        gutter_digit_width: 9,
        gutter_base_width: 18,
        gutter_max_width: 120,
    },
    preview: PreviewMetrics {
        page_margin: 28.0,
        page_gap: 24.0,
        shadow_offset_y: 3.0,
        shadow_expand: 2.0,
        shadow_radius: 3.0,
        page_radius: 1.0,
        page_border_width: 1.0,
        transition_vertical_fraction: 0.42,
        transition_min_top_space: 12.0,
        dark_transform_rgb_percent: [92, 94, 100],
        header_pages_min_width: 185.0,
        header_zoom_min_width: 315.0,
        header_percent_min_width: 400.0,
        zoom_step: 1.15,
    },
    syntax: SyntaxMetrics {
        link_underline_width: 1.0,
        override_sample_background_alpha: 32,
    },
    explorer: ExplorerMetrics {
        header_refresh_width: 24.0,
        header_row_height: 20.0,
        section_header_height: 20.0,
        section_gap: 4.0,
        row_height: 20.0,
        detail_breakpoint: 150.0,
        detail_width: 72.0,
        outline_indent: 10.0,
        outline_max_depth: 6,
        tree_icon_size: Vec2::new(14.0, 12.0),
        tree_icon_stroke: 1.2,
    },
    icon: IconMetrics {
        button_size: Vec2::new(22.0, 20.0),
        button_icon_shrink: 4.0,
        static_size: 16.0,
        static_shrink: 1.0,
        stroke_width: 1.5,
    },
    status_chip: StatusChipMetrics { vertical_margin: 3 },
    menu: MenuMetrics {
        row_height: 24.0,
        file_size: Vec2::new(260.0, 220.0),
        edit_size: Vec2::new(280.0, 320.0),
        workspace_size: Vec2::new(220.0, 182.0),
        editor_size: Vec2::new(220.0, 240.0),
        status_log_size: Vec2::new(360.0, 250.0),
    },
    settings: SettingsMetrics {
        appearance_label_width: 70.0,
        theme_picker_max_height: 435.0,
        tool_gap: 5.0,
        source_preview_trigger_width: 142.0,
        tool_path_row_height: 18.0,
        tool_custom_label_reserve: 76.0,
        tool_custom_min_width: 100.0,
        tool_custom_max_width: 270.0,
        tool_path_min_width: 32.0,
        tool_path_estimated_font_size: 11.0,
        override_role_width: 126.0,
        override_color_width: 94.0,
        override_decoration_width: 50.0,
        override_sample_width: 156.0,
        override_reset_width: 52.0,
    },
    problems: ProblemsMetrics {
        detail_indent: 42.0,
        hover_opacity: 0.28,
    },
    motion: MotionTokens {
        editor_attention: Duration::from_millis(260),
        hover_reset_gap: Duration::from_millis(180),
        hover_poll: Duration::from_millis(32),
        animation_frame: Duration::from_millis(16),
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub accent: Color32,
    pub error: Color32,
    pub warning: Color32,
    pub info: Color32,
    pub success: Color32,
    pub neutral: Color32,
    /// Shared tint for the editor's cursor line and the active explorer row.
    pub active_row: Color32,
    pub attention_rgb: [u8; 3],
    pub attention_max_alpha: u8,
}

pub fn palette(dark_mode: bool) -> Palette {
    if let Some(imported) = imported_palette() {
        let colors = imported.colors;
        return Palette {
            accent: color(colors.accent),
            error: color(colors.error),
            warning: color(colors.warning),
            info: color(colors.info),
            success: color(colors.success),
            neutral: color(colors.muted),
            active_row: color(colors.current_line),
            attention_rgb: [colors.info.r, colors.info.g, colors.info.b],
            attention_max_alpha: if imported.dark_mode { 105 } else { 82 },
        };
    }
    if dark_mode {
        Palette {
            accent: Color32::from_rgb(79, 140, 255),
            error: Color32::from_rgb(237, 135, 150),
            warning: Color32::from_rgb(238, 212, 159),
            info: Color32::from_rgb(125, 196, 228),
            success: Color32::from_rgb(166, 218, 149),
            neutral: Color32::from_rgb(166, 173, 186),
            active_row: Color32::from_rgba_unmultiplied(91, 143, 190, 25),
            attention_rgb: [74, 196, 235],
            attention_max_alpha: 105,
        }
    } else {
        Palette {
            accent: Color32::from_rgb(79, 140, 255),
            error: Color32::from_rgb(176, 36, 55),
            warning: Color32::from_rgb(145, 91, 10),
            info: Color32::from_rgb(0, 102, 148),
            success: Color32::from_rgb(43, 120, 48),
            neutral: Color32::from_rgb(82, 88, 99),
            active_row: Color32::from_rgba_unmultiplied(55, 122, 181, 18),
            attention_rgb: [18, 132, 193],
            attention_max_alpha: 82,
        }
    }
}

/// Editor syntax colors are kept separate from chrome/status colors because
/// they need a wider hue range while sharing the same light/dark derivation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyntaxPalette {
    pub plain: Color32,
    pub comment: Color32,
    pub operator: Color32,
    pub number: Color32,
    pub emphasis: Color32,
    pub link: Color32,
    pub string: Color32,
    pub label: Color32,
    pub heading: Color32,
    pub keyword: Color32,
    pub interpolated: Color32,
    pub error: Color32,
    pub error_background: Color32,
    pub editor_background: Color32,
}

pub fn syntax_palette(dark_mode: bool) -> SyntaxPalette {
    if let Some(imported) = imported_palette() {
        return syntax_palette_from_semantic(imported.colors);
    }
    if dark_mode {
        let semantic = palette(true);
        SyntaxPalette {
            plain: Color32::from_rgb(214, 219, 230),
            comment: Color32::from_rgb(106, 122, 144),
            operator: Color32::from_rgb(145, 215, 227),
            number: Color32::from_rgb(245, 169, 127),
            emphasis: Color32::from_rgb(244, 184, 228),
            link: semantic.info,
            string: semantic.success,
            label: Color32::from_rgb(139, 213, 202),
            heading: semantic.warning,
            keyword: Color32::from_rgb(198, 160, 246),
            interpolated: Color32::from_rgb(183, 189, 248),
            error: semantic.error,
            error_background: with_alpha(semantic.error, 34),
            editor_background: Color32::from_rgb(30, 34, 43),
        }
    } else {
        let error = Color32::from_rgb(190, 36, 54);
        SyntaxPalette {
            plain: Color32::from_rgb(52, 58, 70),
            comment: Color32::from_rgb(120, 126, 140),
            operator: Color32::from_rgb(26, 112, 146),
            number: Color32::from_rgb(190, 88, 40),
            emphasis: Color32::from_rgb(158, 53, 137),
            link: Color32::from_rgb(26, 112, 146),
            string: Color32::from_rgb(58, 128, 78),
            label: Color32::from_rgb(20, 122, 111),
            heading: Color32::from_rgb(145, 93, 16),
            keyword: Color32::from_rgb(126, 69, 174),
            interpolated: Color32::from_rgb(89, 77, 150),
            error,
            error_background: with_alpha(error, 24),
            editor_background: Color32::from_rgb(250, 250, 252),
        }
    }
}

/// Resolve editor syntax roles from one concrete theme without consulting the
/// process-local active palette. Child theme editors use this to preview the
/// inactive light/dark slot accurately.
pub fn syntax_palette_from_semantic(colors: SemanticPalette) -> SyntaxPalette {
    SyntaxPalette {
        plain: color(colors.plain),
        comment: color(colors.comment),
        operator: color(colors.operator),
        number: color(colors.number),
        emphasis: color(colors.emphasis),
        link: color(colors.link),
        string: color(colors.string),
        label: color(colors.label),
        heading: color(colors.heading),
        keyword: color(colors.keyword),
        interpolated: color(colors.interpolated),
        error: color(colors.error),
        error_background: color(colors.error_background),
        editor_background: color(colors.editor_background),
    }
}

fn with_alpha(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewPalette {
    pub shadow: Color32,
    pub page_fill: Color32,
    pub border: Color32,
}

pub fn preview_palette(dark_page: bool) -> PreviewPalette {
    if dark_page {
        PreviewPalette {
            shadow: Color32::from_black_alpha(120),
            page_fill: Color32::from_rgb(20, 22, 28),
            border: Color32::from_gray(74),
        }
    } else {
        PreviewPalette {
            shadow: Color32::from_black_alpha(80),
            page_fill: Color32::WHITE,
            border: Color32::from_gray(150),
        }
    }
}

pub fn configure_styles(context: &egui::Context) {
    context.style_mut_of(egui::Theme::Dark, |style| {
        style.visuals = egui::Visuals::dark();
    });
    context.style_mut_of(egui::Theme::Light, |style| {
        style.visuals = egui::Visuals::light();
    });
    if let Some(imported) = imported_palette() {
        context.all_styles_mut(|style| apply_imported_visuals(&mut style.visuals, imported));
    }
    context.all_styles_mut(|style| {
        style.spacing.item_spacing = METRICS.spacing.global_item;
        style.spacing.button_padding = METRICS.spacing.global_button_padding;
        style.visuals.menu_corner_radius = egui::CornerRadius::same(RADIUS.card);
        style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, editor_font());
    });
}

/// Build a child-viewport style for a concrete light or dark theme slot.
///
/// The main window installs only its active palette globally. Editors that
/// compare the two independently persisted slots use this helper so their
/// controls and samples reflect the slot being edited without mutating the
/// rest of the application.
pub fn style_for_semantic_palette(
    base: &egui::Style,
    dark_mode: bool,
    colors: SemanticPalette,
) -> Arc<egui::Style> {
    let mut style = base.clone();
    style.visuals = if dark_mode {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    apply_imported_visuals(&mut style.visuals, ImportedPalette { dark_mode, colors });
    Arc::new(style)
}

fn apply_imported_visuals(visuals: &mut egui::Visuals, imported: ImportedPalette) {
    let colors = imported.colors;
    let foreground = color(colors.foreground);
    let muted = color(colors.muted);
    let background = color(colors.background);
    let surface = color(colors.surface);
    let elevated = color(colors.elevated_surface);
    let border = color(colors.border);
    let accent = color(colors.accent);
    let selection = color(colors.selection);
    let selection_foreground = color(colors.selection_foreground);

    visuals.dark_mode = imported.dark_mode;
    visuals.override_text_color = Some(foreground);
    visuals.weak_text_color = Some(muted);
    visuals.panel_fill = background;
    visuals.window_fill = elevated;
    visuals.window_stroke = egui::Stroke::new(1.0, border);
    visuals.extreme_bg_color = color(colors.editor_background);
    visuals.text_edit_bg_color = Some(color(colors.editor_background));
    visuals.code_bg_color = color(colors.editor_background);
    visuals.faint_bg_color = color(colors.current_line);
    visuals.hyperlink_color = color(colors.link);
    visuals.warn_fg_color = color(colors.warning);
    visuals.error_fg_color = color(colors.error);
    visuals.selection.bg_fill = selection;
    visuals.selection.stroke = egui::Stroke::new(1.0, selection_foreground);
    visuals.text_cursor.stroke.color = color(colors.caret);

    visuals.widgets.noninteractive.bg_fill = surface;
    visuals.widgets.noninteractive.weak_bg_fill = background;
    visuals.widgets.noninteractive.bg_stroke.color = border;
    visuals.widgets.noninteractive.fg_stroke.color = foreground;
    visuals.widgets.inactive.bg_fill = surface;
    visuals.widgets.inactive.weak_bg_fill = surface;
    visuals.widgets.inactive.bg_stroke.color = border;
    visuals.widgets.inactive.fg_stroke.color = foreground;
    visuals.widgets.hovered.bg_fill = elevated;
    visuals.widgets.hovered.weak_bg_fill = elevated;
    visuals.widgets.hovered.bg_stroke.color = accent;
    visuals.widgets.hovered.fg_stroke.color = foreground;
    visuals.widgets.active.bg_fill = selection;
    visuals.widgets.active.weak_bg_fill = selection;
    visuals.widgets.active.bg_stroke.color = accent;
    visuals.widgets.active.fg_stroke.color = selection_foreground;
    visuals.widgets.open = visuals.widgets.hovered;
}

pub fn content_panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(egui::Margin::symmetric(SPACE.content as i8, 0))
}

/// A compact, theme-aware container for one independently scrolling Explorer
/// section. Keeping this alongside the other shared frames prevents each
/// section from inventing its own surface, border, or corner treatment.
pub fn explorer_section_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::new()
        .fill(style.visuals.widgets.noninteractive.bg_fill)
        .stroke(style.visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(RADIUS.row)
}

pub fn popup_card_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::popup(style)
        .corner_radius(RADIUS.card)
        .shadow(egui::epaint::Shadow::NONE)
}

pub fn dialog_card_frame(style: &egui::Style) -> egui::Frame {
    popup_card_frame(style)
        .outer_margin(egui::Margin::same(SPACE.content as i8))
        .inner_margin(egui::Margin::same(SPACE.card as i8))
}

pub fn tooltip_card_frame(style: &egui::Style) -> egui::Frame {
    popup_card_frame(style)
        .outer_margin(egui::Margin::same(SPACE.content as i8))
        .inner_margin(egui::Margin::same(METRICS.popup.card_inner_margin))
}

pub fn menu_card_frame(style: &egui::Style) -> egui::Frame {
    popup_card_frame(style).outer_margin(egui::Margin::same(METRICS.popup.menu_outer_margin))
}

pub fn popup_viewport_builder(title: impl Into<String>) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_resizable(false)
        .with_transparent(true)
        .with_decorations(false)
        .with_taskbar(false)
        .with_close_button(false)
        .with_minimize_button(false)
        .with_maximize_button(false)
        .with_has_shadow(false)
        .with_always_on_top()
}

pub fn status_chip_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::new()
        .fill(style.visuals.faint_bg_color)
        .stroke(style.visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(RADIUS.chip)
        .inner_margin(egui::Margin::symmetric(
            SPACE.control as i8,
            METRICS.status_chip.vertical_margin,
        ))
}

pub fn settings_title_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::NONE
        .fill(style.visuals.panel_fill)
        .inner_margin(egui::Margin::symmetric(SPACE.content as i8, 0))
}

pub fn settings_content_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::NONE
        .fill(style.visuals.panel_fill)
        .inner_margin(egui::Margin::same(SPACE.content as i8))
}

pub fn apply_compact_control_spacing(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.x = SPACE.small;
    ui.spacing_mut().button_padding = Vec2::new(SPACE.control, SPACE.tight);
}

pub fn apply_dense_toolbar_spacing(ui: &mut egui::Ui) {
    ui.spacing_mut().item_spacing.x = SPACE.tight;
    ui.spacing_mut().button_padding.x = METRICS.spacing.dense_button_padding_x;
}

/// Give selected navigation rows the same quiet emphasis as the cursor line.
pub fn apply_active_row_selection(ui: &mut egui::Ui) {
    ui.visuals_mut().selection.bg_fill = palette(ui.visuals().dark_mode).active_row;
    ui.visuals_mut().selection.stroke = egui::Stroke::NONE;
}

pub fn show_logo(ui: &mut egui::Ui) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            let font = FontId::proportional(TYPE.content);
            ui.label(RichText::new("t").font(font.clone()).strong());
            ui.label(RichText::new("t").font(font.clone()).strong());
            ui.label(
                RichText::new("t")
                    .font(font)
                    .strong()
                    .color(palette(ui.visuals().dark_mode).accent),
            );
        });
    });
}

pub fn panel_header(
    ui: &mut egui::Ui,
    id_salt: &'static str,
    add_contents: impl FnOnce(&mut egui::Ui),
) -> Rect {
    let fill = ui.visuals().panel_fill;
    let available = ui.available_rect_before_wrap();
    let response = egui::Panel::top(ui.id().with(id_salt))
        .exact_size(METRICS.chrome.panel_header_height)
        .frame(egui::Frame::NONE.fill(fill))
        .show(ui, |ui| {
            apply_compact_control_spacing(ui);
            ui.with_layout(Layout::left_to_right(Align::Center), add_contents);
        });
    response.response.rect.intersect(available)
}

pub fn native_theme(theme: egui::Theme) -> egui::SystemTheme {
    match theme {
        egui::Theme::Dark => egui::SystemTheme::Dark,
        egui::Theme::Light => egui::SystemTheme::Light,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    struct ImportedPaletteReset;

    impl Drop for ImportedPaletteReset {
        fn drop(&mut self) {
            set_imported_palette(None);
        }
    }

    #[test]
    fn chrome_and_component_geometry_matches_the_existing_ui() {
        assert_eq!(METRICS.chrome.toolbar_height, 30.0);
        assert_eq!(METRICS.chrome.main_size, Vec2::new(1400.0, 900.0));
        assert_eq!(METRICS.chrome.main_min_size, Vec2::new(220.0, 160.0));
        assert_eq!(METRICS.chrome.status_height, 24.0);
        assert_eq!(METRICS.chrome.panel_header_height, 28.0);
        assert_eq!(METRICS.chrome.settings_width, 620.0);
        assert_eq!(METRICS.chrome.settings_min_size, Vec2::new(360.0, 260.0));
        assert_eq!(METRICS.spacing.global_item, Vec2::new(6.0, 4.0));
        assert_eq!(METRICS.spacing.global_button_padding, Vec2::new(7.0, 3.0));
        assert_eq!(RADIUS.card, 8);
        assert_eq!(METRICS.popup.card_inner_margin, 9);
        assert_eq!(METRICS.icon.button_size, Vec2::new(22.0, 20.0));
        assert_eq!(METRICS.explorer.section_header_height, 20.0);
        assert_eq!(METRICS.explorer.section_gap, 4.0);
        assert_eq!(METRICS.explorer.row_height, 20.0);
        assert_eq!(METRICS.preview.page_margin, 28.0);
        assert_eq!(METRICS.preview.page_gap, 24.0);
        assert_eq!(METRICS.preview.dark_transform_rgb_percent, [92, 94, 100]);
        assert_eq!(METRICS.preview.header_pages_min_width, 185.0);
        assert_eq!(SPACE.tight, 2.0);
        assert_eq!(METRICS.editor.gutter_max_width, 120);
    }

    #[test]
    fn split_pane_layout_preserves_both_panes_at_normal_widths() {
        for available in [220.0, 320.0, 640.0, 1_400.0, 2_400.0] {
            let layout = split_pane_layout(available);
            assert!(layout.editor_minimum <= layout.editor_width);
            assert!(layout.editor_width <= layout.editor_maximum);
            assert!(layout.preview_width >= 0.0);
            assert!((layout.editor_width + layout.preview_width - available).abs() < 0.01);
            assert!(layout.editor_maximum + layout.preview_width >= available - 0.01);
        }
    }

    #[test]
    fn split_pane_layout_degrades_without_invalid_panel_ranges() {
        for available in [0.0, 1.0, 20.0, 79.0] {
            let layout = split_pane_layout(available);
            assert!(layout.editor_minimum >= 0.0);
            assert!(layout.editor_minimum <= layout.editor_maximum);
            assert!(layout.editor_width >= layout.editor_minimum);
            assert!(layout.editor_width <= layout.editor_maximum);
            assert!(layout.preview_width >= 0.0);
            assert!(layout.editor_width + layout.preview_width <= available + 0.01);
        }
    }

    #[test]
    fn syntax_palette_preserves_existing_editor_colors_and_type() {
        let dark = syntax_palette(true);
        let light = syntax_palette(false);
        assert_eq!(dark.plain, Color32::from_rgb(214, 219, 230));
        assert_eq!(light.plain, Color32::from_rgb(52, 58, 70));
        assert_eq!(dark.keyword, Color32::from_rgb(198, 160, 246));
        assert_eq!(light.keyword, Color32::from_rgb(126, 69, 174));
        assert_eq!(dark.error_background.a(), 34);
        assert_eq!(light.error_background.a(), 24);
        assert_eq!(TYPE.content, 15.0);
        assert_eq!(METRICS.syntax.link_underline_width, 1.0);
    }

    #[test]
    fn font_roles_derive_from_the_shared_type_scale() {
        assert_eq!(
            editor_font(),
            FontId::new(
                TYPE.content,
                FontFamily::Name(Arc::from(EDITOR_REGULAR_FAMILY))
            )
        );
        assert_eq!(
            annotation_font(),
            FontId::new(TYPE.annotation, FontFamily::Monospace)
        );
        assert_eq!(
            supporting_font(),
            FontId::new(TYPE.supporting, FontFamily::Proportional)
        );
    }

    #[test]
    fn generic_syntax_theme_tracks_interface_contrast() {
        assert_eq!(generic_syntax_theme_name(true), "base16-ocean.dark");
        assert_eq!(generic_syntax_theme_name(false), "InspiredGitHub");
        assert_ne!(
            generic_syntax_theme_name(true),
            generic_syntax_theme_name(false)
        );
    }

    #[test]
    fn shared_dark_syntax_hues_follow_semantic_status_roles() {
        let semantic = palette(true);
        let syntax = syntax_palette(true);

        assert_eq!(syntax.link, semantic.info);
        assert_eq!(syntax.string, semantic.success);
        assert_eq!(syntax.heading, semantic.warning);
        assert_eq!(syntax.error, semantic.error);
        assert_eq!(syntax.error_background, with_alpha(semantic.error, 34));
    }

    #[test]
    fn semantic_palette_preserves_light_and_dark_contrast_values() {
        let dark = palette(true);
        let light = palette(false);
        assert_eq!(dark.accent, Color32::from_rgb(79, 140, 255));
        assert_eq!(light.accent, dark.accent);
        assert_eq!(dark.error, Color32::from_rgb(237, 135, 150));
        assert_eq!(light.error, Color32::from_rgb(176, 36, 55));
        assert_eq!(
            dark.active_row,
            Color32::from_rgba_unmultiplied(91, 143, 190, 25)
        );
        assert_eq!(
            light.active_row,
            Color32::from_rgba_unmultiplied(55, 122, 181, 18)
        );
        assert_eq!(dark.attention_rgb, [74, 196, 235]);
        assert_eq!(light.attention_rgb, [18, 132, 193]);
        assert_eq!(
            preview_palette(true).page_fill,
            Color32::from_rgb(20, 22, 28)
        );
        assert_eq!(preview_palette(false).page_fill, Color32::WHITE);
    }

    #[test]
    fn component_frames_preserve_radii_margins_and_transparent_corners() {
        let style = egui::Style::default();
        let popup = popup_card_frame(&style);
        assert_eq!(popup.corner_radius, egui::CornerRadius::same(8));
        assert_eq!(popup.shadow, egui::epaint::Shadow::NONE);

        let content = content_panel_frame(&style);
        assert_eq!(content.inner_margin, egui::Margin::symmetric(8, 0));

        let explorer = explorer_section_frame(&style);
        assert_eq!(explorer.corner_radius, egui::CornerRadius::same(RADIUS.row));
        assert_eq!(explorer.fill, style.visuals.widgets.noninteractive.bg_fill);
        assert_eq!(
            explorer.stroke,
            style.visuals.widgets.noninteractive.bg_stroke
        );

        let chip = status_chip_frame(&style);
        assert_eq!(chip.corner_radius, egui::CornerRadius::same(4));
        assert_eq!(chip.inner_margin, egui::Margin::symmetric(6, 3));
    }

    #[test]
    fn light_and_dark_global_styles_keep_identical_geometry() {
        let context = egui::Context::default();
        configure_styles(&context);
        let dark = context.style_of(egui::Theme::Dark);
        let light = context.style_of(egui::Theme::Light);
        assert_eq!(dark.spacing, light.spacing);
        assert_eq!(dark.text_styles, light.text_styles);
        assert_eq!(
            dark.text_styles.get(&egui::TextStyle::Monospace),
            Some(&editor_font())
        );
        assert_eq!(dark.visuals.popup_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(dark.visuals.menu_corner_radius, egui::CornerRadius::same(8));
    }

    #[test]
    fn regular_and_strong_editor_fonts_have_distinct_family_roles() {
        assert_ne!(editor_font(), editor_font_with_weight(true));
        assert_eq!(editor_font(), editor_font_with_weight(false));
    }

    #[test]
    fn motion_values_preserve_attention_and_hover_animation_cadence() {
        assert_eq!(METRICS.motion.editor_attention, Duration::from_millis(260));
        assert_eq!(METRICS.motion.hover_reset_gap, Duration::from_millis(180));
        assert_eq!(METRICS.motion.animation_frame, Duration::from_millis(16));
    }

    #[test]
    fn sublime_semantics_flow_through_shared_chrome_and_editor_tokens() {
        let _reset = ImportedPaletteReset;
        let imported = crate::sublime_theme::import_bytes(
            Path::new("Cohesive.sublime-color-scheme"),
            br##"{
                "name": "Cohesive Dark",
                "globals": {
                    "background": "#101820",
                    "foreground": "#e8eef5",
                    "accent": "#3aa7ff",
                    "selection": "#24527a"
                },
                "rules": [
                    { "scope": "keyword.control", "foreground": "#d29cff" },
                    { "scope": "string.quoted", "foreground": "#8fd694" }
                ]
            }"##,
        )
        .unwrap();
        set_imported_palette(Some((imported.dark_mode, imported.palette)));

        let semantic = palette(false);
        let syntax = syntax_palette(false);
        assert_eq!(semantic.accent, Color32::from_rgb(58, 167, 255));
        assert_eq!(syntax.editor_background, Color32::from_rgb(16, 24, 32));
        assert_eq!(syntax.keyword, Color32::from_rgb(210, 156, 255));

        let context = egui::Context::default();
        configure_styles(&context);
        assert_eq!(
            context.style_of(egui::Theme::Dark).visuals.panel_fill,
            color(imported.palette.background)
        );
        assert_eq!(
            context.style_of(egui::Theme::Light).visuals.panel_fill,
            color(imported.palette.background)
        );
    }
}
