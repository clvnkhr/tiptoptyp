use super::*;

fn virtual_index(source: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, ProjectIndex) {
    let directory = tempfile::tempdir().unwrap();
    let mut overrides = BTreeMap::from([(directory.path().join("main.tex"), source.into())]);
    for (path, source) in files {
        overrides.insert(directory.path().join(path), (*source).into());
    }
    let index = analyze_project(
        directory.path(),
        &directory.path().join("main.tex"),
        &overrides,
    );
    (directory, index)
}

#[test]
fn headings_macros_packages_labels_and_reference_lists_are_indexed() {
    let (_, index) = virtual_index(
        r"\usepackage[unicode]{amsmath, hyperref}
\RequirePackage{cleveref}[2020/01/01]
\newcommand{\energy}[1]{\label{not-executed}\section{Not executed}#1}
\DeclareMathOperator*{\esssup}{ess\,sup}
\newenvironment{proofidea}{\input{not-executed}}{}
\section*[Running {short}]{Main {result}}
\subsection{Équations $u=0$}
\label{eq:first}\label{eq:second}
\eqref{eq:first} \cref{eq:first, eq:second} \autoref*{eq:second}",
        &[],
    );
    assert_eq!(index.packages, ["amsmath", "cleveref", "hyperref"]);
    assert_eq!(
        index
            .outline
            .iter()
            .map(|h| (h.title.as_str(), h.line, h.level))
            .collect::<Vec<_>>(),
        [("Main {result}", 6, 1), ("Équations $u=0$", 7, 2)]
    );
    assert_eq!(
        index
            .symbols
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["\\energy", "\\esssup", "proofidea"]
    );
    assert_eq!(
        index
            .tags
            .iter()
            .map(|t| t.label.as_str())
            .collect::<Vec<_>>(),
        ["eq:first", "eq:second"]
    );
    assert_eq!(index.reference_indexes_for_tag("eq:first"), [0, 1]);
    assert_eq!(index.reference_indexes_for_tag("eq:second"), [2, 3]);
    assert!(index.warning().is_none());
}

#[test]
fn comments_escaped_commands_literal_text_and_macro_bodies_do_not_execute() {
    let (_, index) = virtual_index(
        r"% \section{Comment}\input{absent}
\\section{Escaped control}
\verb|\section{Inline}|
\verb*§\input{absent}§
\lstinline[language=TeX]!\label{example}!
\begin{verbatim}\usepackage{bad}\section{Listing}\end{verbatim}
\begin{minted}{tex}\input{absent}\end{minted}
\begin{comment}
\section{Hidden}
\end{comment}
\string\section{Literal}
\detokenize{\input{absent}\label{literal}}
\def\test#1{\section{Inside definition}\input{absent}}
\NewDocumentCommand\other{m}{\label{body}}
\section{Real\% percent}% trailing comment
\label{real}\ref{real}",
        &[],
    );
    assert_eq!(
        index
            .outline
            .iter()
            .map(|h| h.title.as_str())
            .collect::<Vec<_>>(),
        ["Real\\% percent"]
    );
    assert!(index.packages.is_empty());
    assert_eq!(index.tags.len(), 1);
    assert_eq!(index.references.len(), 1);
    assert!(index.warning().is_none());
}

#[test]
fn literal_inputs_use_master_directory_defaults_cycles_and_unsaved_overrides() {
    let (directory, index) = virtual_index(
        r"\chapter{Book}\input{chapters/a}\include{other}\subfile{third.tex}",
        &[
            ("chapters/a.tex", "\\section{A}\\input{shared}\\input{main}"),
            ("shared.tex", "\\subsection{Shared}"),
            ("other.tex", "\\section{Other}"),
            ("third.tex", "\\section{Third}"),
        ],
    );
    let root = directory.path().canonicalize().unwrap();
    assert_eq!(
        index.subfiles,
        [
            root.join("chapters/a.tex"),
            root.join("other.tex"),
            root.join("shared.tex"),
            root.join("third.tex")
        ]
    );
    assert_eq!(index.outline.len(), 5);
    assert_eq!(
        index
            .outline
            .iter()
            .find(|h| h.title == "Book")
            .unwrap()
            .level,
        1
    );
    assert_eq!(
        index
            .outline
            .iter()
            .find(|h| h.title == "Shared")
            .unwrap()
            .level,
        3
    );
    assert!(index.warning().is_none());
}

#[test]
fn quoted_bare_input_keeps_spaces_and_unicode_in_the_path() {
    let (_, index) = virtual_index(
        r#"\input "Été notes.tex""#,
        &[("Été notes.tex", "\\section{Notes}")],
    );
    assert_eq!(index.outline[0].title, "Notes");
    assert!(index.warning().is_none());
}

#[test]
fn dynamic_outside_missing_and_invalid_inputs_are_explicitly_partial() {
    let (directory, index) = virtual_index(
        r"\section{Available}
\input{\chaptername}
\include{../outside}
\input{absent}
\input bare
\input{directory/../../escape}",
        &[("bare.tex", "\\section{Bare}")],
    );
    assert_eq!(index.outline.len(), 2);
    assert_eq!(index.unresolved_dependencies.len(), 3);
    assert_eq!(index.completeness.unreadable_files.len(), 1);
    assert_eq!(
        index.completeness.unreadable_files[0].path,
        directory.path().canonicalize().unwrap().join("absent.tex")
    );
    assert_eq!(
        index.completeness.unreadable_files[0].kind,
        io::ErrorKind::NotFound
    );
    let warning = index.warning().unwrap();
    assert!(warning.contains("literal TeX inputs"));
    assert!(warning.contains("without expanding macros"));
    assert!(!warning.contains("#include"));
}

#[test]
fn braced_options_and_comments_do_not_shift_line_numbers_or_create_false_commands() {
    let (_, index) = virtual_index(
        "\\section[Short {with ] bracket}]{Long % discarded\n title}\n\\usepackage[option={a]b}]{amsmath,% rest\n cleveref}\n\\label{a}\\Cref{a}",
        &[],
    );
    assert_eq!(index.outline[0].title, "Long  title");
    assert_eq!(index.outline[0].line, 1);
    assert_eq!(index.packages, ["amsmath", "cleveref"]);
    assert_eq!(index.references[0].line, 5);
    assert_eq!(index.reference_indexes_for_tag("a"), [0]);
}

#[test]
fn plain_tex_keys_and_typst_markers_both_link_without_eating_tex_at_signs() {
    let (_, index) = virtual_index(r"\label{@name}\label{name}\ref{@name}\ref{name}", &[]);
    assert_eq!(index.reference_indexes_for_tag("@name"), [0]);
    assert_eq!(index.reference_indexes_for_tag("name"), [1]);
    let mut typst = ProjectIndex {
        tags: vec![ReferenceEntry {
            path: "x.typ".into(),
            line: 1,
            label: "<tag>".into(),
        }],
        references: vec![ReferenceEntry {
            path: "x.typ".into(),
            line: 2,
            label: "@tag".into(),
        }],
        ..Default::default()
    };
    typst.rebuild_tag_references();
    assert_eq!(typst.reference_indexes_for_tag("tag"), [0]);
    assert_eq!(typst.reference_indexes_for_tag("<tag>"), [0]);
}

#[test]
fn traversal_and_pending_work_are_bounded_and_duplicates_are_not_omissions() {
    for extra in [0, 1] {
        let directory = tempfile::tempdir().unwrap();
        let main = directory.path().join("main.tex");
        let mut source = "\\section{Main}\\input{main}".to_owned();
        let mut overrides = BTreeMap::new();
        for child in 0..MAX_PROJECT_FILES - 1 + extra {
            source.push_str(&format!("\\input{{child-{child}}}\\input{{child-{child}}}"));
            overrides.insert(
                directory.path().join(format!("child-{child}.tex")),
                format!("\\section{{Child {child}}}"),
            );
        }
        overrides.insert(main.clone(), source);
        let index = analyze_project(directory.path(), &main, &overrides);
        assert_eq!(index.outline.len(), MAX_PROJECT_FILES);
        assert_eq!(index.subfiles.len(), MAX_PROJECT_FILES - 1);
        assert_eq!(index.completeness.file_limit_reached, extra == 1);
    }
}

#[test]
fn cancellation_interrupts_large_plain_text_and_balanced_macro_bodies() {
    for source in [
        "ordinary text ".repeat(10000),
        format!("\\newcommand{{\\foo}}{{{}}}", "x".repeat(100000)),
    ] {
        let mut calls = 0;
        let mut index = ProjectIndex::default();
        let mut packages = BTreeSet::new();
        assert!(!visit(
            Path::new("main.tex"),
            &source,
            &mut index,
            &mut packages,
            |_, _| true,
            &mut || {
                calls += 1;
                calls >= 3
            }
        ));
        assert_eq!(calls, 3);
    }
}

#[cfg(unix)]
#[test]
fn symlinked_inputs_cannot_escape_the_workspace() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("outside.tex"), "\\section{Outside}").unwrap();
    std::os::unix::fs::symlink(outside.path(), directory.path().join("linked")).unwrap();
    let main = directory.path().join("main.tex");
    fs::write(&main, "\\input{linked/outside}").unwrap();
    let index = analyze_project(directory.path(), &main, &BTreeMap::new());
    assert!(index.outline.is_empty());
    assert_eq!(index.unresolved_dependencies.len(), 1);
    assert!(index.warning().is_some());
}

#[test]
fn every_inline_literal_form_hides_commands_but_preserves_following_real_entries() {
    for literal in [
        r"\mintinline{tex}|\section{Fake}\label{fake}\input{missing}|",
        r"\mintinline[escapeinside=||]{tex}{\section{Fake}\label{fake}}",
        r"\mintinline{tex}%\section{Fake}\label{fake}%",
        r"\Verb|\section{Fake}\label{fake}|",
        r"\Verb[formatcom=\color{blue}]|\label{fake}|",
        r"\lstinline{\section{Fake}\label{fake}}",
        r"\lstinline[language=TeX]{\section{Fake}\label{fake}}",
        r"\lstinline{percent % is literal \label{fake}}",
    ] {
        let source = format!("{literal}\n\\section{{Real}}\\label{{real}}\\ref{{real}}");
        let (_, index) = virtual_index(&source, &[]);
        assert_eq!(index.outline.len(), 1, "{literal}");
        assert_eq!(index.outline[0].title, "Real", "{literal}");
        assert_eq!(index.tags.len(), 1, "{literal}");
        assert_eq!(index.tags[0].label, "real", "{literal}");
        assert_eq!(index.reference_indexes_for_tag("real"), [0], "{literal}");
        assert!(index.warning().is_none(), "{literal}");
    }
}

#[test]
fn incomplete_inline_delimiters_stop_at_the_line_break() {
    for command in [r"\verb", r"\Verb", r"\lstinline", r"\mintinline{tex}"] {
        for newline in ["\n", "\r\n"] {
            let source =
                format!("{command}|\\section{{Fake}}{newline}\\section{{Real}} |\\label{{real}}");
            let (_, index) = virtual_index(&source, &[]);
            assert_eq!(index.outline.len(), 1, "{source}");
            assert_eq!(index.outline[0].title, "Real", "{source}");
            assert_eq!(index.tags[0].label, "real", "{source}");
        }
    }
}

#[test]
fn comment_environment_only_closes_on_its_own_line() {
    let (_, index) = virtual_index(
        r"\begin{comment}
Example \end{comment} \section{Fake inline end}
\label{fake}
\end{comment} trailing text
\usepackage{fake}
  \end{comment}
\section{Real}\label{real}",
        &[],
    );
    assert_eq!(index.outline.len(), 1);
    assert_eq!(index.outline[0].title, "Real");
    assert_eq!(index.tags.len(), 1);
    assert_eq!(index.tags[0].label, "real");
    assert!(index.packages.is_empty());
    assert!(index.warning().is_none());
}

#[test]
fn live_heading_contents_keep_inline_labels_and_references_at_original_lines() {
    let (_, index) = virtual_index(
        r"\section[Short]{Introduction\label{sec:intro}
With \emph{detail} and \ref{sec:prior}}
\subsection{Next\label{sec:next}}",
        &[],
    );
    assert_eq!(index.outline.len(), 2);
    assert_eq!(
        index
            .tags
            .iter()
            .map(|tag| (tag.label.as_str(), tag.line))
            .collect::<Vec<_>>(),
        [("sec:intro", 1), ("sec:next", 3)]
    );
    assert_eq!(
        index
            .references
            .iter()
            .map(|reference| (reference.label.as_str(), reference.line))
            .collect::<Vec<_>>(),
        [("sec:prior", 2)]
    );
    assert!(index.warning().is_none());
}

#[test]
fn subfiles_change_nested_input_context_without_changing_the_master_context() {
    let (directory, index) = virtual_index(
        r"\usepackage{subfiles}\subfile{chapters/a}\input{detail}",
        &[
            (
                "chapters/a.tex",
                r"\section{Chapter}\input{detail}\subfile{nested/b}",
            ),
            (
                "chapters/detail.tex",
                r"\subsection{Chapter detail}\label{chapter:detail}",
            ),
            (
                "chapters/nested/b.tex",
                r"\section{Nested}\input{detail}\input{../shared}",
            ),
            ("chapters/nested/detail.tex", r"\label{nested:detail}"),
            ("chapters/shared.tex", r"\label{chapter:shared}"),
            (
                "detail.tex",
                r"\section{Master detail}\label{master:detail}",
            ),
        ],
    );
    let root = directory.path().canonicalize().unwrap();
    for (label, path) in [
        ("chapter:detail", "chapters/detail.tex"),
        ("nested:detail", "chapters/nested/detail.tex"),
        ("chapter:shared", "chapters/shared.tex"),
        ("master:detail", "detail.tex"),
    ] {
        assert_eq!(
            index
                .tags
                .iter()
                .find(|tag| tag.label == label)
                .unwrap()
                .path,
            root.join(path)
        );
    }
    assert_eq!(index.tags.len(), 4);
    assert!(index.warning().is_none());
}

#[test]
fn reusing_one_source_under_conflicting_import_contexts_is_explicitly_partial() {
    let (_, index) = virtual_index(
        r"\input{chapters/a}\subfile{chapters/a}",
        &[
            ("chapters/a.tex", r"\input{detail}"),
            ("chapters/detail.tex", r"\label{chapter}"),
            ("detail.tex", r"\label{master}"),
        ],
    );
    assert_eq!(index.unresolved_dependencies.len(), 1);
    assert!(
        index
            .warning()
            .unwrap()
            .contains("conflicting import contexts")
    );
    // A bounded index visits a source once, and reports the alternate context
    // rather than silently merging two different meanings of its input path.
    assert_eq!(index.tags.len(), 1);
}

#[test]
fn nested_main_inputs_resolve_beside_the_compiler_entry_not_the_workspace_root() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join("paper/main.tex");
    let overrides = BTreeMap::from([
        (main.clone(), r"\input{detail}\input{../shared}".into()),
        (
            directory.path().join("detail.tex"),
            r"\section{Wrong workspace file}\label{wrong}".into(),
        ),
        (
            directory.path().join("paper/detail.tex"),
            r"\section{Entry sibling}\label{right}".into(),
        ),
        (
            directory.path().join("shared.tex"),
            r"\label{shared}".into(),
        ),
    ]);
    let index = analyze_project(directory.path(), &main, &overrides);
    let root = directory.path().canonicalize().unwrap();
    assert_eq!(index.outline.len(), 1);
    assert_eq!(index.outline[0].title, "Entry sibling");
    assert_eq!(index.outline[0].path, root.join("paper/detail.tex"));
    assert_eq!(
        index
            .tags
            .iter()
            .find(|tag| tag.label == "right")
            .unwrap()
            .path,
        root.join("paper/detail.tex")
    );
    assert!(index.tags.iter().all(|tag| tag.label != "wrong"));
    assert_eq!(
        index.subfiles,
        [root.join("paper/detail.tex"), root.join("shared.tex")]
    );
    assert!(index.warning().is_none());
}

#[test]
fn untitled_source_uses_explicit_compiler_directory_instead_of_virtual_identity() {
    let directory = tempfile::tempdir().unwrap();
    let main = directory.path().join(".tiptoptyp/documents/untitled-1.tex");
    let overrides = BTreeMap::from([
        (main.clone(), r"\input{detail}".into()),
        (
            directory.path().join("detail.tex"),
            r"\section{Workspace input}\label{right}".into(),
        ),
        (
            directory.path().join(".tiptoptyp/documents/detail.tex"),
            r"\section{Wrong generated sibling}\label{wrong}".into(),
        ),
    ]);
    let index = analyze_project_cancellable(
        directory.path(),
        &main,
        directory.path(),
        &overrides,
        || false,
    )
    .unwrap();
    assert_eq!(index.outline.len(), 1);
    assert_eq!(index.outline[0].title, "Workspace input");
    assert_eq!(
        index.outline[0].path,
        directory.path().canonicalize().unwrap().join("detail.tex")
    );
    assert_eq!(index.tags[0].label, "right");
    assert!(index.warning().is_none());
}
