//! Bounded, side-effect-free TeX header directives and root resolution.
//! TeXShop/TeXstudio/TeXworks spellings are document syntax, never shell input.
use super::settings::{BuildDriver, BuildEngine};
use std::{
    borrow::Cow,
    collections::HashSet,
    path::{Component, Path, PathBuf},
};

pub(crate) const MAX_HEADER_LINES: usize = 256;
pub(crate) const MAX_HEADER_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ROOT_CHAIN: usize = 16;
pub(crate) const MAX_ROOT_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
pub(crate) const MAX_ROOT_TOTAL_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Bibliography {
    BibTex,
    Biber,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spellcheck {
    Disabled,
    Language(String),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unsupported {
    pub(crate) line: usize,
    pub(crate) key: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Directives {
    pub(crate) root: Option<String>,
    pub(crate) engine: Option<BuildEngine>,
    pub(crate) driver: Option<BuildDriver>,
    pub(crate) bibliography: Option<Bibliography>,
    pub(crate) encoding: Option<Encoding>,
    pub(crate) spellcheck: Option<Spellcheck>,
    pub(crate) unsupported: Vec<Unsupported>,
    root_line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Error {
    pub(crate) line: usize,
    pub(crate) message: String,
    pub(crate) path: Option<PathBuf>,
}
impl Error {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line,
            message: message.into(),
            path: None,
        }
    }
    fn in_file(mut self, path: &Path) -> Self {
        self.path = Some(path.to_owned());
        self
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(path) = &self.path {
            write!(f, "{}:{}: {}", path.display(), self.line, self.message)
        } else {
            write!(f, "Line {}: {}", self.line, self.message)
        }
    }
}

fn set<T: PartialEq>(slot: &mut Option<T>, value: T, line: usize, key: &str) -> Result<(), Error> {
    if slot.as_ref().is_some_and(|old| old != &value) {
        return Err(Error::at(
            line,
            format!("Conflicting {key} magic comments. Keep one value in the header."),
        ));
    }
    *slot = Some(value);
    Ok(())
}

fn unquote(value: &str, line: usize) -> Result<&str, Error> {
    let value = value.trim();
    if let Some(first @ ('\'' | '"')) = value.chars().next() {
        if value.len() < 2 || !value.ends_with(first) {
            return Err(Error::at(line, "The magic comment has an unmatched quote."));
        }
        Ok(&value[1..value.len() - 1])
    } else {
        Ok(value)
    }
}

fn program(value: &str, line: usize) -> Result<(Option<BuildEngine>, Option<BuildDriver>), Error> {
    let engine = match value.to_ascii_lowercase().as_str() {
        "tectonic" => Some(BuildEngine::Tectonic),
        "pdflatex" | "pdflatexmk" => Some(BuildEngine::PdfLatex),
        "xelatex" | "xelatexmk" => Some(BuildEngine::XeLatex),
        "lualatex" | "lualatexmk" => Some(BuildEngine::LuaLatex),
        "latexmk" => None,
        _ => {
            return Err(Error::at(
                line,
                "This TeX program is not supported. Choose tectonic, pdflatex, xelatex, lualatex or a supported latexmk variant; configure executable paths in Settings.",
            ));
        }
    };
    let driver = value
        .to_ascii_lowercase()
        .ends_with("mk")
        .then_some(BuildDriver::Latexmk);
    Ok((engine, driver))
}

fn language(value: &str, line: usize) -> Result<Spellcheck, Error> {
    if value.eq_ignore_ascii_case("none") {
        return Ok(Spellcheck::Disabled);
    }
    if value.len() > 64 {
        return Err(Error::at(line, "The spellcheck language is too long."));
    }
    let value = value.replace('_', "-");
    let mut parts = value.split('-');
    let first = parts.next().unwrap_or_default();
    if !(2..=8).contains(&first.len()) || !first.bytes().all(|b| b.is_ascii_alphabetic()) {
        return Err(Error::at(
            line,
            "Use a spellcheck language such as en-US or de-DE, or none.",
        ));
    }
    let mut normalized = first.to_ascii_lowercase();
    for part in parts {
        if part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return Err(Error::at(
                line,
                "Use a spellcheck language such as en-US or de-DE, or none.",
            ));
        }
        normalized.push('-');
        if part.len() == 2 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            normalized.push_str(&part.to_ascii_uppercase());
        } else if part.len() == 4 && part.bytes().all(|b| b.is_ascii_alphabetic()) {
            normalized.push_str(&part[..1].to_ascii_uppercase());
            normalized.push_str(&part[1..].to_ascii_lowercase());
        } else {
            normalized.push_str(&part.to_ascii_lowercase());
        }
    }
    Ok(Spellcheck::Language(normalized))
}

/// Only leading blank/full-comment lines are metadata. An embedded example in
/// a TeX body or verbatim environment cannot change build settings.
pub(crate) fn parse(source: &str) -> Result<Directives, Error> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let mut end = source.len().min(MAX_HEADER_BYTES);
    while !source.is_char_boundary(end) {
        end -= 1;
    }
    if end < source.len() && !source[..end].ends_with('\n') {
        end = source[..end].rfind('\n').map_or(0, |index| index + 1);
    }
    let mut result = Directives::default();
    let (mut standard_program, mut ts_program) = (None, None);
    let (mut standard_bib, mut ts_bib) = (None, None);
    for (index, line) in source[..end].lines().take(MAX_HEADER_LINES).enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some(comment) = line.strip_prefix('%') else {
            break;
        };
        let Some(command) = comment.trim_start().strip_prefix('!') else {
            continue;
        };
        let command = command.trim_start();
        let Some((namespace, rest)) = command.split_once(char::is_whitespace) else {
            continue;
        };
        let tex = namespace.eq_ignore_ascii_case("tex");
        let bib = namespace.eq_ignore_ascii_case("bib");
        let lw = namespace.eq_ignore_ascii_case("lw");
        if !tex && !bib && !lw {
            continue;
        }
        let Some((key, value)) = rest.trim_start().split_once('=') else {
            let key = rest.trim();
            if matches!(
                key.to_ascii_lowercase().as_str(),
                "root"
                    | "program"
                    | "ts-program"
                    | "encoding"
                    | "spellcheck"
                    | "options"
                    | "ts-parameter"
            ) {
                return Err(Error::at(
                    number,
                    "Use an equals sign and a value in this magic comment.",
                ));
            }
            continue;
        };
        let key: String = key
            .chars()
            .filter(|c| !c.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect();
        let known = (tex
            && (matches!(
                key.as_str(),
                "root"
                    | "program"
                    | "ts-program"
                    | "encoding"
                    | "spellcheck"
                    | "options"
                    | "ts-parameter"
                    | "txs-script"
            ) || key.starts_with("txs-program:")))
            || (bib
                && matches!(
                    key.as_str(),
                    "program" | "ts-program" | "options" | "ts-parameter"
                ))
            || (lw && key == "recipe");
        if !known {
            continue;
        }
        let value = unquote(value, number)?;
        if value.chars().any(char::is_control) {
            return Err(Error::at(
                number,
                "Magic comment values cannot contain control characters.",
            ));
        }
        match (tex, bib, lw, key.as_str()) {
            (true, _, _, "root") => {
                if value.is_empty()
                    || value.len() > 4096
                    || value.contains("://")
                    || value.starts_with("~/")
                {
                    return Err(Error::at(
                        number,
                        "Use a relative or absolute project LaTeX source file path for the magic root.",
                    ));
                }
                set(&mut result.root, value.to_owned(), number, "root")?;
                result.root_line = number;
            }
            (true, _, _, "program" | "ts-program") => {
                let program = program(value, number)?;
                set(
                    if key == "ts-program" {
                        &mut ts_program
                    } else {
                        &mut standard_program
                    },
                    program,
                    number,
                    "program",
                )?;
            }
            (_, true, _, "program" | "ts-program") => {
                let bibliography = if value.eq_ignore_ascii_case("bibtex") {
                    Bibliography::BibTex
                } else if value.eq_ignore_ascii_case("biber") {
                    Bibliography::Biber
                } else {
                    return Err(Error::at(
                        number,
                        "Use bibtex or biber for the bibliography magic program; configure executable paths in Settings.",
                    ));
                };
                set(
                    if key == "ts-program" {
                        &mut ts_bib
                    } else {
                        &mut standard_bib
                    },
                    bibliography,
                    number,
                    "bibliography program",
                )?;
            }
            (true, _, _, "encoding") => {
                if !value.eq_ignore_ascii_case("utf-8") && !value.eq_ignore_ascii_case("utf8") {
                    return Err(Error::at(
                        number,
                        "This document declares a non-UTF-8 encoding. Import or save it as UTF-8 before building.",
                    ));
                }
                set(&mut result.encoding, Encoding::Utf8, number, "encoding")?;
            }
            (true, _, _, "spellcheck") => {
                set(
                    &mut result.spellcheck,
                    language(value, number)?,
                    number,
                    "spellcheck",
                )?;
            }
            (_, _, _, "options" | "ts-parameter")
            | (true, _, _, "txs-script")
            | (_, _, true, "recipe") => {
                result.unsupported.push(Unsupported { line: number, key: key.clone(), message: format!("The {namespace} {key} magic command is not executed. Choose build options in Settings instead.") });
            }
            (true, _, _, key) if key.starts_with("txs-program:") => {
                result.unsupported.push(Unsupported { line: number, key: key.into(), message: "TeXstudio command overrides are not executed. Choose the compiler and bibliography tools in Settings.".into() });
            }
            _ => {}
        }
    }
    if let Some((engine, driver)) = ts_program.or(standard_program) {
        result.engine = engine;
        result.driver = driver;
    }
    result.bibliography = ts_bib.or(standard_bib);
    Ok(result)
}

#[derive(Clone, Debug)]
pub(crate) struct Resolved<'a> {
    pub(crate) root: PathBuf,
    pub(crate) source: Cow<'a, str>,
    pub(crate) directives: Directives,
    pub(crate) chain: Vec<PathBuf>,
}

fn normalize(path: &Path) -> Result<PathBuf, Error> {
    if !path.is_absolute() {
        return Err(Error::at(
            1,
            "The magic root resolver requires absolute project paths.",
        ));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str())
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized.file_name().is_none() {
                    return Err(Error::at(
                        1,
                        "The magic root path leaves the filesystem root.",
                    ));
                }
                normalized.pop();
            }
        }
    }
    Ok(normalized)
}

fn check_path(path: &Path, allowed_root: &Path, line: usize) -> Result<(), Error> {
    let relative = path.strip_prefix(allowed_root).map_err(|_| {
        Error::at(
            line,
            "The magic root is outside this project. Open its project workspace first.",
        )
    })?;
    if !crate::document::is_tex_path(path)
        || relative.components().any(|part| {
            part.as_os_str()
                .as_encoded_bytes()
                .eq_ignore_ascii_case(b".tiptoptyp")
                || part.as_os_str() == ".git"
        })
    {
        return Err(Error::at(
            line,
            "The magic root must be a project LaTeX source file, outside private editor and Git data.",
        ));
    }
    Ok(())
}

pub(crate) fn resolve<'a>(
    start: &Path,
    source: &'a str,
    allowed_root: &Path,
    mut load: impl FnMut(&Path) -> Result<String, String>,
) -> Result<Resolved<'a>, Error> {
    let allowed_root = normalize(allowed_root)?;
    let mut current = normalize(start)?;
    check_path(&current, &allowed_root, 1)?;
    let mut text = Cow::Borrowed(source);
    let mut seen = HashSet::new();
    let mut chain = Vec::new();
    loop {
        if !seen.insert(current.clone()) {
            return Err(Error::at(1, "The TeX magic root comments form a cycle.").in_file(&current));
        }
        chain.push(current.clone());
        let directives = parse(&text).map_err(|error| error.in_file(&current))?;
        if let Some(unsupported) = directives.unsupported.first() {
            return Err(Error::at(unsupported.line, &unsupported.message).in_file(&current));
        }
        let Some(target) = &directives.root else {
            return Ok(Resolved {
                root: current,
                source: text,
                directives,
                chain,
            });
        };
        let next = normalize(&current.parent().unwrap().join(target))
            .map_err(|error| error.in_file(&current))?;
        check_path(&next, &allowed_root, directives.root_line)
            .map_err(|error| error.in_file(&current))?;
        if next == current {
            return Ok(Resolved {
                root: current,
                source: text,
                directives,
                chain,
            });
        }
        if chain.len() >= MAX_ROOT_CHAIN {
            return Err(Error::at(directives.root_line, "The TeX magic root chain exceeds 16 files. Check for a cycle or point directly to the main document.").in_file(&current));
        }
        let loaded = load(&next).map_err(|message| {
            Error::at(
                directives.root_line,
                format!("Could not load magic root {}: {message}", next.display()),
            )
            .in_file(&current)
        })?;
        current = next;
        text = Cow::Owned(loaded);
    }
}

#[cfg(test)]
mod tests;
