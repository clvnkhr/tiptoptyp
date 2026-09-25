//! Versioned source maps travel with their PDF, never with a mutable filename.
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
#[derive(Debug, Clone)]
pub(crate) struct Artifact {
    pub data: Arc<[u8]>,
    pub compressed: bool,
    pub root: PathBuf,
    pub mirror: PathBuf,
}
#[derive(Debug, Clone)]
pub(crate) enum Query {
    Source {
        path: PathBuf,
        line: usize,
        column: usize,
    },
    Page {
        page: usize,
        x: f32,
        y: f32,
    },
}
#[derive(Debug, PartialEq)]
pub(crate) enum Destination {
    Source {
        path: PathBuf,
        line: usize,
        column: usize,
    },
    Page {
        page: usize,
        x: f32,
        y: f32,
    },
}
pub(crate) fn query(artifact: &Artifact, pdf: &[u8], query: Query) -> Result<Destination, String> {
    let executable = crate::tex::tools::distribution_tool("synctex")
        .ok_or("SyncTeX was not found. Install a TeX distribution with the synctex command.")?;
    let directory = tempfile::tempdir().map_err(|e| e.to_string())?;
    let output = directory.path().join("preview.pdf");
    std::fs::write(&output, pdf).map_err(|e| e.to_string())?;
    std::fs::write(
        output.with_extension(if artifact.compressed {
            "synctex.gz"
        } else {
            "synctex"
        }),
        &artifact.data,
    )
    .map_err(|e| e.to_string())?;
    let mut command = Command::new(executable);
    match &query {
        Query::Source { path, line, column } => {
            let path = path.canonicalize().unwrap_or_else(|_| {
                path.parent()
                    .and_then(|p| p.canonicalize().ok())
                    .and_then(|parent| path.file_name().map(|name| parent.join(name)))
                    .unwrap_or_else(|| path.clone())
            });
            let mirrored = path
                .strip_prefix(&artifact.root)
                .map(|relative| artifact.mirror.join(relative))
                .unwrap_or_else(|_| path.clone());
            command
                .args(["view", "-i"])
                .arg(format!("{line}:{column}:{}", mirrored.display()))
                .arg("-o")
                .arg(&output);
        }
        Query::Page { page, x, y } => {
            command
                .args(["edit", "-o"])
                .arg(format!("{}:{x}:{y}:{}", page + 1, output.display()));
        }
    }
    let log = directory.path().join("result.txt");
    let file = std::fs::File::create(&log).map_err(|e| e.to_string())?;
    command
        .stdin(Stdio::null())
        .stdout(file.try_clone().map_err(|e| e.to_string())?)
        .stderr(file);
    let child = command.spawn().map_err(|e| e.to_string())?;
    crate::process::wait(child, Duration::from_secs(10), || {
        if std::fs::metadata(&log)?.len() > 1024 * 1024 {
            Err(std::io::Error::other("SyncTeX output limit exceeded"))
        } else {
            Ok(())
        }
    })
    .map_err(|e| e.to_string())?;
    let mut text = String::new();
    std::fs::File::open(log)
        .and_then(|file| file.take(1024 * 1024).read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    parse(&text, artifact, matches!(query, Query::Source { .. }))
        .ok_or_else(|| "No SyncTeX location was found for this position".into())
}
fn parse(text: &str, artifact: &Artifact, forward: bool) -> Option<Destination> {
    let output = text
        .split_once("SyncTeX result begin")?
        .1
        .split("SyncTeX result end")
        .next()?;
    let get = |key: &str| output.lines().find_map(|line| line.strip_prefix(key));
    if forward {
        let page = get("Page:")?.parse::<usize>().ok()?.checked_sub(1)?;
        let x = get("x:")?.parse::<f32>().ok()?;
        let y = get("y:")?.parse::<f32>().ok()?;
        (x.is_finite() && y.is_finite()).then_some(Destination::Page { page, x, y })
    } else {
        let path = PathBuf::from(get("Input:")?);
        let path = path
            .strip_prefix(&artifact.mirror)
            .map(|relative| artifact.root.join(relative))
            .unwrap_or(path);
        Some(Destination::Source {
            path,
            line: get("Line:")?.parse::<usize>().ok()?.max(1),
            column: get("Column:")
                .and_then(|v| v.parse::<isize>().ok())
                .unwrap_or(1)
                .max(1) as usize,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_first_result_and_maps_private_sources() {
        let artifact = Artifact {
            data: Arc::from([]),
            compressed: true,
            root: "/project".into(),
            mirror: "/private/source".into(),
        };
        assert_eq!(
            parse(
                "SyncTeX result begin\nInput:/private/source/chapter.tex\nLine:12\nColumn:-1\nSyncTeX result end",
                &artifact,
                false
            ),
            Some(Destination::Source {
                path: "/project/chapter.tex".into(),
                line: 12,
                column: 1
            })
        );
        assert_eq!(
            parse(
                "SyncTeX result begin\nPage:3\nx:42\ny:50\nPage:4\nx:0\ny:0\nSyncTeX result end",
                &artifact,
                true
            ),
            Some(Destination::Page {
                page: 2,
                x: 42.0,
                y: 50.0
            })
        );
    }
}
