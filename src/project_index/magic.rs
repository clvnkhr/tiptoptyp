//! Worker-side reuse of the compiler's explicit TeX root-comment policy.
use super::*;
use crate::tex::magic::{self as directives, MAX_ROOT_SOURCE_BYTES, MAX_ROOT_TOTAL_BYTES};
use std::io::Read;

pub(super) struct Entry<'a> {
    pub(super) path: PathBuf,
    pub(super) source: Cow<'a, str>,
    pub(super) redirected: bool,
    pub(super) unresolved: Option<UnresolvedDependency>,
}

pub(super) fn resolve<'a>(
    root: &Path,
    main: &Path,
    overrides: &'a HashMap<PathBuf, &'a str>,
    paths: &mut PathNormalizer,
    cancelled: &mut impl FnMut() -> bool,
) -> Option<io::Result<Entry<'a>>> {
    if cancelled() {
        return None;
    }
    let initial = match source_for(main, overrides) {
        Ok(source) => source,
        Err(error) => return Some(Err(error)),
    };
    let mut entry = Entry {
        path: main.into(),
        source: initial,
        redirected: false,
        unresolved: None,
    };
    let parsed = match directives::parse(&entry.source) {
        Ok(parsed) => parsed,
        Err(error) => {
            entry.unresolved = Some(failure(main, error));
            return Some(Ok(entry));
        }
    };
    if parsed.root.is_none() {
        return Some(Ok(entry));
    }

    let mut seen = HashSet::from([main.to_owned()]);
    let mut bytes = 0usize;
    let mut aborted = false;
    let resolved = directives::resolve(main, &entry.source, root, |path| {
        if cancelled() {
            aborted = true;
            return Err("Project indexing superseded".into());
        }
        let path = public_source(root, path, paths)?;
        if !seen.insert(path.clone()) {
            return Err("The TeX magic root comments form a cycle through linked files.".into());
        }
        let source = root_source(&path, overrides)?;
        bytes = bytes
            .checked_add(source.len())
            .ok_or("The TeX root chain is too large")?;
        if bytes > MAX_ROOT_TOTAL_BYTES {
            return Err("The TeX root chain exceeds 64 MiB.".into());
        }
        Ok(source)
    });
    if aborted {
        return None;
    }
    match resolved {
        Ok(resolved) => match public_source(root, &resolved.root, paths) {
            Ok(path) if path != main => {
                let source = resolved.source.into_owned();
                entry.path = path;
                entry.source = Cow::Owned(source);
                entry.redirected = true;
            }
            Ok(_) => {}
            Err(error) => {
                entry.unresolved = Some(UnresolvedDependency {
                    path: main.into(),
                    line: 1,
                    kind: DependencyKind::Include,
                    expression: error,
                })
            }
        },
        Err(error) => entry.unresolved = Some(failure(main, error)),
    }
    Some(Ok(entry))
}

fn failure(main: &Path, error: directives::Error) -> UnresolvedDependency {
    UnresolvedDependency {
        path: error.path.as_deref().unwrap_or(main).into(),
        line: error.line,
        kind: DependencyKind::Include,
        expression: error.to_string(),
    }
}

fn public_source(root: &Path, path: &Path, paths: &mut PathNormalizer) -> Result<PathBuf, String> {
    let path = paths.normalize(path);
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "The TeX magic root is outside this project")?;
    if !crate::document::is_tex_path(&path)
        || relative.components().any(|component| {
            let name = component.as_os_str().as_encoded_bytes();
            name.eq_ignore_ascii_case(b".tiptoptyp") || name.eq_ignore_ascii_case(b".git")
        })
    {
        return Err("Private editor or Git data cannot be TeX project roots".into());
    }
    Ok(path)
}

fn root_source(path: &Path, overrides: &HashMap<PathBuf, &str>) -> Result<String, String> {
    if let Some(source) = overrides.get(path) {
        if source.len() as u64 > MAX_ROOT_SOURCE_BYTES {
            return Err("Magic root files must be smaller than 16 MiB".into());
        }
        return Ok((*source).to_owned());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|error| error.to_string())?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > MAX_ROOT_SOURCE_BYTES {
        return Err("Magic roots must be regular UTF-8 sources smaller than 16 MiB".into());
    }
    let mut text = String::new();
    file.take(MAX_ROOT_SOURCE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    if text.len() as u64 > MAX_ROOT_SOURCE_BYTES {
        return Err("The TeX magic root grew beyond 16 MiB while it was read".into());
    }
    Ok(text)
}
