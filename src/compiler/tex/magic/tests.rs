use super::*;

fn context(root: &Path) -> MagicContext {
    let settings = TexSettings::default();
    let mut tools = TexTools::resolve(&settings);
    for engine in [
        BuildEngine::Tectonic,
        BuildEngine::PdfLatex,
        BuildEngine::XeLatex,
        BuildEngine::LuaLatex,
    ] {
        tools.set_distribution_for_test(engine, root.join(engine.executable()));
    }
    MagicContext {
        settings,
        tools,
        overlays: Vec::new(),
    }
}

fn request(root: &Path, source: &str, options: &TexOptions) -> CompileRequest {
    CompileRequest {
        revision: 42,
        input: crate::compiler::CompileInput {
            language: tiptoptyp_core::document::TypesettingLanguage::Tex,
            source: source.into(),
            project_root: root.into(),
            source_dir: root.join("chapters"),
            display_name: "child.tex".into(),
        },
        engine: EngineConfig::Tex(options.clone()),
    }
}

#[test]
fn roots_and_unsaved_overlays_are_resolved_on_the_worker_without_changing_their_files() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("chapters")).unwrap();
    fs::write(
        root.path().join("main.tex"),
        "%!TEX program=pdflatex\ndisk root",
    )
    .unwrap();
    fs::write(root.path().join("chapters/child.tex"), "disk child").unwrap();
    let mut context = context(root.path());
    context.overlays.push((
        root.path().join("main.tex"),
        "%!TEX program=xelatex\nunsaved root".into(),
    ));
    context.overlays.push((
        root.path().join("chapters/child.tex"),
        "stale overlay child".into(),
    ));
    let mut options = TexOptions::configured(&context.settings, &context.tools);
    options.magic = Some(std::sync::Arc::new(context));
    let request = request(
        root.path(),
        "%!TEX root=../main.tex\nactive child",
        &options,
    );
    let prepared = prepare(&request, &options).unwrap();
    assert!(prepared.redirected);
    assert_eq!(
        prepared.entry,
        root.path().canonicalize().unwrap().join("main.tex")
    );
    assert_eq!(prepared.options.engine, BuildEngine::XeLatex);
    assert_eq!(prepared.request.revision, 42);
    assert!(prepared.request.input.source.ends_with("unsaved root"));
    assert!(
        prepared
            .overlays
            .iter()
            .any(|(path, text)| path.ends_with("chapters/child.tex")
                && text.ends_with("active child"))
    );
    assert_eq!(
        fs::read_to_string(root.path().join("main.tex")).unwrap(),
        "%!TEX program=pdflatex\ndisk root"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("chapters/child.tex")).unwrap(),
        "disk child"
    );
}

#[test]
fn no_directives_borrow_existing_options_and_preserve_all_customizations() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("chapters")).unwrap();
    let context = context(root.path());
    let mut options = TexOptions::configured(&context.settings, &context.tools);
    options.only_cached = false;
    options.show_underfull_box_warnings = false;
    options.magic = Some(std::sync::Arc::new(context));
    let request = request(root.path(), "\\documentclass{article}", &options);
    let prepared = prepare(&request, &options).unwrap();
    assert!(matches!(prepared.options, Cow::Borrowed(_)));
    assert!(matches!(prepared.request, Cow::Borrowed(_)));
    assert_eq!(prepared.options.as_ref(), &options);
}

#[test]
fn bibliography_directives_select_the_actual_cached_tool_and_driver() {
    let root = tempfile::tempdir().unwrap();
    let mut context = context(root.path());
    context.settings.build_engine = BuildEngine::PdfLatex;
    context.settings.build_driver = BuildDriver::Direct;
    context.settings.bibliography_enabled = false;
    context.tools.bibtex = Some(root.path().join("custom bibtex"));
    context.tools.biber = Some(root.path().join("custom biber"));
    let options = TexOptions::configured(&context.settings, &context.tools);
    let parsed = directives::parse("%!BIB program=biber").unwrap();
    let configured = effective_options(&options, &context, &parsed).unwrap();
    let system = configured.system.as_ref().unwrap();
    assert_eq!(system.driver, BuildDriver::Latexmk);
    assert!(system.bibliography_enabled);
    assert_eq!(system.bibtex.program, root.path().join("custom biber"));
    assert_eq!(system.biber.program, root.path().join("custom biber"));
    context.settings.build_engine = BuildEngine::Tectonic;
    let tectonic = TexOptions::configured(&context.settings, &context.tools);
    assert!(
        effective_options(&tectonic, &context, &parsed)
            .unwrap_err()
            .contains("Biber needs")
    );
    assert!(
        effective_options(
            &tectonic,
            &context,
            &directives::parse("%!BIB program=bibtex").unwrap()
        )
        .is_ok()
    );
}

#[cfg(unix)]
#[test]
fn root_symlinks_cannot_escape_the_project() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("chapters")).unwrap();
    fs::write(outside.path().join("external.tex"), "outside").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("external.tex"),
        root.path().join("main.tex"),
    )
    .unwrap();
    let context = context(root.path());
    let mut options = TexOptions::configured(&context.settings, &context.tools);
    options.magic = Some(std::sync::Arc::new(context));
    let request = request(root.path(), "%!TEX root=../main.tex", &options);
    let error = prepare(&request, &options).err().unwrap();
    assert!(error.contains("leaves this project"));
    assert!(!root.path().join(".tiptoptyp").exists());
}

#[cfg(unix)]
#[test]
fn root_links_cannot_read_private_editor_or_git_sources() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().canonicalize().unwrap();
    for name in [".git", ".tiptoptyp", ".GIT"] {
        let private = project.join(name);
        fs::create_dir_all(&private).unwrap();
        fs::write(private.join("private.tex"), "private metadata").unwrap();
        let link = project.join("main.tex");
        std::os::unix::fs::symlink(private.join("private.tex"), &link).unwrap();
        assert!(
            source_path(&link, &project)
                .unwrap_err()
                .contains("Private")
        );
        fs::remove_file(link).unwrap();
    }
}
