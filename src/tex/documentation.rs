//! Explicit, local Texdoc searches. Discovery and filesystem work stay on the worker.
use std::{
    collections::HashSet,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

const MAX_QUERY: usize = 256;
const MAX_OUTPUT: u64 = 1024 * 1024;
const MAX_ERROR: u64 = 64 * 1024;
const MAX_MANUAL: u64 = 128 * 1024 * 1024;
const MAX_RESULTS: usize = 100;
const MAX_CANDIDATES: usize = 400;
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Manual {
    pub(crate) path: PathBuf,
    pub(crate) description: String,
    pub(crate) language: String,
}
impl Manual {
    pub(crate) fn name(&self) -> String {
        self.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    }
}

#[derive(Debug, Default)]
pub(crate) struct SearchResult {
    pub(crate) manuals: Vec<Manual>,
    pub(crate) unavailable: bool,
}

pub(crate) fn validate_query(query: &str) -> Result<&str, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("Enter a package, class, or topic.".into());
    }
    if query.len() > MAX_QUERY || query.chars().any(char::is_control) {
        return Err("Use a search of at most 256 bytes without control characters.".into());
    }
    Ok(query)
}

pub(crate) fn online_url(query: &str) -> Result<String, String> {
    let query = validate_query(query)?;
    let mut url = url::Url::parse("https://texdoc.org/serve/").expect("fixed Texdoc URL");
    url.path_segments_mut()
        .expect("fixed hierarchical URL")
        .pop_if_empty()
        .push(query)
        .push("0");
    Ok(url.into())
}

pub(crate) fn search(query: String, cancel: &AtomicBool) -> Result<SearchResult, String> {
    let query = validate_query(&query)?;
    check_cancel(cancel)?;
    let Some(program) = resolve_program() else {
        return Ok(SearchResult {
            unavailable: true,
            ..Default::default()
        });
    };
    search_with_program(query, &program, cancel, TIMEOUT)
}

fn resolve_program() -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    program_in_paths(
        paths
            .into_iter()
            .chain([PathBuf::from("/Library/TeX/texbin")]),
    )
}

fn program_in_paths(paths: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    paths
        .into_iter()
        // An empty/relative PATH entry must not execute a project-local program.
        .filter(|path| path.is_absolute())
        .map(|path| path.join(format!("texdoc{}", std::env::consts::EXE_SUFFIX)))
        .find(|path| executable(path))
}

fn executable(path: &Path) -> bool {
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
    true
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Documentation search cancelled.".into())
    } else {
        Ok(())
    }
}

fn search_with_program(
    query: &str,
    program: &Path,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<SearchResult, String> {
    check_cancel(cancel)?;
    // Files avoid pipe-reader threads that a wrapper's descendants could keep alive.
    let mut output =
        tempfile::tempfile().map_err(|e| format!("Cannot search documentation: {e}"))?;
    let error = tempfile::tempfile().map_err(|e| format!("Cannot search documentation: {e}"))?;
    let mut command = Command::new(program);
    // MacTeX's texdoc is an env-texlua script. Finder-launched apps may find
    // texdoc through MacTeX's stable link while PATH still lacks its interpreter.
    let inherited = std::env::var_os("PATH");
    let mut directories = Vec::new();
    if let Some(parent) = program.parent().filter(|path| path.is_absolute()) {
        directories.push(parent.to_owned());
    }
    directories.extend(
        inherited
            .as_ref()
            .into_iter()
            .flat_map(std::env::split_paths)
            .filter(|path| path.is_absolute()),
    );
    let path = std::env::join_paths(directories)
        .map_err(|_| "Cannot prepare Texdoc’s executable search path.")?;
    command
        .env("PATH", path)
        .args(["--list", "--machine", "--", query])
        .stdin(Stdio::null())
        .stdout(output.try_clone().map_err(|e| e.to_string())?)
        .stderr(error.try_clone().map_err(|e| e.to_string())?);
    let mut child = crate::process::OwnedChild::spawn(&mut command)
        .map_err(|e| format!("Cannot start Texdoc: {e}"))?;
    let started = Instant::now();
    let status = loop {
        check_cancel(cancel)?;
        if output.metadata().map_err(|e| e.to_string())?.len() > MAX_OUTPUT
            || error.metadata().map_err(|e| e.to_string())?.len() > MAX_ERROR
        {
            return Err("Texdoc returned too much output.".into());
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        if started.elapsed() >= timeout {
            return Err("Texdoc search timed out. Try searching online.".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    if !status.success() {
        return Err("Texdoc could not complete this search. Try searching online.".into());
    }
    let listed = read_bounded(&mut output, MAX_OUTPUT)?;
    check_cancel(cancel)?;
    let candidates = parse_machine_output(&listed);
    let mut seen = HashSet::new();
    let mut manuals = Vec::new();
    for mut manual in candidates {
        check_cancel(cancel)?;
        let Ok(path) = manual.path.canonicalize() else {
            continue;
        };
        if !supported_manual(&path)
            || !path
                .metadata()
                .is_ok_and(|m| m.is_file() && m.len() <= MAX_MANUAL)
            || !seen.insert(path.clone())
        {
            continue;
        }
        manual.path = path;
        manuals.push(manual);
        if manuals.len() == MAX_RESULTS {
            break;
        }
    }
    Ok(SearchResult {
        manuals,
        unavailable: false,
    })
}

fn read_bounded(file: &mut File, limit: u64) -> Result<String, String> {
    file.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Texdoc returned too much output.".into());
    }
    String::from_utf8(bytes).map_err(|_| "Texdoc returned invalid UTF-8.".into())
}

fn supported_manual(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        ["pdf", "txt", "text", "md"]
            .iter()
            .any(|kind| extension.eq_ignore_ascii_case(kind))
    }) || (path.extension().is_none()
        && path.file_name().is_some_and(|name| {
            ["readme", "license", "copying", "changelog"]
                .iter()
                .any(|kind| name.eq_ignore_ascii_case(kind))
        }))
}

fn parse_machine_output(output: &str) -> Vec<Manual> {
    output
        .lines()
        .filter_map(|line| {
            // Lua print separates the five fields with tabs. Never split a path on spaces.
            let mut fields = line.trim_end_matches('\r').splitn(5, '\t');
            let _query = fields.next()?;
            let score = fields.next()?.parse::<f64>().ok()?;
            let path = PathBuf::from(fields.next()?);
            let language = fields.next().unwrap_or_default();
            let description = fields.next().unwrap_or_default();
            (score.is_finite()
                && path.is_absolute()
                && !path.as_os_str().as_encoded_bytes().contains(&0))
            .then(|| Manual {
                path,
                language: language
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(32)
                    .collect(),
                description: description
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(512)
                    .collect(),
            })
        })
        .take(MAX_CANDIDATES)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_results_preserve_spaces_order_and_optional_fields() {
        let results = parse_machine_output(
            "pkg\t9.5\t/TeX docs/User guide.pdf\ten\tUser guide\r\npkg\t2\t/a.txt\t\t\npkg\tNaN\t/b.pdf\ten\tbad\npkg\t2\trelative.pdf\ten\tbad\nwarning\n",
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].path, Path::new("/TeX docs/User guide.pdf"));
        assert_eq!(results[0].description, "User guide");
        assert_eq!(results[1].language, "");
        assert_eq!(results[1].description, "");
        assert!(supported_manual(Path::new("/a.PDF")));
        assert!(supported_manual(Path::new("/README")));
        assert!(!supported_manual(Path::new("/a.sh")));
    }

    #[test]
    fn queries_are_bounded_and_online_query_is_a_single_encoded_path_segment() {
        assert!(validate_query(" \n ").is_err());
        assert!(validate_query("a\0b").is_err());
        assert!(validate_query(&"x".repeat(257)).is_err());
        assert_eq!(validate_query(" amsmath ").unwrap(), "amsmath");
        let url = url::Url::parse(&online_url("a/b?x=1 #foo").unwrap()).unwrap();
        assert_eq!(url.host_str(), Some("texdoc.org"));
        assert_eq!(url.path(), "/serve/a%2Fb%3Fx=1%20%23foo/0");
        assert!(url.query().is_none());
        assert!(url.fragment().is_none());
    }

    #[test]
    #[cfg(unix)]
    fn discovery_skips_relative_entries_and_non_executable_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("texdoc");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(program_in_paths([PathBuf::from("."), dir.path().into()]).is_none());
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            program_in_paths([PathBuf::new(), dir.path().into()]),
            Some(program)
        );
    }

    #[cfg(unix)]
    fn script(body: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("texdoc");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        (dir, path)
    }

    #[test]
    #[cfg(unix)]
    fn search_only_lists_and_filters_missing_unsupported_and_duplicate_results() {
        let dir = tempfile::tempdir().unwrap();
        let manual = dir.path().join("User guide.pdf");
        std::fs::write(&manual, b"%PDF-1.7").unwrap();
        let (_scripts, program) = script(&format!(
            "test \"$1\" = --list && test \"$2\" = --machine && test \"$3\" = -- && test \"$4\" = --view || exit 2\nprintf 'pkg\\t10\\t%s\\ten\\tManual\\npkg\\t9\\t%s\\ten\\tDuplicate\\npkg\\t8\\t/missing.pdf\\t\\tMissing\\n' '{}' '{}'",
            manual.display(),
            manual.display()
        ));
        let result =
            search_with_program("--view", &program, &AtomicBool::new(false), TIMEOUT).unwrap();
        assert_eq!(result.manuals.len(), 1);
        assert_eq!(result.manuals[0].path, manual.canonicalize().unwrap());
        assert!(!result.unavailable);
    }

    #[test]
    #[cfg(unix)]
    fn cancellation_timeout_and_output_limit_terminate_the_worker() {
        let (_dir, program) = script("exec sleep 30");
        assert!(
            search_with_program("pkg", &program, &AtomicBool::new(true), TIMEOUT)
                .unwrap_err()
                .contains("cancelled")
        );
        assert!(
            search_with_program("pkg", &program, &AtomicBool::new(false), Duration::ZERO)
                .unwrap_err()
                .contains("timed out")
        );
        let (_dir, program) = script("head -c 1100000 /dev/zero");
        assert!(
            search_with_program("pkg", &program, &AtomicBool::new(false), TIMEOUT)
                .unwrap_err()
                .contains("too much output")
        );
    }
    #[test]
    fn candidate_validation_work_is_bounded_even_with_many_unusable_paths() {
        let output = "pkg\t1\t/missing.pdf\t\tMissing\n".repeat(MAX_CANDIDATES + 100);
        assert_eq!(parse_machine_output(&output).len(), MAX_CANDIDATES);
    }

    #[test]
    #[cfg(unix)]
    fn cancelling_after_spawn_reaps_a_running_search() {
        let ready = tempfile::tempdir().unwrap();
        let receipt = ready.path().join("started");
        let (_scripts, program) = script(&format!(
            "printf ready > '{}'\nexec sleep 30",
            receipt.display()
        ));
        let cancel = std::sync::Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            search_with_program("pkg", &program, &worker_cancel, TIMEOUT)
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !receipt.is_file() {
            assert!(Instant::now() < deadline, "fake Texdoc did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel.store(true, Ordering::Release);
        assert!(worker.join().unwrap().unwrap_err().contains("cancelled"));
    }
    #[test]
    #[cfg(unix)]
    fn a_discovered_texdoc_can_find_its_sibling_script_interpreter() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("texdoc");
        let helper = directory.path().join("tiptoptyp-test-doc-interpreter");
        let manual = directory.path().join("manual.pdf");
        std::fs::write(&program, "#!/usr/bin/env tiptoptyp-test-doc-interpreter\n").unwrap();
        std::fs::write(
            &helper,
            format!(
                "#!/bin/sh\nprintf 'pkg\\t9\\t%s\\ten\\tManual\\n' '{}'\n",
                manual.display()
            ),
        )
        .unwrap();
        std::fs::write(&manual, "%PDF-1.7").unwrap();
        for path in [&program, &helper] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let result =
            search_with_program("pkg", &program, &AtomicBool::new(false), TIMEOUT).unwrap();
        assert_eq!(result.manuals.len(), 1);
        assert_eq!(result.manuals[0].path, manual.canonicalize().unwrap());
    }
}
