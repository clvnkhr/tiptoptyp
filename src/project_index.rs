use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
};

const MAX_PROJECT_FILES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineEntry {
    pub path: PathBuf,
    pub line: usize,
    pub level: usize,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Definition,
    Function,
}

impl SymbolKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Definition => "definition",
            Self::Function => "function",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolEntry {
    pub path: PathBuf,
    pub line: usize,
    pub name: String,
    pub kind: SymbolKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectIndex {
    pub outline: Vec<OutlineEntry>,
    pub subfiles: Vec<PathBuf>,
    pub symbols: Vec<SymbolEntry>,
    pub packages: Vec<String>,
}

/// Build a compact, deterministic index from one Typst entry point.
///
/// This intentionally performs syntax-aware line scanning rather than Typst
/// evaluation: it is fast enough for the UI thread, never downloads packages,
/// and still covers the project-navigation constructs users need at a glance.
/// `overrides` lets the currently edited document participate before it is
/// saved to disk.
pub fn analyze_project(
    root: &Path,
    main: &Path,
    overrides: &BTreeMap<PathBuf, String>,
) -> ProjectIndex {
    let root = canonical_or_owned(root);
    let main = canonical_or_owned(main);
    if !main.starts_with(&root) || main.extension().is_none_or(|extension| extension != "typ") {
        return ProjectIndex::default();
    }

    let mut index = ProjectIndex::default();
    // Reverse so canonical aliases retain BTreeMap's first-match behavior.
    let overrides: HashMap<_, _> = overrides
        .iter()
        .rev()
        .map(|(path, source)| (canonical_or_owned(path), source.as_str()))
        .collect();
    let mut visited = HashSet::new();
    let mut pending = vec![main.clone()];
    let mut packages = BTreeSet::new();

    while visited.len() < MAX_PROJECT_FILES
        && let Some(path) = pending.pop()
    {
        if !visited.insert(path.clone()) {
            continue;
        }
        let Some(source) = source_for(&path, &overrides) else {
            continue;
        };
        if path != main {
            index.subfiles.push(path.clone());
        }
        scan_source(&path, &source, &mut index, &mut packages, |target| {
            let Some(resolved) = resolve_local_typst_path(&root, &path, target) else {
                return;
            };
            if !visited.contains(&resolved) {
                pending.push(resolved);
            }
        });
    }

    index.subfiles.sort();
    index.packages = packages.into_iter().collect();
    index
}

fn source_for<'a>(path: &Path, overrides: &'a HashMap<PathBuf, &'a str>) -> Option<Cow<'a, str>> {
    overrides
        .get(path)
        .map(|source| Cow::Borrowed(*source))
        .or_else(|| fs::read_to_string(path).ok().map(Cow::Owned))
}

fn scan_source(
    path: &Path,
    source: &str,
    index: &mut ProjectIndex,
    packages: &mut BTreeSet<String>,
    mut local_file: impl FnMut(&str),
) {
    for (line_index, raw_line) in source.lines().enumerate() {
        let line_number = line_index + 1;
        let line = strip_line_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }

        if let Some((level, title)) = heading(line) {
            index.outline.push(OutlineEntry {
                path: path.to_path_buf(),
                line: line_number,
                level,
                title: title.to_owned(),
            });
        }
        if let Some((name, kind)) = definition(line) {
            index.symbols.push(SymbolEntry {
                path: path.to_path_buf(),
                line: line_number,
                name: name.to_owned(),
                kind,
            });
        }

        for package in package_specs(line) {
            packages.insert(package.to_owned());
        }
        for keyword in ["#include", "#import"] {
            if let Some(target) = quoted_argument_after(line, keyword) {
                if target.starts_with('@') {
                    packages.insert(target.to_owned());
                } else if target.ends_with(".typ") {
                    local_file(target);
                }
            }
        }
    }
}

fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'=').count();
    if level == 0 {
        return None;
    }
    let title = line.get(level..)?.strip_prefix(char::is_whitespace)?.trim();
    (!title.is_empty()).then_some((level, title))
}

fn definition(line: &str) -> Option<(&str, SymbolKind)> {
    let rest = line.strip_prefix("#let ")?.trim_start();
    let end = rest
        .char_indices()
        .find_map(|(index, character)| (!is_identifier_character(character)).then_some(index))
        .unwrap_or(rest.len());
    let name = &rest[..end];
    if name.is_empty() {
        return None;
    }
    let kind = if rest[end..].trim_start().starts_with('(') {
        SymbolKind::Function
    } else {
        SymbolKind::Definition
    };
    Some((name, kind))
}

fn is_identifier_character(character: char) -> bool {
    character == '_' || character == '-' || character.is_alphanumeric()
}

fn quoted_argument_after<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.split_once(keyword)?.1.trim_start();
    let quote = rest.chars().next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let contents = &rest[quote.len_utf8()..];
    let end = contents.find(quote)?;
    Some(&contents[..end])
}

fn package_specs(line: &str) -> impl Iterator<Item = &str> {
    line.match_indices('@').filter_map(|(start, _)| {
        let tail = &line[start..];
        let end = tail
            .char_indices()
            .skip(1)
            .find_map(|(index, character)| {
                (!matches!(character, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_' | '-' | '/' | ':' | '.'))
                    .then_some(index)
            })
            .unwrap_or(tail.len());
        let package = &tail[..end];
        (package.contains('/') && package.contains(':')).then_some(package)
    })
}

fn strip_line_comment(line: &str) -> &str {
    let mut quote = None;
    let mut escaped = false;
    let bytes = line.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if escaped {
            escaped = false;
        } else if bytes[index] == b'\\' && quote.is_some() {
            escaped = true;
        } else if matches!(bytes[index], b'"' | b'\'') {
            quote = if quote == Some(bytes[index]) {
                None
            } else if quote.is_none() {
                Some(bytes[index])
            } else {
                quote
            };
        } else if bytes[index] == b'/'
            && quote.is_none()
            && bytes.get(index + 1).is_some_and(|next| *next == b'/')
        {
            return &line[..index];
        }
        index += 1;
    }
    line
}

fn resolve_local_typst_path(root: &Path, source: &Path, target: &str) -> Option<PathBuf> {
    let target = Path::new(target);
    let candidate = if target.is_absolute() {
        root.join(target.strip_prefix(Component::RootDir.as_os_str()).ok()?)
    } else {
        source.parent()?.join(target)
    };
    let candidate = canonical_or_owned(&candidate);
    (candidate.starts_with(root)
        && candidate
            .extension()
            .is_some_and(|extension| extension == "typ"))
    .then_some(candidate)
}

fn canonical_or_owned(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_recursive_project_structure_and_unsaved_overrides() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let chapter = project.path().join("chapters/one.typ");
        fs::create_dir_all(chapter.parent().unwrap()).unwrap();
        fs::write(
            &main,
            "= Book\n#import \"chapters/one.typ\"\n#import \"@preview/cetz:0.3.2\"\n#let accent = blue\n",
        )
        .unwrap();
        fs::write(&chapter, "== Old title\n#let old(x) = x\n").unwrap();

        let overrides = BTreeMap::from([(
            chapter.clone(),
            "== New title\n#let render(body) = body\n".to_owned(),
        )]);
        let index = analyze_project(project.path(), &main, &overrides);

        assert_eq!(index.subfiles, vec![chapter.canonicalize().unwrap()]);
        assert_eq!(
            index
                .outline
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Book", "New title"]
        );
        assert_eq!(index.packages, vec!["@preview/cetz:0.3.2"]);
        assert!(
            index
                .symbols
                .iter()
                .any(|symbol| { symbol.name == "render" && symbol.kind == SymbolKind::Function })
        );
        assert!(!index.symbols.iter().any(|symbol| symbol.name == "old"));
    }

    #[test]
    fn ignores_comments_cycles_and_paths_outside_the_project() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let child = project.path().join("child.typ");
        fs::write(
            &main,
            format!(
                "#include \"child.typ\"\n// #include \"missing.typ\"\n#include \"{}\"\n",
                outside.path().join("outside.typ").display()
            ),
        )
        .unwrap();
        fs::write(&child, "#include \"main.typ\"\n").unwrap();
        fs::write(outside.path().join("outside.typ"), "= Secret\n").unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());
        assert_eq!(index.subfiles, vec![child.canonicalize().unwrap()]);
        assert!(index.outline.is_empty());
    }

    #[test]
    fn comment_scanning_keeps_urls_inside_strings() {
        assert_eq!(
            strip_line_comment("#let url = \"https://typst.app\" // comment"),
            "#let url = \"https://typst.app\" "
        );
    }

    #[test]
    fn repeated_aliases_and_cycles_index_each_file_once() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let child = project.path().join("child.typ");
        fs::create_dir(project.path().join("nested")).unwrap();
        fs::write(
            &main,
            "= Root\n#include \"./child.typ\"\n#include \"nested/../child.typ\"\n",
        )
        .unwrap();
        fs::write(&child, "== On disk\n#include \"main.typ\"\n").unwrap();

        let aliased_child = project.path().join("nested/../child.typ");
        let overrides = BTreeMap::from([(
            aliased_child,
            "== Unsaved\n#let render(body) = body\n#include \"main.typ\"\n".to_owned(),
        )]);
        let index = analyze_project(project.path(), &main, &overrides);

        assert_eq!(index.subfiles, vec![child.canonicalize().unwrap()]);
        assert_eq!(
            index
                .outline
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["Root", "Unsaved"]
        );
        assert_eq!(
            index
                .symbols
                .iter()
                .filter(|symbol| symbol.name == "render")
                .count(),
            1
        );
    }

    #[test]
    fn duplicate_references_keep_stack_order_while_public_lists_stay_sorted() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let first = project.path().join("a.typ");
        let second = project.path().join("b.typ");
        fs::write(
            &main,
            "= Main\n#include \"a.typ\"\n#include \"b.typ\"\n#include \"a.typ\"\n#import \"@preview/zeta:1.0\"\n",
        )
        .unwrap();
        fs::write(&first, "== A\n#import \"@preview/alpha:1.0\"\n").unwrap();
        fs::write(&second, "== B\n#import \"@preview/zeta:1.0\"\n").unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());

        assert_eq!(
            index
                .outline
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["Main", "A", "B"]
        );
        assert_eq!(
            index.subfiles,
            [first, second]
                .map(|path| path.canonicalize().unwrap())
                .to_vec()
        );
        assert_eq!(index.packages, ["@preview/alpha:1.0", "@preview/zeta:1.0"]);
    }

    #[test]
    fn first_btree_override_wins_when_paths_alias_the_same_file() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let child = project.path().join("child.typ");
        fs::create_dir(project.path().join("a")).unwrap();
        fs::write(&main, "#include \"child.typ\"\n").unwrap();
        fs::write(&child, "= Disk\n").unwrap();

        let overrides = BTreeMap::from([
            (
                project.path().join("a/../child.typ"),
                "= First\n".to_owned(),
            ),
            (child, "= Second\n".to_owned()),
        ]);
        let index = analyze_project(project.path(), &main, &overrides);

        assert_eq!(index.outline[0].title, "First");
    }
}
