//! These are dependency rules, not behavioral substitutes for adapter tests.
use std::{
    fs,
    path::{Path, PathBuf},
};

#[test]
fn git_repository_execution_and_codecs_do_not_depend_on_views() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git");
    let mut paths = rust_files(&root.join("repository"));
    paths.push(root.join("repository.rs"));
    for path in paths {
        let source = fs::read_to_string(&path).unwrap();
        for forbidden in [
            "egui",
            "eframe",
            "crate::app",
            "crate::git::editor",
            "super::editor",
            "GitPanel",
            "GitEditorState",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} depends on {forbidden}",
                path.display()
            );
        }
    }
    for path in [
        root.with_extension("rs"),
        root.join("editor.rs"),
        root.join("editor/actions.rs"),
    ] {
        let source = fs::read_to_string(&path).unwrap();
        let production = source.split("#[cfg(test)]\nmod tests").next().unwrap();
        for forbidden in [
            "Command::new",
            "fn run_command",
            "fn parse_status",
            "fn parse_hunks",
            "fn change_index",
            "fn compare_buffer",
        ] {
            assert!(
                !production.contains(forbidden),
                "{} still owns {forbidden}",
                path.display()
            );
        }
    }
}

#[test]
fn git_views_have_no_repository_or_worker_effect_handles() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/git");
    for path in [root.join("view.rs"), root.join("editor/view.rs")] {
        let source = fs::read_to_string(&path).unwrap();
        for forbidden in [
            "Repository",
            "ExclusiveJob",
            "LatestJob",
            "std::fs",
            "std::process",
            "Command::new",
            ".execute(",
            ".status(",
            ".change_index(",
        ] {
            assert!(
                !source.contains(forbidden),
                "{} can execute Git through {forbidden}",
                path.display()
            );
        }
    }
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn launch_policy_and_lsp_transport_have_focused_owners() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let screenshot = fs::read_to_string(root.join("screenshot.rs")).unwrap();
    let production = screenshot.split("#[cfg(test)]\nmod tests").next().unwrap();
    for forbidden in [
        "LaunchOptions",
        "LaunchMode",
        "parse_launch_options",
        "crate::launch",
    ] {
        assert!(
            !production.contains(forbidden),
            "capture renderer owns launch policy: {forbidden}"
        );
    }
    for module in ["tinymist/transport.rs", "tinymist/protocol.rs"] {
        let source = fs::read_to_string(root.join(module)).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "crate::",
            "super::",
            "eframe",
            "egui",
            "std::process",
            "std::thread",
        ] {
            assert!(
                !production.contains(forbidden),
                "{module} depends on application/session code: {forbidden}"
            );
        }
    }
}

#[test]
fn capability_derivation_cannot_probe_or_render() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let capabilities = fs::read_to_string(root.join("capabilities.rs")).unwrap();
    let production = capabilities.split("#[cfg(test)]").next().unwrap();
    for forbidden in [
        "eframe",
        "egui",
        "std::fs",
        "std::process",
        "Command::new",
        "std::env",
    ] {
        assert!(
            !production.contains(forbidden),
            "capability derivation owns effect or UI API {forbidden}"
        );
    }
    for view in ["app/settings_panel.rs", "app/settings_view.rs"] {
        let source = fs::read_to_string(root.join(view)).unwrap();
        assert!(
            !source.contains("resolve_path_program"),
            "{view} probes optional tools while presenting capabilities"
        );
    }
}

#[test]
fn window_code_uses_stable_tab_accessors_instead_of_storage_slots() {
    let app = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    for path in rust_files(&app) {
        let relative = path.strip_prefix(&app).unwrap().to_str().unwrap();
        if matches!(relative, "tabs.rs" | "tabs_tests.rs") {
            continue;
        }
        let source = fs::read_to_string(&path).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap();
        let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
        for field in [".tabs.active", ".tabs.preview"] {
            let direct = compact.match_indices(field).any(|(offset, _)| {
                compact.as_bytes().get(offset + field.len()).copied() != Some(b'_')
            });
            assert!(
                !direct,
                "{relative} accesses positional tab storage through {field}"
            );
        }
        for forbidden in [
            ".tabs.parked",
            ".tabs.records",
            ".tabs.ids[",
            ".tab_document(",
        ] {
            assert!(
                !compact.contains(forbidden),
                "{relative} bypasses stable tab accessors through {forbidden}"
            );
        }
    }
}

#[test]
fn tinymist_sync_policy_is_ui_process_and_thread_independent() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let source = fs::read_to_string(root.join("src/tinymist_sync.rs")).unwrap();
    let production = source.split("#[cfg(test)]").next().unwrap();
    for forbidden in [
        "egui",
        "std::process",
        "std::thread",
        "std::fs",
        "TinymistSidecar",
        "EditorApp",
    ] {
        assert!(
            !production.contains(forbidden),
            "synchronization policy owns adapter dependency {forbidden}"
        );
    }
    assert!(production.contains("enum Effect"));
    assert!(production.contains("struct VersionedInput"));
}

#[test]
fn preview_policy_has_one_transition_owner_and_views_consume_status() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let preview = fs::read_to_string(root.join("preview.rs")).unwrap();
    let production = preview.split("#[cfg(test)]\nmod tests").next().unwrap();
    assert!(production.contains("enum PreviewTransitionEvent"));
    assert!(production.contains("enum PreviewEffect"));
    assert!(production.contains("fn status_snapshot("));
    for forbidden in ["TinymistSidecar", "egui::Context", "request_repaint_after("] {
        assert!(
            !production.contains(forbidden),
            "preview policy owns adapter effect {forbidden}"
        );
    }

    let app = fs::read_to_string(root.join("app.rs")).unwrap();
    let view = app
        .split("fn show_preview(&mut self")
        .nth(1)
        .unwrap()
        .split("fn show_preview_header_controls")
        .next()
        .unwrap();
    assert!(view.contains("preview_status_snapshot()"));
    for forbidden in [
        ".interactive_active(",
        ".interactive_transitioning(",
        ".should_attempt_interactive(",
    ] {
        assert!(
            !view.contains(forbidden),
            "preview view recalculates policy through {forbidden}"
        );
    }
}

#[test]
fn project_index_reads_use_the_specialized_bounded_runner() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let runner = fs::read_to_string(root.join("index_jobs.rs")).unwrap();
    let production = runner.split("#[cfg(test)]").next().unwrap();
    assert!(production.contains("const CONCURRENCY: usize = 2"));
    assert!(production.contains("MAX_PENDING_BYTES"));
    assert!(production.contains("analyze_project_cancellable"));
    for forbidden in ["egui", "ExclusiveJob", "ProcessSupervisor", "Command::new"] {
        assert!(
            !production.contains(forbidden),
            "bounded read runner acquired unrelated responsibility {forbidden}"
        );
    }
    let app = fs::read_to_string(root.join("app.rs")).unwrap();
    assert!(!app.contains("LatestJob<ProjectIndex>"));
    assert!(!app.contains("move || Ok(analyze_project"));
}

#[test]
fn workspace_snapshots_are_shared_and_notifications_replace_frame_polling() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let service = fs::read_to_string(root.join("workspace_service.rs")).unwrap();
    let production = service.split("#[cfg(test)]").next().unwrap();
    assert!(production.contains("Arc<WorkspaceSnapshot>"));
    assert!(production.contains("EVENT_QUIET_PERIOD"));
    assert!(production.contains("VERIFICATION_INTERVAL"));
    assert!(production.contains("ModifyKind::Data"));
    assert!(!production.contains("fs::read("));

    let app = fs::read_to_string(root.join("app.rs")).unwrap();
    assert!(!app.contains("WORKSPACE_REFRESH_INTERVAL"));
    assert!(!app.contains("EXTERNAL_FILE_CHECK_INTERVAL"));
    assert!(!app.contains("workspace_scan: LatestJob"));
}

#[test]
fn child_resources_follow_explicit_lifecycle_and_native_properties_are_diffed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let child = fs::read_to_string(root.join("child_view.rs")).unwrap();
    for required in [
        "TemporarilyHidden",
        "DurablyClosed",
        "DormantHosted",
        "callback_is_live",
        "dispose_viewport",
    ] {
        assert!(
            child.contains(required),
            "missing child lifecycle {required}"
        );
    }
    let native = fs::read_to_string(root.join("app/native_views.rs")).unwrap();
    assert!(native.contains("webview_property_diff"));
    assert!(native.contains("webview_applied"));
    let stable_setters = native
        .split("let diff = webview_property_diff")
        .nth(1)
        .unwrap()
        .split("if self.preview.tinymist_state.is_ready()")
        .next()
        .unwrap();
    assert!(stable_setters.contains("if diff.background"));
    assert!(stable_setters.contains("if diff.bounds"));
    assert!(stable_setters.contains("if diff.visible"));
}

#[test]
fn core_has_no_platform_dependencies_or_effect_apis() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: toml::Value = fs::read_to_string(root.join("core/Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let dependencies = manifest["dependencies"].as_table().unwrap();
    assert_eq!(
        dependencies.keys().map(String::as_str).collect::<Vec<_>>(),
        ["serde"]
    );
    for path in rust_files(&root.join("core/src")) {
        let source = fs::read_to_string(&path).unwrap();
        // Tokenize identifiers/punctuation so whitespace cannot bypass the rule.
        // Deliberately conservative: effects named in comments also merit review.
        let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
        for forbidden in [
            "std::fs",
            "std::process",
            "std::thread",
            "std::env",
            "Instant::now(",
            "SystemTime::now(",
            "Command::new(",
            "include_bytes!(",
        ] {
            assert!(
                !compact.contains(forbidden),
                "{} uses forbidden core effect {forbidden}",
                path.display()
            );
        }
        // Grouped `use std::{fs, ...}` imports must not evade qualified-path checks.
        for token in source.split(|c: char| !c.is_alphanumeric() && c != '_') {
            assert!(
                !["fs", "process", "thread", "env", "eframe", "wry", "rfd"].contains(&token),
                "{} contains platform identifier {token}",
                path.display()
            );
        }
    }
}

#[test]
fn pdf_service_is_shared_without_compiler_or_view_ownership() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let pdf = fs::read_to_string(root.join("pdf.rs")).unwrap();
    for forbidden in [
        "crate::compiler",
        "eframe",
        "egui",
        "CompileRequest",
        "CompileArtifact",
    ] {
        assert!(
            !pdf.contains(forbidden),
            "PDF service depends on {forbidden}"
        );
    }
    let asset = fs::read_to_string(root.join("asset.rs")).unwrap();
    assert!(!asset.contains("compiler::"));
    let compiler = fs::read_to_string(root.join("compiler.rs")).unwrap();
    for forbidden in [
        "fn rasterize_pdf",
        "fn parse_pdf_links",
        "quick_xml",
        "image::load_from_memory",
    ] {
        assert!(
            !compiler.contains(forbidden),
            "compiler still owns {forbidden}"
        );
    }
}

#[test]
fn pdf_pixels_are_viewport_requested_and_process_budgeted() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let compiler = fs::read_to_string(root.join("compiler.rs")).unwrap();
    assert!(compiler.contains("PdfDocumentCatalog"));
    assert!(!compiler.contains("Vec<PreviewPage>"));
    assert!(!compiler.contains("PdfRasterMode"));

    let pages = fs::read_to_string(root.join("pdf_pages.rs")).unwrap();
    for required in [
        "MAX_PAGES_PER_REQUEST",
        "ADJACENT_PAGE_PREFETCH",
        "PdfRasterMode::PageRange",
        "latest_token",
        "bounded_prefetch_range",
    ] {
        assert!(
            pages.contains(required),
            "missing PDF demand rule {required}"
        );
    }

    let residency = fs::read_to_string(root.join("pdf_residency.rs")).unwrap();
    for required in [
        "DECODED_PIXEL_BUDGET",
        "TEXTURE_BUDGET",
        "static BUDGET",
        "set_owner_visible",
        "oversized",
    ] {
        assert!(
            residency.contains(required),
            "missing PDF residency rule {required}"
        );
    }
}

#[test]
fn save_handoff_does_not_own_io_ui_or_another_document_identity() {
    let source = include_str!("../src/save_transaction.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    for forbidden in [
        "std::fs",
        "std::thread",
        "eframe",
        "egui",
        "DocumentKey",
        "struct SaveReceipt",
        "struct SaveRequest",
        ".to_owned()",
        ".to_vec()",
    ] {
        assert!(
            !source.contains(forbidden),
            "save handoff duplicates ownership via {forbidden}"
        );
    }
}

#[test]
fn viewport_creation_and_font_installation_stay_in_rendering_adapters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in rust_files(&root) {
        let relative = path.strip_prefix(&root).unwrap().to_str().unwrap();
        let compact: String = fs::read_to_string(&path)
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if compact.contains(".show_viewport_immediate(") {
            assert_eq!(
                relative, "viewport_fonts.rs",
                "Immediate viewports must synchronize the shared font atlas"
            );
        }
        if compact.contains(".set_fonts(") {
            assert!(
                ["theme.rs", "viewport_fonts.rs", "font_preview.rs"].contains(&relative),
                "{relative} bypasses context font ownership"
            );
        }
    }
}

#[test]
fn focused_ui_modules_cannot_take_the_whole_application_or_run_its_services() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    for name in ["settings_panel.rs", "tooltips.rs", "completion_popup.rs"] {
        let source = fs::read_to_string(root.join(name)).unwrap();
        let production = source.split("#[cfg(test)]").next().unwrap();
        let compact: String = production.chars().filter(|c| !c.is_whitespace()).collect();
        for forbidden in [
            "usesuper::*",
            "implEditorApp",
            "&mutEditorApp",
            "&EditorApp",
            "resolve_tool(",
            ".restart_tinymist(",
            ".document.",
        ] {
            assert!(
                !compact.contains(forbidden),
                "{name} crosses ownership boundary: {forbidden}"
            );
        }
    }
}

#[test]
fn screenshot_fixtures_have_one_owner_and_git_has_no_capture_only_window() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let app = fs::read_to_string(root.join("app.rs")).unwrap();
    for forbidden in [
        "struct SceneDocument",
        "snapshot_font_file:",
        "fn apply_snapshot_scene",
        "git_child_window_visible",
        "tiptoptyp-git\"",
    ] {
        assert!(
            !app.contains(forbidden),
            "application regained QA-only state: {forbidden}"
        );
    }
    let views = fs::read_to_string(root.join("app/native_views.rs")).unwrap();
    assert!(!views.contains("show_git_window"));
    assert!(
        fs::read_to_string(root.join("app/qa.rs"))
            .unwrap()
            .contains("struct QaSession")
    );
}
