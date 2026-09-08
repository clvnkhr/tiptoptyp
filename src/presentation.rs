use std::path::PathBuf;

use eframe::egui;

use crate::{
    builtin_themes,
    screenshot::CaptureThemeProfile,
    settings::{
        AppSettings, ColorThemeChoice, DocumentTheme, InterfaceTheme, PreviewPreference,
        SYSTEM_THEME_ID, ToolPreference,
    },
    sublime_theme::{self, ImportedTheme, ThemeFormat},
    syntax_theme::TypstOverrideThemes,
    theme_transform::ThemeTransform,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ThemeSourceRequest {
    Builtin(String),
    Sublime(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveThemeRequest {
    pub(crate) source: ThemeSourceRequest,
    pub(crate) invert: bool,
    pub(crate) hue_shift_degrees: i16,
    pub(crate) fallback_dark: bool,
}

impl ActiveThemeRequest {
    pub(crate) fn transform(&self) -> ThemeTransform {
        ThemeTransform::new(self.invert, f32::from(self.hue_shift_degrees))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontSelection {
    pub(crate) path: Option<String>,
    pub(crate) family: Option<String>,
    pub(crate) face_index: u32,
    pub(crate) weight: u16,
    pub(crate) monospace: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedPresentationRequest {
    pub(crate) theme: ActiveThemeRequest,
    pub(crate) typst_overrides: TypstOverrideThemes,
    pub(crate) document_theme: DocumentTheme,
    pub(crate) preview_preference: PreviewPreference,
    pub(crate) typst: ToolPreference,
    pub(crate) tinymist: ToolPreference,
    pub(crate) ui_scale_percent: u16,
    pub(crate) ui_font: FontSelection,
    pub(crate) code_font: FontSelection,
    pub(crate) font_catalog_revision: u64,
}

impl ResolvedPresentationRequest {
    pub(crate) fn resolve(
        settings: &AppSettings,
        theme: ActiveThemeRequest,
        font_catalog_revision: u64,
    ) -> Self {
        Self {
            theme,
            typst_overrides: settings.typst_overrides.clone(),
            document_theme: settings.document_theme,
            preview_preference: settings.preview_preference,
            typst: settings.typst.clone(),
            tinymist: settings.tinymist.clone(),
            ui_scale_percent: settings.ui_scale_percent,
            ui_font: FontSelection {
                path: settings.ui_font_path.clone(),
                family: settings.ui_font_family.clone(),
                face_index: settings.ui_font_face_index,
                weight: settings.ui_font_weight,
                monospace: settings.ui_font_monospace,
            },
            code_font: FontSelection {
                path: settings.code_font_path.clone(),
                family: settings.code_font_family.clone(),
                face_index: settings.code_font_face_index,
                weight: settings.code_font_weight,
                monospace: true,
            },
            font_catalog_revision,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PresentationChanges {
    pub(crate) theme: bool,
    pub(crate) typst_overrides: bool,
    pub(crate) document_theme: bool,
    pub(crate) preview_preference: bool,
    pub(crate) typst: bool,
    pub(crate) tinymist: bool,
    pub(crate) ui_scale: bool,
    pub(crate) fonts: bool,
}

/// The one applied snapshot against which runtime presentation effects are
/// planned. This prevents independent `applied_*` fields from drifting apart.
#[derive(Clone, Debug)]
pub(crate) struct AppliedPresentation {
    request: ResolvedPresentationRequest,
}

impl AppliedPresentation {
    pub(crate) fn new(request: ResolvedPresentationRequest) -> Self {
        Self { request }
    }

    pub(crate) fn changes(&self, next: &ResolvedPresentationRequest) -> PresentationChanges {
        PresentationChanges {
            theme: self.request.theme != next.theme,
            typst_overrides: self.request.typst_overrides != next.typst_overrides,
            document_theme: self.request.document_theme != next.document_theme,
            preview_preference: self.request.preview_preference != next.preview_preference,
            typst: self.request.typst != next.typst,
            tinymist: self.request.tinymist != next.tinymist,
            ui_scale: self.request.ui_scale_percent != next.ui_scale_percent,
            fonts: self.request.ui_font != next.ui_font
                || self.request.code_font != next.code_font
                || self.request.font_catalog_revision != next.font_catalog_revision,
        }
    }

    pub(crate) fn commit(&mut self, request: ResolvedPresentationRequest) {
        self.request = request;
    }

    pub(crate) fn invalidate_fonts(&mut self) {
        self.request.font_catalog_revision = self.request.font_catalog_revision.wrapping_add(1);
    }

    pub(crate) fn record_ui_scale(&mut self, percent: u16) {
        self.request.ui_scale_percent = percent;
    }
}

pub(crate) fn active_theme_request(
    settings: &AppSettings,
    system_theme: Option<egui::Theme>,
    launch_override: Option<&CaptureThemeProfile>,
) -> ActiveThemeRequest {
    let system_dark = system_theme.unwrap_or(egui::Theme::Dark) == egui::Theme::Dark;
    if let Some(profile) = launch_override {
        let id = if profile.name == SYSTEM_THEME_ID {
            paired_tiptop_theme_id(system_dark)
        } else {
            profile.name.as_str()
        };
        return ActiveThemeRequest {
            source: ThemeSourceRequest::Builtin(id.to_owned()),
            invert: profile.invert,
            hue_shift_degrees: profile.hue_shift_degrees,
            fallback_dark: system_dark,
        };
    }

    let preferred_theme = settings
        .interface_theme
        .resolve(system_theme, egui::Theme::Dark);
    let fallback_dark = preferred_theme == egui::Theme::Dark;
    let source = match settings.color_theme(preferred_theme) {
        ColorThemeChoice::Builtin(id) => ThemeSourceRequest::Builtin(id.clone()),
        ColorThemeChoice::Sublime(path) => ThemeSourceRequest::Sublime(PathBuf::from(path)),
    };
    ActiveThemeRequest {
        source,
        invert: settings.theme_invert,
        hue_shift_degrees: settings.theme_hue_shift_degrees,
        fallback_dark,
    }
}

pub(crate) fn theme_request_for_appearance(
    settings: &AppSettings,
    dark: bool,
) -> ActiveThemeRequest {
    let appearance = if dark {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    };
    let source = match settings.color_theme(appearance) {
        ColorThemeChoice::Builtin(id) => ThemeSourceRequest::Builtin(id.clone()),
        ColorThemeChoice::Sublime(path) => ThemeSourceRequest::Sublime(PathBuf::from(path)),
    };
    ActiveThemeRequest {
        source,
        invert: settings.theme_invert,
        hue_shift_degrees: settings.theme_hue_shift_degrees,
        fallback_dark: dark,
    }
}

pub(crate) fn active_theme_preference(
    interface_theme: InterfaceTheme,
    has_launch_override: bool,
    active_dark: bool,
) -> egui::ThemePreference {
    if !has_launch_override && interface_theme == InterfaceTheme::System {
        egui::ThemePreference::System
    } else if active_dark {
        egui::ThemePreference::Dark
    } else {
        egui::ThemePreference::Light
    }
}

pub(crate) fn load_active_theme(request: &ActiveThemeRequest) -> Result<ImportedTheme, String> {
    let mut imported = match &request.source {
        ThemeSourceRequest::Builtin(id) => {
            let builtin =
                builtin_themes::find(id).ok_or_else(|| format!("Unknown built-in theme {id:?}"))?;
            ImportedTheme {
                name: Some(builtin.name.to_owned()),
                author: Some("tiptoptyp".to_owned()),
                format: ThemeFormat::Builtin,
                dark_mode: builtin.dark_mode,
                palette: builtin.palette,
                syntect_theme: builtin.syntect_theme(),
            }
        }
        ThemeSourceRequest::Sublime(path) => {
            let imported = sublime_theme::import_path(path).map_err(|error| error.to_string())?;
            if imported.dark_mode != request.fallback_dark {
                let inferred = if imported.dark_mode { "dark" } else { "light" };
                let assigned = if request.fallback_dark {
                    "dark"
                } else {
                    "light"
                };
                return Err(format!(
                    "{} is a {inferred} Sublime theme but is assigned to the {assigned} slot",
                    path.display()
                ));
            }
            imported
        }
    };
    request.transform().apply_imported_theme(&mut imported);
    Ok(imported)
}

pub(crate) fn load_active_theme_or_fallback(
    request: &ActiveThemeRequest,
) -> (ImportedTheme, Option<String>) {
    match load_active_theme(request) {
        Ok(theme) => (theme, None),
        Err(error) => {
            let fallback = ActiveThemeRequest {
                source: ThemeSourceRequest::Builtin(
                    paired_tiptop_theme_id(request.fallback_dark).to_owned(),
                ),
                invert: request.invert,
                hue_shift_degrees: request.hue_shift_degrees,
                fallback_dark: request.fallback_dark,
            };
            let theme = load_active_theme(&fallback)
                .expect("the paired built-in Tiptop fallback is always present");
            (theme, Some(error))
        }
    }
}

fn paired_tiptop_theme_id(dark_mode: bool) -> &'static str {
    builtin_themes::default_for_mode(dark_mode).id
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn changes_are_semantic_subrequests() {
        let settings = AppSettings::default();
        let theme = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        let first = ResolvedPresentationRequest::resolve(&settings, theme.clone(), 1);
        let applied = AppliedPresentation::new(first.clone());
        assert_eq!(
            applied.changes(&first),
            PresentationChanges {
                theme: false,
                typst_overrides: false,
                document_theme: false,
                preview_preference: false,
                typst: false,
                tinymist: false,
                ui_scale: false,
                fonts: false,
            }
        );

        let mut scaled = settings.clone();
        scaled.ui_scale_percent += 5;
        let next = ResolvedPresentationRequest::resolve(&scaled, theme, 1);
        let changes = applied.changes(&next);
        assert!(changes.ui_scale);
        assert!(!changes.fonts);
        assert!(!changes.theme);
    }

    #[test]
    fn font_catalog_revision_invalidates_only_fonts() {
        let settings = AppSettings::default();
        let theme = active_theme_request(&settings, Some(egui::Theme::Dark), None);
        let first = ResolvedPresentationRequest::resolve(&settings, theme.clone(), 1);
        let applied = AppliedPresentation::new(first);
        let next = ResolvedPresentationRequest::resolve(&settings, theme, 2);
        let changes = applied.changes(&next);
        assert!(changes.fonts);
        assert!(!changes.theme);
        assert!(!changes.ui_scale);
    }
}
