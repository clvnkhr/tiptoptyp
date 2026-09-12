//! Asynchronous-friendly discovery of installed and project-local OpenType fonts.
//!
//! The scanner owns no UI state. It returns compact metadata and releases each
//! font file before moving to the next one, so callers can run it on a worker
//! without retaining hundreds of megabytes of system font data.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
    time::SystemTime,
};

use skrifa::{MetadataProvider, Tag, string::StringId};

type FontRecord = (String, FontOrigin, FontFace);
static SYSTEM_FONT_RECORDS: OnceLock<Vec<FontRecord>> = OnceLock::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum FontOrigin {
    Workspace,
    System,
}

impl FontOrigin {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Workspace",
            Self::System => "System",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontFace {
    pub(crate) path: PathBuf,
    pub(crate) index: u32,
    pub(crate) weight: u16,
    pub(crate) normal_style: bool,
    pub(crate) stretch_milli: u16,
    pub(crate) variable_weight: Option<(u16, u16, u16)>,
}

/// Normalize a variable-font weight axis before exposing it to sliders or
/// Typst. A malformed font can report its bounds in reverse (or as NaN), and
/// `f32::clamp` panics when its minimum exceeds its maximum.
pub(crate) fn normalize_variable_weight_axis(
    min_value: f32,
    max_value: f32,
    default_value: f32,
) -> (u16, u16, u16) {
    let normalize = |value: f32| {
        if value.is_finite() {
            value.round().clamp(1.0, 1_000.0) as u16
        } else {
            400
        }
    };
    let first = normalize(min_value);
    let second = normalize(max_value);
    let (min, max) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    let default = if default_value.is_finite() {
        default_value.round().clamp(f32::from(min), f32::from(max)) as u16
    } else {
        min
    };
    (min, max, default)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FontFamily {
    pub(crate) name: String,
    pub(crate) origin: FontOrigin,
    pub(crate) faces: Vec<FontFace>,
}

impl FontFamily {
    pub(crate) fn primary_face(&self) -> Option<&FontFace> {
        preferred_faces(&self.faces)
            .min_by_key(|face| face.weight.abs_diff(400))
            .or_else(|| self.faces.first())
    }

    pub(crate) fn contains_path(&self, path: &Path) -> bool {
        self.faces.iter().any(|face| face.path == path)
    }
}

fn preferred_faces(faces: &[FontFace]) -> impl Iterator<Item = &FontFace> {
    let has_normal = faces.iter().any(|face| face.normal_style);
    let closest_stretch = faces
        .iter()
        .filter(|face| !has_normal || face.normal_style)
        .map(|face| face.stretch_milli)
        .min_by_key(|stretch| stretch.abs_diff(1_000));
    faces.iter().filter(move |face| {
        (!has_normal || face.normal_style)
            && closest_stretch.is_none_or(|stretch| face.stretch_milli == stretch)
    })
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FontCatalog {
    families: Vec<FontFamily>,
    workspace_directories: Vec<PathBuf>,
    workspace_files: Vec<PathBuf>,
    workspace_file_fingerprints: Vec<FontFileFingerprint>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FontFileFingerprint {
    path: PathBuf,
    length: Option<u64>,
    modified: Option<SystemTime>,
}

impl FontCatalog {
    pub(crate) fn snapshot_fixture() -> Self {
        let workspace_files = vec![PathBuf::from("/workspace/fonts/Project Sans.ttf")];
        let families = [
            ("Project Sans", FontOrigin::Workspace),
            ("Fira Code", FontOrigin::System),
            ("Inter", FontOrigin::System),
            ("Libertinus Serif", FontOrigin::System),
            ("New Computer Modern", FontOrigin::System),
            ("Source Code Pro", FontOrigin::System),
        ]
        .into_iter()
        .map(|(name, origin)| FontFamily {
            name: name.to_owned(),
            origin,
            faces: vec![FontFace {
                path: PathBuf::from(format!("/fonts/{name}.ttf")),
                index: 0,
                weight: 400,
                normal_style: true,
                stretch_milli: 1_000,
                variable_weight: None,
            }],
        })
        .collect();
        Self {
            families,
            workspace_directories: vec![PathBuf::from("/workspace/fonts")],
            workspace_file_fingerprints: font_file_fingerprints(&workspace_files),
            workspace_files,
        }
    }

    pub(crate) fn discover(workspace_root: &Path) -> Self {
        let mut records = Vec::new();
        let workspace_files = workspace_font_roots(workspace_root)
            .map_or_else(Vec::new, |roots| collect_font_files(&roots, true));
        for path in &workspace_files {
            append_font_file(&mut records, path, FontOrigin::Workspace);
        }

        records.extend(
            SYSTEM_FONT_RECORDS
                .get_or_init(|| {
                    let mut records = Vec::new();
                    for path in collect_font_files(&system_font_roots(), false) {
                        append_font_file(&mut records, &path, FontOrigin::System);
                    }
                    records
                })
                .iter()
                .cloned(),
        );

        let mut grouped = BTreeMap::<(FontOrigin, String), FontFamily>::new();
        for (name, origin, face) in records {
            let key = (origin, name.to_lowercase());
            grouped
                .entry(key)
                .or_insert_with(|| FontFamily {
                    name,
                    origin,
                    faces: Vec::new(),
                })
                .faces
                .push(face);
        }
        let mut families = grouped.into_values().collect::<Vec<_>>();
        for family in &mut families {
            family.faces.sort_by_key(|face| {
                (
                    !face.normal_style,
                    face.stretch_milli.abs_diff(1_000),
                    face.weight.abs_diff(400),
                    face.path.clone(),
                    face.index,
                )
            });
        }
        families.sort_by(|left, right| {
            left.origin
                .cmp(&right.origin)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });

        let mut workspace_directories = workspace_files
            .iter()
            .filter_map(|path| path.parent().map(Path::to_path_buf))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        workspace_directories.sort_by_key(|path| path.components().count());
        let mut minimal_directories = Vec::<PathBuf>::new();
        for directory in workspace_directories {
            if !minimal_directories
                .iter()
                .any(|parent| directory.starts_with(parent))
            {
                minimal_directories.push(directory);
            }
        }
        minimal_directories.sort();
        let workspace_file_fingerprints = font_file_fingerprints(&workspace_files);
        Self {
            families,
            workspace_directories: minimal_directories,
            workspace_files,
            workspace_file_fingerprints,
        }
    }

    pub(crate) fn families(&self) -> &[FontFamily] {
        &self.families
    }

    pub(crate) fn workspace_directories(&self) -> &[PathBuf] {
        &self.workspace_directories
    }

    pub(crate) fn workspace_files(&self) -> &[PathBuf] {
        &self.workspace_files
    }

    pub(crate) fn workspace_files_match(&self, files: &[PathBuf]) -> bool {
        self.workspace_file_fingerprints == font_file_fingerprints(files)
    }

    pub(crate) fn selected_family(
        &self,
        path: &Path,
        family_name: Option<&str>,
    ) -> Option<&FontFamily> {
        self.families.iter().find(|family| {
            family.contains_path(path)
                && family_name.is_none_or(|name| family.name.eq_ignore_ascii_case(name))
        })
    }

    /// Families accepted by Typst, with workspace copies winning duplicate
    /// names just as project-local font paths do at compile time.
    pub(crate) fn document_family_names(&self) -> Vec<&str> {
        let mut seen = BTreeSet::new();
        self.families
            .iter()
            .filter_map(|family| {
                seen.insert(family.name.to_lowercase())
                    .then_some(family.name.as_str())
            })
            .collect()
    }
}

fn workspace_font_roots(workspace_root: &Path) -> Option<Vec<PathBuf>> {
    // A Finder launch can inherit the filesystem root as its working
    // directory. Treat that as “no workspace”: recursively walking the whole
    // volume is both expensive and likely to hit protected directories.
    workspace_root
        .parent()
        .is_some()
        .then(|| vec![workspace_root.to_path_buf()])
}

fn font_file_fingerprints(files: &[PathBuf]) -> Vec<FontFileFingerprint> {
    files
        .iter()
        .map(|path| {
            let metadata = fs::metadata(path).ok();
            FontFileFingerprint {
                path: path.clone(),
                length: metadata.as_ref().map(fs::Metadata::len),
                modified: metadata.and_then(|metadata| metadata.modified().ok()),
            }
        })
        .collect()
}

fn append_font_file(records: &mut Vec<FontRecord>, path: &Path, origin: FontOrigin) {
    let Ok(bytes) = fs::read(path) else {
        return;
    };
    let weight_tag = Tag::new(b"wght");
    for (index, font) in skrifa::FontRef::fonts(&bytes).enumerate() {
        let Ok(font) = font else {
            continue;
        };
        let name = [StringId::TYPOGRAPHIC_FAMILY_NAME, StringId::FAMILY_NAME]
            .into_iter()
            .find_map(|id| {
                font.localized_strings(id)
                    .english_or_first()
                    .map(|name| name.to_string())
                    .filter(|name| !name.trim().is_empty())
            })
            .or_else(|| {
                path.file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_owned)
            });
        let Some(name) = name else {
            continue;
        };
        let attributes = font.attributes();
        let variable_weight = font.axes().get_by_tag(weight_tag).map(|axis| {
            normalize_variable_weight_axis(axis.min_value(), axis.max_value(), axis.default_value())
        });
        records.push((
            name,
            origin,
            FontFace {
                path: path.to_path_buf(),
                index: index as u32,
                weight: attributes.weight.value().round().clamp(1.0, 1_000.0) as u16,
                normal_style: attributes.style == skrifa::attribute::Style::Normal,
                stretch_milli: (attributes.stretch.ratio() * 1_000.0)
                    .round()
                    .clamp(1.0, f32::from(u16::MAX)) as u16,
                variable_weight,
            },
        ));
    }
}

fn system_font_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if cfg!(target_os = "macos") {
        roots.extend([
            PathBuf::from("/System/Library/Fonts"),
            PathBuf::from("/Library/Fonts"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Library/Fonts"));
        }
    } else if cfg!(target_os = "windows") {
        if let Some(windows) = std::env::var_os("WINDIR") {
            roots.push(PathBuf::from(windows).join("Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
    } else {
        roots.extend([
            PathBuf::from("/usr/share/fonts"),
            PathBuf::from("/usr/local/share/fonts"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".local/share/fonts"));
            roots.push(home.join(".fonts"));
        }
    }
    roots
}

fn collect_font_files(roots: &[PathBuf], workspace: bool) -> Vec<PathBuf> {
    let mut files = BTreeSet::new();
    let mut visited = HashSet::new();
    let mut pending = roots.iter().cloned().collect::<VecDeque<_>>();
    while let Some(directory) = pending.pop_front() {
        let canonical = directory
            .canonicalize()
            .unwrap_or_else(|_| directory.clone());
        if !visited.insert(canonical) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !workspace || !ignored_workspace_directory(&path) {
                    pending.push_back(path);
                }
            } else if file_type.is_file() && is_font_path(&path) {
                files.insert(path);
            }
        }
    }
    files.into_iter().collect()
}

pub(crate) fn is_font_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
}

pub(crate) fn ignored_workspace_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name,
                ".git" | ".tiptoptyp" | "target" | "node_modules" | ".venv" | "venv"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_scan_finds_supported_extensions_and_skips_build_trees() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir_all(directory.path().join("fonts/nested")).unwrap();
        fs::create_dir_all(directory.path().join("target/cache")).unwrap();
        fs::write(directory.path().join("fonts/local.OTF"), b"font").unwrap();
        fs::write(
            directory.path().join("fonts/nested/collection.ttc"),
            b"font",
        )
        .unwrap();
        fs::write(directory.path().join("fonts/readme.txt"), b"font").unwrap();
        fs::write(directory.path().join("target/cache/ignored.ttf"), b"font").unwrap();

        let files = collect_font_files(&[directory.path().to_path_buf()], true);
        assert_eq!(files.len(), 2);
        assert!(files.iter().any(|path| path.ends_with("local.OTF")));
        assert!(files.iter().any(|path| path.ends_with("collection.ttc")));
    }

    #[test]
    fn filesystem_root_is_never_treated_as_a_font_workspace() {
        let root = Path::new(std::path::MAIN_SEPARATOR_STR);
        assert_eq!(workspace_font_roots(root), None);
    }

    #[test]
    fn workspace_font_fingerprints_detect_added_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("local.ttf");
        fs::write(&path, b"first").unwrap();
        let files = vec![path.clone()];
        let catalog = FontCatalog {
            workspace_files: files.clone(),
            workspace_file_fingerprints: font_file_fingerprints(&files),
            ..FontCatalog::default()
        };
        assert!(catalog.workspace_files_match(&files));

        let added = directory.path().join("second.otf");
        fs::write(&added, b"second").unwrap();
        assert!(!catalog.workspace_files_match(&[path, added]));
    }

    #[test]
    fn workspace_font_fingerprints_detect_in_place_replacements() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("local.ttf");
        fs::write(&path, b"first").unwrap();
        let files = vec![path.clone()];
        let catalog = FontCatalog {
            workspace_files: files.clone(),
            workspace_file_fingerprints: font_file_fingerprints(&files),
            ..FontCatalog::default()
        };
        assert!(catalog.workspace_files_match(&files));

        fs::write(&path, b"a different font payload").unwrap();
        assert!(!catalog.workspace_files_match(&files));
    }

    #[test]
    fn workspace_families_precede_system_families_and_shadow_document_names() {
        let face = |path: &str| FontFace {
            path: PathBuf::from(path),
            index: 0,
            weight: 400,
            normal_style: true,
            stretch_milli: 1_000,
            variable_weight: None,
        };
        let catalog = FontCatalog {
            families: vec![
                FontFamily {
                    name: "Example".to_owned(),
                    origin: FontOrigin::Workspace,
                    faces: vec![face("local.ttf")],
                },
                FontFamily {
                    name: "Example".to_owned(),
                    origin: FontOrigin::System,
                    faces: vec![face("system.ttf")],
                },
                FontFamily {
                    name: "Second".to_owned(),
                    origin: FontOrigin::System,
                    faces: vec![face("second.ttf")],
                },
            ],
            workspace_directories: Vec::new(),
            workspace_files: Vec::new(),
            workspace_file_fingerprints: Vec::new(),
        };
        assert_eq!(catalog.document_family_names(), vec!["Example", "Second"]);
        assert_eq!(
            catalog
                .selected_family(Path::new("local.ttf"), Some("example"))
                .unwrap()
                .origin,
            FontOrigin::Workspace
        );
    }

    #[test]
    fn open_type_metadata_produces_a_named_selectable_face() {
        let definitions = eframe::egui::FontDefinitions::default();
        let bytes = definitions
            .font_data
            .values()
            .next()
            .expect("egui has a built-in font")
            .font
            .as_ref();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("fixture.ttf");
        fs::write(&path, bytes).unwrap();

        let mut records = Vec::new();
        append_font_file(&mut records, &path, FontOrigin::Workspace);
        assert!(!records.is_empty());
        assert!(records.iter().all(|(name, origin, face)| {
            !name.is_empty()
                && *origin == FontOrigin::Workspace
                && face.path == path
                && face.weight > 0
        }));
    }

    #[test]
    fn reversed_or_non_finite_variable_weight_bounds_are_safe() {
        assert_eq!(normalize_variable_weight_axis(9.0, 8.0, 8.5), (8, 9, 9));
        assert_eq!(
            normalize_variable_weight_axis(f32::NAN, f32::INFINITY, f32::NAN),
            (400, 400, 400)
        );
    }
}
