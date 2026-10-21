//! Optional LTeX+ grammar worker. All process and protocol work stays off the UI thread.
//!
//! LTeX+ publishes diagnostics without document versions. We therefore run one
//! explicit check at a time and bind its publication to that request's snapshot.
//! Automatic server-side checking is disabled; the latest-slot scheduler supplies
//! live checks without queuing a document for every keystroke.
use crate::{
    diagnostics::{
        Diagnostic, DiagnosticCorrection, DiagnosticLocation, DiagnosticSeverity, DiagnosticSource,
    },
    document::DocumentKey,
    lsp::{protocol, transport},
    process::OwnedChild,
};
use eframe::egui;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    io::{BufReader, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};
use tiptoptyp_core::text::{LspPosition, LspRange};

const DEBOUNCE: Duration = Duration::from_millis(300);
const START_TIMEOUT: Duration = Duration::from_secs(60);
const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
const SOURCE_LIMIT: usize = 2 * 1024 * 1024;
const DIAGNOSTIC_LIMIT: usize = 4096;
const STDERR_LIMIT: usize = 8192;

mod settings;
pub(crate) use settings::LtexSettings;

#[derive(Clone)]
pub(crate) struct LtexSnapshot {
    pub(crate) key: DocumentKey,
    pub(crate) uri: String,
    pub(crate) language_id: String,
    pub(crate) source: Arc<str>,
}
impl LtexSnapshot {
    fn same(&self, other: &Self) -> bool {
        self.key == other.key && self.uri == other.uri && self.language_id == other.language_id
    }
}
pub(crate) enum LtexEvent {
    Ready,
    Diagnostics {
        key: DocumentKey,
        uri: String,
        diagnostics: Vec<Diagnostic>,
    },
    Error(String),
}
struct Shared {
    desired: Option<LtexSnapshot>,
    changed: Instant,
    serial: u64,
    stop: bool,
    ready: bool,
    result: Option<LtexEvent>,
    error: Option<String>,
}
/// One bounded input slot and one result slot; `update` never sends source to a pipe.
pub(crate) struct LtexService {
    shared: Arc<Mutex<Shared>>,
    wake: SyncSender<()>,
    worker: Option<thread::JoinHandle<()>>,
}
impl LtexService {
    pub(crate) fn spawn(settings: LtexSettings, root: PathBuf, context: egui::Context) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            desired: None,
            changed: Instant::now(),
            serial: 0,
            stop: false,
            ready: false,
            result: None,
            error: None,
        }));
        let (wake, receiver) = mpsc::sync_channel(1);
        let worker_shared = shared.clone();
        let worker_wake = wake.clone();
        let worker = thread::Builder::new()
            .name("ltex-grammar".into())
            .spawn(move || {
                if let Err(error) = run(
                    settings,
                    root,
                    &worker_shared,
                    receiver,
                    worker_wake,
                    &context,
                ) {
                    worker_shared.lock().unwrap().error = Some(error);
                    context.request_repaint();
                }
            })
            .expect("LTeX worker thread");
        Self {
            shared,
            wake,
            worker: Some(worker),
        }
    }
    /// Begin all windows' shutdown before waiting on any one worker.
    pub(crate) fn begin_stop(&self) {
        self.shared.lock().unwrap().stop = true;
        let _ = self.wake.try_send(());
    }
    /// App-exit cleanup only. Ordinary settings/document changes use nonblocking Drop.
    pub(crate) fn shutdown(mut self) {
        self.begin_stop();
        if let Some(worker) = self.worker.take() {
            crate::process::finish_reader_with_timeout(worker, Duration::from_secs(2));
        }
    }
    pub(crate) fn update(&self, snapshot: LtexSnapshot) {
        let mut shared = self.shared.lock().unwrap();
        if shared
            .desired
            .as_ref()
            .is_some_and(|old| old.same(&snapshot))
        {
            return;
        }
        shared.desired = Some(snapshot);
        shared.changed = Instant::now();
        shared.serial = shared.serial.wrapping_add(1);
        drop(shared);
        let _ = self.wake.try_send(());
    }
    pub(crate) fn close(&self) {
        let mut shared = self.shared.lock().unwrap();
        shared.desired = None;
        shared.result = None;
        shared.serial = shared.serial.wrapping_add(1);
        drop(shared);
        let _ = self.wake.try_send(());
    }
    pub(crate) fn try_recv(&self) -> Option<LtexEvent> {
        let mut shared = self.shared.lock().unwrap();
        if let Some(error) = shared.error.take() {
            return Some(LtexEvent::Error(error));
        }
        if shared.ready {
            shared.ready = false;
            return Some(LtexEvent::Ready);
        }
        shared.result.take()
    }
}
impl Drop for LtexService {
    fn drop(&mut self) {
        self.begin_stop();
        // No join on the UI thread. The worker owns graceful shutdown and reaping.
    }
}

struct Session {
    child: Option<OwnedChild>,
    outgoing: Option<SyncSender<Value>>,
    incoming: Option<Receiver<Result<Value, String>>>,
    readers: Vec<thread::JoinHandle<()>>,
    stderr: Arc<Mutex<VecDeque<u8>>>,
}
impl Session {
    fn start(
        settings: &LtexSettings,
        root: &std::path::Path,
        wake: SyncSender<()>,
    ) -> Result<Self, String> {
        let mut command = Command::new(settings.executable.trim());
        command.current_dir(root);
        settings
            .command
            .apply(&mut command)
            .map_err(|e| format!("Invalid LTeX+ command: {e}"))?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = OwnedChild::spawn(&mut command).map_err(|e| format!("Could not start LTeX+: {e}. Install ltex-ls-plus or select its executable in Settings → Writing."))?;
        let mut stdin = child.child_mut().stdin.take().unwrap();
        let stdout = child.child_mut().stdout.take().unwrap();
        let mut stderr_pipe = child.child_mut().stderr.take().unwrap();
        let (outgoing, output) = mpsc::sync_channel::<Value>(8);
        let (input, incoming) = mpsc::sync_channel(8);
        let errors = input.clone();
        let error_wake = wake.clone();
        let writer = thread::spawn(move || {
            for message in output {
                if let Err(error) = transport::write_lsp_message(&mut stdin, &message) {
                    let _ = errors.send(Err(format!("LTeX+ input: {error}")));
                    let _ = error_wake.try_send(());
                    break;
                }
            }
        });
        let reader = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let message = match transport::read_lsp_message(&mut reader) {
                    Ok(Some(value)) => Ok(value),
                    Ok(None) => Err("LTeX+ closed its output".into()),
                    Err(error) => Err(format!("LTeX+ output: {error}")),
                };
                let stop = message.is_err();
                if input.send(message).is_err() {
                    break;
                }
                let _ = wake.try_send(());
                if stop {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LIMIT)));
        let tail = stderr.clone();
        let log_reader = thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = stderr_pipe.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let mut tail = tail.lock().unwrap();
                let discard = tail
                    .len()
                    .saturating_add(count)
                    .saturating_sub(STDERR_LIMIT);
                tail.drain(..discard);
                tail.extend(&buffer[..count]);
            }
        });
        Ok(Self {
            child: Some(child),
            outgoing: Some(outgoing),
            incoming: Some(incoming),
            readers: vec![reader, writer, log_reader],
            stderr,
        })
    }
    fn send(&self, value: Value) -> Result<(), String> {
        self.outgoing
            .as_ref()
            .ok_or("LTeX+ stopped")?
            .try_send(value)
            .map_err(|_| "LTeX+ input is blocked or full".into())
    }
    fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
    }
    fn shutdown(&mut self) {
        let _ = self.send(json!({"jsonrpc":"2.0","id":0,"method":"shutdown","params":null}));
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if let Ok(Ok(message)) = self
                .incoming
                .as_ref()
                .unwrap()
                .recv_timeout(Duration::from_millis(10))
                && message["id"] == 0
            {
                break;
            }
        }
        let _ = self.notify("exit", Value::Null);
        let deadline = Instant::now() + Duration::from_millis(150);
        while Instant::now() < deadline {
            if self
                .child
                .as_mut()
                .is_none_or(|child| child.try_wait().is_ok_and(|status| status.is_some()))
            {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.outgoing.take();
        self.incoming.take();
        self.child.take();
        for reader in self.readers.drain(..) {
            crate::process::finish_reader_with_timeout(reader, Duration::from_millis(100));
        }
    }
}

struct Pending {
    id: u64,
    serial: u64,
    snapshot: LtexSnapshot,
    diagnostics: Vec<Value>,
    started: Instant,
    canceled: bool,
    actions: bool,
}
fn run(
    settings: LtexSettings,
    root: PathBuf,
    shared: &Mutex<Shared>,
    wake: Receiver<()>,
    incoming_wake: SyncSender<()>,
    context: &egui::Context,
) -> Result<(), String> {
    if shared.lock().unwrap().stop {
        return Ok(());
    }
    settings.validate()?;
    let mut session = Session::start(&settings, &root, incoming_wake)?;
    let result = run_session(&settings, &root, shared, wake, context, &mut session);
    session.shutdown();
    result.map_err(|message| {
        let tail = session
            .stderr
            .lock()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>();
        let tail = String::from_utf8_lossy(&tail);
        let tail = tail.trim();
        if tail.is_empty() {
            message
        } else {
            format!("{message}\n{tail}")
        }
    })
}
fn run_session(
    settings: &LtexSettings,
    root: &std::path::Path,
    shared: &Mutex<Shared>,
    wake: Receiver<()>,
    context: &egui::Context,
    session: &mut Session,
) -> Result<(), String> {
    let root_uri = url::Url::from_directory_path(root)
        .map_err(|()| "Invalid LTeX+ workspace path")?
        .to_string();
    session.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "processId":std::process::id(),"rootUri":root_uri,"workspaceFolders":[{"uri":root_uri,"name":"Writing workspace"}],
        "clientInfo":{"name":"tiptoptyp"},"locale":"en",
        "capabilities":{"general":{"positionEncodings":["utf-16"]},"workspace":{"configuration":true},"textDocument":{"publishDiagnostics":{"versionSupport":true},"codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix"]}}}}},
        "initializationOptions":{"customCapabilities":{"workspaceSpecificConfiguration":true}}
    }}))?;
    let created = Instant::now();
    let configuration = settings.effective_configuration();
    let mut initialized = false;
    let mut next_id = 2u64;
    let mut last_serial = 0;
    let mut opened: Option<LtexSnapshot> = None;
    let mut version = 0i32;
    let mut pending: Option<Pending> = None;
    loop {
        let (stop, desired, serial, changed) = {
            let shared = shared.lock().unwrap();
            (
                shared.stop,
                shared.desired.clone(),
                shared.serial,
                shared.changed,
            )
        };
        if stop {
            return Ok(());
        }
        if !initialized && created.elapsed() > START_TIMEOUT {
            return Err("LTeX+ initialization timed out".into());
        }
        if let Some(status) = session
            .child
            .as_mut()
            .map(|child| child.try_wait())
            .transpose()
            .map_err(|e| e.to_string())?
            .flatten()
        {
            return Err(format!("LTeX+ exited with {status}"));
        }
        if let Some(check) = &mut pending {
            if check.serial != serial && !check.canceled {
                // A canceled CompletableFuture may reply before its Java task
                // stops publishing unversioned diagnostics. Drain explicit
                // checks to their normal completion; only action requests are
                // safe to cancel because they never publish diagnostics.
                if check.actions {
                    session.notify("$/cancelRequest", json!({"id":check.id}))?;
                }
                check.canceled = true;
            }
            if check.started.elapsed() > CHECK_TIMEOUT {
                return Err("LTeX+ grammar check timed out".into());
            }
        }
        if initialized
            && pending.is_none()
            && serial != last_serial
            && (desired.is_none() || changed.elapsed() >= DEBOUNCE)
        {
            if let Some(old) = &opened
                && desired.as_ref().is_none_or(|new| {
                    old.uri != new.uri || !crate::diagnostics::same_document(old.key, new.key)
                })
            {
                session.notify(
                    "textDocument/didClose",
                    json!({"textDocument":{"uri":old.uri}}),
                )?;
                opened = None;
            }
            last_serial = serial;
            if let Some(snapshot) = desired {
                if snapshot.source.len() > SOURCE_LIMIT {
                    return Err(
                        "LTeX+ live checking is limited to documents smaller than 2 MiB".into(),
                    );
                }
                if !matches!(snapshot.language_id.as_str(), "latex" | "typst") {
                    return Err("LTeX+ supports LaTeX and Typst documents here".into());
                }
                let id = next_id;
                next_id += 1;
                session.send(json!({"jsonrpc":"2.0","id":id,"method":"workspace/executeCommand","params":{"command":"_ltex.checkDocument","arguments":[{"uri":snapshot.uri,"codeLanguageId":snapshot.language_id,"text":snapshot.source.as_ref()}]}}))?;
                pending = Some(Pending {
                    id,
                    serial,
                    snapshot,
                    diagnostics: Vec::new(),
                    started: Instant::now(),
                    canceled: false,
                    actions: false,
                });
            }
        }
        for _ in 0..16 {
            let message = match session.incoming.as_ref().unwrap().try_recv() {
                Ok(message) => message?,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(_) => return Err("LTeX+ protocol stopped unexpectedly".into()),
            };
            if let Some(method) = message["method"].as_str() {
                if let Some(id) = message.get("id") {
                    let result = match method {
                        "workspace/configuration" | "ltex/workspaceSpecificConfiguration" => {
                            configuration_response(&configuration, &message["params"])
                        }
                        "workspace/workspaceFolders" => {
                            json!([{"uri":root_uri,"name":"Writing workspace"}])
                        }
                        "client/registerCapability"
                        | "client/unregisterCapability"
                        | "window/workDoneProgress/create" => Value::Null,
                        _ => {
                            session.send(json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported client method"}}))?;
                            continue;
                        }
                    };
                    session.send(json!({"jsonrpc":"2.0","id":id,"result":result}))?;
                } else if method == "textDocument/publishDiagnostics"
                    && let Some(check) = &mut pending
                    && !check.actions
                    && message["params"]["uri"].as_str() == Some(&check.snapshot.uri)
                    && let Some(diagnostics) = message["params"]["diagnostics"].as_array()
                {
                    check.diagnostics =
                        diagnostics.iter().take(DIAGNOSTIC_LIMIT).cloned().collect();
                }
                continue;
            }
            if message["id"] == 1 {
                if let Some(error) = message.get("error") {
                    return Err(protocol::rpc_error_message(error));
                }
                if message["result"]["capabilities"]["positionEncoding"]
                    .as_str()
                    .is_some_and(|s| s != "utf-16")
                {
                    return Err("LTeX+ selected unsupported position encoding".into());
                }
                session.notify("initialized", json!({}))?;
                initialized = true;
                shared.lock().unwrap().ready = true;
                context.request_repaint();
                continue;
            }
            if !pending
                .as_ref()
                .is_some_and(|check| message["id"].as_u64() == Some(check.id))
            {
                continue;
            }
            let mut check = pending.take().unwrap();
            let current = shared.lock().unwrap().serial == check.serial;
            if !current || check.canceled {
                continue;
            }
            if let Some(error) = message.get("error") {
                if check.actions {
                    publish(shared, context, &check, &Value::Null);
                    continue;
                }
                return Err(format!(
                    "LTeX+ check failed: {}",
                    protocol::rpc_error_message(error)
                ));
            }
            if check.actions {
                publish(shared, context, &check, &message["result"]);
            } else {
                if message["result"]["success"] != true {
                    return Err("LTeX+ did not complete its grammar check".into());
                }
                // Publish immediately; enrich this same snapshot with suggestions later.
                publish(shared, context, &check, &Value::Null);
                if !check.diagnostics.is_empty() {
                    // The first explicit check pulls manual mode before any didOpen.
                    // LTeX otherwise starts with automatic edit checking and publishes
                    // an untagged result, even if didChangeConfiguration was sent.
                    version = version
                        .checked_add(1)
                        .ok_or("LTeX+ document version exhausted")?;
                    if opened.is_none() {
                        session.notify("textDocument/didOpen", json!({"textDocument":{"uri":check.snapshot.uri,"languageId":check.snapshot.language_id,"version":version,"text":check.snapshot.source.as_ref()}}))?;
                    } else {
                        session.notify("textDocument/didChange", json!({"textDocument":{"uri":check.snapshot.uri,"version":version},"contentChanges":[{"text":check.snapshot.source.as_ref()}]}))?;
                    }
                    opened = Some(check.snapshot.clone());

                    let end = end_position(&check.snapshot.source);
                    let id = next_id;
                    next_id += 1;
                    session.send(json!({"jsonrpc":"2.0","id":id,"method":"textDocument/codeAction","params":{"textDocument":{"uri":check.snapshot.uri},"range":{"start":{"line":0,"character":0},"end":end},"context":{"diagnostics":check.diagnostics,"only":["quickfix"]}}}))?;
                    check.id = id;
                    check.actions = true;
                    check.started = Instant::now();
                    pending = Some(check);
                }
            }
        }
        let delay = if initialized && pending.is_none() && serial == last_serial {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(20)
        };
        let _ = wake.recv_timeout(delay);
    }
}
fn publish(shared: &Mutex<Shared>, context: &egui::Context, check: &Pending, actions: &Value) {
    let diagnostics = convert_diagnostics(&check.snapshot, &check.diagnostics, actions);
    let mut shared = shared.lock().unwrap();
    if shared.serial == check.serial && !shared.stop {
        shared.result = Some(LtexEvent::Diagnostics {
            key: check.snapshot.key,
            uri: check.snapshot.uri.clone(),
            diagnostics,
        });
        drop(shared);
        context.request_repaint();
    }
}
fn configuration_response(settings: &Value, params: &Value) -> Value {
    Value::Array(
        params["items"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let section = item["section"].as_str().unwrap_or("");
                        if section.is_empty() {
                            return json!({"ltex":settings});
                        }
                        if section == "ltex" {
                            return settings.clone();
                        }
                        section
                            .strip_prefix("ltex.")
                            .and_then(|path| {
                                path.split('.')
                                    .try_fold(settings, |value, part| value.get(part))
                            })
                            .cloned()
                            .unwrap_or(Value::Null)
                    })
                    .collect()
            })
            .unwrap_or_default(),
    )
}
fn end_position(source: &str) -> LspPosition {
    let line = source.bytes().filter(|&b| b == b'\n').count() as u32;
    let column = source
        .rsplit('\n')
        .next()
        .unwrap_or("")
        .encode_utf16()
        .count() as u32;
    LspPosition::new(line, column)
}

/// Resolve sorted UTF-16 endpoints in one forward scan, even when thousands
/// of diagnostics share one very long line. Invalid positions are never clamped.
type SourceCoordinates = (usize, usize, usize); // byte offset, scalar offset, one-based column
struct SourceIndex {
    positions: BTreeMap<(u32, u32), Option<SourceCoordinates>>,
    #[cfg(test)]
    scanned_scalars: usize,
}
impl SourceIndex {
    fn new(source: &str, positions: impl IntoIterator<Item = LspPosition>) -> Self {
        let mut positions: BTreeMap<_, _> = positions
            .into_iter()
            .map(|p| ((p.line.get(), p.character.get()), None))
            .collect();
        let mut chars = source.char_indices().peekable();
        let (mut line, mut utf16, mut scalar, mut column, mut byte) =
            (0u32, 0u32, 0usize, 1usize, 0usize);
        for (&target, result) in &mut positions {
            while (line, utf16) < target {
                let Some((at, ch)) = chars.next() else {
                    break;
                };
                byte = at + ch.len_utf8();
                scalar += 1;
                if ch == '\n' {
                    line += 1;
                    utf16 = 0;
                    column = 1;
                } else {
                    utf16 += ch.len_utf16() as u32;
                    column += 1;
                }
            }
            if (line, utf16) == target {
                *result = Some((byte, scalar, column));
            }
        }
        Self {
            positions,
            #[cfg(test)]
            scanned_scalars: scalar,
        }
    }
    fn position(&self, position: LspPosition) -> Option<(usize, usize, usize)> {
        self.positions
            .get(&(position.line.get(), position.character.get()))
            .copied()
            .flatten()
    }
}
type RangeKey = (u32, u32, u32, u32);
fn range_key(range: LspRange) -> RangeKey {
    (
        range.start.line.get(),
        range.start.character.get(),
        range.end.line.get(),
        range.end.character.get(),
    )
}
fn action_replacements(uri: &str, actions: &Value) -> BTreeMap<RangeKey, Vec<String>> {
    let mut by_range: BTreeMap<RangeKey, Vec<String>> = BTreeMap::new();
    let mut insert = |edit: &Value| {
        let (Some(range), Some(text)) = (
            edit.get("range")
                .and_then(|v| serde_json::from_value::<LspRange>(v.clone()).ok()),
            edit["newText"].as_str(),
        ) else {
            return;
        };
        if text.len() > SOURCE_LIMIT {
            return;
        }
        let replacements = by_range.entry(range_key(range)).or_default();
        if replacements.len() < 32 && !replacements.iter().any(|old| old == text) {
            replacements.push(text.to_owned());
        }
    };
    if let Some(actions) = actions.as_array() {
        for action in actions {
            // Extract individual same-file replacements; never execute commands
            // or apply bulk edits from the server's grouped quick fixes.
            if let Some(changes) = action["edit"]["documentChanges"].as_array() {
                if changes
                    .iter()
                    .any(|change| change["textDocument"]["uri"].as_str() != Some(uri))
                {
                    continue;
                }
                for change in changes {
                    if let Some(edits) = change["edits"].as_array() {
                        for edit in edits {
                            insert(edit);
                        }
                    }
                }
            } else if let Some(changes) = action["edit"]["changes"].as_object()
                && changes.len() == 1
                && let Some(edits) = changes.get(uri).and_then(Value::as_array)
            {
                for edit in edits {
                    insert(edit);
                }
            }
        }
    }
    by_range
}
fn convert_diagnostics(snapshot: &LtexSnapshot, raw: &[Value], actions: &Value) -> Vec<Diagnostic> {
    let parsed = raw
        .iter()
        .filter_map(protocol::parse_diagnostic)
        .collect::<Vec<_>>();
    let index = SourceIndex::new(
        &snapshot.source,
        parsed.iter().flat_map(|d| [d.range.start, d.range.end]),
    );
    let replacements = action_replacements(&snapshot.uri, actions);
    parsed
        .into_iter()
        .filter_map(|lsp| {
            let (start_byte, start_scalar, column) = index.position(lsp.range.start)?;
            let (end_byte, end_scalar, _) = index.position(lsp.range.end)?;
            if end_byte < start_byte {
                return None;
            }
            let rule = lsp.code.as_ref().map(|code| {
                code.as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| code.to_string())
            });
            let mut replacements = replacements
                .get(&range_key(lsp.range))
                .cloned()
                .unwrap_or_default();
            let original = &snapshot.source[start_byte..end_byte];
            if original.contains(if snapshot.language_id == "latex" {
                &['\\', '{', '}', '$'][..]
            } else {
                &['#', '[', ']', '$', '*', '_'][..]
            }) {
                replacements.clear();
            }
            let mut correction = DiagnosticCorrection::new(
                start_scalar..end_scalar,
                snapshot.source[start_byte..end_byte].to_owned(),
                rule.clone(),
                None,
                replacements,
            );
            correction.document = Some(snapshot.key);
            let severity = match lsp.severity {
                Some(protocol::DiagnosticSeverity::Error) => DiagnosticSeverity::Error,
                Some(protocol::DiagnosticSeverity::Warning) => DiagnosticSeverity::Warning,
                Some(protocol::DiagnosticSeverity::Hint) => DiagnosticSeverity::Help,
                _ => DiagnosticSeverity::Note,
            };
            Some(Diagnostic {
                provider: Some("LTeX+".into()),
                severity,
                source: DiagnosticSource::Main,
                location: Some(DiagnosticLocation {
                    line: lsp.range.start.line.get() as usize + 1,
                    column,
                }),
                message: lsp.message,
                details: rule
                    .as_ref()
                    .map(|rule| vec![format!("Code: {rule}")])
                    .unwrap_or_default(),
                correction: Some(correction),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
