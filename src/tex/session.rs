//! Concrete stdio sessions for TexLab and Badness. Never operates the editor.
use super::{Event, Provider, Request, RequestKind, Snapshot};
use crate::{
    lsp::{protocol, transport},
    private_workspace::PrivateWorkspace,
    process::OwnedChild,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, File},
    io::BufReader,
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

const MAX_PENDING: usize = 16;
const PIPE_LIMIT: usize = 8;
const TIMEOUT: Duration = Duration::from_secs(15);

pub(super) struct Session {
    child: OwnedChild,
    outgoing: Option<SyncSender<Value>>,
    incoming: Option<Receiver<Result<Value, String>>>,
    readers: Vec<thread::JoinHandle<()>>,
    _directory: tempfile::TempDir,
    log: std::path::PathBuf,
    provider: Provider,
    initialized: bool,
    created: Instant,
    last_health_check: Instant,
    opened: Option<super::Identity>,
    next_id: u64,
    pending: HashMap<u64, (Request, Instant)>,
    snapshot: Snapshot,
}
impl Session {
    pub(super) fn start(provider: Provider, snapshot: &Snapshot) -> Result<Self, String> {
        let resolution = match provider {
            Provider::Texlab => &snapshot.tools.texlab,
            Provider::Badness => &snapshot.tools.badness,
        };
        if !resolution.is_available() {
            return Err(format!(
                "{} is unavailable; choose its executable in Settings → Tools",
                provider.label()
            ));
        }
        let directory = PrivateWorkspace::open(&snapshot.root)
            .and_then(|p| p.temp_dir("tex-lsp-"))
            .map_err(|e| e.to_string())?;
        let log = directory.path().join("stderr.log");
        let mut command = Command::new(&resolution.program);
        command.current_dir(&snapshot.root);
        resolution
            .command
            .apply(&mut command)
            .map_err(|e| e.to_string())?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(File::create(&log).map_err(|e| e.to_string())?);
        let mut child = OwnedChild::spawn(&mut command)
            .map_err(|e| format!("Could not start {}: {e}", provider.label()))?;
        let stdin = child.child_mut().stdin.take().unwrap();
        let stdout = child.child_mut().stdout.take().unwrap();
        let (outgoing, writer_rx) = mpsc::sync_channel(PIPE_LIMIT);
        let (reader_tx, incoming) = mpsc::sync_channel(PIPE_LIMIT);
        let error_tx = reader_tx.clone();
        let writer = thread::spawn(move || {
            let mut stdin = stdin;
            for message in writer_rx {
                if let Err(error) = transport::write_lsp_message(&mut stdin, &message) {
                    let _ = error_tx.send(Err(error.to_string()));
                    break;
                }
            }
        });
        let reader = thread::spawn(move || {
            let mut stdout = BufReader::new(stdout);
            loop {
                let message = match transport::read_lsp_message(&mut stdout) {
                    Ok(Some(message)) => Ok(message),
                    Ok(None) => Err("Language server closed its output".into()),
                    Err(error) => Err(error.to_string()),
                };
                let stop = message.is_err();
                if reader_tx.send(message).is_err() || stop {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            outgoing: Some(outgoing),
            incoming: Some(incoming),
            readers: vec![writer, reader],
            _directory: directory,
            log,
            provider,
            initialized: false,
            created: Instant::now(),
            last_health_check: Instant::now(),
            opened: None,
            next_id: 2,
            pending: HashMap::new(),
            snapshot: snapshot.clone(),
        };
        let root = url::Url::from_directory_path(&snapshot.root)
            .map_err(|()| "Invalid TeX workspace path")?
            .to_string();
        session.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "processId":std::process::id(),"rootUri":root,
            "workspaceFolders":[{"uri":root,"name":"TeX workspace"}],
            "clientInfo":{"name":"tiptoptyp"},
            "capabilities":{
                "general":{"positionEncodings":["utf-16"]},
                "workspace":{"configuration":true,"workspaceFolders":true},
                "textDocument":{"publishDiagnostics":{"versionSupport":true},"hover":{"contentFormat":["markdown","plaintext"]},"completion":{"completionItem":{"snippetSupport":true,"documentationFormat":["markdown","plaintext"]}}}
            },"initializationOptions":configuration(provider, &snapshot.settings)
        }}))?;
        Ok(session)
    }
    fn send(&mut self, value: Value) -> Result<(), String> {
        self.outgoing
            .as_ref()
            .ok_or("Language server stopped")?
            .try_send(value)
            .map_err(|_| "Language server input is blocked or full".into())
    }
    fn notify(&mut self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
    pub(super) fn sync(&mut self, snapshot: &Snapshot) -> Result<Vec<Event>, String> {
        // Drain before advancing provenance: an unversioned publication already
        // in the mailbox belongs to the source the server had before didChange.
        // The mailbox holds at most PIPE_LIMIT entries and poll drains that many,
        // so no notification admitted before this drain is relabeled as newer.
        // Unversioned responses arriving later cannot prove their source revision.
        let mut events = if self.snapshot.identity() != snapshot.identity() {
            self.poll()?
        } else {
            Vec::new()
        };
        self.snapshot = snapshot.clone();
        if !self.initialized {
            return Ok(events);
        }
        let version = snapshot.version();
        if self.opened.as_ref() == Some(&snapshot.identity()) {
            return Ok(events);
        }
        // Old interactive replies must not accumulate while the source changes.
        for id in self.pending.keys().copied().collect::<Vec<_>>() {
            self.notify("$/cancelRequest", json!({"id":id}))?;
        }
        self.pending.clear();
        if let Some(old) = &self.opened
            && (old.uri != snapshot.uri
                || old.key.owner != snapshot.key.owner
                || old.key.epoch != snapshot.key.epoch)
        {
            self.notify(
                "textDocument/didClose",
                json!({"textDocument":{"uri":old.uri}}),
            )?;
            self.opened = None;
        }
        if self.opened.is_none() {
            self.notify("textDocument/didOpen", json!({"textDocument":{"uri":snapshot.uri,"languageId":"latex","version":version,"text":snapshot.source}}))?;
        } else {
            self.notify("textDocument/didChange", json!({"textDocument":{"uri":snapshot.uri,"version":version},"contentChanges":[{"text":snapshot.source}]}))?;
        }
        self.opened = Some(snapshot.identity());
        // Readiness belongs to this reusable server session, rather than its
        // previous document. After didOpen/didChange succeeds, a just-initialized
        // session is ready for the new tab too. Source-bound replies stay old.
        for event in &mut events {
            if let Event::Ready {
                snapshot: ready, ..
            } = event
            {
                *ready = snapshot.identity();
            }
        }
        Ok(events)
    }
    pub(super) fn request(&mut self, request: Request) -> Result<(), String> {
        if !self.initialized {
            return Err(format!("{} is still starting", self.provider.label()));
        }
        if !request.matches(&self.snapshot) {
            return Ok(());
        }
        // At most one in-flight request of each kind per document.
        let obsolete = self
            .pending
            .iter()
            .filter(|(_, (old, _))| {
                std::mem::discriminant(&old.kind) == std::mem::discriminant(&request.kind)
            })
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in obsolete {
            self.notify("$/cancelRequest", json!({"id":id}))?;
            self.pending.remove(&id);
        }
        if self.pending.len() >= MAX_PENDING {
            return Err("Too many outstanding TeX requests".into());
        }
        let (method, params) = match request.kind {
            RequestKind::Format => (
                "textDocument/formatting",
                formatting_params(self.provider, &self.snapshot.uri, &self.snapshot.settings),
            ),
            RequestKind::Completion { position, .. } => (
                "textDocument/completion",
                protocol::completion_document_params(&self.snapshot.uri, position),
            ),
            RequestKind::Hover { position, .. } => (
                "textDocument/hover",
                protocol::hover_document_params(&self.snapshot.uri, position),
            ),
        };
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        self.pending.insert(id, (request, Instant::now()));
        Ok(())
    }
    pub(super) fn poll(&mut self) -> Result<Vec<Event>, String> {
        let mut events = Vec::new();
        if !self.initialized && self.created.elapsed() > TIMEOUT {
            return Err(format!(
                "{} initialization timed out",
                self.provider.label()
            ));
        }
        if self.last_health_check.elapsed() >= Duration::from_secs(2) {
            self.last_health_check = Instant::now();
            if fs::metadata(&self.log).is_ok_and(|m| m.len() > 8 * 1024 * 1024) {
                return Err(format!("{} exceeded its log limit", self.provider.label()));
            }
        }
        for _ in 0..PIPE_LIMIT {
            let message = match self.incoming.as_ref().unwrap().try_recv() {
                Ok(message) => message?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(_) => return Err(format!("{} stopped unexpectedly", self.provider.label())),
            };
            if let Some(method) = message["method"].as_str() {
                if let Some(id) = message.get("id") {
                    let result = match method {
                        "workspace/configuration" => configuration_response(
                            self.provider,
                            &self.snapshot.settings,
                            &message["params"],
                        ),
                        "workspace/workspaceFolders" => {
                            json!([{"uri":url::Url::from_directory_path(&self.snapshot.root).ok().map(|u| u.to_string()),"name":"TeX workspace"}])
                        }
                        "client/registerCapability"
                        | "client/unregisterCapability"
                        | "window/workDoneProgress/create" => Value::Null,
                        _ => {
                            self.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported client method"}}))?;
                            continue;
                        }
                    };
                    self.send(json!({"jsonrpc":"2.0","id":id,"result":result}))?;
                } else if method == "textDocument/publishDiagnostics" {
                    let params = &message["params"];
                    if params["uri"].as_str() == Some(&self.snapshot.uri)
                        && params["version"]
                            .as_i64()
                            .is_none_or(|v| v == i64::from(self.snapshot.version()))
                    {
                        let enabled = match self.provider {
                            Provider::Texlab => self.snapshot.settings.diagnostics,
                            Provider::Badness => self.snapshot.settings.lint,
                        };
                        if enabled {
                            let diagnostics = params["diagnostics"]
                                .as_array()
                                .map(|array| {
                                    array
                                        .iter()
                                        .filter_map(protocol::parse_diagnostic)
                                        .collect()
                                })
                                .unwrap_or_default();
                            events.push(Event::Diagnostics {
                                snapshot: self.snapshot.identity(),
                                provider: self.provider,
                                diagnostics,
                            });
                        }
                    }
                }
                continue;
            }
            let Some(id) = message["id"].as_u64() else {
                continue;
            };
            if id == 1 {
                if let Some(error) = message.get("error") {
                    return Err(protocol::rpc_error_message(error));
                }
                if message["result"]["capabilities"]["positionEncoding"]
                    .as_str()
                    .is_some_and(|s| s != "utf-16")
                {
                    return Err("TeX language server selected unsupported position encoding".into());
                }
                self.initialized = true;
                self.notify("initialized", json!({}))?;
                self.notify(
                    "workspace/didChangeConfiguration",
                    json!({"settings":configuration(self.provider, &self.snapshot.settings)}),
                )?;
                events.extend(self.sync(&self.snapshot.clone())?);
                events.push(Event::Ready {
                    snapshot: self.snapshot.identity(),
                    provider: self.provider,
                });
            } else if let Some((request, _)) = self.pending.remove(&id) {
                if !request.matches(&self.snapshot) {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    events.push(Event::RequestFailed {
                        request,
                        message: protocol::rpc_error_message(error),
                    });
                    continue;
                }
                let result = &message["result"];
                let event = match request.kind {
                    RequestKind::Format => protocol::parse_format_document_result(result.clone())
                        .map_err(|e| e.to_string())
                        .map(|edits| Event::Formatted {
                            request: request.clone(),
                            edits,
                        }),
                    RequestKind::Completion { .. } => protocol::parse_completion_result(result)
                        .map(|(is_incomplete, items)| Event::Completed {
                            request: request.clone(),
                            is_incomplete,
                            items,
                        }),
                    RequestKind::Hover { .. } => Ok(Event::Hovered {
                        request: request.clone(),
                        contents: protocol::parse_hover_result(result).0,
                    }),
                };
                events.push(
                    event.unwrap_or_else(|message| Event::RequestFailed { request, message }),
                );
            }
        }
        let expired = self
            .pending
            .iter()
            .filter(|(_, (_, start))| start.elapsed() > TIMEOUT)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        for id in expired {
            let (request, _) = self.pending.remove(&id).unwrap();
            self.notify("$/cancelRequest", json!({"id":id}))?;
            events.push(Event::RequestFailed {
                request,
                message: format!("{} request timed out", self.provider.label()),
            });
        }
        if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("{} exited with {status}", self.provider.label()));
        }
        Ok(events)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        // Closing queues breaks readers blocked on delivery. Killing the owned
        // process breaks pipe IO; bounded joins also cover hostile wrappers.
        self.outgoing.take();
        self.incoming.take();
        let _ = self.child.child_mut().kill();
        for reader in self.readers.drain(..) {
            let _ = crate::process::finish_reader_with_timeout(reader, Duration::from_millis(100));
        }
    }
}
fn formatting_params(
    provider: Provider,
    uri: &str,
    settings: &crate::tex::settings::TexSettings,
) -> Value {
    let mut params = protocol::format_document_params(uri);
    if provider == Provider::Badness
        && let Some(indent) = settings
            .badness_configuration
            .get("indentWidth")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
    {
        // Badness uses the request's indentation over its JSON fallback. Keep
        // them in agreement; discovered TOML remains authoritative upstream.
        params["options"]["tabSize"] = json!(indent);
    }
    params
}

fn configuration(provider: Provider, settings: &crate::tex::settings::TexSettings) -> Value {
    match provider {
        Provider::Texlab => settings.texlab_configuration.clone(),
        Provider::Badness => settings.badness_configuration.clone(),
    }
}
fn configuration_response(
    provider: Provider,
    settings: &crate::tex::settings::TexSettings,
    params: &Value,
) -> Value {
    let settings = configuration(provider, settings);
    let items = params["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let Some(section) = item["section"].as_str().filter(|s| !s.is_empty()) else {
                        return settings.clone();
                    };
                    section
                        .split('.')
                        .fold(&settings, |value, part| &value[part])
                        .clone()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn queued_unversioned_publication_keeps_its_original_snapshot_after_sync() {
        exercise_queued_publications(true, false);
    }

    #[cfg(unix)]
    #[test]
    fn queued_initialization_readies_new_document_without_relabeling_old_diagnostics() {
        exercise_queued_publications(false, true);
    }

    #[cfg(unix)]
    fn exercise_queued_publications(initialized_before_sync: bool, switch_document: bool) {
        use std::{os::unix::fs::PermissionsExt, sync::Arc};
        use tiptoptyp_core::document::{DocumentKey, WindowSessionId};
        let directory = tempfile::tempdir().unwrap();
        let program = directory.path().join("fake-lsp");
        let wire = directory.path().join("messages.lsp");
        fs::write(
            &program,
            "#!/bin/sh\n/bin/cat \"$TTT_TEST_MESSAGES\"\nexec /bin/cat >/dev/null\n",
        )
        .unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        let diagnostic_capacity = PIPE_LIMIT - usize::from(!initialized_before_sync);
        for (clear, queued_count) in [
            (false, 1),
            (true, 1),
            (false, diagnostic_capacity),
            (true, diagnostic_capacity),
        ] {
            let settings = crate::tex::settings::TexSettings::default();
            let mut snapshot = Snapshot {
                generation: crate::lsp::Generation(1),
                key: DocumentKey::new(WindowSessionId::new(1), 2, 0),
                uri: url::Url::from_file_path(directory.path().join("main.tex"))
                    .unwrap()
                    .into(),
                source: Arc::from("old"),
                root: directory.path().into(),
                tools: crate::tex::tools::TexTools::resolve(&settings),
                settings,
            };
            snapshot.tools.texlab.program = program.clone();
            snapshot.tools.texlab.origin = crate::toolchain::ToolOrigin::Custom;
            snapshot.tools.texlab.command.environment =
                json!({"TTT_TEST_MESSAGES":wire}).to_string();
            let diagnostics = if clear {
                json!([])
            } else {
                json!([{
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}},
                    "message":"old source warning","severity":2
                }])
            };
            let mut bytes = Vec::new();
            transport::write_lsp_message(
                &mut bytes,
                &json!({"jsonrpc":"2.0","id":1,
                "result":{"capabilities":{"positionEncoding":"utf-16"}}}),
            )
            .unwrap();
            for _ in 0..queued_count {
                transport::write_lsp_message(
                    &mut bytes,
                    &json!({"jsonrpc":"2.0", "method":"textDocument/publishDiagnostics",
                    "params":{"uri":snapshot.uri,"diagnostics":diagnostics}}),
                )
                .unwrap();
            }
            fs::write(&wire, bytes).unwrap();
            let mut session = Session::start(Provider::Texlab, &snapshot).unwrap();
            // Route actual framed subprocess output through a test-owned mailbox.
            // Receiving each real pipe message first proves deterministic queue
            // admission, without sleeps or assumptions about reader scheduling.
            let incoming = session.incoming.take().unwrap();
            let (queued, receiver) = mpsc::sync_channel(PIPE_LIMIT);
            session.incoming = Some(receiver);
            queued
                .send(incoming.recv_timeout(Duration::from_secs(5)).unwrap())
                .unwrap();
            if initialized_before_sync {
                assert!(
                    session
                        .poll()
                        .unwrap()
                        .iter()
                        .any(|event| matches!(event, Event::Ready { .. }))
                );
            }
            for _ in 0..queued_count {
                let publication = incoming
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .unwrap();
                assert_eq!(publication["method"], "textDocument/publishDiagnostics");
                queued.send(Ok(publication)).unwrap();
            }
            let old = snapshot.identity();
            if switch_document {
                snapshot.key = DocumentKey::new(snapshot.key.owner, snapshot.key.epoch + 1, 0);
                snapshot.uri = url::Url::from_file_path(directory.path().join("new-document.tex"))
                    .unwrap()
                    .into();
            } else {
                snapshot.key = snapshot.key.after_edit();
            }
            snapshot.source = Arc::from("newer");
            let mut events = session.sync(&snapshot).unwrap();
            events.extend(session.poll().unwrap());
            let ready: Vec<_> = events
                .iter()
                .filter_map(|event| match event {
                    Event::Ready { snapshot, .. } => Some(snapshot.clone()),
                    _ => None,
                })
                .collect();
            let publications: Vec<_> = events
                .into_iter()
                .filter_map(|event| match event {
                    Event::Diagnostics {
                        snapshot,
                        diagnostics,
                        ..
                    } => Some((snapshot, diagnostics)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                publications.len(),
                queued_count,
                "every queued publication must remain available, including a full mailbox"
            );
            for (identity, diagnostics) in publications {
                assert_eq!(diagnostics.len(), usize::from(!clear));
                assert_eq!(
                    identity, old,
                    "old diagnostics, including empty clears, cannot become a fresh revision"
                );
            }
            assert_eq!(
                ready,
                if initialized_before_sync {
                    Vec::new()
                } else {
                    vec![snapshot.identity()]
                },
                "the initialized session must become usable for the new document, including a tab/URI switch"
            );
        }
    }
    #[test]
    fn badness_format_request_honors_configured_indentation() {
        let mut settings = crate::tex::settings::TexSettings::default();
        let uri = "file:///workspace/main.tex";
        for indent in [0, 2, 4, 8] {
            settings.badness_configuration["indentWidth"] = json!(indent);
            let params = formatting_params(Provider::Badness, uri, &settings);
            assert_eq!(params["options"]["tabSize"], indent);
            assert_eq!(params["textDocument"]["uri"], uri);
            assert_eq!(params["options"]["insertSpaces"], true);
            assert_eq!(
                formatting_params(Provider::Texlab, uri, &settings)["options"]["tabSize"],
                2
            );
        }
    }

    #[test]
    fn user_configuration_overrides_reach_whole_and_section_requests() {
        let settings = crate::tex::settings::TexSettings {
            texlab_configuration: json!({"texlab":{"build":{"onSave":true}}}),
            badness_configuration: json!({"lineWidth":100,"indentWidth":4}),
            ..Default::default()
        };
        assert_eq!(
            configuration(Provider::Texlab, &settings),
            settings.texlab_configuration
        );
        assert_eq!(
            configuration_response(
                Provider::Texlab,
                &settings,
                &json!({"items":[{"section":"texlab.build.onSave"},{"section":"missing"}]})
            ),
            json!([true, null])
        );
        assert_eq!(
            configuration_response(Provider::Badness, &settings, &json!({"items":[{}]})),
            json!([settings.badness_configuration])
        );
    }

    #[test]
    fn texlab_cannot_duplicate_build_lint_or_format_roles() {
        let config = configuration(
            Provider::Texlab,
            &crate::tex::settings::TexSettings::default(),
        );
        assert_eq!(config["texlab"]["build"]["onSave"], false);
        assert_eq!(config["texlab"]["chktex"]["onEdit"], false);
        assert_eq!(config["texlab"]["latexFormatter"], "none");
        assert_eq!(
            configuration_response(
                Provider::Texlab,
                &crate::tex::settings::TexSettings::default(),
                &json!({"items":[{"section":"texlab"},{"section":"texlab.build.onSave"}]})
            )[1],
            false
        );
    }
}
