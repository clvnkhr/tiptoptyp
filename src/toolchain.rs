//! Resolves the compiler and language-server sidecars.
//!
//! Packaged builds prefer the pinned binaries shipped beside tiptoptyp. A user
//! may select an explicit executable instead. Development-only environment and
//! `PATH` discovery remain recovery routes, but are surfaced as fallbacks.

use std::{
    env,
    path::{Path, PathBuf},
};

use crate::settings::{ToolMode, ToolPreference};

pub(crate) const BUNDLED_TYPST_VERSION: &str = "0.15.1";
pub(crate) const BUNDLED_TINYMIST_VERSION: &str = "0.15.2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolKind {
    Typst,
    Tinymist,
}

impl ToolKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Typst => "Typst",
            Self::Tinymist => "Tinymist",
        }
    }

    pub(crate) fn binary_name(self) -> &'static str {
        match self {
            Self::Typst => "typst",
            Self::Tinymist => "tinymist",
        }
    }

    pub(crate) fn bundled_version(self) -> &'static str {
        match self {
            Self::Typst => BUNDLED_TYPST_VERSION,
            Self::Tinymist => BUNDLED_TINYMIST_VERSION,
        }
    }

    fn environment_variable(self) -> &'static str {
        match self {
            Self::Typst => "MYTYPST_TYPST",
            Self::Tinymist => "MYTYPST_TINYMIST",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolOrigin {
    Bundled,
    Custom,
    Environment,
    Path,
    Missing,
}

impl ToolOrigin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Bundled => "Bundled",
            Self::Custom => "Custom path",
            Self::Environment => "Environment override",
            Self::Path => "PATH fallback",
            Self::Missing => "Unavailable",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolResolution {
    pub(crate) kind: ToolKind,
    pub(crate) program: PathBuf,
    pub(crate) origin: ToolOrigin,
    pub(crate) fallback_reason: Option<String>,
}

impl ToolResolution {
    pub(crate) fn is_available(&self) -> bool {
        self.origin != ToolOrigin::Missing
    }

    pub(crate) fn detail(&self) -> String {
        if self.origin == ToolOrigin::Bundled {
            format!(
                "{} {} · {}",
                self.origin.label(),
                self.kind.bundled_version(),
                self.program.display()
            )
        } else {
            format!("{} · {}", self.origin.label(), self.program.display())
        }
    }
}

pub(crate) fn resolve_tool(kind: ToolKind, preference: &ToolPreference) -> ToolResolution {
    let bundled = bundled_candidates(kind);
    resolve_from(
        kind,
        preference,
        &bundled,
        || env::var_os(kind.environment_variable()).map(PathBuf::from),
        || find_on_path(kind.binary_name()),
    )
}

fn resolve_from<Environment, SearchPath>(
    kind: ToolKind,
    preference: &ToolPreference,
    bundled: &[PathBuf],
    environment: Environment,
    search_path: SearchPath,
) -> ToolResolution
where
    Environment: FnOnce() -> Option<PathBuf>,
    SearchPath: FnOnce() -> Option<PathBuf>,
{
    let mut problems = Vec::new();

    if preference.mode == ToolMode::Custom {
        let custom = preference.custom_path.trim();
        if custom.is_empty() {
            problems.push(format!("No custom {} path is selected", kind.label()));
        } else {
            let custom = PathBuf::from(custom);
            if let Some(program) = absolute_executable(&custom) {
                return ToolResolution {
                    kind,
                    program,
                    origin: ToolOrigin::Custom,
                    fallback_reason: None,
                };
            }
            problems.push(format!(
                "Custom {} path is not an executable file: {}",
                kind.label(),
                custom.display()
            ));
        }
    }

    if let Some(program) = bundled
        .iter()
        .find_map(|candidate| absolute_executable(candidate))
    {
        return ToolResolution {
            kind,
            program,
            origin: ToolOrigin::Bundled,
            fallback_reason: (!problems.is_empty()).then(|| {
                format!(
                    "{}; using bundled {} {}",
                    problems.join("; "),
                    kind.label(),
                    kind.bundled_version()
                )
            }),
        };
    }

    if preference.mode == ToolMode::Bundled {
        problems.push(format!(
            "Bundled {} {} is not present",
            kind.label(),
            kind.bundled_version()
        ));
    } else {
        problems.push(format!(
            "Bundled {} {} is also not present",
            kind.label(),
            kind.bundled_version()
        ));
    }

    if let Some(program) = environment() {
        if let Some(program) = absolute_executable(&program) {
            let variable = kind.environment_variable();
            return ToolResolution {
                kind,
                program,
                origin: ToolOrigin::Environment,
                fallback_reason: Some(format!(
                    "{}; using the {variable} development override",
                    problems.join("; ")
                )),
            };
        }
        problems.push(format!(
            "{} does not point to an executable file",
            kind.environment_variable()
        ));
    }

    if let Some(program) = search_path() {
        return ToolResolution {
            kind,
            program,
            origin: ToolOrigin::Path,
            fallback_reason: Some(format!(
                "{}; using the executable found on PATH",
                problems.join("; ")
            )),
        };
    }

    ToolResolution {
        kind,
        program: PathBuf::from(executable_file_name(kind.binary_name())),
        origin: ToolOrigin::Missing,
        fallback_reason: Some(format!(
            "{}; no {} executable was found on PATH",
            problems.join("; "),
            kind.label()
        )),
    }
}

fn bundled_candidates(kind: ToolKind) -> Vec<PathBuf> {
    let binary = executable_file_name(kind.binary_name());
    let sidecar = sidecar_artifact_name(kind.binary_name());
    let mut candidates = Vec::new();

    if let Ok(current_executable) = env::current_exe()
        && let Some(executable_dir) = current_executable.parent()
    {
        // cargo-packager installs external binaries alongside the application
        // executable, with the target suffix removed.
        candidates.push(executable_dir.join(&binary));
        candidates.push(executable_dir.join(&sidecar));

        // Also accept resource-style bundles so a distributor can package the
        // exact same fetched files without cargo-packager.
        candidates.push(
            executable_dir
                .join("../Resources/toolchain")
                .join(target_triple())
                .join(&binary),
        );
        candidates.push(
            executable_dir
                .join("toolchain")
                .join(target_triple())
                .join(&binary),
        );
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    candidates.push(manifest_dir.join("toolchain/bin").join(&sidecar));
    candidates.push(
        manifest_dir
            .join("resources/toolchain")
            .join(target_triple())
            .join(binary),
    );
    candidates
}

fn executable_file_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

fn sidecar_artifact_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}-{}.exe", target_triple())
    } else {
        format!("{name}-{}", target_triple())
    }
}

pub(crate) fn target_triple() -> &'static str {
    #[cfg(all(target_arch = "aarch64", target_os = "macos"))]
    return "aarch64-apple-darwin";
    #[cfg(all(target_arch = "x86_64", target_os = "macos"))]
    return "x86_64-apple-darwin";
    #[cfg(all(target_arch = "aarch64", target_os = "windows", target_env = "msvc"))]
    return "aarch64-pc-windows-msvc";
    #[cfg(all(target_arch = "x86_64", target_os = "windows", target_env = "msvc"))]
    return "x86_64-pc-windows-msvc";
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "gnu"))]
    return "aarch64-unknown-linux-gnu";
    #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
    return "x86_64-unknown-linux-gnu";
    #[cfg(all(target_arch = "aarch64", target_os = "linux", target_env = "musl"))]
    return "aarch64-unknown-linux-musl";
    #[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "musl"))]
    return "x86_64-unknown-linux-musl";
    #[allow(unreachable_code)]
    "unsupported-target"
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    for directory in env::split_paths(&path) {
        #[cfg(windows)]
        {
            let extensions = env::var_os("PATHEXT").unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into());
            for extension in extensions.to_string_lossy().split(';') {
                let candidate = directory.join(format!("{name}{extension}"));
                if let Some(candidate) = absolute_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = directory.join(name);
            if let Some(candidate) = absolute_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn absolute_executable(path: &Path) -> Option<PathBuf> {
    is_executable(path)
        .then(|| path.canonicalize().ok())
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn executable(directory: &Path, name: &str) -> PathBuf {
        let path = directory.join(name);
        std::fs::write(&path, b"tool").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).unwrap();
        }
        path
    }

    #[test]
    fn bundled_is_the_default_without_a_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let bundled = executable(directory.path(), "typst");
        let resolved = resolve_from(
            ToolKind::Typst,
            &ToolPreference::default(),
            std::slice::from_ref(&bundled),
            || None,
            || None,
        );
        assert_eq!(resolved.program, bundled.canonicalize().unwrap());
        assert_eq!(resolved.origin, ToolOrigin::Bundled);
        assert_eq!(resolved.fallback_reason, None);
    }

    #[test]
    fn valid_custom_path_wins_over_the_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let bundled = executable(directory.path(), "typst-bundled");
        let custom = executable(directory.path(), "typst-custom");
        let preference = ToolPreference {
            mode: ToolMode::Custom,
            custom_path: custom.display().to_string(),
        };
        let resolved = resolve_from(ToolKind::Typst, &preference, &[bundled], || None, || None);
        assert_eq!(resolved.program, custom.canonicalize().unwrap());
        assert_eq!(resolved.origin, ToolOrigin::Custom);
        assert_eq!(resolved.fallback_reason, None);
    }

    #[test]
    fn broken_custom_path_falls_back_visibly_to_the_bundle() {
        let directory = tempfile::tempdir().unwrap();
        let bundled = executable(directory.path(), "tinymist-bundled");
        let missing = directory.path().join("missing");
        let preference = ToolPreference {
            mode: ToolMode::Custom,
            custom_path: missing.display().to_string(),
        };
        let resolved = resolve_from(
            ToolKind::Tinymist,
            &preference,
            &[bundled],
            || None,
            || None,
        );
        assert_eq!(resolved.origin, ToolOrigin::Bundled);
        assert!(
            resolved
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("Custom Tinymist path"))
        );
    }

    #[test]
    fn path_recovery_is_never_silent() {
        let directory = tempfile::tempdir().unwrap();
        let path_tool = executable(directory.path(), "typst-path");
        let resolved = resolve_from(
            ToolKind::Typst,
            &ToolPreference::default(),
            &[],
            || None,
            || Some(path_tool.clone()),
        );
        assert_eq!(resolved.program, path_tool);
        assert_eq!(resolved.origin, ToolOrigin::Path);
        assert!(resolved.fallback_reason.is_some());
    }

    #[test]
    fn complete_absence_is_reported() {
        let resolved = resolve_from(
            ToolKind::Tinymist,
            &ToolPreference::default(),
            &[],
            || None,
            || None,
        );
        assert_eq!(resolved.origin, ToolOrigin::Missing);
        assert!(!resolved.is_available());
        assert!(
            resolved
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("no Tinymist executable"))
        );
    }

    #[test]
    fn recovery_discovery_is_lazy_when_the_bundle_is_valid() {
        use std::cell::Cell;

        let directory = tempfile::tempdir().unwrap();
        let bundled = executable(directory.path(), "typst");
        let environment_called = Cell::new(false);
        let path_called = Cell::new(false);
        let resolved = resolve_from(
            ToolKind::Typst,
            &ToolPreference::default(),
            &[bundled],
            || {
                environment_called.set(true);
                None
            },
            || {
                path_called.set(true);
                None
            },
        );

        assert_eq!(resolved.origin, ToolOrigin::Bundled);
        assert!(!environment_called.get());
        assert!(!path_called.get());
    }

    #[test]
    fn bundled_version_labels_match_the_packaging_manifest() {
        let manifest = include_str!("../toolchain/manifest.tsv");
        for (tool, expected) in [
            ("typst", BUNDLED_TYPST_VERSION),
            ("tinymist", BUNDLED_TINYMIST_VERSION),
        ] {
            let versions = manifest
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.starts_with('#'))
                .filter_map(|line| {
                    let fields = line.split_whitespace().collect::<Vec<_>>();
                    (fields.first() == Some(&tool)).then_some(fields[3])
                })
                .collect::<Vec<_>>();
            assert_eq!(versions.len(), 6, "{tool}");
            assert!(
                versions.iter().all(|version| *version == expected),
                "{tool} runtime label {expected} drifted from {versions:?}"
            );
        }
    }
}
