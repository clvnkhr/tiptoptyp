use super::*;

fn index(source_name: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, ProjectIndex) {
    let directory = tempfile::tempdir().unwrap();
    let overrides = files
        .iter()
        .map(|(path, source)| (directory.path().join(path), (*source).into()))
        .collect();
    let index = analyze_project(
        directory.path(),
        &directory.path().join(source_name),
        &overrides,
    );
    (directory, index)
}

#[test]
fn tex_bibliographies_appear_once_as_leaves_in_the_correct_import_context() {
    let (directory, index) = index(
        "main.tex",
        &[
            (
                "main.tex",
                r"\section{Main}\input{chapters/plain}\subfile{chapters/nested}\bibliography{bib/master,bib/other,bib/master}",
            ),
            (
                "chapters/plain.tex",
                r"\addbibresource[location=local]{bib/master.bib}",
            ),
            (
                "chapters/nested.tex",
                r"\section{Nested}\addbibresource{bib/local.BIB}",
            ),
            ("bib/master.bib", r"\section{Not source}\input{absent}"),
            ("bib/other.bib", r"\label{not-a-tex-label}"),
            ("chapters/bib/local.BIB", "@article{local, title={Local}}"),
        ],
    );
    let root = directory.path().canonicalize().unwrap();
    assert_eq!(
        index.subfiles,
        [
            root.join("bib/master.bib"),
            root.join("bib/other.bib"),
            root.join("chapters/bib/local.BIB"),
            root.join("chapters/nested.tex"),
            root.join("chapters/plain.tex"),
        ]
    );
    assert_eq!(
        index
            .outline
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        ["Main", "Nested"]
    );
    assert!(index.tags.is_empty());
    assert!(index.warning().is_none());
}

#[test]
fn child_magic_root_uses_unsaved_parent_bibliography_declarations() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::create_dir(root.join("chapters")).unwrap();
    let parent = root.join("main.tex");
    let child = root.join("chapters/child.tex");
    fs::write(&parent, "\\bibliography{old}").unwrap();
    fs::write(&child, "% !TeX root = ../main.tex\n\\subsection{Child}").unwrap();
    fs::write(root.join("old.bib"), "@article{old}").unwrap();
    fs::write(root.join("new.bib"), "@article{new}").unwrap();
    let overrides = BTreeMap::from([(
        parent,
        "\\section{Unsaved parent}\\input{chapters/child}\\bibliography{new}".into(),
    )]);
    let index = analyze_project(root, &child, &overrides);
    let root = root.canonicalize().unwrap();
    assert_eq!(
        index.subfiles,
        [root.join("chapters/child.tex"), root.join("new.bib")]
    );
    assert_eq!(
        index
            .outline
            .iter()
            .map(|entry| entry.title.as_str())
            .collect::<Vec<_>>(),
        ["Unsaved parent", "Child"]
    );
    assert!(index.warning().is_none());
}

#[test]
fn tex_library_sources_keep_the_same_bibliography_discovery() {
    for extension in crate::document::TEX_SOURCE_EXTENSIONS {
        let name = format!("main.{extension}");
        let (_, index) = index(
            &name,
            &[
                (
                    name.as_str(),
                    "\\newcommand{\\example}{}\\bibliography{references}",
                ),
                ("references.bib", "@article{reference}"),
            ],
        );
        assert_eq!(index.subfiles.len(), 1, "{extension}");
        assert_eq!(index.subfiles[0].file_name().unwrap(), "references.bib");
        assert!(
            index
                .symbols
                .iter()
                .any(|symbol| symbol.name == "\\example")
        );
        assert!(index.warning().is_none());
    }
}

#[test]
fn invalid_magic_roots_leave_available_child_entries_explicitly_partial() {
    for target in [
        "missing.tex",
        "../outside.tex",
        ".tiptoptyp/private.tex",
        "next.tex",
    ] {
        let main = format!("% !TeX root = {target}\n\\section{{Available child}}");
        let (_, index) = index(
            "main.tex",
            &[("main.tex", &main), ("next.tex", "% !TeX root = main.tex")],
        );
        assert_eq!(index.outline.len(), 1);
        assert_eq!(index.outline[0].title, "Available child");
        assert_eq!(index.unresolved_dependencies.len(), 1, "{target}");
        assert!(index.warning().is_some());
        assert!(index.subfiles.is_empty());
    }
}

#[test]
fn root_resolution_checks_cancellation_and_rejects_oversized_files_before_reading() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.tex");
    let mut overrides = BTreeMap::new();
    for entry in 0..crate::tex::magic::MAX_ROOT_CHAIN {
        let path = if entry == 0 {
            main.clone()
        } else {
            directory.path().join(format!("root-{entry}.tex"))
        };
        overrides.insert(path, format!("% !TeX root = root-{}.tex", entry + 1));
    }
    let mut checks = 0;
    assert!(
        analyze_project_cancellable(
            directory.path(),
            &main,
            directory.path(),
            &overrides,
            || {
                checks += 1;
                checks >= 4
            }
        )
        .is_none()
    );
    assert_eq!(checks, 4);

    fs::write(&main, "% !TeX root = oversized.tex\n\\section{Available}").unwrap();
    let file = fs::File::create(directory.path().join("oversized.tex")).unwrap();
    file.set_len(crate::tex::magic::MAX_ROOT_SOURCE_BYTES + 1)
        .unwrap();
    let index = analyze_project(directory.path(), &main, &BTreeMap::new());
    assert_eq!(index.outline[0].title, "Available");
    assert_eq!(index.unresolved_dependencies.len(), 1);
    assert!(
        index.unresolved_dependencies[0]
            .expression
            .contains("16 MiB")
    );
}

#[test]
fn tex_commented_literal_and_definition_examples_do_not_add_bibliographies() {
    let (_, index) = index(
        "main.tex",
        &[
            (
                "main.tex",
                r"% \bibliography{commented}
\verb|\addbibresource{inline.bib}|
\begin{verbatim}\bibliography{listing}\end{verbatim}
\newcommand{\example}{\bibliography{definition}}
\string\bibliography{literal}
\bibliography{real}",
            ),
            ("real.bib", "@article{real}"),
        ],
    );
    assert_eq!(index.subfiles.len(), 1);
    assert_eq!(index.subfiles[0].file_name().unwrap(), "real.bib");
    assert!(index.warning().is_none());
}

#[test]
fn tex_dynamic_external_and_missing_bibliographies_are_explicitly_partial() {
    let (_, index) = index(
        "main.tex",
        &[
            (
                "main.tex",
                r"\addbibresource{\computed}
\addbibresource{../outside.bib}
\addbibresource[location=remote]{https://example.invalid/library.bib}
\bibliography{present,missing,present}",
            ),
            ("present.bib", "@article{present}"),
        ],
    );
    assert_eq!(index.subfiles.len(), 1);
    assert_eq!(index.subfiles[0].file_name().unwrap(), "present.bib");
    assert_eq!(index.unresolved_dependencies.len(), 3);
    assert!(
        index
            .unresolved_dependencies
            .iter()
            .all(|d| d.kind == DependencyKind::Bibliography)
    );
    assert_eq!(index.completeness.unreadable_files.len(), 1);
    assert_eq!(
        index.completeness.unreadable_files[0]
            .path
            .file_name()
            .unwrap(),
        "missing.bib"
    );
    assert_eq!(
        index.completeness.unreadable_files[0].kind,
        io::ErrorKind::NotFound
    );
}

#[test]
fn typst_bibliography_strings_arrays_and_root_relative_paths_are_leaf_dependencies() {
    let (directory, index) = index(
        "main.typ",
        &[
            (
                "main.typ",
                "= Main\n#include \"chapters/a.typ\"\n#bibliography(\"été notes.bib\")",
            ),
            (
                "chapters/a.typ",
                "== Chapter\n#bibliography((\"../refs.bib\", \"/sources.yml\", \"../refs.bib\"), title: [References])\n#std.bibliography(\"../more.YAML\")",
            ),
            (
                "refs.bib",
                "= Not source\n#let not_a_symbol = 1\n#include \"absent.typ\"",
            ),
            ("été notes.bib", "@article{été}"),
            ("sources.yml", "source: {type: article, title: Title}"),
            ("more.YAML", "other: {type: book, title: Other}"),
        ],
    );
    let root = directory.path().canonicalize().unwrap();
    assert_eq!(
        index.subfiles,
        [
            root.join("chapters/a.typ"),
            root.join("more.YAML"),
            root.join("refs.bib"),
            root.join("sources.yml"),
            root.join("été notes.bib")
        ]
    );
    assert_eq!(index.outline.len(), 2);
    assert!(index.symbols.is_empty());
    assert!(index.warning().is_none());
}

#[test]
fn typst_dynamic_arrays_retain_literal_files_without_executing_code_or_examples() {
    let (_, index) = index(
        "main.typ",
        &[
            (
                "main.typ",
                "// #bibliography(\"commented.bib\")\n`#bibliography(\"raw.bib\")`\n#other.bibliography(\"unrelated.bib\")\n#bibliography((\"real.bib\", dynamic, ..more))\n#bibliography(\"../outside.bib\")",
            ),
            ("real.bib", "@article{real}"),
        ],
    );
    assert_eq!(index.subfiles.len(), 1);
    assert_eq!(index.subfiles[0].file_name().unwrap(), "real.bib");
    assert_eq!(index.unresolved_dependencies.len(), 2);
    assert!(
        index
            .unresolved_dependencies
            .iter()
            .all(|d| d.kind == DependencyKind::Bibliography)
    );
    assert!(index.completeness.unreadable_files.is_empty());
}

#[test]
fn bibliography_leaves_are_not_read_or_parsed_as_utf8_source() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.typ");
    let bibliography = directory.path().join("references.bib");
    fs::write(&main, "#bibliography(\"references.bib\")").unwrap();
    fs::write(&bibliography, [0xff, 0xfe]).unwrap();
    let index = analyze_project(directory.path(), &main, &BTreeMap::new());
    assert_eq!(index.subfiles, [bibliography.canonicalize().unwrap()]);
    assert!(
        index.warning().is_none(),
        "bibliography syntax/encoding diagnostics belong to its language service/compiler"
    );
}

#[cfg(unix)]
#[test]
fn bibliography_symlinks_cannot_escape_the_workspace() {
    for tex in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let bibliography = outside.path().join("outside.bib");
        fs::write(&bibliography, "@article{outside}").unwrap();
        std::os::unix::fs::symlink(&bibliography, directory.path().join("escape.bib")).unwrap();
        let main = directory
            .path()
            .join(if tex { "main.tex" } else { "main.typ" });
        fs::write(
            &main,
            if tex {
                "\\bibliography{escape}"
            } else {
                "#bibliography(\"escape.bib\")"
            },
        )
        .unwrap();
        let index = analyze_project(directory.path(), &main, &BTreeMap::new());
        assert!(index.subfiles.is_empty());
        assert_eq!(index.unresolved_dependencies.len(), 1);
        assert!(index.completeness.unreadable_files.is_empty());
    }
}

#[test]
fn bibliography_dependencies_share_the_source_file_budget() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("main.tex");
    let mut source = String::new();
    let mut overrides = BTreeMap::new();
    for child in 0..MAX_PROJECT_FILES {
        source.push_str(&format!(
            "\\bibliography{{library-{child},library-{child}}}\n"
        ));
        overrides.insert(
            directory.path().join(format!("library-{child}.bib")),
            "@article{entry}".into(),
        );
    }
    overrides.insert(main.clone(), source);
    let index = analyze_project(directory.path(), &main, &overrides);
    assert_eq!(index.subfiles.len(), MAX_PROJECT_FILES - 1);
    assert!(index.completeness.file_limit_reached);
    assert!(index.completeness.unreadable_files.is_empty());
}

#[test]
fn bibliography_collection_checks_cancellation_before_enumerating_long_lists() {
    let names = (0..5000)
        .map(|entry| format!("library-{entry}.bib"))
        .collect::<Vec<_>>();
    for tex in [false, true] {
        let source = if tex {
            format!("\\bibliography{{{}}}", names.join(","))
        } else {
            format!(
                "#bibliography(({}))",
                names
                    .iter()
                    .map(|name| format!("\"{name}\""))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let requests = std::cell::Cell::new(0);
        let mut index = ProjectIndex::default();
        let mut packages = BTreeSet::new();
        let mut cancelled = || requests.get() >= 256;
        let complete = if tex {
            super::tex::visit(
                Path::new("main.tex"),
                &source,
                &mut index,
                &mut packages,
                |_, _| {
                    requests.set(requests.get() + 1);
                    true
                },
                &mut cancelled,
            )
        } else {
            visit_source_cancellable(
                Path::new("main.typ"),
                &source,
                &mut index,
                &mut packages,
                |_, _| {
                    requests.set(requests.get() + 1);
                    true
                },
                &mut cancelled,
            )
        };
        assert!(!complete, "superseded bibliography indexing must stop");
        assert!(
            (256..=512).contains(&requests.get()),
            "{tex}: {} requests after cancellation",
            requests.get()
        );
    }
}
