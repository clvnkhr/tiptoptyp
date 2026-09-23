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
        if provider == Provider::Badness {
            command.arg("lsp");
        }
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
            },"initializationOptions":configuration(provider)
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
    pub(super) fn sync(&mut self, snapshot: &Snapshot) -> Result<(), String> {
        self.snapshot = snapshot.clone();
        if !self.initialized {
            return Ok(());
        }
        let version = snapshot.version();
        if self.opened.as_ref() == Some(&snapshot.identity()) {
            return Ok(());
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
        Ok(())
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
                protocol::format_document_params(&self.snapshot.uri),
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
                        "workspace/configuration" => {
                            configuration_response(self.provider, &message["params"])
                        }
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
                    json!({"settings":configuration(self.provider)}),
                )?;
                self.sync(&self.snapshot.clone())?;
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
fn configuration(provider: Provider) -> Value {
    match provider {
        Provider::Texlab => {
            json!({"texlab": {"build":{"onSave":false,"forwardSearchAfter":false},"chktex":{"onOpenAndSave":false,"onEdit":false},"latexFormatter":"none","bibtexFormatter":"none","hover":{"symbols":"glyph"}}})
        }
        Provider::Badness => json!({}),
    }
}
fn configuration_response(provider: Provider, params: &Value) -> Value {
    let settings = configuration(provider);
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
    #[test]
    fn texlab_cannot_duplicate_build_lint_or_format_roles() {
        let config = configuration(Provider::Texlab);
        assert_eq!(config["texlab"]["build"]["onSave"], false);
        assert_eq!(config["texlab"]["chktex"]["onEdit"], false);
        assert_eq!(config["texlab"]["latexFormatter"], "none");
        assert_eq!(
            configuration_response(
                Provider::Texlab,
                &json!({"items":[{"section":"texlab"},{"section":"texlab.build.onSave"}]})
            )[1],
            false
        );
    }
}
