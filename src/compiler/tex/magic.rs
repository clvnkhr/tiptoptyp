//! Worker-only root loading and immutable per-build magic settings.
use super::TexOptions;
use crate::{
    compiler::{CompileRequest, EngineConfig},
    tex::{
        magic::{self as directives, Bibliography},
        settings::{BuildDriver, BuildEngine, TexSettings},
        tools::TexTools,
    },
};
use std::{
    borrow::Cow,
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use directives::{MAX_ROOT_SOURCE_BYTES, MAX_ROOT_TOTAL_BYTES};
const MAX_OVERLAYS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MagicContext {
    pub(crate) settings: TexSettings,
    pub(crate) tools: TexTools,
    pub(crate) overlays: Vec<(PathBuf, String)>,
}

pub(super) struct Prepared<'a> {
    pub(super) request: Cow<'a, CompileRequest>,
    pub(super) options: Cow<'a, TexOptions>,
    pub(super) entry: PathBuf,
    pub(super) redirected: bool,
    pub(super) overlays: Vec<(PathBuf, &'a str)>,
}

fn source_path(path: &Path, project: &Path) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or("The TeX source has no parent folder")?
        .canonicalize()
        .map_err(|e| format!("The TeX source folder is unavailable: {e}"))?;
    if !parent.starts_with(project) {
        return Err("The TeX source is outside this project.".into());
    }
    let path = parent.join(path.file_name().ok_or("The TeX source has no filename")?);
    public_source(&path, project)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let real = path.canonicalize().map_err(|e| e.to_string())?;
            if !real.starts_with(project) || !real.is_file() {
                return Err(
                    "The TeX root link leaves this project or is not a regular file.".into(),
                );
            }
            public_source(&real, project)?;
            Ok(real)
        }
        Ok(metadata) if metadata.is_file() => Ok(path),
        Ok(_) => Err("The TeX root is not a regular source file.".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => Err(error.to_string()),
    }
}

fn public_source(path: &Path, project: &Path) -> Result<(), String> {
    let relative = path
        .strip_prefix(project)
        .map_err(|_| "The TeX source is outside this project.")?;
    if relative.components().any(|part| {
        let name = part.as_os_str().as_encoded_bytes();
        name.eq_ignore_ascii_case(b".tiptoptyp") || name.eq_ignore_ascii_case(b".git")
    }) {
        return Err("Private editor and Git data cannot be TeX build sources.".into());
    }
    Ok(())
}

fn read_source(path: &Path) -> Result<String, String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_ROOT_SOURCE_BYTES {
        return Err("Magic root files must be regular UTF-8 sources smaller than 16 MiB.".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_ROOT_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_ROOT_SOURCE_BYTES {
        return Err("The TeX root grew beyond 16 MiB while it was read.".into());
    }
    String::from_utf8(bytes)
        .map_err(|_| "The TeX root is not UTF-8. Import or save it as UTF-8 first.".into())
}

pub(super) fn prepare<'a>(
    request: &'a CompileRequest,
    options: &'a TexOptions,
) -> Result<Prepared<'a>, String> {
    let project = request
        .input
        .project_root
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let initial = source_path(
        &request.input.source_dir.join(&request.input.display_name),
        &project,
    )?;
    let Some(context) = &options.magic else {
        return Ok(Prepared {
            request: Cow::Borrowed(request),
            options: Cow::Borrowed(options),
            entry: initial,
            redirected: false,
            overlays: Vec::new(),
        });
    };
    let mut bytes = 0_usize;
    let mut overlays: HashMap<PathBuf, &'a str> = HashMap::new();
    for (path, source) in &context.overlays {
        if !path.starts_with(&request.input.project_root) && !path.starts_with(&project) {
            continue;
        }
        let path = source_path(path, &project)?;
        if path == initial {
            continue;
        } // The submitted active snapshot wins.
        if overlays.len() >= MAX_OVERLAYS && !overlays.contains_key(&path) {
            return Err(
                "Too many unsaved TeX sources for one build. Save or close some files first."
                    .into(),
            );
        }
        bytes = bytes
            .checked_add(source.len())
            .ok_or("Unsaved TeX sources are too large")?;
        if bytes > MAX_ROOT_TOTAL_BYTES {
            return Err(
                "Unsaved TeX sources exceed 64 MiB. Save some files before building.".into(),
            );
        }
        if overlays
            .insert(path, source)
            .is_some_and(|previous| previous != source)
        {
            return Err("Two open TeX buffers disagree about the same file. Keep one version before building.".into());
        }
    }
    let mut loaded = 0_usize;
    let resolved = directives::resolve(&initial, &request.input.source, &project, |path| {
        let path = source_path(path, &project)?;
        let source = overlays
            .get(&path)
            .map_or_else(|| read_source(&path), |source| Ok((*source).to_owned()))?;
        loaded = loaded
            .checked_add(source.len())
            .ok_or("The TeX root chain is too large")?;
        if loaded > MAX_ROOT_TOTAL_BYTES {
            return Err("The TeX root chain exceeds 64 MiB.".into());
        }
        Ok(source)
    })
    .map_err(|e| e.to_string())?;
    let entry = source_path(&resolved.root, &project)?;
    let redirected = entry != initial;
    let mut seen = std::collections::HashSet::new();
    for path in &resolved.chain {
        if !seen.insert(source_path(path, &project)?) {
            return Err("The TeX magic root comments form a cycle through linked files.".into());
        }
    }
    let mut effective = effective_options(options, context, &resolved.directives)?;
    let mut mirrored = Vec::with_capacity(overlays.len() + usize::from(redirected));
    for (path, source) in overlays {
        if path != entry {
            mirrored.push((path, source));
        }
    }
    if redirected {
        mirrored.push((initial.clone(), request.input.source.as_str()));
    }
    mirrored.sort_by(|left, right| left.0.cmp(&right.0));
    let prepared_request = if redirected {
        let mut next = request.clone();
        next.input.source = resolved.source.into_owned();
        next.input.source_dir = entry
            .parent()
            .ok_or("The TeX root has no parent folder")?
            .to_owned();
        next.input.project_root = project;
        next.input.display_name = entry
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("The TeX root filename is not UTF-8")?
            .to_owned();
        next.engine = EngineConfig::Tex(effective.as_ref().clone());
        Cow::Owned(next)
    } else {
        Cow::Borrowed(request)
    };
    if let Cow::Owned(options) = &mut effective {
        options.magic = None;
    }
    Ok(Prepared {
        request: prepared_request,
        options: effective,
        entry,
        redirected,
        overlays: mirrored,
    })
}

fn effective_options<'a>(
    original: &'a TexOptions,
    context: &MagicContext,
    directives: &directives::Directives,
) -> Result<Cow<'a, TexOptions>, String> {
    if directives.engine.is_none()
        && directives.driver.is_none()
        && directives.bibliography.is_none()
    {
        return Ok(Cow::Borrowed(original));
    }
    let mut settings = context.settings.clone();
    settings.build_engine = directives.engine.unwrap_or(original.engine);
    if let Some(driver) = directives.driver {
        settings.build_driver = driver;
    }
    if directives.driver == Some(BuildDriver::Latexmk)
        && settings.build_engine == BuildEngine::Tectonic
    {
        settings.build_engine = BuildEngine::PdfLatex;
    }
    if directives.bibliography == Some(Bibliography::Biber)
        && settings.build_engine == BuildEngine::Tectonic
    {
        return Err("Biber needs a system TeX engine with latexmk. Choose pdflatex, xelatex or lualatex for this document.".into());
    }
    if directives.bibliography.is_some() && settings.build_engine != BuildEngine::Tectonic {
        settings.build_driver = BuildDriver::Latexmk;
        settings.bibliography_enabled = true;
    }
    let mut configured = if settings.build_engine == original.engine {
        let mut options = original.clone();
        if let Some(system) = &mut options.system {
            system.driver = settings.build_driver;
            system.bibliography_enabled = settings.bibliography_enabled;
        }
        options
    } else {
        TexOptions::configured(&settings, &context.tools)
    };
    if let Some(bibliography) = directives.bibliography
        && let Some(system) = &mut configured.system
    {
        let selected = match bibliography {
            Bibliography::BibTex => system.bibtex.clone(),
            Bibliography::Biber => system.biber.clone(),
        };
        system.bibtex = selected.clone();
        system.biber = selected;
    }
    Ok(Cow::Owned(configured))
}

#[cfg(test)]
mod tests;
