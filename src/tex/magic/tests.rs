use super::*;

#[test]
fn canonical_and_texshop_headers_have_typed_values_and_ignore_spacing_case() {
    let header = "\u{feff}%!tEx root = \"chapters/main file.tex\"\r\n% ! TEX TS-program = XeLaTeXmk\r\n%!BIB TS-program = BIBER\r\n% !TeX encoding = uTf8\r\n% !TeX spellcheck = en_gb\r\n\\documentclass{article}\n";
    let parsed = parse(header).unwrap();
    assert_eq!(parsed.root.as_deref(), Some("chapters/main file.tex"));
    assert_eq!(parsed.engine, Some(BuildEngine::XeLatex));
    assert_eq!(parsed.driver, Some(BuildDriver::Latexmk));
    assert_eq!(parsed.bibliography, Some(Bibliography::Biber));
    assert_eq!(parsed.encoding, Some(Encoding::Utf8));
    assert_eq!(
        parsed.spellcheck,
        Some(Spellcheck::Language("en-GB".into()))
    );
}

#[test]
fn programs_are_known_tools_not_executable_paths_or_argument_strings() {
    for (name, engine) in [
        ("tectonic", BuildEngine::Tectonic),
        ("pdflatex", BuildEngine::PdfLatex),
        ("XeLaTeX", BuildEngine::XeLatex),
        ("lualatexmk", BuildEngine::LuaLatex),
    ] {
        assert_eq!(
            parse(&format!("%!TEX program={name}\n")).unwrap().engine,
            Some(engine)
        );
    }
    let driver = parse("%!TEX program=latexmk\n").unwrap();
    assert_eq!(driver.engine, None);
    assert_eq!(driver.driver, Some(BuildDriver::Latexmk));
    for command in [
        "sh",
        "/tmp/pdflatex",
        "pdflatex -shell-escape",
        "$(command)",
        "xelatex;other",
    ] {
        assert!(
            parse(&format!("%!TEX program={command}\n")).is_err(),
            "{command}"
        );
    }
}

#[test]
fn only_the_leading_header_is_active_and_editor_metadata_is_ignored() {
    let source = "% normal comment\n%!TeX author = \"unfinished editor text\n\n\\documentclass{article}\n%!TEX program=sh\n";
    assert_eq!(parse(source).unwrap(), Directives::default());
    assert_eq!(parse("%%!TEX program=sh\n").unwrap(), Directives::default());
    assert_eq!(
        parse("\\begin{verbatim}\n%!TEX root=bad.tex\n\\end{verbatim}").unwrap(),
        Directives::default()
    );
}

#[test]
fn header_work_is_bounded_and_never_interprets_a_truncated_line() {
    let source = format!(
        "{}%!TEX program=sh\n",
        "% comment\n".repeat(MAX_HEADER_LINES)
    );
    assert_eq!(parse(&source).unwrap(), Directives::default());
    let source = format!(
        "% !TEX root={}\n%!TEX program=sh\n",
        "a".repeat(MAX_HEADER_BYTES)
    );
    assert_eq!(parse(&source).unwrap(), Directives::default());
    let source = format!("% {}\n%!TEX program=sh\n", "😀".repeat(MAX_HEADER_BYTES));
    assert_eq!(parse(&source).unwrap(), Directives::default());
}

#[test]
fn texshop_programs_take_precedence_and_conflicting_same_commands_are_errors() {
    let parsed = parse("%!TeX program=pdflatex\n%!TeX TS-program=xelatex\n%!BIB program=bibtex\n%!BIB TS-program=biber\n").unwrap();
    assert_eq!(parsed.engine, Some(BuildEngine::XeLatex));
    assert_eq!(parsed.bibliography, Some(Bibliography::Biber));
    assert!(
        parse("%!TEX root=a.tex\n%!TEX root=b.tex\n")
            .unwrap_err()
            .message
            .contains("Conflicting")
    );
    assert!(parse("%!TEX program=pdflatex\n%!TEX program=xelatex\n").is_err());
    assert!(
        parse("%!TEX encoding=latin1\n")
            .unwrap_err()
            .message
            .contains("UTF-8")
    );
}

#[test]
fn spellcheck_supports_language_variants_and_explicit_disable() {
    for (value, normalized) in [
        ("EN_us", "en-US"),
        ("de-DE", "de-DE"),
        ("zh_hant_tw", "zh-Hant-TW"),
    ] {
        assert_eq!(
            parse(&format!("%!TEX spellcheck={value}\n"))
                .unwrap()
                .spellcheck,
            Some(Spellcheck::Language(normalized.into()))
        );
    }
    assert_eq!(
        parse("%!TEX spellcheck=NONE\n").unwrap().spellcheck,
        Some(Spellcheck::Disabled)
    );
    assert!(parse("%!TEX spellcheck=../en\n").is_err());
}

#[test]
fn arbitrary_options_scripts_and_recipes_are_recognized_but_never_executed() {
    for line in [
        "%!TEX options=-shell-escape %DOC%",
        "%!BIB options=%DOCFILE%",
        "%!TeX TXS-SCRIPT=anything",
        "%!TeX TXS-program:compile=txs:///user0",
        "%!LW recipe=custom",
    ] {
        let parsed = parse(line).unwrap();
        assert_eq!(parsed.unsupported.len(), 1);
        let error = resolve(
            Path::new("/project/main.tex"),
            line,
            Path::new("/project"),
            |_| panic!("unsupported commands never load or run anything"),
        )
        .unwrap_err();
        assert!(error.message.contains("Settings"));
    }
}

#[test]
fn root_chains_are_relative_to_the_declaring_file_and_final_root_controls_the_build() {
    let active = "%!TeX root=../bridge.tex\n%!TeX program=tectonic\n%!TeX spellcheck=en_US\nchild";
    let mut loads = Vec::new();
    let resolved = resolve(
        Path::new("/project/chapters/child.tex"),
        active,
        Path::new("/project"),
        |path| {
            loads.push(path.to_owned());
            match path.to_str().unwrap() {
                "/project/bridge.tex" => Ok("%!TEX root=main.tex\n".into()),
                "/project/main.tex" => Ok("%!TEX program=lualatex\nmain".into()),
                _ => panic!("unexpected path"),
            }
        },
    )
    .unwrap();
    assert_eq!(resolved.root, Path::new("/project/main.tex"));
    assert_eq!(resolved.directives.engine, Some(BuildEngine::LuaLatex));
    assert_eq!(parse(active).unwrap().engine, Some(BuildEngine::Tectonic));
    assert_eq!(
        parse(active).unwrap().spellcheck,
        Some(Spellcheck::Language("en-US".into()))
    );
    assert_eq!(loads.len(), 2);
    assert_eq!(resolved.chain.len(), 3);
}

#[test]
fn self_root_is_a_valid_declaration_but_multi_file_cycles_fail() {
    let resolved = resolve(
        Path::new("/project/main.tex"),
        "%!TEX root=./main.tex\nmain",
        Path::new("/project"),
        |_| panic!("self root must not reload"),
    )
    .unwrap();
    assert_eq!(resolved.chain.len(), 1);
    assert!(matches!(resolved.source, Cow::Borrowed(_)));
    let error = resolve(
        Path::new("/project/a.tex"),
        "%!TEX root=b.tex",
        Path::new("/project"),
        |_| Ok("%!TEX root=a.tex".into()),
    )
    .unwrap_err();
    assert!(error.message.contains("cycle"));
}

#[test]
fn root_paths_cannot_escape_the_project_or_enter_generated_private_data() {
    for root in [
        "../outside.tex",
        "/elsewhere/main.tex",
        ".tiptoptyp/generated.tex",
        ".TIPTOPTYP/generated.tex",
        ".git/internal.tex",
        "figure.pdf",
    ] {
        assert!(
            resolve(
                Path::new("/project/main.tex"),
                &format!("%!TEX root={root}"),
                Path::new("/project"),
                |_| panic!("invalid paths must not be loaded")
            )
            .is_err(),
            "{root}"
        );
    }
}

#[test]
fn class_style_and_alternate_sources_can_resolve_a_typed_magic_root() {
    for extension in crate::document::TEX_SOURCE_EXTENSIONS {
        let child = format!("/project/styles/source.{}", extension.to_ascii_uppercase());
        let resolved = resolve(
            Path::new(&child),
            "%!TEX root=../main.LaTeX\n\\newcommand{\\name}{x}",
            Path::new("/project"),
            |path| {
                assert_eq!(path, Path::new("/project/main.LaTeX"));
                Ok("\\documentclass{article}".into())
            },
        )
        .unwrap();
        assert_eq!(resolved.root, Path::new("/project/main.LaTeX"));
        assert_eq!(resolved.source, "\\documentclass{article}");
    }
    assert!(
        check_path(
            Path::new("/project/.git/generated.cls"),
            Path::new("/project"),
            1
        )
        .is_err()
    );
    assert!(
        check_path(
            Path::new("/project/.tiptoptyp/generated.sty"),
            Path::new("/project"),
            1
        )
        .is_err()
    );
}

#[test]
fn missing_root_and_long_chains_have_explicit_bounded_failures() {
    let missing = resolve(
        Path::new("/project/child.tex"),
        "%!TEX root=main.tex",
        Path::new("/project"),
        |_| Err("file is missing".into()),
    )
    .unwrap_err();
    assert!(missing.message.contains("file is missing"));
    assert_eq!(
        missing.path.as_deref(),
        Some(Path::new("/project/child.tex"))
    );
    let mut loads = 0;
    let error = resolve(
        Path::new("/project/0.tex"),
        "%!TEX root=1.tex",
        Path::new("/project"),
        |_| {
            loads += 1;
            Ok(format!("%!TEX root={}.tex", loads + 1))
        },
    )
    .unwrap_err();
    assert!(error.message.contains("16 files"));
    assert_eq!(loads, MAX_ROOT_CHAIN - 1);
}
