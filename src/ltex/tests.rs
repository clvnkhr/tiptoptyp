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
    assert!(settings.enabled);
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
    assert!(settings.enabled);
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
fn valid_oxford_spelling_variants_are_quiet_by_default_and_explicitly_opt_in() {
    let source = "The generalised equation.";
    let request = snapshot(1, source);
    let raw = json!({"range":{"start":{"line":0,"character":4},"end":{"line":0,"character":15}},"message":"Both spellings are correct.","code":"OXFORD_SPELLING_Z_NOT_S"});
    assert!(!LtexSettings::default().oxford_spelling);
    assert!(
        convert_diagnostics(&request, std::slice::from_ref(&raw), &Value::Null, None).is_empty()
    );
    assert_eq!(
        convert_diagnostics_with_prose(&request, &[raw], &Value::Null, None, None, true).len(),
        1
    );
    let settings = LtexSettings {
        oxford_spelling: true,
        ..Default::default()
    };
    let encoded = serde_json::to_value(&settings).unwrap();
    assert!(
        serde_json::from_value::<LtexSettings>(encoded)
            .unwrap()
            .oxford_spelling
    );
}

#[test]
fn typst_syntax_diagnostics_are_rejected_but_real_prose_errors_and_corrections_remain() {
    let source = "#align(center)[This are a test.] $nabla + frac(1,2)$";
    let mut request = snapshot(1, source);
    request.language_id = "typst".into();
    let diagnostic = |word: &str, code: &str| {
        let byte = source.find(word).unwrap();
        let start = source[..byte].encode_utf16().count();
        json!({"range":{"start":{"line":0,"character":start},"end":{"line":0,"character":start+word.encode_utf16().count()}},"message":"fixture","code":code})
    };
    let grammar = diagnostic("are", "SUBJECT_VERB_AGREEMENT");
    let raw = [
        diagnostic("center", "MORFOLOGIK_RULE_EN_GB"),
        diagnostic("nabla", "MORFOLOGIK_RULE_EN_GB"),
        diagnostic("frac", "MORFOLOGIK_RULE_EN_GB"),
        grammar.clone(),
    ];
    let actions = json!([action(&request.uri, &grammar, "is")]);
    let findings = convert_diagnostics(&request, &raw, &actions, Some(1));
    assert_eq!(findings.len(), 1);
    let correction = findings[0].correction.as_ref().unwrap();
    assert_eq!(correction.original, "are");
    assert_eq!(correction.replacements, vec!["is"]);
}

#[test]
#[ignore = "requires TIPTOPTYP_LTEX_TEST_EXECUTABLE pointing to a local LTeX+ launcher"]
fn official_server_filters_typst_mathematical_template_false_positives_and_keeps_grammar() {
    let settings = LtexSettings {
        executable: std::env::var("TIPTOPTYP_LTEX_TEST_EXECUTABLE").expect("LTeX+ executable"),
        mode: crate::settings::ToolMode::Custom,
        ..Default::default()
    };
    let service = LtexService::spawn(settings, std::env::temp_dir(), egui::Context::default());
    let mut request = snapshot(
        1,
        include_str!("../../tests/fixtures/ltex/typst-mathematical-prose.typ"),
    );
    request.language_id = "typst".into();
    request.uri = "file:///tmp/ttt-ltex-mathematical-prose.typ".into();
    service.update(request.clone());
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        assert!(
            Instant::now() < deadline,
            "LTeX+ did not return the mathematical fixture"
        );
        match service.try_recv() {
            Some(LtexEvent::Error(error)) => panic!("{error}"),
            Some(LtexEvent::Diagnostics {
                key, diagnostics, ..
            }) if key == request.key
                && diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .correction
                        .as_ref()
                        .is_some_and(|correction| !correction.replacements.is_empty())
                }) =>
            {
                assert!(
                    !diagnostics.is_empty(),
                    "real grammar mistakes must remain visible"
                );
                for diagnostic in &diagnostics {
                    let correction = diagnostic
                        .correction
                        .as_ref()
                        .expect("source-bound correction");
                    assert!(
                        matches!(correction.original.as_str(), "This" | "are" | "has"),
                        "unexpected false positive: {diagnostic:?}"
                    );
                }
                eprintln!(
                    "mathematical Typst fixture: {} genuine grammar diagnostics; no syntax/name/Oxford false positives",
                    diagnostics.len()
                );
                break;
            }
            _ => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    service.shutdown();
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
fn tex_magic_spellcheck_selects_document_language_without_changing_preferences() {
    let mut settings = LtexSettings::default();
    let saved = settings.clone();
    for (source, expected) in [
        ("% !TeX spellcheck = en_US\nText", "en-US"),
        ("%!TEX spellcheck=de-DE\nText", "de-DE"),
        ("% !TeX spellcheck = fr_FR\nText", "fr-FR"),
    ] {
        let mut request = snapshot(1, source);
        request.writing_language = crate::settings::WritingLanguage::British;
        assert_eq!(settings.language_for_snapshot(&request), expected);
    }
    assert_eq!(settings, saved);
    settings.follow_document_language = false;
    assert_eq!(
        settings.language_for_snapshot(&snapshot(1, "% !TeX spellcheck = de-DE\nText")),
        "en-GB"
    );
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
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &actions, Some(1));
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
        convert_diagnostics(&snapshot, &[raw_diagnostic(1, 2)], &Value::Null, None).is_empty(),
        "split surrogate must be rejected"
    );
    assert!(
        convert_diagnostics(&snapshot, &[raw_diagnostic(40, 41)], &Value::Null, None).is_empty()
    );
}
#[test]
fn suggestions_never_execute_commands_bulk_edits_or_cross_markup() {
    let snapshot = snapshot(1, r"a \emph{bad} are");
    let diagnostic = raw_diagnostic(2, 12);
    let actions = json!([action(&snapshot.uri, &diagnostic, "good")]);
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &actions, Some(1));
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
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &actions, Some(1));
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
fn focused_action_batch_uses_only_visible_findings_and_preserves_atomic_fixes() {
    let source = "#align(center)[This are a test. We has two result.]";
    let mut request = snapshot(1, source);
    request.language_id = "typst".into();
    let diagnostic = |word: &str| {
        let start = source.find(word).unwrap() as u32;
        raw_diagnostic(start, start + word.len() as u32)
    };
    let hidden = diagnostic("center");
    let first = diagnostic("are");
    let second = diagnostic("has");
    let mut oxford = first.clone();
    oxford["code"] = "OXFORD_SPELLING_Z_NOT_S".into();
    let state = shared(request.clone());
    let observed = Arc::new(Mutex::new(0));
    let requests = observed.clone();
    let (release, delayed) = mpsc::sync_channel(1);
    let mut delayed = Some(delayed);
    let mut session = mock_session(move |message, responses| {
        if initialize(&message, responses) {
            return;
        }
        match message["method"].as_str().unwrap_or("") {
            "workspace/executeCommand" => {
                responses.send(Ok(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":request.uri,"diagnostics":[hidden,first,oxford,second]}}))).unwrap();
                responses
                    .send(Ok(
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"success":true}}),
                    ))
                    .unwrap();
            }
            "textDocument/codeAction" => {
                let mut requests = requests.lock().unwrap();
                *requests += 1;
                let (diagnostic, replacement) = match *requests {
                    1 => (&first, "is"),
                    2 => (&second, "have"),
                    _ => panic!("hidden findings must not request suggestions"),
                };
                assert_eq!(message["params"]["range"], diagnostic["range"]);
                assert_eq!(
                    message["params"]["context"]["diagnostics"],
                    json!([diagnostic])
                );
                assert_eq!(message["params"]["context"]["only"], json!(["quickfix"]));
                if let Some(delayed) = delayed.take() {
                    drop(requests);
                    delayed.recv().unwrap();
                }
                let mut bulk = action(&request.uri, diagnostic, "bulk must stay atomic");
                bulk["edit"]["documentChanges"][0]["edits"].as_array_mut().unwrap()
                    .push(action(&request.uri, &hidden, "hidden")["edit"]["documentChanges"][0]["edits"][0].clone());
                responses.send(Ok(json!({"jsonrpc":"2.0","id":message["id"],"result":[bulk,action(&request.uri,diagnostic,replacement)]}))).unwrap();
            }
            _ => {}
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
    wait_until(|| *observed.lock().unwrap() == 1);
    {
        let state = state.lock().unwrap();
        let Some(LtexEvent::Diagnostics { diagnostics, .. }) = &state.result else {
            panic!("initial findings must publish before suggestions finish");
        };
        assert_eq!(diagnostics.len(), 2);
        assert!(
            diagnostics
                .iter()
                .all(|d| d.correction.as_ref().unwrap().replacements.is_empty())
        );
    }
    release.send(()).unwrap();
    wait_until(|| {
        matches!(state.lock().unwrap().result.as_ref(),
        Some(LtexEvent::Diagnostics { diagnostics, .. })
        if diagnostics.len() == 2
            && diagnostics[0].correction.as_ref().unwrap().replacements == ["is"]
            && diagnostics[1].correction.as_ref().unwrap().replacements == ["have"])
    });
    assert_eq!(*observed.lock().unwrap(), 2);
    state.lock().unwrap().stop.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
}

#[test]
fn superseded_action_batch_cancels_current_request_and_does_not_start_the_next() {
    let state = shared(snapshot(1, "old are are"));
    let observed = Arc::new(Mutex::new(0));
    let requests = observed.clone();
    let canceled = Arc::new(AtomicBool::new(false));
    let cancellation = canceled.clone();
    let mut delayed = None;
    let mut session = mock_session(move |message, responses| {
        if initialize(&message, responses) {
            return;
        }
        match message["method"].as_str().unwrap_or("") {
            "workspace/executeCommand" => {
                let diagnostics = if message["params"]["arguments"][0]["text"] == "old are are" {
                    json!([raw_diagnostic(4, 7), raw_diagnostic(8, 11)])
                } else {
                    json!([])
                };
                responses.send(Ok(json!({"jsonrpc":"2.0","method":"textDocument/publishDiagnostics","params":{"uri":"file:///tmp/paper.tex","diagnostics":diagnostics}}))).unwrap();
                responses
                    .send(Ok(
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"success":true}}),
                    ))
                    .unwrap();
            }
            "textDocument/codeAction" => {
                *requests.lock().unwrap() += 1;
                assert!(
                    delayed.is_none(),
                    "a superseded batch must not request its second finding"
                );
                delayed = Some(message["id"].clone());
            }
            "$/cancelRequest" => {
                let id = delayed.take().expect("cancel the outstanding action");
                assert_eq!(message["params"]["id"], id);
                cancellation.store(true, Ordering::Release);
                // Even a late successful response may not enrich the old revision.
                responses.send(Ok(json!({"jsonrpc":"2.0","id":id,"result":[action("file:///tmp/paper.tex",&raw_diagnostic(4,7),"is")]}))).unwrap();
            }
            _ => {}
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
    wait_until(|| *observed.lock().unwrap() == 1);
    {
        let mut state = state.lock().unwrap();
        state.desired = Some(snapshot(2, "new is"));
        state.serial = 2;
        state.changed = Instant::now() - DEBOUNCE;
    }
    wait_until(|| {
        matches!(state.lock().unwrap().result.as_ref(),
        Some(LtexEvent::Diagnostics { key, diagnostics, .. }) if key.revision == 2 && diagnostics.is_empty())
    });
    assert!(canceled.load(Ordering::Acquire));
    assert_eq!(*observed.lock().unwrap(), 1);
    state.lock().unwrap().stop.store(true, Ordering::Release);
    worker.join().unwrap().unwrap();
}

#[test]
fn focused_action_queue_is_bounded_and_uses_original_utf16_coordinates() {
    let mut request = snapshot(1, "😀 are".repeat(100).as_str());
    request.language_id = "latex".into();
    let raw: Vec<_> = (0..100)
        .map(|i| raw_diagnostic(i * 6 + 3, i * 6 + 6))
        .collect();
    let converted = convert_diagnostics(&request, &raw, &Value::Null, None);
    let queued = visible_action_diagnostics(&request, &raw, &converted);
    assert_eq!(queued.len(), ACTION_REQUEST_LIMIT);
    assert_eq!(queued[0], raw[0]);
    assert_eq!(
        queued[ACTION_REQUEST_LIMIT - 1],
        raw[ACTION_REQUEST_LIMIT - 1]
    );
}

#[test]
fn focused_action_results_are_bounded_across_responses() {
    let mut collected = Vec::new();
    let response = Value::Array((0..400).map(|i| json!({"title":i})).collect());
    collect_actions(&mut collected, &response);
    assert_eq!(collected.len(), 400);
    collect_actions(&mut collected, &response);
    assert_eq!(collected.len(), ACTION_RESULT_LIMIT);
    collect_actions(&mut collected, &response);
    assert_eq!(collected.len(), ACTION_RESULT_LIMIT);
    assert_eq!(collected.last(), Some(&json!({"title":111})));
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
    let mut checks = 0;
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
                assert!(!text.contains("#set"));
                assert!(text.contains("This is a result."));
                checks += 1;
                let expected = if checks == 1 { "en-US" } else { "en-GB" };
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
fn grouped_quick_fixes_remain_atomic_and_independent_choices_stay_available() {
    let snapshot = snapshot(1, "are are");
    let first = raw_diagnostic(0, 3);
    let second = raw_diagnostic(4, 7);
    let first_action = action(&snapshot.uri, &first, "is");
    let second_action = action(&snapshot.uri, &second, "is");
    let mut multiple_documents = first_action.clone();
    multiple_documents["edit"]["documentChanges"]
        .as_array_mut()
        .unwrap()
        .push(second_action["edit"]["documentChanges"][0].clone());
    let mut multiple_edits = first_action.clone();
    multiple_edits["edit"]["documentChanges"][0]["edits"]
        .as_array_mut()
        .unwrap()
        .push(second_action["edit"]["documentChanges"][0]["edits"][0].clone());
    let mut legacy = json!({"kind":"quickfix","edit":{"changes":{}}});
    legacy["edit"]["changes"][snapshot.uri.as_str()] = json!([
        first_action["edit"]["documentChanges"][0]["edits"][0],
        second_action["edit"]["documentChanges"][0]["edits"][0],
    ]);
    for grouped in [multiple_documents, multiple_edits, legacy] {
        let diagnostics = convert_diagnostics(
            &snapshot,
            &[first.clone(), second.clone()],
            &json!([grouped]),
            Some(1),
        );
        assert_eq!(diagnostics.len(), 2);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic
                .correction
                .as_ref()
                .unwrap()
                .replacements
                .is_empty()
        }));
    }
    let mut legacy = json!({"kind":"quickfix","edit":{"changes":{}}});
    legacy["edit"]["changes"][snapshot.uri.as_str()] =
        json!([second_action["edit"]["documentChanges"][0]["edits"][0],]);
    let diagnostics = convert_diagnostics(
        &snapshot,
        &[first.clone(), second],
        &json!([
            first_action.clone(),
            action(&snapshot.uri, &first, "was"),
            first_action,
            legacy
        ]),
        Some(1),
    );
    assert_eq!(
        diagnostics[0].correction.as_ref().unwrap().replacements,
        ["is", "was"]
    );
    assert_eq!(
        diagnostics[1].correction.as_ref().unwrap().replacements,
        ["is"]
    );
}

#[test]
fn malformed_disabled_command_or_resource_actions_never_offer_partial_fixes() {
    let snapshot = snapshot(1, "We are wrong.");
    let diagnostic = raw_diagnostic(3, 6);
    let valid = action(&snapshot.uri, &diagnostic, "is");
    let invalid_fields = [
        ("/disabled", json!({"reason":"Unavailable"})),
        ("/disabled", Value::Null),
        ("/command", json!({"command":"_ltex.requiredSideEffect"})),
        ("/kind", json!("source.fixAll")),
        ("/kind", json!(false)),
        ("/edit/documentChanges", json!([])),
        ("/edit/documentChanges", json!(false)),
        ("/edit/documentChanges/0/kind", json!("delete")),
        (
            "/edit/documentChanges/0/textDocument/uri",
            json!("file:///tmp/other.tex"),
        ),
        ("/edit/documentChanges/0/textDocument/uri", Value::Null),
        ("/edit/documentChanges/0/edits", json!([])),
        ("/edit/documentChanges/0/edits", json!(false)),
        ("/edit/documentChanges/0/edits/0/newText", Value::Null),
        (
            "/edit/documentChanges/0/edits/0/range",
            json!({"start":{"line":0,"character":6},"end":{"line":0,"character":3}}),
        ),
        (
            "/edit/documentChanges/0/edits/0/annotationId",
            json!("requires-confirmation"),
        ),
        (
            "/edit/changeAnnotations",
            json!({"requires-confirmation":{"label":"Review first","needsConfirmation":true}}),
        ),
        ("/edit/changes", json!({})),
    ];
    for (path, value) in invalid_fields {
        let mut invalid = valid.clone();
        // Optional fields are not necessarily present in the valid baseline.
        let (parent, field) = path.rsplit_once('/').unwrap();
        invalid.pointer_mut(parent).unwrap()[field] = value;
        let converted = convert_diagnostics(
            &snapshot,
            std::slice::from_ref(&diagnostic),
            &json!([invalid]),
            Some(1),
        );
        assert!(
            converted[0]
                .correction
                .as_ref()
                .unwrap()
                .replacements
                .is_empty(),
            "{path}"
        );
    }
    for invalid in [
        Value::Null,
        json!([]),
        json!({"title":"Command only","command":"_ltex.changeSettings"}),
        json!({"edit":{"documentChanges":[{"kind":"delete","uri":snapshot.uri}]}}),
    ] {
        let converted = convert_diagnostics(
            &snapshot,
            std::slice::from_ref(&diagnostic),
            &json!([invalid]),
            Some(1),
        );
        assert!(
            converted[0]
                .correction
                .as_ref()
                .unwrap()
                .replacements
                .is_empty()
        );
    }
    let mut other_file = json!({"edit":{"changes":{}}});
    other_file["edit"]["changes"][snapshot.uri.as_str()] =
        json!([valid["edit"]["documentChanges"][0]["edits"][0],]);
    other_file["edit"]["changes"]["file:///tmp/other.tex"] = json!([]);
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &json!([other_file]), Some(1));
    assert!(
        converted[0]
            .correction
            .as_ref()
            .unwrap()
            .replacements
            .is_empty()
    );
}

#[test]
fn versioned_actions_match_the_exact_wire_version_not_the_document_revision() {
    let snapshot = snapshot(99, "We are wrong.");
    let diagnostic = raw_diagnostic(3, 6);
    let mut action = action(&snapshot.uri, &diagnostic, "is");
    for version in [
        json!(1),
        json!(6),
        json!(8),
        json!(99),
        json!(7.0),
        json!("7"),
        json!(false),
    ] {
        action["edit"]["documentChanges"][0]["textDocument"]["version"] = version;
        let converted = convert_diagnostics(
            &snapshot,
            std::slice::from_ref(&diagnostic),
            &json!([action.clone()]),
            Some(7),
        );
        assert!(
            converted[0]
                .correction
                .as_ref()
                .unwrap()
                .replacements
                .is_empty()
        );
    }
    for version in [json!(7), Value::Null] {
        action["edit"]["documentChanges"][0]["textDocument"]["version"] = version;
        let converted = convert_diagnostics(
            &snapshot,
            std::slice::from_ref(&diagnostic),
            &json!([action.clone()]),
            Some(7),
        );
        assert_eq!(
            converted[0].correction.as_ref().unwrap().replacements,
            ["is"]
        );
    }
    for version in [json!(7), json!("7"), json!(false)] {
        action["edit"]["documentChanges"][0]["textDocument"]["version"] = version;
        let converted = convert_diagnostics(
            &snapshot,
            std::slice::from_ref(&diagnostic),
            &json!([action.clone()]),
            None,
        );
        assert!(
            converted[0]
                .correction
                .as_ref()
                .unwrap()
                .replacements
                .is_empty()
        );
    }
    action["edit"]["documentChanges"][0]["textDocument"]
        .as_object_mut()
        .unwrap()
        .remove("version");
    let converted = convert_diagnostics(&snapshot, &[diagnostic], &json!([action]), None);
    assert_eq!(
        converted[0].correction.as_ref().unwrap().replacements,
        ["is"]
    );
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

#[test]
fn check_phases_keep_suggestion_provenance_and_superseded_drain_policy() {
    let mut check = Pending {
        id: 3,
        serial: 9,
        snapshot: snapshot(1, "He are here."),
        prose: None,
        oxford_spelling: true,
        diagnostics: Vec::new(),
        started: Instant::now(),
        validity: CheckValidity::Current,
        phase: CheckPhase::Checking,
    };
    assert!(check.checking());
    assert_eq!(check.action_version(), None);
    assert!(
        !check.supersede(),
        "explicit checks drain rather than cancel"
    );
    assert_eq!(check.validity, CheckValidity::Superseded);
    assert!(!check.supersede(), "supersession is idempotent");

    check.validity = CheckValidity::Current;
    check.phase = CheckPhase::Suggestions {
        document_version: 7,
        remaining: VecDeque::from([raw_diagnostic(0, 2)]),
        collected: Vec::new(),
    };
    assert!(!check.checking());
    assert_eq!(check.action_version(), Some(7));
    assert!(
        check.supersede(),
        "only suggestions own a cancellable request"
    );
    assert!(!check.supersede(), "cancel is sent at most once");
    assert_eq!(
        check.action_version(),
        Some(7),
        "supersession cannot relabel an action"
    );
}

#[test]
fn worker_initialization_owns_open_document_lifetime() {
    let mut phase = WorkerPhase::Initializing {
        started: Instant::now(),
    };
    assert!(!phase.is_ready());
    assert!(phase.opened().is_none());
    phase.initialized();
    phase.open_document(snapshot(1, "first"));
    phase.initialized();
    assert_eq!(phase.opened().unwrap().source.as_ref(), "first");
    phase.close_document();
    assert!(phase.is_ready());
    assert!(phase.opened().is_none());
}
