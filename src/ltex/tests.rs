use super::*;
use tiptoptyp_core::document::WindowSessionId;

fn snapshot(revision: u64, source: &str) -> LtexSnapshot {
    LtexSnapshot {
        key: DocumentKey::new(WindowSessionId::new(7), 2, revision),
        uri: "file:///tmp/paper.tex".into(),
        language_id: "latex".into(),
        writing_language: crate::settings::WritingLanguage::Auto,
        source: Arc::from(source),
    }
}
fn raw_diagnostic(start: u32, end: u32) -> Value {
    json!({"range":{"start":{"line":0,"character":start},"end":{"line":0,"character":end}},"message":"Use 'is'.","code":"SUBJECT_VERB_AGREEMENT","severity":2})
}
fn action(uri: &str, diagnostic: &Value, replacement: &str) -> Value {
    json!({"kind":"quickfix","edit":{"documentChanges":[{"textDocument":{"uri":uri,"version":1},"edits":[{"range":diagnostic["range"],"newText":replacement}]}]}})
}
#[test]
fn settings_are_explicit_local_and_validate_types_and_owned_mode() {
    let settings = LtexSettings::default();
    settings.validate().unwrap();
    assert!(!settings.enabled);
    assert!(
        settings
            .configuration
            .get("languageToolHttpServerUri")
            .is_none()
    );
    for (field, value) in [
        ("checkFrequency", json!("edit")),
        ("checkFrequency", json!("nope")),
        ("language", json!(3)),
        ("language", json!("xx-Garbage")),
        ("latex", json!({"commands":{"\\foo{}":"perhaps"}})),
        ("dictionary", json!({"en-GB":"word"})),
        ("diagnosticSeverity", json!({"default":"urgent"})),
        ("completionEnabled", json!("yes")),
        ("languageToolHttpServerUri", json!("file:///tmp/server")),
    ] {
        let mut changed = settings.clone();
        changed.configuration[field] = value;
        assert!(changed.validate().unwrap_err().contains(field), "{field}");
    }
    let encoded = serde_json::to_value(&settings).unwrap();
    assert_eq!(
        serde_json::from_value::<LtexSettings>(encoded).unwrap(),
        settings
    );
    assert!(serde_json::from_value::<LtexSettings>(json!({"retiredSetting":true})).is_err());
    let mut remote = settings;
    remote.configuration["languageToolHttpServerUri"] = "https://example.com/v2".into();
    remote.validate().unwrap();
}
#[test]
fn academic_defaults_are_visible_and_preserve_spelling_and_grammar() {
    let settings = LtexSettings::default();
    settings.validate().unwrap();
    assert!(!settings.enabled);
    assert!(settings.follow_document_language);
    assert_eq!(settings.configuration["language"], "en-GB");
    assert_eq!(settings.configuration["checkFrequency"], "manual");
    assert_eq!(settings.configuration["completionEnabled"], false);
    assert_eq!(
        settings.configuration["additionalRules"]["enablePickyRules"],
        false
    );
    assert_eq!(settings.configuration["ltex-ls"]["logLevel"], "warning");
    assert_eq!(settings.effective_configuration(), settings.configuration);
    for language in ["en-GB", "en-US"] {
        let disabled = settings.configuration["disabledRules"][language]
            .as_array()
            .unwrap();
        assert_eq!(disabled.len(), 11);
        for rule in [
            "WHITESPACE_RULE",
            "TOO_LONG_SENTENCE",
            "EN_PLAIN_ENGLISH_REPLACE",
        ] {
            assert!(disabled.contains(&json!(rule)));
        }
        for rule in [
            "MORFOLOGIK_RULE_EN_GB",
            "MORFOLOGIK_RULE_EN_US",
            "EN_A_VS_AN",
            "ENGLISH_WORD_REPEAT_RULE",
            "SUBJECT_VERB_AGREEMENT",
            "UPPERCASE_SENTENCE_START",
            "EN_UNPAIRED_BRACKETS",
            "EN_UNPAIRED_QUOTES",
        ] {
            assert!(!disabled.contains(&json!(rule)), "{language}: {rule}");
        }
        let words = settings.configuration["dictionary"][language]
            .as_array()
            .unwrap();
        for word in include_str!("../../assets/dictionaries/mathematics.txt").lines() {
            assert!(words.contains(&json!(word)), "{language}: {word}");
        }
        assert!(words.contains(&json!("well-posedness")));
        assert!(words.contains(&json!("well\u{2010}posedness")));
        assert!(words.contains(&json!("well\u{2011}posedness")));
        assert!(!words.contains(&json!("posedness")));
        assert!(!words.contains(&json!("well-posedniss")));
    }
    assert_eq!(
        settings.configuration["dictionary"]["en-GB"],
        settings.configuration["dictionary"]["en-US"]
    );
}

#[test]
fn edited_ltex_profile_round_trips_without_hidden_dictionary_or_rule_additions() {
    let mut settings = LtexSettings::default();
    settings.configuration["dictionary"] = json!({"en-GB":["Romito"]});
    settings.configuration["disabledRules"] = json!({"en-GB":["EN_DASH_RULE"]});
    settings.configuration["language"] = "fr-FR".into();
    settings.follow_document_language = false;
    let encoded = serde_json::to_value(&settings).unwrap();
    let restored: LtexSettings = serde_json::from_value(encoded).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored.effective_configuration(), settings.configuration);
    assert_eq!(
        restored.language_for_snapshot(&snapshot(1, "This is a result.")),
        "fr-FR"
    );
    let error = serde_json::from_value::<LtexSettings>(json!({"follow_document_language":"yes"}))
        .unwrap_err();
    assert!(error.to_string().contains("boolean"));
}

#[test]
fn ltex_follows_effective_document_language_with_uk_fallback() {
    use crate::settings::WritingLanguage;
    let settings = LtexSettings::default();
    for (language_id, source, preference, expected) in [
        (
            "typst",
            "#set text(lang: \"en\", region: \"us\")\nThis is a result.",
            WritingLanguage::Auto,
            "en-US",
        ),
        (
            "typst",
            "#set text(lang: \"en-GB\")\nThis is a result.",
            WritingLanguage::Auto,
            "en-GB",
        ),
        (
            "typst",
            "#set text(lang: \"fr\")\nThis is a result.",
            WritingLanguage::Auto,
            "fr-FR",
        ),
        (
            "typst",
            "#set text(lang: \"de\")\nDies ist ein Ergebnis.",
            WritingLanguage::Auto,
            "de-DE",
        ),
        (
            "typst",
            "#set text(lang: \"de\", region: \"at\")\nDies ist ein Ergebnis.",
            WritingLanguage::Auto,
            "de-AT",
        ),
        (
            "typst",
            "#set text(lang: \"fr\", region: \"ca\")\nCe résultat est correct.",
            WritingLanguage::Auto,
            "fr-CA",
        ),
        (
            "typst",
            "#set text(lang: \"fr\")\nCe résultat est correct.",
            WritingLanguage::American,
            "fr-FR",
        ),
        (
            "typst",
            "#set text(lang: \"xx-invalid\")\nThis is a result.",
            WritingLanguage::Auto,
            "en-GB",
        ),
        ("typst", "This is a result.", WritingLanguage::Auto, "en-GB"),
        (
            "typst",
            "// #set text(lang: \"en\", region: \"us\")\nThis is a result.",
            WritingLanguage::Auto,
            "en-GB",
        ),
        (
            "typst",
            "#let example() = { set text(lang: \"en\", region: \"us\") }\nThis is a result.",
            WritingLanguage::Auto,
            "en-GB",
        ),
        (
            "typst",
            "#set text(lang: \"en-US\")\nThis is a result.",
            WritingLanguage::British,
            "en-GB",
        ),
        (
            "typst",
            "#set text(lang: \"en-GB\")\nThis is a result.",
            WritingLanguage::American,
            "en-US",
        ),
        ("latex", "This is a result.", WritingLanguage::Auto, "en-GB"),
        (
            "latex",
            "This is a result.",
            WritingLanguage::American,
            "en-US",
        ),
    ] {
        let mut request = snapshot(1, source);
        request.language_id = language_id.into();
        request.writing_language = preference;
        assert_eq!(
            settings.language_for_snapshot(&request),
            expected,
            "{source}"
        );
    }
    assert_eq!(settings.configuration["language"], "en-GB");
}

#[test]
fn missing_or_unsupported_document_language_keeps_configured_fallback() {
    let mut settings = LtexSettings::default();
    settings.configuration["language"] = "de-DE".into();
    let mut request = snapshot(1, "#set text(lang: \"xx-invalid\")\nText.");
    request.language_id = "typst".into();
    assert_eq!(settings.language_for_snapshot(&request), "de-DE");
    request.source = Arc::from("Text.");
    assert_eq!(settings.language_for_snapshot(&request), "de-DE");
    request.source = Arc::from("#set text(lang: \"fr\")\nTexte.");
    assert_eq!(settings.language_for_snapshot(&request), "fr-FR");
    assert_eq!(settings.configuration["language"], "de-DE");
}

#[test]
fn explicit_ltex_language_override_ignores_document_and_shared_language() {
    let mut settings = LtexSettings {
        follow_document_language: false,
        ..Default::default()
    };
    let mut request = snapshot(1, "#set text(lang: \"en-US\")\nThis is a result.");
    request.language_id = "typst".into();
    request.writing_language = crate::settings::WritingLanguage::American;
    for language in ["en-GB", "en-US", "fr-FR", "auto"] {
        settings.configuration["language"] = language.into();
        settings.validate().unwrap();
        assert_eq!(settings.language_for_snapshot(&request), language);
        assert_eq!(settings.configuration["language"], language);
    }
}

#[test]
fn configuration_pulls_keep_nested_language_maps_and_sections() {
    let settings = json!({"language":"en-US","dictionary":{"en-US":["semigroup"]},"latex":{"commands":{"\\foo{}":"ignore"}}});
    assert_eq!(
        configuration_response(
            &settings,
            &json!({"items":[{"section":"ltex"},{"section":"ltex.language"},{"section":"ltex.dictionary"},{"section":"ltex.latex.commands"},{"section":"missing"}]})
        ),
        json!([settings,"en-US",{"en-US":["semigroup"]},{"\\foo{}":"ignore"},null])
    );
}
#[test]
fn diagnostics_map_utf16_strictly_and_keep_safe_corrections() {
    let snapshot = snapshot(5, "😀 We are wrong.");
    let diagnostic = raw_diagnostic(6, 9);
    let actions = json!([action(&snapshot.uri, &diagnostic, "is")]);
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &actions);
    assert_eq!(
        converted[0].location,
        Some(DiagnosticLocation { line: 1, column: 6 })
    );
    let correction = converted[0].correction.as_ref().unwrap();
    assert_eq!(correction.range, 5..8);
    assert_eq!(correction.original, "are");
    assert_eq!(correction.replacements, ["is"]);
    assert_eq!(correction.document, Some(snapshot.key));
    assert!(
        convert_diagnostics(&snapshot, &[raw_diagnostic(1, 2)], &Value::Null).is_empty(),
        "split surrogate must be rejected"
    );
    assert!(convert_diagnostics(&snapshot, &[raw_diagnostic(40, 41)], &Value::Null).is_empty());
}
#[test]
fn suggestions_never_execute_commands_bulk_edits_or_cross_markup() {
    let snapshot = snapshot(1, r"a \emph{bad} are");
    let diagnostic = raw_diagnostic(2, 12);
    let actions = json!([action(&snapshot.uri, &diagnostic, "good")]);
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &actions);
    assert!(
        converted[0]
            .correction
            .as_ref()
            .unwrap()
            .replacements
            .is_empty()
    );
    let diagnostic = raw_diagnostic(13, 16);
    let mut bulk = action(&snapshot.uri, &diagnostic, "is");
    bulk["edit"]["documentChanges"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"delete","uri":"file:///tmp/other.tex"}));
    let actions = json!([bulk, {"command":{"command":"dangerous"}}]);
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &actions);
    assert!(
        converted[0]
            .correction
            .as_ref()
            .unwrap()
            .replacements
            .is_empty()
    );
}

fn shared(snapshot: LtexSnapshot) -> Arc<Mutex<Shared>> {
    Arc::new(Mutex::new(Shared {
        desired: Some(snapshot),
        changed: Instant::now() - DEBOUNCE,
        serial: 1,
        stop: Arc::default(),
        ready: false,
        result: None,
        error: None,
    }))
}
fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "mock LTeX protocol timed out");
        thread::sleep(Duration::from_millis(5));
    }
}
/// In-memory peer uses the real scheduler and request handling without Python,
/// Java, a desktop, or wall-time performance assertions.
fn mock_session(
    mut server: impl FnMut(Value, &SyncSender<Result<Value, String>>) + Send + 'static,
) -> Session {
    let (outgoing, requests) = mpsc::sync_channel::<Value>(8);
    let (responses, incoming) = mpsc::sync_channel(8);
    let peer = thread::spawn(move || {
        for message in requests {
            server(message, &responses);
        }
    });
    Session {
        child: None,
        outgoing: Some(outgoing),
        incoming: Some(incoming),
        readers: vec![peer],
        stderr: Arc::new(Mutex::new(VecDeque::new())),
    }
}
fn initialize(message: &Value, responses: &SyncSender<Result<Value, String>>) -> bool {
    if message["method"] == "initialize" {
        assert_eq!(
            message["params"]["initializationOptions"]["customCapabilities"]["workspaceSpecificConfiguration"],
            true
        );
        responses
            .send(Ok(
                json!({"jsonrpc":"2.0","id":1,"result":{"capabilities":{}}}),
            ))
            .unwrap();
        true
    } else {
        false
    }
}
#[test]
fn real_scheduler_checks_before_opening_and_enriches_current_diagnostics() {
    let snapshot = snapshot(1, "This are a test. $x$ \\emph{Really}.");
    let state = shared(snapshot.clone());
    let mut has_checked = false;
    let mut config_replied = false;
    let mut session = mock_session(move |message, responses| {
        if initialize(&message, responses) {
            return;
        }
        match message["method"].as_str().unwrap_or("") {
            "workspace/didChangeConfiguration" => {
                panic!("configuration notifications trigger unversioned background checks")
            }
            "workspace/executeCommand" => {
                assert_eq!(
                    message["params"]["arguments"][0]["text"],
                    "This are a test. $x$ \\emph{Really}."
                );
                assert_eq!(
                    message["params"]["arguments"][0]["uri"],
                    "file:///tmp/paper.tex"
                );
                responses.send(Ok(json!({"jsonrpc":"2.0","id":101,"method":"workspace/configuration","params":{"items":[{"section":"ltex"}]}}))).unwrap();
                responses.send(Ok(json!({"jsonrpc":"2.0","id":102,"method":"ltex/workspaceSpecificConfiguration","params":{"items":[{"section":"ltex.dictionary"}]}}))).unwrap();
                responses.send(Ok(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///tmp/paper.tex","diagnostics":[raw_diagnostic(5,8)]}}))).unwrap();
                responses
                    .send(Ok(
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"success":true}}),
                    ))
                    .unwrap();
                has_checked = true;
            }
            "textDocument/didOpen" => assert!(
                has_checked,
                "first didOpen must not cause an automatic check"
            ),
            "textDocument/codeAction" => {
                let diagnostic = raw_diagnostic(5, 8);
                responses.send(Ok(json!({"jsonrpc":"2.0","id":message["id"],"result":[action("file:///tmp/paper.tex", &diagnostic,"is")]}))).unwrap();
            }
            _ => {
                if message["id"] == 101 {
                    assert_eq!(message["result"][0]["checkFrequency"], "manual");
                    config_replied = true;
                }
                if message["id"] == 102 {
                    assert!(config_replied);
                }
            }
        }
    });
    let worker_state = state.clone();
    let (_wake, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        run_session(
            &LtexSettings::default(),
            std::path::Path::new("/tmp"),
            &worker_state,
            receiver,
            &egui::Context::default(),
            &mut session,
        )
    });
    wait_until(
        || matches!(state.lock().unwrap().result.as_ref(), Some(LtexEvent::Diagnostics { diagnostics,.. }) if diagnostics.first().and_then(|d|d.correction.as_ref()).is_some_and(|c|!c.replacements.is_empty())),
    );
    state.lock().unwrap().stop.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
}
#[test]
fn sequential_checks_pull_each_document_language_without_automatic_checks() {
    let mut first = snapshot(
        1,
        "#set text(lang: \"en\", region: \"us\")\nThis is a result.",
    );
    first.language_id = "typst".into();
    let state = shared(first);
    let languages = Arc::new(Mutex::new(Vec::new()));
    let received = languages.clone();
    let mut pending_request = None;
    let mut session = mock_session(move |message, responses| {
        if initialize(&message, responses) {
            return;
        }
        match message["method"].as_str().unwrap_or("") {
            "workspace/didChangeConfiguration" => {
                panic!("language following must not trigger an unversioned automatic check")
            }
            "workspace/executeCommand" => {
                let id = message["id"].as_u64().unwrap();
                let text = message["params"]["arguments"][0]["text"].as_str().unwrap();
                let expected = if text.contains("region: \"us\"") {
                    "en-US"
                } else {
                    "en-GB"
                };
                pending_request = Some((id, expected));
                responses.send(Ok(json!({"jsonrpc":"2.0","id":100 + id,"method":"workspace/configuration","params":{"items":[{"section":"ltex.language"},{"section":"ltex.dictionary"},{"section":"ltex.disabledRules"}]}}))).unwrap();
            }
            _ if message["id"].as_u64().is_some_and(|id| id >= 100) => {
                let (id, expected) = pending_request.take().unwrap();
                assert_eq!(message["id"], 100 + id);
                assert_eq!(message["result"][0], expected);
                assert!(
                    message["result"][1][expected]
                        .as_array()
                        .unwrap()
                        .contains(&json!("semigroup"))
                );
                assert_eq!(message["result"][2][expected].as_array().unwrap().len(), 11);
                received.lock().unwrap().push(expected);
                responses.send(Ok(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///tmp/paper.tex","diagnostics":[]}}))).unwrap();
                responses
                    .send(Ok(
                        json!({"jsonrpc":"2.0","id":id,"result":{"success":true}}),
                    ))
                    .unwrap();
            }
            _ => {}
        }
    });
    let worker_state = state.clone();
    let (wake, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        run_session(
            &LtexSettings::default(),
            std::path::Path::new("/tmp"),
            &worker_state,
            receiver,
            &egui::Context::default(),
            &mut session,
        )
    });
    wait_until(
        || matches!(state.lock().unwrap().result.as_ref(), Some(LtexEvent::Diagnostics { key, .. }) if key.revision == 1),
    );
    {
        let mut shared = state.lock().unwrap();
        let mut next = snapshot(
            2,
            "#set text(lang: \"en\", region: \"gb\")\nThis is a result.",
        );
        next.language_id = "typst".into();
        shared.desired = Some(next);
        shared.serial += 1;
        shared.changed = Instant::now() - DEBOUNCE;
    }
    wake.try_send(()).unwrap();
    wait_until(
        || matches!(state.lock().unwrap().result.as_ref(), Some(LtexEvent::Diagnostics { key, .. }) if key.revision == 2),
    );
    state.lock().unwrap().stop.store(true, Ordering::Release);
    let _ = wake.try_send(());
    worker.join().unwrap().unwrap();
    assert_eq!(*languages.lock().unwrap(), ["en-US", "en-GB"]);
}

#[test]
fn superseded_checks_drain_the_original_response_without_unsafe_cancellation() {
    let state = shared(snapshot(1, "old are"));
    let observed_first = Arc::new(Mutex::new(false));
    let observed = observed_first.clone();
    let completed_first = Arc::new(Mutex::new(false));
    let completed = completed_first.clone();
    let (release, delayed) = mpsc::sync_channel(1);
    let mut delayed = Some(delayed);
    let mut session = mock_session(move |message, responses| {
        if initialize(&message, responses) {
            return;
        }
        if message["method"] == "workspace/executeCommand" {
            if message["params"]["arguments"][0]["text"] == "old are" {
                *observed.lock().unwrap() = true;
                let delayed = delayed.take().unwrap();
                let responses = responses.clone();
                let completed = completed.clone();
                thread::spawn(move || {
                    delayed.recv().unwrap();
                    // Publish after the source has already been superseded.
                    responses.send(Ok(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///tmp/paper.tex","diagnostics":[raw_diagnostic(4,7)]}}))).unwrap();
                    *completed.lock().unwrap() = true;
                    responses
                        .send(Ok(
                            json!({"jsonrpc":"2.0","id":message["id"],"result":{"success":true}}),
                        ))
                        .unwrap();
                });
            } else {
                assert!(
                    *completed.lock().unwrap(),
                    "latest check must wait for its predecessor's actual completion"
                );
                responses.send(Ok(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///tmp/paper.tex","diagnostics":[]}}))).unwrap();
                responses
                    .send(Ok(
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"success":true}}),
                    ))
                    .unwrap();
            }
        } else if message["method"] == "$/cancelRequest" {
            panic!("never cancel a versionless publication request before its task finishes");
        }
    });
    let worker_state = state.clone();
    let (_wake, receiver) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        run_session(
            &LtexSettings::default(),
            std::path::Path::new("/tmp"),
            &worker_state,
            receiver,
            &egui::Context::default(),
            &mut session,
        )
    });
    wait_until(|| *observed_first.lock().unwrap());
    {
        let mut state = state.lock().unwrap();
        state.desired = Some(snapshot(2, "new is"));
        state.serial = 2;
        state.changed = Instant::now() - DEBOUNCE;
    }
    release.send(()).unwrap();
    wait_until(
        || matches!(state.lock().unwrap().result.as_ref(), Some(LtexEvent::Diagnostics { key,diagnostics,.. }) if key.revision==2 && diagnostics.is_empty()),
    );
    state.lock().unwrap().stop.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
}

#[test]
fn diagnostic_endpoint_mapping_is_one_forward_scan_for_long_lines() {
    let source = "x".repeat(50_000);
    let index = SourceIndex::new(
        &source,
        (0..=50_000).rev().map(|column| LspPosition::new(0, column)),
    );
    assert_eq!(index.scanned_scalars, source.len());
    assert_eq!(index.positions.len(), 50_001);
    assert_eq!(
        index.position(LspPosition::new(0, 40_000)),
        Some((40_000, 40_000, 40_001))
    );
}

#[test]
fn grouped_quick_fixes_become_individual_replacements_only() {
    let snapshot = snapshot(1, "are are");
    let first = raw_diagnostic(0, 3);
    let second = raw_diagnostic(4, 7);
    let mut grouped = action(&snapshot.uri, &first, "is");
    grouped["edit"]["documentChanges"]
        .as_array_mut()
        .unwrap()
        .push(action(&snapshot.uri, &second, "is")["edit"]["documentChanges"][0].clone());
    let diagnostics = convert_diagnostics(&snapshot, &[first, second], &json!([grouped]));
    assert_eq!(diagnostics.len(), 2);
    for diagnostic in diagnostics {
        let correction = diagnostic.correction.unwrap();
        assert_eq!(correction.original, "are");
        assert_eq!(correction.replacements, ["is"]);
        assert_eq!(correction.range.len(), 3);
    }
}

#[test]
fn queued_old_language_result_is_retired_before_rechecking_the_same_revision() {
    let request = snapshot(1, "This is a result.");
    let state = shared(request.clone());
    let old_result = || LtexEvent::Diagnostics {
        key: request.key,
        uri: request.uri.clone(),
        diagnostics: Vec::new(),
    };
    let (wake, _) = mpsc::sync_channel(1);
    let service = LtexService {
        shared: state.clone(),
        wake,
        worker: None,
    };
    state.lock().unwrap().result = Some(old_result());
    service.update(request.clone());
    assert!(matches!(
        service.try_recv(),
        Some(LtexEvent::Diagnostics { .. })
    ));
    state.lock().unwrap().result = Some(old_result());
    let mut changed_language = request.clone();
    changed_language.writing_language = crate::settings::WritingLanguage::American;
    service.update(changed_language);
    assert!(service.try_recv().is_none());
    assert_eq!(
        state.lock().unwrap().desired.as_ref().unwrap().key,
        request.key
    );
    assert_eq!(
        state
            .lock()
            .unwrap()
            .desired
            .as_ref()
            .unwrap()
            .writing_language,
        crate::settings::WritingLanguage::American
    );
}

#[test]
fn latest_slot_replaces_inputs_and_clear_flushes_results() {
    let state = shared(snapshot(1, "old"));
    let (wake, _) = mpsc::sync_channel(1);
    let service = LtexService {
        shared: state.clone(),
        wake,
        worker: None,
    };
    for revision in 2..1000 {
        service.update(snapshot(revision, "latest"));
    }
    assert_eq!(
        state.lock().unwrap().desired.as_ref().unwrap().key.revision,
        999
    );
    let serial = state.lock().unwrap().serial;
    let mut changed_language = snapshot(999, "latest");
    changed_language.writing_language = crate::settings::WritingLanguage::American;
    service.update(changed_language);
    assert_eq!(state.lock().unwrap().serial, serial + 1);
    assert_eq!(
        state
            .lock()
            .unwrap()
            .desired
            .as_ref()
            .unwrap()
            .writing_language,
        crate::settings::WritingLanguage::American
    );
    service.close();
    assert!(state.lock().unwrap().desired.is_none());
    assert!(service.try_recv().is_none());
}

#[test]
#[cfg(unix)]
fn app_exit_shutdown_reaps_the_owned_process_without_protocol_cooperation() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().unwrap();
    let executable = directory.path().join("unresponsive-ltex");
    let pid = directory.path().join("pid");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s' \"$$\" > \"$PID_FILE\"\nexec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut settings = LtexSettings {
        mode: crate::settings::ToolMode::Custom,
        executable: executable.to_str().unwrap().into(),
        ..Default::default()
    };
    settings.command.environment = json!({"PID_FILE":pid}).to_string();
    let service = LtexService::spawn(
        settings,
        directory.path().to_owned(),
        egui::Context::default(),
    );
    wait_until(|| std::fs::read_to_string(&pid).is_ok_and(|s| !s.is_empty()));
    let process = std::fs::read_to_string(pid)
        .unwrap()
        .parse::<i32>()
        .unwrap();
    service.begin_stop();
    service.shutdown();
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(process), None),
        Err(nix::errno::Errno::ESRCH)
    );
}

#[test]
#[ignore = "optional real LTeX+ server; set TIPTOPTYP_LTEX_TEST_EXECUTABLE"]
fn official_server_checks_latex_and_typst_and_returns_suggestions() {
    let executable = std::env::var("TIPTOPTYP_LTEX_TEST_EXECUTABLE").expect("LTeX+ executable");
    let settings = LtexSettings {
        enabled: true,
        mode: crate::settings::ToolMode::Custom,
        executable,
        ..Default::default()
    };
    let context = egui::Context::default();
    let service = LtexService::spawn(settings, std::env::temp_dir(), context);
    let mut request = snapshot(1, "");
    for (revision, language, source, expect_errors) in [
        (
            1,
            "latex",
            "\\documentclass{article}\n\\begin{document}\nThis are a test. We has two result.\n\\end{document}",
            true,
        ),
        (
            2,
            "latex",
            "\\documentclass{article}\n\\begin{document}\nThis is a test. We have two results.\n\\end{document}",
            false,
        ),
        (3, "typst", "This are a test. We has two result.", true),
        (4, "typst", "This is a test. We have two results.", false),
    ] {
        request.key.revision = revision;
        request.language_id = language.into();
        request.uri = format!(
            "file:///tmp/ltex-live-probe.{}",
            if language == "latex" { "tex" } else { "typ" }
        );
        request.source = Arc::from(source);
        service.update(request.clone());
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            assert!(
                Instant::now() < deadline,
                "LTeX+ live check timed out for {language}, revision {revision}"
            );
            match service.try_recv() {
                Some(LtexEvent::Error(error)) => panic!("{error}"),
                Some(LtexEvent::Diagnostics {
                    key, diagnostics, ..
                }) if key == request.key
                    && if expect_errors {
                        diagnostics.iter().any(|d| {
                            d.correction
                                .as_ref()
                                .is_some_and(|c| !c.replacements.is_empty())
                        })
                    } else {
                        diagnostics.is_empty()
                    } =>
                {
                    break;
                }
                _ => thread::sleep(Duration::from_millis(10)),
            }
        }
    }
    // Keep the complete document identity and text fixed: a language-only
    // update must invalidate LanguageTool's cached paragraph result too.
    request = snapshot(
        5,
        "\\documentclass{article}\n\\begin{document}\nThe color is red.\n\\end{document}",
    );
    for writing_language in [
        crate::settings::WritingLanguage::British,
        crate::settings::WritingLanguage::American,
    ] {
        request.writing_language = writing_language;
        service.update(request.clone());
        let deadline = Instant::now() + Duration::from_secs(90);
        loop {
            assert!(
                Instant::now() < deadline,
                "LTeX+ language-only live check timed out for {writing_language:?}"
            );
            match service.try_recv() {
                Some(LtexEvent::Error(error)) => panic!("{error}"),
                Some(LtexEvent::Diagnostics {
                    key,
                    uri,
                    diagnostics,
                }) if key == request.key && uri == request.uri => {
                    if writing_language == crate::settings::WritingLanguage::British {
                        let correction = diagnostics
                            .iter()
                            .find_map(|diagnostic| {
                                diagnostic.correction.as_ref().filter(|correction| {
                                    correction.rule.as_deref() == Some("MORFOLOGIK_RULE_EN_GB")
                                        && correction.original == "color"
                                })
                            })
                            .unwrap_or_else(|| {
                                panic!("UK spelling should flag color: {diagnostics:?}")
                            });
                        // Diagnostics arrive before their code-action enrichment.
                        // Await the real replacement before switching languages.
                        if correction.replacements.is_empty() {
                            continue;
                        }
                        assert!(
                            correction.replacements.iter().any(|word| word == "colour"),
                            "UK spelling must offer colour: {diagnostics:?}"
                        );
                    } else {
                        assert!(
                            diagnostics.is_empty(),
                            "US spelling must not reuse the UK paragraph result: {diagnostics:?}"
                        );
                    }
                    break;
                }
                _ => thread::sleep(Duration::from_millis(10)),
            }
        }
    }
    service.shutdown();
}

#[test]
fn bundled_launch_uses_its_java_and_explicit_custom_environment_still_wins() {
    let prepared = crate::ltex_resources::Prepared {
        program: "/private/bundled/bin/ltex-ls-plus".into(),
        java_home: "/private/bundled/jdk".into(),
    };
    let mut settings = LtexSettings::default();
    assert_eq!(settings.mode, crate::settings::ToolMode::Bundled);
    settings.validate().unwrap();
    let command = launch_command(&settings, std::path::Path::new("/tmp"), Some(&prepared)).unwrap();
    assert_eq!(command.get_program(), prepared.program.as_os_str());
    let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        environment[std::ffi::OsStr::new("JAVA_HOME")],
        Some(prepared.java_home.as_os_str())
    );
    assert_eq!(
        environment[std::ffi::OsStr::new("JAVACMD")],
        Some(std::ffi::OsStr::new("/private/bundled/jdk/bin/java"))
    );
    settings.command.environment =
        r#"{"JAVA_HOME":"/chosen/java","JAVACMD":"/chosen/java/bin/java"}"#.into();
    let command = launch_command(&settings, std::path::Path::new("/tmp"), Some(&prepared)).unwrap();
    let environment: std::collections::BTreeMap<_, _> = command.get_envs().collect();
    assert_eq!(
        environment[std::ffi::OsStr::new("JAVA_HOME")],
        Some(std::ffi::OsStr::new("/chosen/java"))
    );
    settings.mode = crate::settings::ToolMode::Custom;
    assert!(settings.validate().is_err());
    settings.executable = "/custom/ltex".into();
    let command = launch_command(&settings, std::path::Path::new("/tmp"), None).unwrap();
    assert_eq!(command.get_program(), std::ffi::OsStr::new("/custom/ltex"));
}
