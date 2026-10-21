use super::settings::{BuildDriver, BuildEngine, SystemTool, TexSettings};
use crate::toolchain::{ToolKind, ToolResolution, resolve_tool};
use std::{
    path::{Path, PathBuf},
    sync::{
        RwLock, RwLockReadGuard,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TexTools {
    pub(crate) tectonic: ToolResolution,
    pub(crate) texlab: ToolResolution,
    pub(crate) badness: ToolResolution,
    pub(crate) tex_fmt: ToolResolution,
    distributions: [Option<std::path::PathBuf>; 3],
    pub(crate) synctex: Option<std::path::PathBuf>,
    pub(crate) latexmk: Option<PathBuf>,
    pub(crate) bibtex: Option<PathBuf>,
    pub(crate) biber: Option<PathBuf>,
}
impl TexTools {
    pub(crate) fn distribution(&self, engine: BuildEngine) -> Option<&std::path::Path> {
        let index = match engine {
            BuildEngine::Tectonic => return None,
            BuildEngine::PdfLatex => 0,
            BuildEngine::XeLatex => 1,
            BuildEngine::LuaLatex => 2,
        };
        self.distributions[index].as_deref()
    }
    pub(crate) fn build_options(&self, settings: &TexSettings) -> Option<Box<SystemBuildOptions>> {
        (settings.build_engine != BuildEngine::Tectonic).then(|| {
            Box::new(SystemBuildOptions {
                driver: settings.build_driver,
                bibliography_enabled: settings.bibliography_enabled,
                latexmk: self.latexmk.clone().map(|program| SystemProgram {
                    program,
                    arguments: settings.latexmk.arguments.clone(),
                }),
                bibtex: SystemProgram {
                    program: self.bibtex.clone().unwrap_or_else(|| "bibtex".into()),
                    arguments: settings.bibtex.arguments.clone(),
                },
                biber: SystemProgram {
                    program: self.biber.clone().unwrap_or_else(|| "biber".into()),
                    arguments: settings.biber.arguments.clone(),
                },
            })
        })
    }
    pub(crate) fn resolve(settings: &TexSettings) -> Self {
        Self {
            distributions: ["pdflatex", "xelatex", "lualatex"].map(distribution_tool),
            synctex: distribution_tool("synctex"),
            latexmk: resolve_system_tool("latexmk", &settings.latexmk),
            bibtex: resolve_system_tool("bibtex", &settings.bibtex),
            biber: resolve_system_tool("biber", &settings.biber),
            tectonic: resolve_tool(ToolKind::Tectonic, &settings.tectonic),
            texlab: resolve_tool(ToolKind::Texlab, &settings.texlab),
            badness: resolve_tool(ToolKind::Badness, &settings.badness),
            tex_fmt: resolve_tool(ToolKind::TexFmt, &settings.tex_fmt),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemBuildOptions {
    pub(crate) driver: BuildDriver,
    pub(crate) bibliography_enabled: bool,
    pub(crate) latexmk: Option<SystemProgram>,
    pub(crate) bibtex: SystemProgram,
    pub(crate) biber: SystemProgram,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemProgram {
    pub(crate) program: PathBuf,
    pub(crate) arguments: String,
}

fn resolve_system_tool(name: &str, settings: &SystemTool) -> Option<PathBuf> {
    if settings.executable.trim().is_empty() {
        distribution_tool(name)
    } else {
        Some(absolute_executable(settings.executable.trim().into()))
    }
}

// A cache reset must not remove a PAR runtime while latexmk/Biber is using it,
// including builds in another editor window. Readers never wait on deletion.
static BIBER_CACHE: RwLock<()> = RwLock::new(());
pub(crate) fn acquire_build_cache() -> std::io::Result<RwLockReadGuard<'static, ()>> {
    BIBER_CACHE.try_read().map_err(|_| {
        std::io::Error::other("Biber cache is being cleared; retry the build when it finishes")
    })
}

/// Worker-only maintenance of the exact cache reported by the detected binary.
pub(crate) fn clear_biber_cache(
    executable: &Path,
    project_root: &Path,
    environment: &str,
    cancel: &AtomicBool,
) -> Result<PathBuf, String> {
    clear_biber_cache_with_gate(
        executable,
        project_root,
        environment,
        cancel,
        &std::env::temp_dir(),
        &BIBER_CACHE,
    )
}

fn clear_biber_cache_with_gate(
    executable: &Path,
    project_root: &Path,
    environment: &str,
    cancel: &AtomicBool,
    temporary_root: &Path,
    gate: &RwLock<()>,
) -> Result<PathBuf, String> {
    use std::{
        fs,
        io::Read,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let _exclusive = gate.try_write().map_err(
        |_| "A TeX build is running; wait for it to finish before clearing the Biber cache",
    )?;
    let canceled = || cancel.load(Ordering::Acquire);
    if canceled() {
        return Err("Biber cache reset canceled".into());
    }
    let private = crate::private_workspace::PrivateWorkspace::open(project_root)
        .map_err(|e| e.to_string())?;
    let logs = private
        .temp_dir("biber-cache-")
        .map_err(|e| e.to_string())?;
    let stdout = logs.path().join("stdout");
    let stderr = logs.path().join("stderr");
    let mut command = Command::new(executable);
    command.arg("--cache").env("LC_ALL", "C");
    // Match the build's environment, including PAR cache overrides, while
    // retaining only the maintenance command's own arguments/directory.
    crate::tool_command::CommandCustomization {
        environment: environment.to_owned(),
        ..Default::default()
    }
    .apply(&mut command)
    .map_err(|e| format!("Invalid Biber cache environment: {e}"))?;
    command
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).map_err(|e| e.to_string())?)
        .stderr(fs::File::create(&stderr).map_err(|e| e.to_string())?);
    let mut child = crate::process::OwnedChild::spawn(&mut command)
        .map_err(|e| format!("Could not query Biber cache: {e}"))?;
    let started = Instant::now();
    let status = loop {
        if canceled() {
            return Err("Biber cache reset canceled".into());
        }
        if started.elapsed() >= Duration::from_secs(30) {
            return Err("Biber cache query timed out".into());
        }
        for path in [&stdout, &stderr] {
            if fs::metadata(path).map_err(|e| e.to_string())?.len() > 16 * 1024 {
                return Err("Biber cache query exceeded the output limit".into());
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // Drop the complete query process group before inspecting or removing files.
    drop(child);
    if !status.success() {
        return Err(format!("Biber cache query exited with {status}"));
    }
    let mut result = String::new();
    fs::File::open(&stdout)
        .and_then(|file| file.take(16 * 1024 + 1).read_to_string(&mut result))
        .map_err(|e| e.to_string())?;
    if result.len() > 16 * 1024
        || fs::metadata(&stderr).map_err(|e| e.to_string())?.len() > 16 * 1024
    {
        return Err("Biber cache query exceeded the output limit".into());
    }
    let mut lines = result
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let reported = lines
        .next()
        .ok_or("This Biber installation did not report a removable cache")?;
    if lines.next().is_some() {
        return Err("Biber reported more than one cache path".into());
    }
    let cache = validated_biber_cache(Path::new(reported), temporary_root)?;
    if canceled() {
        return Err("Biber cache reset canceled".into());
    }
    // Move the checked leaf aside atomically. New Biber launches can recreate
    // their original leaf; removal cannot traverse that newly created runtime.
    let quarantine = tempfile::Builder::new()
        .prefix("tiptoptyp-biber-reset-")
        .tempdir_in(cache.parent().unwrap())
        .map_err(|e| e.to_string())?;
    let retired = quarantine.path().join("cache");
    fs::rename(&cache, &retired).map_err(|e| format!("Could not retire Biber cache: {e}"))?;
    fs::remove_dir_all(&retired)
        .map_err(|e| format!("Could not remove retired Biber cache: {e}"))?;
    Ok(cache)
}

fn validated_biber_cache(reported: &Path, temporary_root: &Path) -> Result<PathBuf, String> {
    let unsafe_path =
        || "Biber did not report a recognized private cache; no files were removed".to_owned();
    if !reported.is_absolute() {
        return Err(unsafe_path());
    }
    let metadata = std::fs::symlink_metadata(reported).map_err(|_| unsafe_path())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unsafe_path());
    }
    let cache = reported.canonicalize().map_err(|_| unsafe_path())?;
    let root = temporary_root.canonicalize().map_err(|_| unsafe_path())?;
    let parent = cache.parent().ok_or_else(unsafe_path)?;
    let leaf = cache
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("cache-"))
        .ok_or_else(unsafe_path)?;
    let owner = parent
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.strip_prefix("par-"))
        .ok_or_else(unsafe_path)?;
    if parent.parent() != Some(root.as_path())
        || leaf.len() != 40
        || !leaf.bytes().all(|c| c.is_ascii_hexdigit())
        || owner.is_empty()
        || !owner.bytes().all(|c| c.is_ascii_hexdigit())
    {
        return Err(unsafe_path());
    }
    let parent_metadata = std::fs::symlink_metadata(reported.parent().ok_or_else(unsafe_path)?)
        .map_err(|_| unsafe_path())?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(unsafe_path());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let uid = nix::unistd::getuid().as_raw();
        if metadata.uid() != uid
            || parent_metadata.uid() != uid
            || metadata.mode() & 0o022 != 0
            || parent_metadata.mode() & 0o022 != 0
        {
            return Err(unsafe_path());
        }
    }
    for marker in ["biber", "inc/lib/Biber.pm"] {
        let metadata = std::fs::symlink_metadata(cache.join(marker)).map_err(|_| unsafe_path())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(unsafe_path());
        }
    }
    Ok(cache)
}

/// GUI launches may not inherit the login shell PATH. MacTeX's stable link
/// avoids hardcoding a distribution year. Resolution does not execute programs.
pub(crate) fn distribution_tool(name: &str) -> Option<std::path::PathBuf> {
    let paths = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    paths
        .into_iter()
        .chain([std::path::PathBuf::from("/Library/TeX/texbin")])
        .map(|dir| dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| path.is_file())
        .map(absolute_executable)
}

// TeX selects its format from argv[0]: resolving pdflatex's symlink to
// pdftex would silently select plain TeX. Only make the path absolute.
fn absolute_executable(path: std::path::PathBuf) -> std::path::PathBuf {
    std::path::absolute(&path).unwrap_or(path)
}

#[cfg(test)]
mod tests {
    #[test]
    #[cfg(unix)]
    fn executable_resolution_preserves_the_tex_format_symlink_name() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("pdftex"), "engine").unwrap();
        let alias = directory.path().join("pdflatex");
        std::os::unix::fs::symlink("pdftex", &alias).unwrap();
        let resolved = super::absolute_executable(alias);
        assert!(resolved.is_absolute());
        assert_eq!(resolved.file_name().unwrap(), "pdflatex");
        assert_eq!(std::fs::read_to_string(resolved).unwrap(), "engine");
    }
    #[cfg(unix)]
    fn private_cache(root: &std::path::Path) -> std::path::PathBuf {
        let path = root
            .join("par-6167656e74")
            .join(format!("cache-{}", "a".repeat(40)));
        std::fs::create_dir_all(path.join("inc/lib")).unwrap();
        std::fs::write(path.join("inc/lib/Biber.pm"), "package Biber;").unwrap();
        std::fs::write(path.join("biber"), "runtime").unwrap();
        path
    }

    #[cfg(unix)]
    fn fake_biber(root: &std::path::Path, cache: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake-biber");
        std::fs::write(&path, format!("#!/bin/sh\n[ \"$#\" -eq 1 ] && [ \"$1\" = --cache ] || exit 9\nprintf 'locale warning\\n' >&2\nprintf '%s\\n' {}\n", shlex::try_quote(cache.to_str().unwrap()).unwrap())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn cache_clear_removes_only_reported_biber_leaf_and_respects_builds() {
        use super::*;
        let root = tempfile::tempdir().unwrap();
        let cache = private_cache(root.path());
        let sibling = cache
            .parent()
            .unwrap()
            .join(format!("cache-{}", "b".repeat(40)));
        std::fs::create_dir(&sibling).unwrap();
        std::fs::write(sibling.join("keep"), "other runtime").unwrap();
        let binary = fake_biber(root.path(), &cache);
        let gate = RwLock::new(());
        let cancel = AtomicBool::new(false);
        let build = gate.read().unwrap();
        let error =
            clear_biber_cache_with_gate(&binary, root.path(), "{}", &cancel, root.path(), &gate)
                .unwrap_err();
        assert!(error.contains("build is running"));
        assert!(cache.exists());
        drop(build);
        let removed =
            clear_biber_cache_with_gate(&binary, root.path(), "{}", &cancel, root.path(), &gate)
                .unwrap();
        assert_eq!(
            removed,
            cache.canonicalize().unwrap_or_else(|_| root
                .path()
                .canonicalize()
                .unwrap()
                .join(cache.strip_prefix(root.path()).unwrap()))
        );
        assert!(!cache.exists());
        assert_eq!(
            std::fs::read_to_string(sibling.join("keep")).unwrap(),
            "other runtime"
        );
        assert!(
            std::fs::read_dir(root.path().join(".tiptoptyp"))
                .unwrap()
                .next()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn cache_validation_rejects_tex_installation_roots_aliases_and_missing_markers() {
        use super::*;
        let root = tempfile::tempdir().unwrap();
        let cache = private_cache(root.path());
        assert!(validated_biber_cache(&cache, root.path()).is_ok());
        assert!(validated_biber_cache(root.path(), root.path()).is_err());
        assert!(validated_biber_cache(cache.parent().unwrap(), root.path()).is_err());
        assert!(validated_biber_cache(Path::new("relative/cache"), root.path()).is_err());
        let alias = cache
            .parent()
            .unwrap()
            .join(format!("cache-{}", "c".repeat(40)));
        std::os::unix::fs::symlink(&cache, &alias).unwrap();
        assert!(validated_biber_cache(&alias, root.path()).is_err());
        std::fs::remove_file(cache.join("inc/lib/Biber.pm")).unwrap();
        assert!(validated_biber_cache(&cache, root.path()).is_err());
        let binary = fake_biber(root.path(), root.path());
        assert!(
            clear_biber_cache_with_gate(
                &binary,
                root.path(),
                "{}",
                &AtomicBool::new(false),
                root.path(),
                &RwLock::new(())
            )
            .is_err()
        );
        assert!(root.path().exists());
        assert!(cache.join("biber").exists());
    }
    #[cfg(unix)]
    #[test]
    fn cache_query_uses_build_environment_without_accepting_extra_arguments() {
        use super::*;
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let cache = private_cache(root.path());
        let binary = root.path().join("environment-biber");
        std::fs::write(&binary, "#!/bin/sh\n[ \"$#\" -eq 1 ] && [ \"$1\" = --cache ] || exit 9\nprintf '%s\\n' \"$BIBER_CACHE_FIXTURE\"\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let environment =
            serde_json::json!({"BIBER_CACHE_FIXTURE": cache.to_str().unwrap()}).to_string();
        clear_biber_cache_with_gate(
            &binary,
            root.path(),
            &environment,
            &AtomicBool::new(false),
            root.path(),
            &RwLock::new(()),
        )
        .unwrap();
        assert!(!cache.exists());
        let outside = tempfile::tempdir().unwrap();
        let other = private_cache(outside.path());
        let environment =
            serde_json::json!({"BIBER_CACHE_FIXTURE": other.to_str().unwrap()}).to_string();
        assert!(
            clear_biber_cache_with_gate(
                &binary,
                root.path(),
                &environment,
                &AtomicBool::new(false),
                root.path(),
                &RwLock::new(())
            )
            .is_err()
        );
        assert!(other.join("inc/lib/Biber.pm").exists());
    }
}
