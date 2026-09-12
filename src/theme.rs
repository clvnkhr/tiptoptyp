//! Central visual tokens and component styles for tiptoptyp.
//!
//! Keep behavior and document-domain constants out of this module. Values here
//! describe rendered geometry, typography, color, or UI motion and are shared
//! by the main and child viewports.

use std::{
    cell::Cell,
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use eframe::egui::{self, Align, Color32, FontFamily, FontId, Layout, Rect, RichText, Vec2};
use skrifa::{MetadataProvider as _, Tag, attribute::Style as FontStyle};

use crate::{
    font_catalog::{FontFace, FontFamily as CatalogFontFamily, normalize_variable_weight_axis},
    sublime_theme::{Rgba, SemanticPalette},
};

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

pub const FONT_WEIGHT_NORMAL: u16 = 400;
pub const FONT_WEIGHT_BOLD: u16 = 700;
pub const EDITOR_FONT_WEIGHTS: [u16; 9] = [100, 200, 300, 400, 500, 600, 700, 800, 900];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontWeightSupport {
    Continuous { min: u16, max: u16, default: u16 },
    Discrete { values: Vec<u16>, default: u16 },
}

impl FontWeightSupport {
    pub fn default_weight(&self) -> u16 {
        match self {
            Self::Continuous { default, .. } | Self::Discrete { default, .. } => *default,
        }
    }

    pub fn clamp(&self, weight: u16) -> u16 {
        match self {
            Self::Continuous { min, max, .. } => weight.clamp(*min, *max),
            Self::Discrete { values, default } => values
                .iter()
                .copied()
                .min_by_key(|candidate| candidate.abs_diff(weight))
                .unwrap_or(*default),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontConfiguration {
    pub custom_ui_loaded: bool,
    pub custom_editor_loaded: bool,
    pub weighted_ui_loaded: bool,
    pub ui_weight_support: Option<FontWeightSupport>,
    pub code_weight_support: Option<FontWeightSupport>,
    pub editor_weight_support: FontWeightSupport,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FontRequest<'a> {
    pub family: Option<&'a CatalogFontFamily>,
    pub fallback_path: Option<&'a Path>,
    pub fallback_face_index: u32,
}

/// Primary editor text, shared by Typst and generic-file highlighters.
pub fn editor_font() -> FontId {
    editor_font_with_weight(FONT_WEIGHT_NORMAL)
}

const EDITOR_WEIGHT_FAMILY_PREFIX: &str = "tiptoptyp-editor-weight";
const WEIGHTED_UI_FAMILY: &str = "tiptoptyp-weighted-ui";
const WEIGHTED_UI_DATA: &str = "tiptoptyp-weighted-ui-data";
const WEIGHTED_UI_STRONG_FAMILY: &str = "tiptoptyp-weighted-ui-strong";
const WEIGHTED_UI_STRONG_DATA: &str = "tiptoptyp-weighted-ui-strong-data";

fn editor_weight_family(weight: u16) -> String {
    format!(
        "{EDITOR_WEIGHT_FAMILY_PREFIX}-{}",
        nearest_editor_weight(weight)
    )
}

pub fn nearest_editor_weight(weight: u16) -> u16 {
    EDITOR_FONT_WEIGHTS
        .into_iter()
        .min_by_key(|candidate| candidate.abs_diff(weight))
        .unwrap_or(FONT_WEIGHT_NORMAL)
}

fn editor_weight_from_base(role_weight: u16, base_weight: u16) -> u16 {
    (i32::from(base_weight) + i32::from(role_weight) - i32::from(FONT_WEIGHT_NORMAL))
        .clamp(1, 1_000) as u16
}

/// Read and validate a user-provided font before giving it to egui.
///
/// `FontDefinitions` panics when it receives malformed font bytes, so this
/// validation is deliberately performed at the file-picker boundary and again
/// during runtime reload. Collections are accepted through face index zero.
pub fn load_ui_font_bytes(path: &Path) -> Result<Vec<u8>, String> {
    load_ui_font_bytes_at(path, 0)
}

fn load_ui_font_bytes_at(path: &Path, face_index: u32) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("Could not read UI font {}: {error}", path.display()))?;
    skrifa::FontRef::from_index(&bytes, face_index)
        .map_err(|_| format!("{} is not a readable TTF, OTF, or TTC font", path.display()))?;
    Ok(bytes)
}

/// Select one of the editor's registered OpenType weight roles.
pub fn editor_font_with_weight(weight: u16) -> FontId {
    FontId::new(
        TYPE.content,
        FontFamily::Name(Arc::from(editor_weight_family(weight))),
    )
}

/// A heavier version of the selected interface font for active navigation
/// entries. Unlike `RichText::strong`, this changes glyph weight rather than
/// only increasing foreground contrast.
pub fn strong_ui_font() -> FontId {
    FontId::new(
        TYPE.content,
        FontFamily::Name(Arc::from(WEIGHTED_UI_STRONG_FAMILY)),
    )
}

#[derive(Clone, Debug)]
struct FontSelection {
    index: u32,
    coordinate: Option<f32>,
    support: Option<FontWeightSupport>,
}

#[derive(Clone, Debug)]
struct PreparedFontFace {
    index: u32,
    weight: u16,
    normal_style: bool,
    stretch_distance: f32,
    variable_weight: Option<(u16, u16, u16)>,
}

/// Immutable bytes plus normalized face metadata for one font file.
///
/// A configure pass may derive eleven egui entries from one selected family
/// (nine editor roles and two UI roles). Parsing the collection once keeps all
/// of those derivations on the same face/axis snapshot.
#[derive(Clone, Debug)]
struct PreparedFontFile {
    bytes: Arc<[u8]>,
    faces: Vec<PreparedFontFace>,
}

impl PreparedFontFile {
    fn from_bytes(bytes: Vec<u8>) -> Self {
        let faces = skrifa::FontRef::fonts(&bytes)
            .enumerate()
            .filter_map(|(index, font)| {
                let font = font.ok()?;
                let attributes = font.attributes();
                let variable_weight = font.axes().get_by_tag(Tag::new(b"wght")).map(|axis| {
                    normalize_variable_weight_axis(
                        axis.min_value(),
                        axis.max_value(),
                        axis.default_value(),
                    )
                });
                Some(PreparedFontFace {
                    index: index as u32,
                    weight: attributes.weight.value().round().clamp(1.0, 1_000.0) as u16,
                    normal_style: attributes.style == FontStyle::Normal,
                    stretch_distance: (attributes.stretch.ratio() - 1.0).abs(),
                    variable_weight,
                })
            })
            .collect();
        Self {
            bytes: bytes.into(),
            faces,
        }
    }

    fn face(&self, index: u32) -> Option<&PreparedFontFace> {
        self.faces.iter().find(|face| face.index == index)
    }

    fn selection(&self, requested_weight: u16) -> Option<FontSelection> {
        if let Some(face) = self
            .faces
            .iter()
            .filter(|face| face.normal_style && face.variable_weight.is_some())
            .min_by(|left, right| left.stretch_distance.total_cmp(&right.stretch_distance))
        {
            let (min, max, default) = face.variable_weight?;
            return Some(FontSelection {
                index: face.index,
                coordinate: Some(f32::from(requested_weight.clamp(min, max))),
                support: Some(FontWeightSupport::Continuous { min, max, default }),
            });
        }

        let has_normal = self.faces.iter().any(|face| face.normal_style);
        let eligible = self
            .faces
            .iter()
            .filter(|face| !has_normal || face.normal_style)
            .collect::<Vec<_>>();
        let closest_stretch = eligible
            .iter()
            .map(|face| face.stretch_distance)
            .min_by(f32::total_cmp)?;
        let normal_width_faces = eligible
            .into_iter()
            .filter(|face| (face.stretch_distance - closest_stretch).abs() < f32::EPSILON)
            .collect::<Vec<_>>();
        let face = normal_width_faces
            .iter()
            .copied()
            .min_by_key(|face| face.weight.abs_diff(requested_weight))?;
        let mut values = normal_width_faces
            .iter()
            .map(|face| face.weight)
            .collect::<Vec<_>>();
        values.sort_unstable();
        values.dedup();
        let default = values
            .iter()
            .copied()
            .min_by_key(|weight| weight.abs_diff(FONT_WEIGHT_NORMAL))
            .unwrap_or(FONT_WEIGHT_NORMAL);
        let support = (values.len() > 1).then_some(FontWeightSupport::Discrete { values, default });
        Some(FontSelection {
            index: face.index,
            coordinate: None,
            support,
        })
    }

    fn font_data(&self, index: u32, coordinate: Option<f32>) -> Option<egui::FontData> {
        self.face(index)?;
        let mut data = egui::FontData::from_owned(self.bytes.as_ref().to_vec());
        data.index = index;
        if let Some(coordinate) = coordinate {
            data.tweak.coords = egui::epaint::text::VariationCoords::new([(b"wght", coordinate)]);
        }
        Some(data)
    }

    fn weighted_data(
        &self,
        requested_weight: u16,
    ) -> Option<(egui::FontData, Option<FontWeightSupport>)> {
        let selection = self.selection(requested_weight)?;
        let data = self.font_data(selection.index, selection.coordinate)?;
        Some((data, selection.support))
    }
}

#[cfg(test)]
fn font_selection(bytes: &[u8], requested_weight: u16) -> Option<FontSelection> {
    PreparedFontFile::from_bytes(bytes.to_vec()).selection(requested_weight)
}

#[cfg(test)]
fn weighted_font_data(
    bytes: Vec<u8>,
    requested_weight: u16,
) -> Option<(egui::FontData, Option<FontWeightSupport>)> {
    PreparedFontFile::from_bytes(bytes).weighted_data(requested_weight)
}

struct FontFileCache<'a, F> {
    read: &'a mut F,
    files: HashMap<PathBuf, Option<Arc<PreparedFontFile>>>,
}

impl<'a, F> FontFileCache<'a, F>
where
    F: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    fn new(read: &'a mut F) -> Self {
        Self {
            read,
            files: HashMap::new(),
        }
    }

    fn load(&mut self, path: &Path) -> Option<Arc<PreparedFontFile>> {
        if let Some(prepared) = self.files.get(path) {
            return prepared.clone();
        }
        let prepared = (self.read)(path)
            .ok()
            .map(PreparedFontFile::from_bytes)
            .map(Arc::new);
        self.files.insert(path.to_owned(), prepared.clone());
        prepared
    }
}

fn preferred_catalog_faces(family: &CatalogFontFamily) -> Vec<&FontFace> {
    let has_normal = family.faces.iter().any(|face| face.normal_style);
    let eligible = family
        .faces
        .iter()
        .filter(|face| !has_normal || face.normal_style)
        .collect::<Vec<_>>();
    let closest_stretch = eligible
        .iter()
        .map(|face| face.stretch_milli)
        .min_by_key(|stretch| stretch.abs_diff(1_000));
    eligible
        .into_iter()
        .filter(|face| closest_stretch.is_none_or(|stretch| face.stretch_milli == stretch))
        .collect()
}

fn catalog_family_support(faces: &[&FontFace]) -> FontWeightSupport {
    if let Some((min, max, default)) = faces.iter().find_map(|face| face.variable_weight) {
        return FontWeightSupport::Continuous { min, max, default };
    }
    let mut values = faces.iter().map(|face| face.weight).collect::<Vec<_>>();
    values.sort_unstable();
    values.dedup();
    let default = values
        .iter()
        .copied()
        .min_by_key(|weight| weight.abs_diff(FONT_WEIGHT_NORMAL))
        .unwrap_or(FONT_WEIGHT_NORMAL);
    FontWeightSupport::Discrete { values, default }
}

#[derive(Clone)]
struct PreparedCatalogFace {
    file: Option<Arc<PreparedFontFile>>,
    index: u32,
    weight: u16,
    variable_weight: Option<(u16, u16, u16)>,
}

#[derive(Clone)]
enum PreparedFontSource {
    Collection(Arc<PreparedFontFile>),
    ExactFace {
        file: Arc<PreparedFontFile>,
        index: u32,
        support: Option<FontWeightSupport>,
    },
    Catalog {
        faces: Vec<PreparedCatalogFace>,
        support: FontWeightSupport,
    },
}

impl PreparedFontSource {
    fn weighted_data(
        &self,
        requested_weight: u16,
    ) -> Option<(egui::FontData, Option<FontWeightSupport>)> {
        match self {
            Self::Collection(file) => file.weighted_data(requested_weight),
            Self::ExactFace {
                file,
                index,
                support,
            } => {
                let coordinate = match support {
                    Some(FontWeightSupport::Continuous { min, max, .. }) => {
                        Some(f32::from(requested_weight.clamp(*min, *max)))
                    }
                    Some(FontWeightSupport::Discrete { .. }) | None => None,
                };
                Some((file.font_data(*index, coordinate)?, support.clone()))
            }
            Self::Catalog { faces, support } => {
                let face = faces
                    .iter()
                    .find(|face| face.variable_weight.is_some())
                    .or_else(|| {
                        faces
                            .iter()
                            .min_by_key(|face| face.weight.abs_diff(requested_weight))
                    })?;
                let coordinate = face
                    .variable_weight
                    .map(|(min, max, _)| f32::from(requested_weight.clamp(min, max)));
                let data = face.file.as_ref()?.font_data(face.index, coordinate)?;
                Some((data, Some(support.clone())))
            }
        }
    }
}

fn prepare_requested_font<F>(
    request: FontRequest<'_>,
    files: &mut FontFileCache<'_, F>,
) -> Option<PreparedFontSource>
where
    F: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    if let Some(family) = request.family {
        let preferred = preferred_catalog_faces(family);
        let support = catalog_family_support(&preferred);
        let faces = preferred
            .into_iter()
            .map(|face| PreparedCatalogFace {
                file: files.load(&face.path),
                index: face.index,
                weight: face.weight,
                variable_weight: face.variable_weight,
            })
            .collect();
        return Some(PreparedFontSource::Catalog { faces, support });
    }

    let file = files.load(request.fallback_path?)?;
    let face = file.face(request.fallback_face_index)?;
    if request.fallback_face_index == 0 {
        return Some(PreparedFontSource::Collection(file));
    }

    // While the asynchronous catalog is loading, honor the exact persisted
    // collection face instead of deriving weight support from another face.
    let support = face
        .variable_weight
        .map(|(min, max, default)| FontWeightSupport::Continuous { min, max, default });
    Some(PreparedFontSource::ExactFace {
        file,
        index: request.fallback_face_index,
        support,
    })
}

fn variable_editor_font<F>(files: &mut FontFileCache<'_, F>) -> Option<PreparedFontSource>
where
    F: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/SFNSMono.ttf"]
    } else if cfg!(target_os = "windows") {
        &["C:\\Windows\\Fonts\\CascadiaMono.ttf"]
    } else {
        &[
            "/usr/share/fonts/truetype/noto/NotoSansMono-VariableFont_wdth,wght.ttf",
            "/usr/share/fonts/truetype/cascadia-code/CascadiaMono.ttf",
        ]
    };
    candidates.iter().find_map(|path| {
        let file = files.load(Path::new(path))?;
        matches!(
            file.selection(FONT_WEIGHT_NORMAL)?.support,
            Some(FontWeightSupport::Continuous { .. })
        )
        .then_some(PreparedFontSource::Collection(file))
    })
}

fn default_proportional_font<F>(files: &mut FontFileCache<'_, F>) -> Option<PreparedFontSource>
where
    F: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/System/Library/Fonts/SFNS.ttf"]
    } else if cfg!(target_os = "windows") {
        &["C:\\Windows\\Fonts\\SegoeUIVariable.ttf"]
    } else {
        &[]
    };
    candidates.iter().find_map(|path| {
        files
            .load(Path::new(path))
            .map(PreparedFontSource::Collection)
    })
}

/// Register editor weight roles and normal/strong weighted UI families while
/// retaining every bundled glyph fallback.
pub fn configure_editor_fonts(
    context: &egui::Context,
    ui_font: FontRequest<'_>,
    editor_font: FontRequest<'_>,
    ui_font_monospace: bool,
    ui_font_weight: u16,
    code_font_weight: u16,
) -> FontConfiguration {
    configure_editor_fonts_with_reader(
        context,
        ui_font,
        editor_font,
        ui_font_monospace,
        ui_font_weight,
        code_font_weight,
        |path| std::fs::read(path),
    )
}

fn configure_editor_fonts_with_reader<F>(
    context: &egui::Context,
    ui_font: FontRequest<'_>,
    editor_font: FontRequest<'_>,
    ui_font_monospace: bool,
    ui_font_weight: u16,
    code_font_weight: u16,
    mut read_font: F,
) -> FontConfiguration
where
    F: FnMut(&Path) -> io::Result<Vec<u8>>,
{
    let mut definitions = egui::FontDefinitions::default();
    let mut proportional_fallback = definitions
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut monospace_fallback = definitions
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let mut editor_primary = std::collections::BTreeMap::new();
    let mut editor_weight_support = FontWeightSupport::Discrete {
        values: vec![FONT_WEIGHT_NORMAL, FONT_WEIGHT_BOLD],
        default: FONT_WEIGHT_NORMAL,
    };
    let mut code_weight_support = None;
    let mut prepared_files = FontFileCache::new(&mut read_font);

    let custom_editor_requested =
        editor_font.family.is_some() || editor_font.fallback_path.is_some();
    let prepared_editor = if custom_editor_requested {
        prepare_requested_font(editor_font, &mut prepared_files)
    } else {
        variable_editor_font(&mut prepared_files)
    };
    if let Some(prepared_editor) = &prepared_editor {
        for weight in EDITOR_FONT_WEIGHTS {
            let requested_weight = editor_weight_from_base(weight, code_font_weight);
            let Some((data, support)) = prepared_editor.weighted_data(requested_weight) else {
                continue;
            };
            if custom_editor_requested {
                if let Some(support) = support {
                    code_weight_support = Some(support.clone());
                    editor_weight_support = support;
                }
            } else if let Some(FontWeightSupport::Continuous { .. }) = &support {
                code_weight_support.clone_from(&support);
                editor_weight_support = FontWeightSupport::Discrete {
                    values: EDITOR_FONT_WEIGHTS.to_vec(),
                    default: FONT_WEIGHT_NORMAL,
                };
            }
            let name = format!("tiptoptyp-editor-data-{weight}");
            definitions.font_data.insert(name.clone(), Arc::new(data));
            editor_primary.insert(weight, name);
        }
    }
    let custom_editor_loaded = custom_editor_requested && !editor_primary.is_empty();

    // Fall back to Menlo's regular/bold faces on macOS. Other platforms keep
    // egui's bundled monospace font when no variable system font is available.
    #[cfg(target_os = "macos")]
    if editor_primary.is_empty()
        && let Ok(bytes) = std::fs::read("/System/Library/Fonts/Menlo.ttc")
    {
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
        code_weight_support = Some(FontWeightSupport::Discrete {
            values: vec![FONT_WEIGHT_NORMAL, FONT_WEIGHT_BOLD],
            default: FONT_WEIGHT_NORMAL,
        });
        for weight in EDITOR_FONT_WEIGHTS {
            editor_primary.insert(
                weight,
                if weight < 600 {
                    REGULAR_FACE
                } else {
                    BOLD_FACE
                }
                .to_owned(),
            );
        }
    }

    // Keep Latin editor/UI metrics stable while adding a platform-provided
    // CJK fallback. These paths are optional: missing fonts simply leave
    // egui's bundled fallback chain in place, and no font bytes are shipped.
    let cjk_candidates: &[(&str, &str)] = if cfg!(target_os = "macos") {
        &[
            (
                "tiptoptyp-cjk-pingfang",
                "/System/Library/Fonts/PingFang.ttc",
            ),
            (
                "tiptoptyp-cjk-hiragino",
                "/System/Library/Fonts/Hiragino Sans GB.ttc",
            ),
        ]
    } else if cfg!(target_os = "windows") {
        &[(
            "tiptoptyp-cjk-microsoft-yahei",
            "C:\\Windows\\Fonts\\msyh.ttc",
        )]
    } else {
        &[
            (
                "tiptoptyp-cjk-noto",
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            ),
            (
                "tiptoptyp-cjk-noto-truetype",
                "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            ),
        ]
    };
    for (name, path) in cjk_candidates {
        let path = Path::new(path);
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        let data = egui::FontData::from_owned(bytes);
        definitions
            .font_data
            .insert((*name).to_owned(), Arc::new(data));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            definitions
                .families
                .entry(family)
                .or_default()
                .push((*name).to_owned());
        }
        proportional_fallback.push((*name).to_owned());
        monospace_fallback.push((*name).to_owned());
        break;
    }

    for weight in EDITOR_FONT_WEIGHTS {
        let mut family = editor_primary
            .get(&weight)
            .cloned()
            .into_iter()
            .collect::<Vec<_>>();
        family.extend(monospace_fallback.iter().cloned());
        definitions.families.insert(
            FontFamily::Name(Arc::from(editor_weight_family(weight))),
            family,
        );
    }

    let custom_ui_requested = ui_font.family.is_some() || ui_font.fallback_path.is_some();
    let prepared_ui = if ui_font_monospace {
        prepared_editor.clone()
    } else if custom_ui_requested {
        if ui_font == editor_font {
            prepared_editor.clone()
        } else {
            prepare_requested_font(ui_font, &mut prepared_files)
        }
    } else {
        default_proportional_font(&mut prepared_files)
    };
    let selected_ui_font = prepared_ui
        .as_ref()
        .and_then(|font| font.weighted_data(ui_font_weight));
    let selected_strong_ui_font = prepared_ui.as_ref().and_then(|font| {
        font.weighted_data(editor_weight_from_base(FONT_WEIGHT_BOLD, ui_font_weight))
    });
    let custom_ui_loaded = custom_ui_requested && selected_ui_font.is_some();
    let mut ui_weight_support = None;
    let ui_fallback = if ui_font_monospace {
        monospace_fallback.clone()
    } else {
        proportional_fallback.clone()
    };
    let strong_ui_primary = selected_strong_ui_font.map(|(data, _)| {
        definitions
            .font_data
            .insert(WEIGHTED_UI_STRONG_DATA.to_owned(), Arc::new(data));
        WEIGHTED_UI_STRONG_DATA.to_owned()
    });
    let weighted_ui_loaded = if let Some((data, support)) = selected_ui_font {
        definitions
            .font_data
            .insert(WEIGHTED_UI_DATA.to_owned(), Arc::new(data));
        let mut family = vec![WEIGHTED_UI_DATA.to_owned()];
        family.extend(ui_fallback.clone());
        definitions
            .families
            .insert(FontFamily::Name(Arc::from(WEIGHTED_UI_FAMILY)), family);
        ui_weight_support = support;
        true
    } else if ui_font_monospace {
        let family = definitions
            .families
            .get(&FontFamily::Name(Arc::from(editor_weight_family(
                ui_font_weight,
            ))))
            .cloned()
            .unwrap_or_else(|| monospace_fallback.clone());
        definitions
            .families
            .insert(FontFamily::Name(Arc::from(WEIGHTED_UI_FAMILY)), family);
        ui_weight_support = code_weight_support.clone();
        true
    } else {
        false
    };
    let mut strong_family = strong_ui_primary.into_iter().collect::<Vec<_>>();
    if strong_family.is_empty() && ui_font_monospace {
        strong_family.extend(
            definitions
                .families
                .get(&FontFamily::Name(Arc::from(editor_weight_family(
                    FONT_WEIGHT_BOLD,
                ))))
                .cloned()
                .unwrap_or_default(),
        );
    }
    strong_family.extend(ui_fallback);
    definitions.families.insert(
        FontFamily::Name(Arc::from(WEIGHTED_UI_STRONG_FAMILY)),
        strong_family,
    );
    context.set_fonts(definitions);
    FontConfiguration {
        custom_ui_loaded,
        custom_editor_loaded,
        weighted_ui_loaded,
        ui_weight_support,
        code_weight_support,
        editor_weight_support,
    }
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
    pub find_overlay_max_width: f32,
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
    pub view_size: Vec2,
    pub status_log_size: Vec2,
    pub font_selector_size: Vec2,
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
    pub override_row_height: f32,
    pub override_role_width: f32,
    pub override_color_width: f32,
    pub override_weight_width: f32,
    pub ui_font_weight_width: f32,
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
        tooltip_max_width: 620.0,
        tooltip_title_height: 26.0,
        tooltip_min_height: 68.0,
        tooltip_max_height: 256.0,
    },
    text: TextMetrics {
        approximate_character_ratio: 0.56,
    },
    editor: EditorMetrics {
        find_field_width: 180.0,
        find_overlay_max_width: 620.0,
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
        file_size: Vec2::new(280.0, 240.0),
        edit_size: Vec2::new(280.0, 320.0),
        workspace_size: Vec2::new(220.0, 182.0),
        editor_size: Vec2::new(220.0, 240.0),
        view_size: Vec2::new(220.0, 190.0),
        status_log_size: Vec2::new(360.0, 250.0),
        font_selector_size: Vec2::new(360.0, 390.0),
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
        override_row_height: 24.0,
        override_role_width: 126.0,
        override_color_width: 94.0,
        override_weight_width: 86.0,
        ui_font_weight_width: 180.0,
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
        // Popups must be immediately usable and fully opaque. Area fade-in is
        // time-based, which can strand menus in a translucent state when a
        // secondary viewport is not receiving regular repaint ticks.
        style.animation_time = 0.0;
        // Most labels in the application are chrome: toolbar captions,
        // settings descriptions, tree entries, and status text. Making all of
        // them selectable creates accidental text selections during ordinary
        // navigation. Document-like surfaces opt back in locally (the source
        // editor, diagnostic bodies, and rich tooltip text).
        style.interaction.selectable_labels = false;
        style.spacing.item_spacing = METRICS.spacing.global_item;
        style.spacing.button_padding = METRICS.spacing.global_button_padding;
        style.visuals.menu_corner_radius = egui::CornerRadius::same(RADIUS.card);
        style.visuals.popup_shadow = egui::epaint::Shadow::NONE;
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, editor_font());
    });
}

/// Apply the user-selected interface typeface while keeping the editor's
/// syntax roles independent. This is intentionally a small, explicit set of
/// text roles: changing the UI font must not make code samples lose their
/// monospace face.
pub fn configure_ui_font(context: &egui::Context, weighted_ui_loaded: bool) {
    let family = if weighted_ui_loaded {
        FontFamily::Name(Arc::from(WEIGHTED_UI_FAMILY))
    } else {
        FontFamily::Proportional
    };
    context.all_styles_mut(|style| {
        for text_style in [
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Heading,
            egui::TextStyle::Small,
        ] {
            let size = style
                .text_styles
                .get(&text_style)
                .map_or(TYPE.content, |font| font.size);
            style
                .text_styles
                .insert(text_style, FontId::new(size, family.clone()));
        }
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

/// Keep chrome and navigation labels inert while selectable labels remain
/// enabled for document-like content such as diagnostics and tooltip bodies.
pub fn nonselectable_label(text: impl Into<egui::WidgetText>) -> egui::Label {
    egui::Label::new(text).selectable(false)
}

pub fn show_logo(ui: &mut egui::Ui) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            let font = FontId::proportional(TYPE.content);
            ui.add(nonselectable_label(
                RichText::new("t").font(font.clone()).strong(),
            ));
            ui.add(nonselectable_label(
                RichText::new("t").font(font.clone()).strong(),
            ));
            ui.add(nonselectable_label(
                RichText::new("t")
                    .font(font)
                    .strong()
                    .color(palette(ui.visuals().dark_mode).accent),
            ));
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
    fn nonselectable_chrome_labels_override_global_text_selection() {
        use egui_kittest::{Harness, kittest::Queryable as _};

        fn drag_select(selectable: bool) -> bool {
            let mut harness = Harness::builder()
                .with_size(Vec2::new(260.0, 100.0))
                .build_ui(move |ui| {
                    ui.style_mut().interaction.selectable_labels = true;
                    if selectable {
                        ui.add(egui::Label::new("drag across this label").selectable(true));
                    } else {
                        ui.add(nonselectable_label("drag across this label"));
                    }
                });
            harness.run();

            let rect = harness.get_by_label("drag across this label").rect();
            let start = egui::Pos2::new(rect.left() + 1.0, rect.center().y);
            let end = egui::Pos2::new(rect.right() - 1.0, rect.center().y);
            harness.hover_at(start);
            harness.run();
            harness.drag_at(start);
            harness.run();
            harness.hover_at(end);
            harness.run();
            harness.drop_at(end);
            harness.run();

            harness
                .ctx
                .plugin::<egui::text_selection::LabelSelectionState>()
                .lock()
                .has_selection()
        }

        assert!(
            drag_select(true),
            "positive selection control should select"
        );
        assert!(!drag_select(false), "chrome label must remain inert");
    }

    #[test]
    fn configured_styles_make_labels_inert_by_default() {
        let context = egui::Context::default();
        configure_styles(&context);

        assert!(
            !context
                .style_of(egui::Theme::Dark)
                .interaction
                .selectable_labels
        );
        assert!(
            !context
                .style_of(egui::Theme::Light)
                .interaction
                .selectable_labels
        );
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
                FontFamily::Name(Arc::from(editor_weight_family(FONT_WEIGHT_NORMAL)))
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
        assert_eq!(strong_ui_font().size, TYPE.content);
        assert_ne!(strong_ui_font().family, FontFamily::Proportional);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn strong_system_ui_role_uses_a_heavier_variable_font_coordinate() {
        let mut read = |path: &Path| std::fs::read(path);
        let mut files = FontFileCache::new(&mut read);
        let font = default_proportional_font(&mut files)
            .expect("macOS system UI font should be available");
        let (normal, _) = font
            .weighted_data(FONT_WEIGHT_NORMAL)
            .expect("select normal UI font");
        let (strong, _) = font
            .weighted_data(FONT_WEIGHT_BOLD)
            .expect("select strong UI font");
        assert!(
            strong.tweak.coords.as_ref()[0].1 > normal.tweak.coords.as_ref()[0].1,
            "strong UI role should select a heavier OpenType coordinate"
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
        assert_eq!(dark.animation_time, 0.0);
        assert_eq!(light.animation_time, 0.0);
        assert_eq!(dark.visuals.popup_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(dark.visuals.menu_corner_radius, egui::CornerRadius::same(8));
    }

    #[test]
    fn regular_and_strong_editor_fonts_have_distinct_family_roles() {
        assert_ne!(editor_font(), editor_font_with_weight(FONT_WEIGHT_BOLD));
        assert_eq!(editor_font(), editor_font_with_weight(FONT_WEIGHT_NORMAL));
        assert_eq!(nearest_editor_weight(549), 500);
        assert_eq!(nearest_editor_weight(550), 500);
        assert_eq!(nearest_editor_weight(551), 600);
    }

    #[test]
    fn code_font_base_weight_preserves_relative_syntax_emphasis() {
        assert_eq!(editor_weight_from_base(FONT_WEIGHT_NORMAL, 525), 525);
        assert_eq!(editor_weight_from_base(FONT_WEIGHT_BOLD, 525), 825);
        assert_eq!(editor_weight_from_base(100, 100), 1);
        assert_eq!(editor_weight_from_base(900, 900), 1_000);
    }

    #[test]
    fn font_weight_support_clamps_continuous_and_discrete_choices() {
        let continuous = FontWeightSupport::Continuous {
            min: 250,
            max: 750,
            default: 400,
        };
        assert_eq!(continuous.clamp(100), 250);
        assert_eq!(continuous.clamp(900), 750);

        let discrete = FontWeightSupport::Discrete {
            values: vec![300, 500, 700],
            default: 500,
        };
        assert_eq!(discrete.clamp(620), 700);
        assert_eq!(discrete.clamp(580), 500);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_variable_monospace_font_exposes_and_applies_weight_axis() {
        let bytes = std::fs::read("/System/Library/Fonts/SFNSMono.ttf")
            .expect("macOS ships its system monospace font");
        let selection = font_selection(&bytes, 650).expect("select variable font face");
        assert!(matches!(
            selection.support,
            Some(FontWeightSupport::Continuous { min, max, .. })
                if min <= 400 && max >= 700
        ));
        assert_eq!(selection.coordinate, Some(650.0));

        let (data, _) = weighted_font_data(bytes, 650).expect("configure variable font");
        assert_eq!(data.tweak.coords.as_ref(), &[(Tag::new(b"wght"), 650.0)]);
    }

    #[test]
    fn custom_ui_font_is_validated_loaded_and_keeps_editor_font_independent() {
        let definitions = egui::FontDefinitions::default();
        let source_font = definitions
            .font_data
            .values()
            .next()
            .expect("egui ships a fallback font");
        let directory = tempfile::tempdir().expect("create temporary font directory");
        let path = directory.path().join("custom-ui.ttf");
        std::fs::write(&path, source_font.font.as_ref()).expect("write test font");

        let context = egui::Context::default();
        assert!(load_ui_font_bytes(&path).is_ok());
        let configuration = configure_editor_fonts(
            &context,
            FontRequest {
                fallback_path: Some(&path),
                ..Default::default()
            },
            FontRequest::default(),
            false,
            FONT_WEIGHT_NORMAL,
            FONT_WEIGHT_NORMAL,
        );
        assert!(configuration.custom_ui_loaded);
        configure_styles(&context);
        configure_ui_font(&context, configuration.weighted_ui_loaded);

        let body = context.style_of(egui::Theme::Light);
        assert_eq!(
            body.text_styles[&egui::TextStyle::Body].family,
            FontFamily::Name(Arc::from(WEIGHTED_UI_FAMILY))
        );
        assert_eq!(body.text_styles[&egui::TextStyle::Monospace], editor_font());
    }

    #[test]
    fn configure_reuses_one_prepared_font_file_for_every_weight_role() {
        let definitions = egui::FontDefinitions::default();
        let source_font = definitions
            .font_data
            .values()
            .next()
            .expect("egui ships a fallback font");
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shared-ui-and-editor.ttf");
        std::fs::write(&path, source_font.font.as_ref()).unwrap();
        let request = FontRequest {
            fallback_path: Some(&path),
            fallback_face_index: source_font.index,
            ..Default::default()
        };
        let reads = Cell::new(0_u32);
        let context = egui::Context::default();

        let configuration = configure_editor_fonts_with_reader(
            &context,
            request,
            request,
            false,
            FONT_WEIGHT_NORMAL,
            FONT_WEIGHT_NORMAL,
            |candidate| {
                if candidate == path {
                    reads.set(reads.get() + 1);
                }
                std::fs::read(candidate)
            },
        );

        assert!(configuration.custom_ui_loaded);
        assert!(configuration.custom_editor_loaded);
        assert_eq!(
            reads.get(),
            1,
            "one byte/metadata snapshot must serve nine editor and two UI weights"
        );
    }

    #[test]
    fn selected_code_fonts_keep_open_type_ligature_shaping_enabled() {
        let definitions = egui::FontDefinitions::default();
        let proportional_name = definitions.families[&FontFamily::Proportional][0].clone();
        let source_font = &definitions.font_data[&proportional_name];
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ligature-code.ttf");
        std::fs::write(&path, source_font.font.as_ref()).unwrap();
        let family = CatalogFontFamily {
            name: "Ligature fixture".to_owned(),
            origin: crate::font_catalog::FontOrigin::Workspace,
            faces: vec![FontFace {
                path,
                index: source_font.index,
                weight: FONT_WEIGHT_NORMAL,
                normal_style: true,
                stretch_milli: 1_000,
                variable_weight: None,
            }],
        };
        let context = egui::Context::default();
        let configuration = configure_editor_fonts(
            &context,
            FontRequest::default(),
            FontRequest {
                family: Some(&family),
                ..Default::default()
            },
            false,
            FONT_WEIGHT_NORMAL,
            FONT_WEIGHT_NORMAL,
        );
        assert!(configuration.custom_editor_loaded);

        let mut advances = Vec::new();
        context
            .run_ui(egui::RawInput::default(), |ui| {
                let galley = ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap("ffi".to_owned(), editor_font(), Color32::WHITE)
                });
                advances.extend(
                    galley.rows[0]
                        .glyphs
                        .iter()
                        .map(|glyph| glyph.advance_width),
                );
            })
            .drop_without_applying_deltas();
        assert_eq!(advances.len(), 3);
        assert!(
            advances.iter().skip(1).any(|advance| *advance == 0.0),
            "the selected font's ffi ligature should retain continuation glyphs: {advances:?}"
        );
    }

    #[test]
    fn malformed_custom_ui_font_is_rejected_before_egui_loads_it() {
        let directory = tempfile::tempdir().expect("create temporary font directory");
        let path = directory.path().join("not-a-font.ttf");
        std::fs::write(&path, b"not a font").expect("write malformed font");

        assert!(load_ui_font_bytes(&path).is_err());
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
