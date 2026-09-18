use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs, io,
    path::{Component, Path, PathBuf},
};
use typst_syntax::{LinkedNode, Source, SyntaxKind, ast};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceEntry {
    pub path: PathBuf,
    pub line: usize,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyKind {
    Import,
    Include,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedDependency {
    pub path: PathBuf,
    pub line: usize,
    pub kind: DependencyKind,
    pub expression: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IndexCompleteness {
    pub file_limit_reached: bool,
    pub unreadable_files: Vec<IndexReadFailure>,
    // Built once by the worker, not formatted during Explorer repainting.
    warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexReadFailure {
    pub path: PathBuf,
    pub kind: io::ErrorKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectIndex {
    pub outline: Vec<OutlineEntry>,
    pub subfiles: Vec<PathBuf>,
    pub symbols: Vec<SymbolEntry>,
    pub packages: Vec<String>,
    pub tags: Vec<ReferenceEntry>,
    pub references: Vec<ReferenceEntry>,
    /// Import/include expressions which require Typst evaluation. The indexer
    /// deliberately reports rather than follows them.
    pub unresolved_dependencies: Vec<UnresolvedDependency>,
    pub completeness: IndexCompleteness,
}

impl ProjectIndex {
    pub fn warning(&self) -> Option<&str> {
        self.completeness.warning.as_deref()
    }

    fn update_warning(&mut self) {
        let mut reasons = Vec::new();
        if self.completeness.file_limit_reached {
            reasons.push(format!("{MAX_PROJECT_FILES}-file limit reached"));
        }
        if !self.completeness.unreadable_files.is_empty() {
            reasons.push(format!(
                "unreadable files: {}",
                self.completeness.unreadable_files.len()
            ));
        }
        if !self.unresolved_dependencies.is_empty() {
            reasons.push(format!(
                "dynamic dependencies: {}",
                self.unresolved_dependencies.len()
            ));
        }
        self.completeness.warning = (!reasons.is_empty())
            .then(|| format!("Partial project index · {}", reasons.join(" · ")));
    }
}

/// Build a compact, deterministic index from one Typst entry point.
///
/// This walks Typst's error-tolerant syntax tree without evaluating code or
/// downloading packages. Only literal local import/include paths are followed.
/// `overrides` lets the currently edited document participate before it is
/// saved to disk.
#[cfg(test)]
pub fn analyze_project(
    root: &Path,
    main: &Path,
    overrides: &BTreeMap<PathBuf, String>,
) -> ProjectIndex {
    analyze_project_cancellable(root, main, overrides, || false).unwrap_or_default()
}

pub(crate) fn analyze_project_cancellable(
    root: &Path,
    main: &Path,
    overrides: &BTreeMap<PathBuf, String>,
    mut cancelled: impl FnMut() -> bool,
) -> Option<ProjectIndex> {
    if cancelled() {
        return None;
    }
    let mut paths = PathNormalizer::default();
    let root = paths.normalize(root);
    let main = paths.normalize(main);
    if !main.starts_with(&root)
        || main
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("typ"))
    {
        return Some(ProjectIndex::default());
    }

    let mut index = ProjectIndex::default();
    // Reverse so canonical aliases retain BTreeMap's first-match behavior.
    let overrides: HashMap<_, _> = overrides
        .iter()
        .rev()
        .map(|(path, source)| (paths.normalize(path), source.as_str()))
        .collect();
    let mut visited = HashSet::new();
    let mut pending = vec![main.clone()];
    let mut packages = BTreeSet::new();

    while visited.len() < MAX_PROJECT_FILES
        && let Some(path) = pending.pop()
    {
        if cancelled() {
            return None;
        }
        if !visited.insert(path.clone()) {
            continue;
        }
        let source = match source_for(&path, &overrides) {
            Ok(source) => source,
            Err(error) => {
                index.completeness.unreadable_files.push(IndexReadFailure {
                    path,
                    kind: error.kind(),
                });
                continue;
            }
        };
        if path != main {
            index.subfiles.push(path.clone());
        }
        if !visit_source_cancellable(
            &path,
            &source,
            &mut index,
            &mut packages,
            |target| {
                let Some(resolved) = resolve_local_typst_path(&root, &path, target, &mut paths)
                else {
                    return;
                };
                if !visited.contains(&resolved) {
                    pending.push(resolved);
                }
            },
            &mut cancelled,
        ) {
            return None;
        }
    }

    // Duplicate/cyclic pending entries do not mean that anything was omitted.
    index.completeness.file_limit_reached = pending.iter().any(|path| !visited.contains(path));
    index
        .completeness
        .unreadable_files
        .sort_by(|a, b| a.path.cmp(&b.path));
    index.update_warning();
    index.subfiles.sort();
    index.packages = packages.into_iter().collect();
    Some(index)
}

fn source_for<'a>(
    path: &Path,
    overrides: &'a HashMap<PathBuf, &'a str>,
) -> io::Result<Cow<'a, str>> {
    if let Some(source) = overrides.get(path) {
        Ok(Cow::Borrowed(*source))
    } else {
        fs::read_to_string(path).map(Cow::Owned)
    }
}

#[cfg(test)]
fn visit_source(
    path: &Path,
    source: &str,
    index: &mut ProjectIndex,
    packages: &mut BTreeSet<String>,
    local_file: impl FnMut(&str),
) {
    let _ = visit_source_cancellable(path, source, index, packages, local_file, &mut || false);
}

fn visit_source_cancellable(
    path: &Path,
    source: &str,
    index: &mut ProjectIndex,
    packages: &mut BTreeSet<String>,
    mut local_file: impl FnMut(&str),
    cancelled: &mut impl FnMut() -> bool,
) -> bool {
    let parsed = Source::detached(source);
    let lines = LineIndex::new(source);
    let mut visitor = ProjectVisitor {
        path,
        source,
        lines: &lines,
        index,
        packages,
        local_file: &mut local_file,
        cancelled,
        nodes_since_check: 0,
    };
    visitor.visit(LinkedNode::new(parsed.root()))
}

struct ProjectVisitor<'a, F, C> {
    path: &'a Path,
    source: &'a str,
    lines: &'a LineIndex,
    index: &'a mut ProjectIndex,
    packages: &'a mut BTreeSet<String>,
    local_file: &'a mut F,
    cancelled: &'a mut C,
    nodes_since_check: usize,
}

impl<F: FnMut(&str), C: FnMut() -> bool> ProjectVisitor<'_, F, C> {
    fn visit(&mut self, node: LinkedNode<'_>) -> bool {
        self.nodes_since_check += 1;
        if self.nodes_since_check >= 256 {
            self.nodes_since_check = 0;
            if (self.cancelled)() {
                return false;
            }
        }
        match node.kind() {
            SyntaxKind::Heading => self.heading(&node),
            SyntaxKind::LetBinding => self.binding(&node),
            SyntaxKind::ModuleImport => self.dependency(&node, DependencyKind::Import),
            SyntaxKind::ModuleInclude => self.dependency(&node, DependencyKind::Include),
            SyntaxKind::Label | SyntaxKind::RefMarker => self.reference(&node),
            _ => {}
        }
        for child in node.children() {
            if !self.visit(child) {
                return false;
            }
        }
        true
    }

    fn heading(&mut self, node: &LinkedNode<'_>) {
        let Some(heading) = node.get().cast::<ast::Heading>() else {
            return;
        };
        let body = node
            .children()
            .find(|child| child.kind() == SyntaxKind::Markup)
            .map(|child| self.source[child.range()].trim())
            .unwrap_or_default();
        if !body.is_empty() {
            self.index.outline.push(OutlineEntry {
                path: self.path.to_owned(),
                line: self.lines.line_at(node.offset()),
                level: heading.depth().get(),
                title: body.to_owned(),
            });
        }
    }

    fn binding(&mut self, node: &LinkedNode<'_>) {
        let Some(binding) = node.get().cast::<ast::LetBinding>() else {
            return;
        };
        let (bindings, kind) = match binding.kind() {
            ast::LetBindingKind::Closure(name) => (vec![name], SymbolKind::Function),
            ast::LetBindingKind::Normal(pattern) => (pattern.bindings(), SymbolKind::Definition),
        };
        for binding in bindings {
            self.index.symbols.push(SymbolEntry {
                path: self.path.to_owned(),
                line: self.lines.line_at(node.offset()),
                name: binding.as_str().to_owned(),
                kind,
            });
        }
    }

    fn dependency(&mut self, node: &LinkedNode<'_>, kind: DependencyKind) {
        let expression = match kind {
            DependencyKind::Import => node
                .get()
                .cast::<ast::ModuleImport>()
                .map(ast::ModuleImport::source),
            DependencyKind::Include => node
                .get()
                .cast::<ast::ModuleInclude>()
                .map(ast::ModuleInclude::source),
        };
        let Some(expression) = expression else {
            return;
        };
        if let ast::Expr::Str(target) = expression {
            let target = target.get();
            if target.starts_with('@') {
                self.packages.insert(target.into());
            } else if target
                .rsplit_once('.')
                .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("typ"))
            {
                (self.local_file)(&target);
            }
        } else {
            self.index
                .unresolved_dependencies
                .push(UnresolvedDependency {
                    path: self.path.to_owned(),
                    line: self.lines.line_at(node.offset()),
                    kind,
                    expression: self.source[node.range()].trim().to_owned(),
                });
        }
    }

    fn reference(&mut self, node: &LinkedNode<'_>) {
        let entries = if node.kind() == SyntaxKind::Label {
            &mut self.index.tags
        } else {
            &mut self.index.references
        };
        entries.push(ReferenceEntry {
            path: self.path.to_owned(),
            line: self.lines.line_at(node.offset()),
            label: self.source[node.range()].to_owned(),
        });
    }
}

struct LineIndex {
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            source
                .match_indices('\n')
                .map(|(byte, newline)| byte + newline.len()),
        );
        Self { starts }
    }

    fn line_at(&self, byte: usize) -> usize {
        self.starts.partition_point(|start| *start <= byte)
    }
}

fn resolve_local_typst_path(
    root: &Path,
    source: &Path,
    target: &str,
    paths: &mut PathNormalizer,
) -> Option<PathBuf> {
    let target = Path::new(target);
    let candidate = if target.is_absolute() {
        root.join(target.strip_prefix(Component::RootDir.as_os_str()).ok()?)
    } else {
        source.parent()?.join(target)
    };
    let candidate = paths.normalize(&candidate);
    (candidate.starts_with(root)
        && candidate
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("typ")))
    .then_some(candidate)
}

#[derive(Default)]
struct PathNormalizer {
    // Per traversal, bounded independently of the number of missing imports.
    parents: HashMap<PathBuf, PathBuf>,
    #[cfg(test)]
    attempts: usize,
}

impl PathNormalizer {
    fn normalize(&mut self, path: &Path) -> PathBuf {
        #[cfg(test)]
        {
            self.attempts += 1;
        }
        path.canonicalize().unwrap_or_else(|_| {
            // Missing/unsaved files must use the same workspace identity as existing
            // files, including aliases such as /var -> /private/var on macOS.
            match (path.parent(), path.file_name()) {
                (Some(parent), Some(name)) if !parent.as_os_str().is_empty() => {
                    if let Some(canonical) = self.parents.get(parent) {
                        return canonical.join(name);
                    }
                    let canonical = self.normalize(parent);
                    if self.parents.len() < MAX_PROJECT_FILES {
                        self.parents.insert(parent.to_owned(), canonical.clone());
                    }
                    canonical.join(name)
                }
                _ => path.to_owned(),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_siblings_reuse_one_parent_lookup_and_cache_is_bounded() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let mut paths = PathNormalizer::default();
        for index in 0..64 {
            let missing = root.join(format!("missing-{index}.typ"));
            assert_eq!(paths.normalize(&missing), missing);
        }
        assert_eq!(
            paths.attempts, 65,
            "one attempt per file plus one shared parent"
        );
        assert_eq!(paths.parents.len(), 1);
        for index in 0..MAX_PROJECT_FILES + 10 {
            paths.normalize(&root.join(format!("missing-dir-{index}/file.typ")));
        }
        assert_eq!(paths.parents.len(), MAX_PROJECT_FILES);
        assert!(
            PathNormalizer::default().parents.is_empty(),
            "no cache survives an indexing pass"
        );
    }

    fn virtual_project(children: usize) -> ProjectIndex {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let mut source = "= Main\n#include \"main.typ\"\n".to_owned();
        let mut overrides = BTreeMap::new();
        for child in 0..children {
            source.push_str(&format!("#include \"child-{child}.typ\"\n"));
            overrides.insert(
                project.path().join(format!("child-{child}.typ")),
                format!("= Child {child}\n"),
            );
        }
        // Keep a duplicate pending at the exact traversal boundary.
        if children > 0 {
            source.push_str("#include \"child-0.typ\"\n");
        }
        overrides.insert(main.clone(), source);
        analyze_project(project.path(), &main, &overrides)
    }

    #[test]
    fn traversal_cutoff_reports_only_unvisited_files_and_bounds_work() {
        let complete = virtual_project(MAX_PROJECT_FILES - 1);
        assert_eq!(complete.outline.len(), MAX_PROJECT_FILES);
        assert!(!complete.completeness.file_limit_reached);
        assert!(
            complete.warning().is_none(),
            "duplicates and cycles are not omissions"
        );
        let partial = virtual_project(MAX_PROJECT_FILES);
        assert_eq!(partial.outline.len(), MAX_PROJECT_FILES);
        assert_eq!(partial.subfiles.len(), MAX_PROJECT_FILES - 1);
        assert!(partial.completeness.file_limit_reached);
        assert!(partial.completeness.unreadable_files.is_empty());
        assert_eq!(
            partial.warning(),
            Some("Partial project index · 256-file limit reached")
        );
        assert!(std::ptr::eq(
            partial.warning().unwrap(),
            partial.warning().unwrap()
        ));
    }

    #[test]
    fn read_failures_are_deduplicated_and_overrides_recover_without_disk_reads() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let missing = project.path().join("missing.typ");
        let invalid = project.path().join("invalid.typ");
        fs::write(&main, "= Available\n#include \"missing.typ\"\n#include \"missing.typ\"\n#include \"invalid.typ\"\n#include target\n").unwrap();
        fs::write(&invalid, [0xff, 0xfe]).unwrap();
        let index = analyze_project(project.path(), &main, &BTreeMap::new());
        assert_eq!(index.outline.len(), 1);
        assert_eq!(index.unresolved_dependencies.len(), 1);
        assert_eq!(
            index.completeness.unreadable_files,
            vec![
                IndexReadFailure {
                    path: invalid.canonicalize().unwrap(),
                    kind: io::ErrorKind::InvalidData
                },
                IndexReadFailure {
                    path: project.path().canonicalize().unwrap().join("missing.typ"),
                    kind: io::ErrorKind::NotFound
                },
            ]
        );
        assert_eq!(
            index.warning(),
            Some("Partial project index · unreadable files: 2 · dynamic dependencies: 1")
        );
        let overrides = BTreeMap::from([
            (missing, "= Unsaved\n".to_owned()),
            (invalid, "= Recovered\n".to_owned()),
            (
                main.clone(),
                "#include \"missing.typ\"\n#include \"invalid.typ\"\n".to_owned(),
            ),
        ]);
        let recovered = analyze_project(project.path(), &main, &overrides);
        assert_eq!(recovered.outline.len(), 2);
        assert!(recovered.completeness.unreadable_files.is_empty());
        assert!(recovered.warning().is_none());
    }

    #[test]
    fn missing_entry_is_partial_but_package_policy_is_not_a_read_failure() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let missing = analyze_project(project.path(), &main, &BTreeMap::new());
        assert_eq!(missing.completeness.unreadable_files.len(), 1);
        assert!(missing.warning().is_some());
        fs::write(&main, "#import \"@preview/nonexistent-package:0.0.0\": *\n").unwrap();
        let package = analyze_project(project.path(), &main, &BTreeMap::new());
        assert_eq!(package.packages, ["@preview/nonexistent-package:0.0.0"]);
        assert!(package.warning().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn missing_files_and_unsaved_overrides_share_symlinked_workspace_identity() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().join("workspace");
        fs::create_dir(&root).unwrap();
        let alias = project.path().join("alias");
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let main = alias.join("not-created-yet/main.typ");
        let missing = analyze_project(&alias, &main, &BTreeMap::new());
        assert_eq!(
            missing.completeness.unreadable_files[0].path,
            root.canonicalize()
                .unwrap()
                .join("not-created-yet/main.typ")
        );
        let overrides = BTreeMap::from([(main.clone(), "= Unsaved\n".to_owned())]);
        let unsaved = analyze_project(&root, &main, &overrides);
        assert_eq!(unsaved.outline[0].title, "Unsaved");
        assert!(unsaved.warning().is_none());
    }

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
    fn indexes_document_tags_and_references_but_not_package_names() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        fs::write(
            &main,
            "= Main <chapter>
See @chapter and @figure. #link(<appendix>)[Appendix]
#import \"@preview/cetz:0.3.2\"
",
        )
        .unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());

        assert_eq!(
            index
                .references
                .iter()
                .map(|reference| reference.label.as_str())
                .collect::<Vec<_>>(),
            ["@chapter", "@figure"]
        );
        assert_eq!(
            index
                .tags
                .iter()
                .map(|entry| (entry.label.as_str(), entry.line))
                .collect::<Vec<_>>(),
            [("<chapter>", 1), ("<appendix>", 2)]
        );
        assert!(index.references.iter().all(|reference| reference.line == 2));
    }

    #[test]
    fn project_index_accepts_case_insensitive_typst_extensions() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.TYP");
        fs::write(&main, "#include \"chapter.TyP\"").unwrap();
        fs::write(project.path().join("chapter.TyP"), "= Chapter").unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());

        assert_eq!(
            index.subfiles,
            vec![project.path().join("chapter.TyP").canonicalize().unwrap()]
        );
    }

    #[test]
    fn reference_index_ignores_strings_comments_and_raw_blocks() {
        let mut index = ProjectIndex::default();
        let mut packages = BTreeSet::new();
        visit_source(
            Path::new("main.typ"),
            "// @comment <comment>\n/* @block <block> */\n#let s = \"@string <string>\"\n`@raw <raw>`\n= Chapter <real>\nSee @real.",
            &mut index,
            &mut packages,
            |_| {},
        );
        assert_eq!(
            index
                .references
                .iter()
                .map(|r| (r.label.as_str(), r.line))
                .collect::<Vec<_>>(),
            vec![("@real", 6)]
        );
    }

    #[test]
    fn tag_index_ignores_strings_comments_and_raw_blocks() {
        let mut index = ProjectIndex::default();
        visit_source(
            Path::new("main.typ"),
            "// <comment>\n`<raw>`\n#let s = \"<string>\"\n= Chapter <real>\nSee @real.",
            &mut index,
            &mut BTreeSet::new(),
            |_| {},
        );
        assert_eq!(
            index
                .tags
                .iter()
                .map(|entry| (entry.label.as_str(), entry.line))
                .collect::<Vec<_>>(),
            [("<real>", 4)]
        );
    }

    #[test]
    fn comments_and_raw_examples_contribute_no_live_index_entries() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let child = project.path().join("child.typ");
        fs::write(&child, "= Child\n").unwrap();
        fs::write(
            &main,
            r###"/*
= Comment heading
#let comment_definition = 1
#include "child.typ"
*/
```typ
= Sample heading
#let sample_definition = 1
#include "child.typ"
```
= Real heading
#let real = 2
"###,
        )
        .unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());
        assert_eq!(
            index
                .outline
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["Real heading"]
        );
        assert_eq!(
            index
                .symbols
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            ["real"]
        );
        assert!(index.subfiles.is_empty());
    }

    #[test]
    fn multiline_syntax_and_code_block_bindings_are_visited() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let child = project.path().join("child.typ");
        fs::write(
            &main,
            "#import \"child.typ\": (\n  chapter,\n)\n#let render(\n  body,\n) = body\n#let values = {\n  let nested = 1\n  nested\n}\n",
        )
        .unwrap();
        fs::write(&child, "= Child\n").unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());
        assert_eq!(index.subfiles, vec![child.canonicalize().unwrap()]);
        assert!(index.symbols.iter().any(|symbol| {
            symbol.name == "render" && symbol.kind == SymbolKind::Function && symbol.line == 4
        }));
        assert!(index.symbols.iter().any(|symbol| {
            symbol.name == "nested" && symbol.kind == SymbolKind::Definition && symbol.line == 8
        }));
    }

    #[test]
    fn dynamic_dependencies_are_reported_but_not_followed() {
        let project = tempfile::tempdir().unwrap();
        let main = project.path().join("main.typ");
        let child = project.path().join("child.typ");
        fs::write(
            &main,
            "#let target = \"child.typ\"\n#include target\n#import target\n",
        )
        .unwrap();
        fs::write(&child, "= Child\n").unwrap();

        let index = analyze_project(project.path(), &main, &BTreeMap::new());
        assert!(index.subfiles.is_empty());
        assert_eq!(
            index
                .unresolved_dependencies
                .iter()
                .map(|dependency| (dependency.kind, dependency.line))
                .collect::<Vec<_>>(),
            [(DependencyKind::Include, 2), (DependencyKind::Import, 3)]
        );
    }

    #[test]
    fn line_index_maps_many_offsets_without_rescanning_prefixes() {
        let source = (1..=10_000)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let lines = LineIndex::new(&source);
        let offsets = source.match_indices("line ").map(|(byte, _)| byte);
        assert_eq!(
            offsets
                .enumerate()
                .map(|(index, byte)| lines.line_at(byte) == index + 1)
                .filter(|correct| *correct)
                .count(),
            10_000
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
