//! Pure LSP data/feature codecs and Tinymist preview extensions. No UI, process or session state.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use tiptoptyp_core::text::{LineIndex, LspPosition, LspRange, LspTextEdit, ScalarColumn};

pub(crate) const DEFAULT_PREVIEW_TASK_ID: &str = "default_preview";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocument {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Serialize)]
pub(crate) struct Notification<'a, Params> {
    pub(crate) jsonrpc: &'static str,
    pub(crate) method: &'a str,
    pub(crate) params: Params,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DidOpenParams<'a> {
    pub(crate) text_document: &'a TextDocument,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DidChangeParams<'a> {
    pub(crate) text_document: VersionedDocument<'a>,
    pub(crate) content_changes: [ContentChange<'a>; 1],
}

#[derive(Serialize)]
pub(crate) struct VersionedDocument<'a> {
    pub(crate) uri: &'a str,
    pub(crate) version: i32,
}

#[derive(Serialize)]
pub(crate) struct ContentChange<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) range: Option<LspRange>,
    pub(crate) text: &'a str,
}
impl<'a> ContentChange<'a> {
    pub(crate) fn full(text: &'a str) -> Self {
        Self { range: None, text }
    }

    /// A bounded linear scan, with no document-sized line index or diff matrix.
    /// The payload borrows only the changed span of the new source. This runs
    /// in each provider's worker, never the editor paint path.
    pub(crate) fn between(before: &str, after: &'a str) -> Self {
        // Bare CR is a line delimiter in LSP, while the editor's coordinate
        // index uses LF/CRLF. A full change is valid for incremental servers
        // too and avoids sending an ambiguous range for that source.
        if before
            .match_indices('\r')
            .any(|(byte, _)| before.as_bytes().get(byte + 1) != Some(&b'\n'))
        {
            return Self::full(after);
        }
        let mut prefix = before
            .bytes()
            .zip(after.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let mut suffix = before[prefix..]
            .bytes()
            .rev()
            .zip(after[prefix..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let mut end = before.len() - suffix;
        while !before.is_char_boundary(end) {
            end += 1;
        }
        suffix = before.len() - end;
        // Neither endpoint may lie in the middle of the CRLF line delimiter.
        if prefix > 0
            && before.as_bytes()[prefix - 1] == b'\r'
            && before.as_bytes().get(prefix) == Some(&b'\n')
        {
            prefix -= 1;
        }
        if end > 0
            && before.as_bytes()[end - 1] == b'\r'
            && before.as_bytes().get(end) == Some(&b'\n')
        {
            end += 1;
            suffix -= 1;
        }
        let position = |byte: usize| {
            let prefix = &before[..byte];
            let line = prefix.bytes().filter(|&byte| byte == b'\n').count() as u32;
            let tail = prefix.rsplit_once('\n').map_or(prefix, |(_, tail)| tail);
            LspPosition::new(line, tail.encode_utf16().count() as u32)
        };
        Self {
            range: Some(LspRange {
                start: position(prefix),
                end: position(end),
            }),
            text: &after[prefix..after.len() - suffix],
        }
    }
}

pub(crate) fn incremental_sync(capabilities: &Value) -> bool {
    let sync = &capabilities["textDocumentSync"];
    sync.as_u64() == Some(2) || sync["change"].as_u64() == Some(2)
}

/// All source coordinates and edits use UTF-16. Omission selects the LSP
/// default; an explicitly different or malformed encoding cannot be used.
pub(crate) fn validate_position_encoding(capabilities: &Value) -> Result<(), String> {
    if capabilities
        .get("positionEncoding")
        .is_some_and(|encoding| encoding.as_str() != Some("utf-16"))
    {
        return Err(
            "Language server selected an unsupported position encoding; UTF-16 is required".into(),
        );
    }
    Ok(())
}

/// Where a candidate was created, independent of its snippet/text edit format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionOrigin {
    LanguageServer,
    Snippet,
    BuiltIn,
    Font,
}
impl CompletionOrigin {
    pub fn label(self) -> &'static str {
        match self {
            Self::LanguageServer => "Language server",
            Self::Snippet => "Snippet",
            Self::BuiltIn => "Built-in completion",
            Self::Font => "Installed font",
        }
    }
}

/// One completion candidate returned by a server or local provider.
///
/// The model intentionally keeps only the standard fields the editor can
/// apply safely. Commands attached to completion items are not executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    pub origin: CompletionOrigin,
    pub label: String,
    pub detail: Option<String>,
    pub documentation: Option<String>,
    pub filter_text: Option<String>,
    pub sort_text: Option<String>,
    pub insert_text: String,
    pub insert_text_is_snippet: bool,
    pub text_edit: Option<LspTextEdit>,
    pub additional_text_edits: Vec<LspTextEdit>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
    Other(u64),
}

impl DiagnosticSeverity {
    fn from_lsp(value: u64) -> Self {
        match value {
            1 => Self::Error,
            2 => Self::Warning,
            3 => Self::Information,
            4 => Self::Hint,
            other => Self::Other(other),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LspDiagnostic {
    pub range: LspRange,
    pub severity: Option<DiagnosticSeverity>,
    pub code: Option<Value>,
    pub source: Option<String>,
    pub message: String,
    /// The complete diagnostic, including related information, tags and data.
    pub raw: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompileStatus {
    Compiling,
    CompileSuccess,
    CompileError,
}

pub(crate) fn format_document_params(uri: &str) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "options": {
            "tabSize": 2,
            "insertSpaces": true,
        },
    })
}

pub(crate) fn hover_document_params(uri: &str, position: LspPosition) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": position,
    })
}

pub(crate) fn completion_document_params(uri: &str, position: LspPosition) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": position,
        "context": { "triggerKind": 1 },
    })
}

pub(crate) fn parse_hover_result(result: &Value) -> (Option<String>, Option<LspRange>) {
    let Some(object) = result.as_object() else {
        return (None, None);
    };
    let range = object
        .get("range")
        .and_then(|range| serde_json::from_value(range.clone()).ok());
    let contents = object
        .get("contents")
        .and_then(flatten_hover_contents)
        .map(|contents| contents.trim().to_owned())
        .filter(|contents| !contents.is_empty());
    (contents, range)
}

fn flatten_hover_contents(contents: &Value) -> Option<String> {
    match contents {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => {
            let parts = parts
                .iter()
                .filter_map(flatten_hover_contents)
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>();
            (!parts.is_empty()).then(|| parts.join("\n\n"))
        }
        Value::Object(object) => {
            let value = object.get("value")?.as_str()?;
            let language = object.get("language").and_then(Value::as_str);
            Some(language.map_or_else(
                || value.to_owned(),
                |language| format!("```{}\n{value}\n```", language.trim()),
            ))
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => None,
    }
}

pub(crate) fn parse_completion_result(
    result: &Value,
) -> std::result::Result<(bool, Vec<CompletionItem>), String> {
    if result.is_null() {
        return Ok((false, Vec::new()));
    }
    let (is_incomplete, items, defaults) = if let Some(items) = result.as_array() {
        (false, items.as_slice(), None)
    } else {
        let object = result
            .as_object()
            .ok_or_else(|| "completion result is neither a list nor CompletionList".to_owned())?;
        let items = object
            .get("items")
            .and_then(Value::as_array)
            .ok_or_else(|| "CompletionList.items is not an array".to_owned())?;
        (
            object
                .get("isIncomplete")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            items.as_slice(),
            object.get("itemDefaults").and_then(Value::as_object),
        )
    };

    let default_range = defaults
        .and_then(|defaults| defaults.get("editRange"))
        .and_then(completion_edit_range);
    let default_snippet = defaults
        .and_then(|defaults| defaults.get("insertTextFormat"))
        .and_then(Value::as_u64)
        == Some(2);
    let mut parsed = Vec::with_capacity(items.len());
    for value in items {
        let object = value
            .as_object()
            .ok_or_else(|| "completion item is not an object".to_owned())?;
        let label = object
            .get("label")
            .and_then(Value::as_str)
            .ok_or_else(|| "completion item has no string label".to_owned())?
            .to_owned();
        let inserted = object
            .get("textEditText")
            .or_else(|| object.get("insertText"))
            .and_then(Value::as_str)
            .unwrap_or(&label)
            .to_owned();
        let text_edit = match object.get("textEdit") {
            Some(value) => Some(parse_completion_text_edit(value)?),
            None => default_range.map(|range| LspTextEdit {
                range,
                new_text: inserted.clone(),
            }),
        };
        let additional_text_edits = object
            .get("additionalTextEdits")
            .map(parse_completion_additional_edits)
            .transpose()?
            .unwrap_or_default();
        parsed.push(CompletionItem {
            origin: CompletionOrigin::LanguageServer,
            label,
            detail: object
                .get("detail")
                .and_then(Value::as_str)
                .map(str::to_owned),
            documentation: object
                .get("documentation")
                .and_then(completion_documentation),
            filter_text: object
                .get("filterText")
                .and_then(Value::as_str)
                .map(str::to_owned),
            sort_text: object
                .get("sortText")
                .and_then(Value::as_str)
                .map(str::to_owned),
            insert_text: inserted,
            insert_text_is_snippet: object
                .get("insertTextFormat")
                .and_then(Value::as_u64)
                .map_or(default_snippet, |format| format == 2),
            text_edit,
            additional_text_edits,
        });
    }
    Ok((is_incomplete, parsed))
}

fn completion_edit_range(value: &Value) -> Option<LspRange> {
    serde_json::from_value(value.clone()).ok().or_else(|| {
        value
            .get("replace")
            .or_else(|| value.get("insert"))
            .and_then(|range| serde_json::from_value(range.clone()).ok())
    })
}

fn parse_completion_text_edit(value: &Value) -> std::result::Result<LspTextEdit, String> {
    let new_text = value
        .get("newText")
        .and_then(Value::as_str)
        .ok_or_else(|| "completion textEdit has no newText".to_owned())?
        .to_owned();
    let range = value
        .get("range")
        .or_else(|| value.get("replace"))
        .or_else(|| value.get("insert"))
        .and_then(|range| serde_json::from_value(range.clone()).ok())
        .ok_or_else(|| "completion textEdit has no valid range".to_owned())?;
    Ok(LspTextEdit { range, new_text })
}

fn parse_completion_additional_edits(
    value: &Value,
) -> std::result::Result<Vec<LspTextEdit>, String> {
    let values = value
        .as_array()
        .ok_or_else(|| "additionalTextEdits is not an array".to_owned())?;
    values.iter().map(parse_completion_text_edit).collect()
}

fn completion_documentation(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .map(str::to_owned),
        _ => None,
    }
}

pub(crate) fn parse_format_document_result(
    result: Value,
) -> std::result::Result<Vec<LspTextEdit>, serde_json::Error> {
    // LSP formatting permits a successful null result (Badness uses it for
    // unchanged documents). Unsupported methods arrive as JSON-RPC errors.
    if result.is_null() {
        Ok(Vec::new())
    } else {
        serde_json::from_value(result)
    }
}

pub(crate) fn scroll_preview_params(
    path: &Path,
    line: LineIndex,
    character: ScalarColumn,
) -> Value {
    json!({
        "command": "tinymist.scrollPreview",
        "arguments": [
            DEFAULT_PREVIEW_TASK_ID,
            {
                "event": "panelScrollTo",
                "filepath": path.to_string_lossy(),
                "line": line,
                "character": character,
            }
        ],
    })
}

pub(crate) fn parse_port(value: &Value) -> Option<u16> {
    let port = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())?;
    let port = u16::try_from(port).ok()?;
    (port != 0).then_some(port)
}

pub(crate) fn rpc_error_message(error: &Value) -> String {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown JSON-RPC error");
    match error.get("code").and_then(Value::as_i64) {
        Some(code) => format!("{message} (JSON-RPC error {code})"),
        None => message.to_owned(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShowDocumentParams {
    pub(crate) uri: String,
    pub(crate) external: Option<bool>,
    pub(crate) take_focus: Option<bool>,
    pub(crate) selection: Option<LspRange>,
}

pub(crate) fn configuration_response(params: &Value, settings: &Value) -> Value {
    let Some(items) = params.get("items").and_then(Value::as_array) else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .map(|item| {
                item.get("section")
                    .and_then(Value::as_str)
                    .map(|section| configuration_section(settings, section))
                    .unwrap_or_else(|| settings.clone())
            })
            .collect(),
    )
}

fn configuration_section(settings: &Value, section: &str) -> Value {
    if section.is_empty() || section == "tinymist" {
        return settings.clone();
    }
    let section = section.strip_prefix("tinymist.").unwrap_or(section);
    section
        .split('.')
        .try_fold(settings, |value, component| value.get(component))
        .cloned()
        .unwrap_or(Value::Null)
}

pub(crate) fn parse_diagnostic(raw: &Value) -> Option<LspDiagnostic> {
    let range = serde_json::from_value(raw.get("range")?.clone()).ok()?;
    let message = raw.get("message")?.as_str()?.to_owned();
    let severity = raw
        .get("severity")
        .and_then(Value::as_u64)
        .map(DiagnosticSeverity::from_lsp);
    let code = raw.get("code").filter(|code| !code.is_null()).cloned();
    let source = raw
        .get("source")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    Some(LspDiagnostic {
        range,
        severity,
        code,
        source,
        message,
        raw: raw.clone(),
    })
}

#[derive(Deserialize)]
pub(crate) struct CompileReport {
    pub(crate) path: String,
    pub(crate) status: CompileStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_encoding_defaults_to_utf16_and_rejects_other_or_malformed_choices() {
        for capabilities in [json!({}), json!({"positionEncoding":"utf-16"})] {
            assert!(validate_position_encoding(&capabilities).is_ok());
        }
        for encoding in [
            json!("utf-8"),
            json!("utf-32"),
            json!(false),
            json!(null),
            json!(16),
        ] {
            assert!(validate_position_encoding(&json!({"positionEncoding":encoding})).is_err());
        }
    }

    /// A deliberately simple receiver: decode the serialized UTF-16 range
    /// against complete lines, independently of both the diff and core index.
    fn receive_change(source: &str, wire: &Value) -> String {
        let replacement = wire["text"].as_str().unwrap();
        let Some(range) = wire.get("range") else {
            return replacement.into();
        };
        let position = |value: &Value| {
            let line = value["line"].as_u64().unwrap() as usize;
            let column = value["character"].as_u64().unwrap() as usize;
            let mut lines = source.split('\n');
            let mut base = 0;
            for _ in 0..line {
                base += lines.next().expect("range line exists").len() + 1;
            }
            let raw = lines.next().expect("range line exists");
            let content = raw.strip_suffix('\r').unwrap_or(raw);
            let mut units = 0;
            for (byte, character) in content.char_indices() {
                if units == column {
                    return base + byte;
                }
                units += character.len_utf16();
                assert!(units <= column, "range splits a surrogate: {wire}");
            }
            assert_eq!(units, column, "range extends past EOL: {wire}");
            base + content.len()
        };
        let start = position(&range["start"]);
        let end = position(&range["end"]);
        assert!(start <= end, "reversed range: {wire}");
        let mut result = source.to_owned();
        result.replace_range(start..end, replacement);
        result
    }

    fn short_sources() -> Vec<String> {
        // Shared UTF-8 prefixes, supplementary scalars, combining sequences,
        // and every line-ending transition, including unsupported bare CR.
        const TOKENS: [&str; 7] = ["a", "😀", "😁", "文", "e\u{301}", "\n", "\r"];
        let mut sources = vec![String::new()];
        let mut generation = vec![String::new()];
        for _ in 0..3 {
            generation = generation
                .iter()
                .flat_map(|prefix| TOKENS.map(|token| format!("{prefix}{token}")))
                .collect();
            sources.extend(generation.iter().cloned());
        }
        sources
    }

    #[test]
    fn generated_incremental_wire_changes_reconstruct_every_short_source_pair() {
        // Exactly 400 short sources / 160,000 ordered transitions, without
        // randomness, shrinking, external processes, or unbounded input sizes.
        let sources = short_sources();
        for before in &sources {
            for after in &sources {
                let change = ContentChange::between(before, after);
                let wire = serde_json::to_value(&change).unwrap();
                assert_eq!(
                    receive_change(before, &wire),
                    *after,
                    "before={before:?}, after={after:?}, wire={wire}"
                );
                let bare_cr = before
                    .as_bytes()
                    .windows(2)
                    .any(|pair| pair[0] == b'\r' && pair[1] != b'\n')
                    || before.ends_with('\r');
                assert_eq!(change.range.is_none(), bare_cr, "before={before:?}");
            }
        }
    }

    #[test]
    fn generated_sequential_wire_changes_never_use_stale_coordinates() {
        // Fixed 32 seeds × 32 edits. Failure messages identify the exact
        // replayable seed/step; this is bounded generation, not a fuzz runner.
        let sources = short_sources();
        for seed in 0..32 {
            let mut sender = format!("before-{seed} α😀\r\nlast");
            let mut receiver = sender.clone();
            for step in 0..32 {
                let scalars = sender.chars().collect::<Vec<_>>();
                let start = (seed * 17 + step * 11) % (scalars.len() + 1);
                let end = start + (seed + step * 3) % (scalars.len() - start + 1);
                let replacement = &sources[(seed * 31 + step * 53) % sources.len()];
                let after = scalars[..start]
                    .iter()
                    .chain(replacement.chars().collect::<Vec<_>>().iter())
                    .chain(scalars[end..].iter())
                    .collect::<String>();
                let wire = serde_json::to_value(ContentChange::between(&sender, &after)).unwrap();
                receiver = receive_change(&receiver, &wire);
                assert_eq!(receiver, after, "seed={seed}, step={step}, wire={wire}");
                sender = after;
            }
        }
    }

    #[test]
    fn incremental_payload_borrows_only_the_changed_span_of_large_sources() {
        let change = ContentChange::between("文😀 old", "文😀 new");
        assert_eq!(change.range.unwrap().start, LspPosition::new(0, 4));
        let before = format!(
            "{}old{}",
            "unchanged\n".repeat(10_000),
            "tail\n".repeat(10_000)
        );
        let after = before.replacen("old", "new", 1);
        let change = ContentChange::between(&before, &after);
        assert_eq!(change.text, "new");
        assert_eq!(change.range.unwrap().start, LspPosition::new(10_000, 0));
        assert_eq!(change.text.as_ptr(), after[100_000..].as_ptr());
        assert!(serde_json::to_string(&change).unwrap().len() < 150);
    }

    #[test]
    fn incremental_sync_requires_an_explicit_server_capability() {
        for capabilities in [
            json!({}),
            json!({"textDocumentSync":0}),
            json!({"textDocumentSync":1}),
            json!({"textDocumentSync":{"change":1}}),
        ] {
            assert!(!incremental_sync(&capabilities));
        }
        for capabilities in [
            json!({"textDocumentSync":2}),
            json!({"textDocumentSync":{"change":2}}),
        ] {
            assert!(incremental_sync(&capabilities));
        }
        assert_eq!(
            serde_json::to_value(ContentChange::full("whole")).unwrap(),
            json!({"text":"whole"})
        );
    }

    #[test]
    fn notifications_borrow_document_text_and_preserve_wire_fields() {
        let document = TextDocument {
            uri: "file:///project/main.typ".into(),
            language_id: "typst".into(),
            version: 7,
            text: "λ\n\"quoted\"".into(),
        };
        let open = Notification {
            jsonrpc: "2.0",
            method: "textDocument/didOpen",
            params: DidOpenParams {
                text_document: &document,
            },
        };
        assert!(std::ptr::eq(open.params.text_document, &document));
        assert_eq!(
            serde_json::to_value(open).unwrap(),
            json!({
                "jsonrpc": "2.0", "method": "textDocument/didOpen",
                "params": { "textDocument": {
                    "uri": document.uri, "languageId": "typst", "version": 7, "text": document.text,
                } },
            })
        );
        let change = Notification {
            jsonrpc: "2.0",
            method: "textDocument/didChange",
            params: DidChangeParams {
                text_document: VersionedDocument {
                    uri: &document.uri,
                    version: 8,
                },
                content_changes: [ContentChange::full(&document.text)],
            },
        };
        assert!(std::ptr::eq(
            change.params.content_changes[0].text,
            document.text.as_str()
        ));
        assert_eq!(
            serde_json::to_value(change).unwrap(),
            json!({
                "jsonrpc": "2.0", "method": "textDocument/didChange",
                "params": {
                    "textDocument": { "uri": document.uri, "version": 8 },
                    "contentChanges": [{ "text": document.text }],
                },
            })
        );
    }

    #[test]
    fn server_dtos_keep_optional_fields_and_reject_invalid_payloads() {
        let request: ShowDocumentParams =
            serde_json::from_value(json!({"uri":"file:///main.typ"})).unwrap();
        assert_eq!(request.uri, "file:///main.typ");
        assert_eq!(request.selection, None);
        assert_eq!(request.external, None);
        assert_eq!(request.take_focus, None);
        assert!(serde_json::from_value::<ShowDocumentParams>(json!({"uri":42})).is_err());
        let report: CompileReport =
            serde_json::from_value(json!({"path":"main.typ","status":"compileSuccess"})).unwrap();
        assert_eq!(report.status, CompileStatus::CompileSuccess);
        assert_eq!(report.path, "main.typ");
        assert!(
            serde_json::from_value::<CompileReport>(json!({"path":"main.typ","status":"unknown"}))
                .is_err()
        );
    }
}
