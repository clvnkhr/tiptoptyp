//! Backing-file ordering tests use the real protocol worker and a local LSP
//! stand-in; no installed Tinymist, browser, GUI or network is required.
use super::*;
use std::{fs, sync::mpsc};

pub(crate) fn await_event(
    sidecar: &TinymistSidecar,
    mut accept: impl FnMut(&TinymistEvent) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while let Some(event) = sidecar.try_recv() {
            if accept(&event) {
                return;
            }
            if let TinymistEvent::Error {
                fatal: true,
                message,
                ..
            } = event
            {
                panic!("unexpected worker failure: {message}");
            }
        }
        assert!(
            Instant::now() < deadline,
            "worker did not complete its operation"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
pub(crate) fn server(directory: &Path, backing: &Path) -> (TinymistConfig, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let script = directory.join("backing-lsp.sh");
    let captured = directory.join("requests.txt");
    let formatted = directory.join("format-source.typ");
    let initialized = serde_json::to_string(&json!({
        "jsonrpc":"2.0", "id":1,
        "result":{"capabilities":{"textDocumentSync":1}}
    }))
    .unwrap();
    fs::write(&script, format!(
        "#!/bin/sh\nprintf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\nwhile IFS= read -r line || [ -n \"$line\" ]; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in *textDocument/formatting*) cat '{}' > '{}' ;; esac\ndone\n",
        initialized.len(), initialized, captured.display(), backing.display(), formatted.display()
    )).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let mut config =
        TinymistConfig::new(directory).with_command(script, std::iter::empty::<OsString>());
    config.start_preview = false;
    (config, captured, formatted)
}

#[test]
fn backed_sync_shares_one_source_allocation_and_rejects_stale_generation() {
    let project = tempfile::tempdir().unwrap();
    let backing =
        UnsavedTextDocument::create(project.path(), project.path(), "Untitled.typ", "initial")
            .unwrap();
    let (commands, receiver) = mpsc::channel();
    let (_events, events) = mpsc::channel();
    let sidecar = TinymistSidecar {
        commands: Some(commands),
        events,
        disconnected: AtomicBool::new(false),
        worker: None,
        next_generation: AtomicU64::new(2),
        current_generation: Arc::new(AtomicU64::new(1)),
        process_supervisor: Arc::default(),
        worker_done: None,
    };
    let source = "Canonical #mi(`\\alpha`) and 😀".to_owned();
    let pointer = source.as_ptr();
    sidecar
        .did_change(
            Generation(1),
            backing.uri.clone(),
            2,
            source,
            vec![backing.clone()],
        )
        .unwrap();
    let WorkerCommand::DidChange { text, backings, .. } = receiver.recv().unwrap() else {
        panic!("wrong worker command");
    };
    assert_eq!(
        text.as_ptr(),
        pointer,
        "backing and LSP must share the moved String"
    );
    assert_eq!(backings[0].path(), backing.path());
    assert_eq!(
        fs::read_to_string(backing.path()).unwrap(),
        "initial",
        "admission must not perform file IO"
    );
    assert!(matches!(
        sidecar.did_change(
            Generation(99),
            backing.uri.clone(),
            3,
            "stale",
            vec![backing]
        ),
        Err(TinymistError::StaleGeneration { .. })
    ));
    assert!(receiver.try_recv().is_err());
}

#[cfg(unix)]
#[test]
fn initial_open_owns_the_mirror_before_any_edit_until_close() {
    let project = tempfile::tempdir().unwrap();
    let backing = UnsavedTextDocument::create(
        project.path(),
        project.path(),
        "Untitled.typ",
        "placeholder",
    )
    .unwrap();
    let path = backing.path().to_owned();
    let uri = backing.uri().to_owned();
    let canonical = "Canonical #mi(`\\alpha`) and 😀.\n";
    let (config, captured, formatted) = server(project.path(), &path);
    let sidecar = TinymistSidecar::new(crate::worker::RepaintTarget::test());
    let generation = sidecar.start_workspace(config).unwrap();
    sidecar
        .did_open(
            generation,
            backing.text_document(7, canonical),
            vec![backing.clone()],
        )
        .unwrap();
    // No edit, update command or UI owner is needed to keep this queued/open
    // source alive. didOpen refreshes its backing before LSP publication.
    drop(backing);
    await_event(&sidecar, |event| {
        matches!(event, TinymistEvent::Initialized { .. })
    });
    sidecar.format_document(generation, &uri, 7, 17).unwrap();
    sidecar
        .set_preview_refresh(generation, PreviewRefresh::OnSave)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while fs::read_to_string(&formatted).ok().as_deref() != Some(canonical) {
        assert!(
            Instant::now() < deadline,
            "initial-open backing was unavailable to formatting"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::read_to_string(&path).unwrap(), canonical);
    let captured = fs::read_to_string(captured).unwrap();
    assert!(captured.contains("textDocument/didOpen"));
    assert!(!captured.contains("textDocument/didChange"));
    sidecar.did_close(generation, &uri).unwrap();
    sidecar.stop_workspace(generation).unwrap();
    await_event(&sidecar, |event| {
        matches!(event, TinymistEvent::Stopped { .. })
    });
    drop(sidecar);
    assert!(
        !path.exists(),
        "the mirror must retire after its queued close"
    );
}

#[cfg(unix)]
#[test]
fn slow_backing_writes_do_not_block_admission_and_formatting_sees_latest_bytes() {
    use std::os::unix::fs::MetadataExt;
    let project = tempfile::tempdir().unwrap();
    fs::create_dir(project.path().join("chapters")).unwrap();
    fs::write(project.path().join("chapters/removed.typ"), "remove").unwrap();
    let backing = UnsavedTextDocument::create(
        project.path(),
        project.path().join("chapters"),
        "Untitled.typ",
        "initial",
    )
    .unwrap();
    let path = backing.path().to_owned();
    let uri = backing.uri.clone();
    let inode = fs::metadata(&path).unwrap().ino();
    let (entered, entered_rx) = mpsc::channel();
    let (release, release_rx) = mpsc::channel();
    let (written, written_rx) = mpsc::channel();
    let mut first = true;
    let caller = thread::current().id();
    let sidecar = TinymistSidecar::with_backing_writer(
        crate::worker::RepaintTarget::test(),
        move |document, source| {
            assert_ne!(thread::current().id(), caller);
            if first {
                first = false;
                entered.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
            }
            document.update_backing_source(source)?;
            written.send(source.to_owned()).unwrap();
            Ok(())
        },
    );
    let (config, captured, formatted) = server(project.path(), &path);
    let generation = sidecar.start_workspace(config).unwrap();
    sidecar
        .did_open(generation, backing.text_document(1, "initial"), Vec::new())
        .unwrap();
    await_event(&sidecar, |event| {
        matches!(event, TinymistEvent::Initialized { .. })
    });
    sidecar
        .did_change(generation, &uri, 2, "first", vec![backing.clone()])
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    // These calls finish while the worker remains blocked inside the first IO.
    // They do not spawn another writer or wait for a formatting barrier.
    let latest = "Canonical #mi(`\\alpha`) and 😀";
    sidecar
        .did_change(
            generation,
            &uri,
            1,
            "rejected stale source",
            vec![backing.clone()],
        )
        .unwrap();
    for (version, text) in [(3, "superseded three"), (4, "superseded four"), (5, latest)] {
        sidecar
            .did_change(generation, &uri, version, text, vec![backing.clone()])
            .unwrap();
    }
    sidecar.format_document(generation, &uri, 5, 17).unwrap();
    sidecar
        .set_preview_refresh(generation, PreviewRefresh::OnSave)
        .unwrap();
    assert!(written_rx.try_recv().is_err(), "writer is still blocked");
    assert!(sidecar.try_recv().is_none());
    fs::remove_file(project.path().join("chapters/removed.typ")).unwrap();
    fs::write(project.path().join("chapters/new.typ"), "new sibling").unwrap();
    drop(backing);
    assert!(path.exists(), "queued command retains the mirror");
    release.send(()).unwrap();
    assert_eq!(
        written_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        "first"
    );
    assert_eq!(
        written_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
        latest
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while fs::read_to_string(&formatted).ok().as_deref() != Some(latest) {
        assert!(
            Instant::now() < deadline,
            "formatting did not observe the latest backing bytes"
        );
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(
        fs::read_to_string(path.parent().unwrap().join("new.typ")).unwrap(),
        "new sibling"
    );
    assert!(!path.parent().unwrap().join("removed.typ").exists());
    let captured = fs::read_to_string(captured).unwrap();
    assert!(captured.contains("textDocument/formatting"));
    assert!(!captured.contains("rejected stale source"));
    assert!(!captured.contains("superseded three"));
    assert!(!captured.contains("superseded four"));
    sidecar.did_close(generation, &uri).unwrap();
    sidecar.stop_workspace(generation).unwrap();
    await_event(&sidecar, |event| {
        matches!(event, TinymistEvent::Stopped { .. })
    });
    drop(sidecar);
    assert!(!path.exists(), "mirror is cleaned after queued close/stop");
}

#[cfg(unix)]
#[test]
fn backing_failure_prevents_later_sync_and_formatting() {
    let project = tempfile::tempdir().unwrap();
    let backing =
        UnsavedTextDocument::create(project.path(), project.path(), "Untitled.typ", "initial")
            .unwrap();
    let (config, captured, _) = server(project.path(), backing.path());
    let (release, permit) = mpsc::channel();
    let sidecar = TinymistSidecar::with_backing_writer(
        crate::worker::RepaintTarget::test(),
        move |document, _| {
            permit.recv_timeout(Duration::from_secs(10)).unwrap();
            Err(TinymistError::PrivateDocument {
                project_root: document.project_root.clone(),
                message: "injected backing write failure".into(),
            })
        },
    );
    let generation = sidecar.start_workspace(config).unwrap();
    let uri = backing.uri.clone();
    sidecar
        .did_open(generation, backing.text_document(1, "initial"), Vec::new())
        .unwrap();
    await_event(&sidecar, |event| {
        matches!(event, TinymistEvent::Initialized { .. })
    });
    sidecar
        .did_change(generation, &uri, 2, "must not publish", vec![backing])
        .unwrap();
    sidecar.format_document(generation, &uri, 2, 17).unwrap();
    release.send(()).unwrap();
    await_event(&sidecar, |event| {
        matches!(
            event,
            TinymistEvent::Error {
                stage: "backing",
                fatal: true,
                ..
            }
        )
    });
    await_event(&sidecar, |event| {
        matches!(event, TinymistEvent::Stopped { .. })
    });
    drop(sidecar);
    let captured = fs::read_to_string(captured).unwrap();
    assert!(!captured.contains("must not publish"));
    assert!(!captured.contains("textDocument/formatting"));
}

#[test]
fn coalescing_is_bounded_and_preserves_each_ordering_barrier() {
    fn change(generation: u64, uri: &str, version: i32) -> WorkerCommand {
        WorkerCommand::DidChange {
            generation: Generation(generation),
            uri: uri.into(),
            version,
            text: version.to_string(),
            backings: Vec::new(),
        }
    }
    for barrier in [
        WorkerCommand::DidOpen {
            generation: Generation(1),
            document: TextDocument::typst("file:///main.typ", 4, "opened"),
            backings: Vec::new(),
        },
        WorkerCommand::FormatDocument {
            generation: Generation(1),
            uri: "file:///main.typ".into(),
            version: 3,
            request_token: 17,
        },
        WorkerCommand::DidClose {
            generation: Generation(1),
            uri: "file:///main.typ".into(),
        },
        WorkerCommand::Stop {
            generation: Generation(1),
        },
        change(1, "file:///other.typ", 4),
        change(2, "file:///main.typ", 4),
        change(1, "file:///main.typ", 3),
        change(1, "file:///main.typ", 1),
    ] {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(change(1, "file:///main.typ", 3))
            .unwrap_or_else(|_| panic!("test queue closed"));
        sender
            .send(barrier)
            .unwrap_or_else(|_| panic!("test queue closed"));
        sender
            .send(change(1, "file:///main.typ", 5))
            .unwrap_or_else(|_| panic!("test queue closed"));
        let mut pending = None;
        let command = coalesce_adjacent_changes(
            change(1, "file:///main.typ", 2),
            Some(1),
            &receiver,
            &mut pending,
        );
        assert!(matches!(
            command,
            WorkerCommand::DidChange { version: 3, .. }
        ));
        assert!(
            pending.is_some(),
            "barrier must be retained for the next worker iteration"
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            WorkerCommand::DidChange { version: 5, .. }
        ));
    }
    let (sender, receiver) = mpsc::channel();
    for version in 3..=100 {
        sender
            .send(change(1, "file:///main.typ", version))
            .unwrap_or_else(|_| panic!("test queue closed"));
    }
    let mut pending = None;
    let command = coalesce_adjacent_changes(
        change(1, "file:///main.typ", 2),
        Some(1),
        &receiver,
        &mut pending,
    );
    assert!(matches!(
        command,
        WorkerCommand::DidChange { version: 65, .. }
    ));
    assert!(pending.is_none());
    assert!(matches!(
        receiver.try_recv().unwrap(),
        WorkerCommand::DidChange { version: 66, .. }
    ));
    // An initially invalid command itself must not disappear into a valid one.
    let command = coalesce_adjacent_changes(
        change(1, "file:///main.typ", 1),
        Some(1),
        &receiver,
        &mut pending,
    );
    assert!(matches!(
        command,
        WorkerCommand::DidChange { version: 1, .. }
    ));
    assert!(matches!(
        receiver.try_recv().unwrap(),
        WorkerCommand::DidChange { version: 67, .. }
    ));
}

#[test]
fn a_replacement_mirror_owner_is_a_coalescing_barrier() {
    let project = tempfile::tempdir().unwrap();
    let old =
        UnsavedTextDocument::create(project.path(), project.path(), "Untitled.typ", "old").unwrap();
    let replacement =
        UnsavedTextDocument::create(project.path(), project.path(), "Untitled.typ", "new").unwrap();
    let (sender, receiver) = mpsc::channel();
    sender
        .send(WorkerCommand::DidChange {
            generation: Generation(1),
            uri: "file:///main.typ".into(),
            version: 3,
            text: "new".into(),
            backings: vec![replacement],
        })
        .unwrap_or_else(|_| panic!("test queue closed"));
    let mut pending = None;
    let command = coalesce_adjacent_changes(
        WorkerCommand::DidChange {
            generation: Generation(1),
            uri: "file:///main.typ".into(),
            version: 2,
            text: "old".into(),
            backings: vec![old],
        },
        Some(1),
        &receiver,
        &mut pending,
    );
    assert!(matches!(
        command,
        WorkerCommand::DidChange { version: 2, .. }
    ));
    assert!(matches!(
        pending,
        Some(WorkerCommand::DidChange { version: 3, .. })
    ));
}

#[test]
#[ignore = "optimized temporary-backing baseline versus UI queue admission; excludes worker completion"]
fn backing_update_admission_cost_probe() {
    assert!(
        !std::hint::black_box(cfg!(debug_assertions)),
        "run with --release"
    );
    const SAMPLES: usize = 80;
    const WARMUP: usize = 8;
    let project = tempfile::tempdir().unwrap();
    for index in 0..256 {
        fs::write(
            project.path().join(format!("sibling-{index}.typ")),
            "#let value = 1\n",
        )
        .unwrap();
    }
    let source = "Canonical prose #mi(`\\alpha`) and 😀.\n".repeat(2048);
    let backing =
        UnsavedTextDocument::create(project.path(), project.path(), "Untitled.typ", &source)
            .unwrap();
    let (commands, receiver) = mpsc::channel();
    let (_events, events) = mpsc::channel();
    let sidecar = TinymistSidecar {
        commands: Some(commands),
        events,
        disconnected: AtomicBool::new(false),
        worker: None,
        next_generation: AtomicU64::new(2),
        current_generation: Arc::new(AtomicU64::new(1)),
        process_supervisor: Arc::default(),
        worker_done: None,
    };
    let mut legacy = Vec::new();
    let mut admission = Vec::new();
    for index in 0..WARMUP + SAMPLES {
        let started = Instant::now();
        // Reconstruct the previous UI path, including its disk durability wait.
        // The actual before/after paths share the same optimized test binary.
        let text = source.clone();
        backing.update_backing_source(&text).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(backing.path())
            .unwrap()
            .sync_data()
            .unwrap();
        let old = started.elapsed().as_secs_f64() * 1_000_000.0;
        let started = Instant::now();
        let text = source.clone();
        sidecar
            .did_change(
                Generation(1),
                backing.uri.clone(),
                index as i32 + 1,
                text,
                vec![backing.clone()],
            )
            .unwrap();
        let new = started.elapsed().as_secs_f64() * 1_000_000.0;
        std::hint::black_box(receiver.recv().unwrap());
        if index >= WARMUP {
            legacy.push(old);
            admission.push(new);
        }
    }
    for (name, mut samples) in [
        ("legacy_blocking", legacy),
        ("ordered_admission", admission),
    ] {
        samples.sort_by(f64::total_cmp);
        println!(
            "backing_update path={name} profile=release os={} arch={} fixture_bytes={} lines=2048 siblings=256 warmup={WARMUP} samples={SAMPLES} p50_us={:.3} p95_us={:.3}; admission_excludes_worker_completion=true",
            std::env::consts::OS,
            std::env::consts::ARCH,
            source.len(),
            samples[SAMPLES / 2],
            samples[SAMPLES * 95 / 100]
        );
    }
}
