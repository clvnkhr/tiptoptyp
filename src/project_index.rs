use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
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
pub struct ProjectIndex {
    pub outline: Vec<OutlineEntry>,
    pub subfiles: Vec<PathBuf>,
    pub symbols: Vec<SymbolEntry>,
    pub packages: Vec<String>,
    pub references: Vec<ReferenceEntry>,
    /// Import/include expressions which require Typst evaluation. The indexer
    /// deliberately reports rather than follows them.
    pub unresolved_dependencies: Vec<UnresolvedDependency>,
}

/// Build a compact, deterministic index from one Typst entry point.
///
/// This walks Typst's error-tolerant syntax tree without evaluating code or
/// downloading packages. Only literal local import/include paths are followed.
/// `overrides` lets the currently edited document participate before it is
/// saved to disk.
pub fn analyze_project(
    root: &Path,
    main: &Path,
    overrides: &BTreeMap<PathBuf, String>,
) -> ProjectIndex {
    let root = canonical_or_owned(root);
    let main = canonical_or_owned(main);
    if !main.starts_with(&root)
        || main
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("typ"))
    {
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
        visit_source(&path, &source, &mut index, &mut packages, |target| {
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

fn visit_source(
    path: &Path,
    source: &str,
    index: &mut ProjectIndex,
    packages: &mut BTreeSet<String>,
    mut local_file: impl FnMut(&str),
) {
    let parsed = Source::detached(source);
    let lines = LineIndex::new(source);
    let mut visitor = ProjectVisitor {
        path,
        source,
        lines: &lines,
        index,
        packages,
        local_file: &mut local_file,
    };
    visitor.visit(LinkedNode::new(parsed.root()));
}

struct ProjectVisitor<'a, F> {
    path: &'a Path,
    source: &'a str,
    lines: &'a LineIndex,
    index: &'a mut ProjectIndex,
    packages: &'a mut BTreeSet<String>,
    local_file: &'a mut F,
}

impl<F: FnMut(&str)> ProjectVisitor<'_, F> {
    fn visit(&mut self, node: LinkedNode<'_>) {
        match node.kind() {
            SyntaxKind::Heading => self.heading(&node),
            SyntaxKind::LetBinding => self.binding(&node),
            SyntaxKind::ModuleImport => self.dependency(&node, DependencyKind::Import),
            SyntaxKind::ModuleInclude => self.dependency(&node, DependencyKind::Include),
            SyntaxKind::Label | SyntaxKind::RefMarker => self.reference(&node),
            _ => {}
        }
        for child in node.children() {
            self.visit(child);
        }
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
        self.index.references.push(ReferenceEntry {
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
            .is_some_and(|extension| extension.eq_ignore_ascii_case("typ")))
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
            ["<chapter>", "@chapter", "@figure", "<appendix>"]
        );
        assert!(
            index
                .references
                .iter()
                .all(|reference| reference.line == 1 || reference.line == 2)
        );
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
            vec![("<real>", 5), ("@real", 6)]
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
