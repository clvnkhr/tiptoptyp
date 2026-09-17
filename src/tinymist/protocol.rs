//! Pure Tinymist JSON-RPC data and feature codecs. No UI, process or session state.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;
use tiptoptyp_core::text::{LineIndex, LspPosition, LspRange, LspTextEdit, ScalarColumn};

pub(super) const DEFAULT_PREVIEW_TASK_ID: &str = "default_preview";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocument {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

#[derive(Serialize)]
pub(super) struct Notification<'a, Params> {
    pub(super) jsonrpc: &'static str,
    pub(super) method: &'a str,
    pub(super) params: Params,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DidOpenParams<'a> {
    pub(super) text_document: &'a TextDocument,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DidChangeParams<'a> {
    pub(super) text_document: VersionedDocument<'a>,
    pub(super) content_changes: [ContentChange<'a>; 1],
}

#[derive(Serialize)]
pub(super) struct VersionedDocument<'a> {
    pub(super) uri: &'a str,
    pub(super) version: i32,
}

#[derive(Serialize)]
pub(super) struct ContentChange<'a> {
    pub(super) text: &'a str,
}

/// One completion candidate returned by Tinymist.
///
/// The model intentionally keeps only the standard fields the editor can
/// apply safely. Commands attached to completion items are not executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
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
pub struct TinymistDiagnostic {
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

pub(super) fn format_document_params(uri: &str) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "options": {
            "tabSize": 2,
            "insertSpaces": true,
        },
    })
}

pub(super) fn hover_document_params(uri: &str, position: LspPosition) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": position,
    })
}

pub(super) fn completion_document_params(uri: &str, position: LspPosition) -> Value {
    json!({
        "textDocument": { "uri": uri },
        "position": position,
        "context": { "triggerKind": 1 },
    })
}

pub(super) fn parse_hover_result(result: &Value) -> (Option<String>, Option<LspRange>) {
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

pub(super) fn parse_completion_result(
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

pub(super) fn parse_format_document_result(
    result: Value,
) -> std::result::Result<Option<Vec<LspTextEdit>>, serde_json::Error> {
    serde_json::from_value(result)
}

pub(super) fn scroll_preview_params(
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

pub(super) fn parse_port(value: &Value) -> Option<u16> {
    let port = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())?;
    let port = u16::try_from(port).ok()?;
    (port != 0).then_some(port)
}

pub(super) fn rpc_error_message(error: &Value) -> String {
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
pub(super) struct ShowDocumentParams {
    pub(super) uri: String,
    pub(super) external: Option<bool>,
    pub(super) take_focus: Option<bool>,
    pub(super) selection: Option<LspRange>,
}

pub(super) fn configuration_response(params: &Value, settings: &Value) -> Value {
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

pub(super) fn parse_diagnostic(raw: &Value) -> Option<TinymistDiagnostic> {
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
    Some(TinymistDiagnostic {
        range,
        severity,
        code,
        source,
        message,
        raw: raw.clone(),
    })
}

#[derive(Deserialize)]
pub(super) struct CompileReport {
    pub(super) path: String,
    pub(super) status: CompileStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

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
                content_changes: [ContentChange {
                    text: &document.text,
                }],
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
