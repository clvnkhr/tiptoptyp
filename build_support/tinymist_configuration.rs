//! One configuration contract for the build script and the running app.
//!
//! A bundled Tinymist upgrade must explicitly review this contract. Known
//! defaults use typed choices so an obsolete Boolean cannot compile as a mode.
use serde_json::{Map, Value, json};

pub const REVIEWED_VERSION: &str = "0.15.12";

/// Also usable in the runtime module's compile-time assertion.
pub const fn supports_version(version: &str) -> bool {
    let expected = REVIEWED_VERSION.as_bytes();
    let actual = version.as_bytes();
    if actual.len() != expected.len() {
        return false;
    }
    let mut index = 0;
    while index < expected.len() {
        if actual[index] != expected[index] {
            return false;
        }
        index += 1;
    }
    true
}

trait Choice: Copy + 'static {
    const ALL: &'static [Self];
    fn as_str(self) -> &'static str;
}

#[derive(Clone, Copy)]
enum Formatter {
    Typstyle,
    Typstfmt,
    Disable,
}
impl Choice for Formatter {
    const ALL: &'static [Self] = &[Self::Typstyle, Self::Typstfmt, Self::Disable];
    fn as_str(self) -> &'static str {
        match self {
            Self::Typstyle => "typstyle",
            Self::Typstfmt => "typstfmt",
            Self::Disable => "disable",
        }
    }
}

#[derive(Clone, Copy)]
enum ProseWrap {
    None,
    Fill,
    Sentence,
}
impl Choice for ProseWrap {
    const ALL: &'static [Self] = &[Self::None, Self::Fill, Self::Sentence];
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Fill => "fill",
            Self::Sentence => "sentence",
        }
    }
}

#[derive(Clone, Copy)]
enum Trigger {
    Never,
    OnSave,
    OnType,
}
impl Choice for Trigger {
    const ALL: &'static [Self] = &[Self::Never, Self::OnSave, Self::OnType];
    fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnSave => "onSave",
            Self::OnType => "onType",
        }
    }
}

struct Defaults {
    formatter: Formatter,
    print_width: u32,
    indent_size: u32,
    prose_wrap: ProseWrap,
    export_pdf: Trigger,
    lint_enabled: bool,
    lint_when: Trigger,
}

const DEFAULTS: Defaults = Defaults {
    formatter: Formatter::Typstyle,
    print_width: 120,
    indent_size: 2,
    prose_wrap: ProseWrap::None,
    export_pdf: Trigger::Never,
    lint_enabled: false,
    lint_when: Trigger::OnType,
};

pub fn defaults() -> Value {
    json!({
        "formatterMode": DEFAULTS.formatter.as_str(),
        "formatterPrintWidth": DEFAULTS.print_width,
        "formatterIndentSize": DEFAULTS.indent_size,
        "formatterProseWrap": DEFAULTS.prose_wrap.as_str(),
        "exportPdf": DEFAULTS.export_pdf.as_str(),
        "lint": { "enabled": DEFAULTS.lint_enabled, "when": DEFAULTS.lint_when.as_str() },
    })
}

fn choice<C: Choice>(object: &Map<String, Value>, key: &str) -> Result<(), String> {
    if object.get(key).is_some_and(|value| {
        !value
            .as_str()
            .is_some_and(|value| C::ALL.iter().any(|choice| choice.as_str() == value))
    }) {
        let allowed = C::ALL
            .iter()
            .map(|choice| choice.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!("{key}: choose {allowed}"));
    }
    Ok(())
}

/// Reject obsolete/invalid known settings; custom server keys remain available.
pub fn validate(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or("Configuration must be a JSON object")?;
    if object.contains_key("tinymist") {
        return Err("Use flat Tinymist configuration keys, e.g. {\"formatterPrintWidth\":100}, without a tinymist wrapper".into());
    }
    for key in ["preview", "compileStatus", "customizedShowDocument"] {
        if object.contains_key(key) {
            return Err(format!(
                "{key} is managed by the app; use the preview controls instead"
            ));
        }
    }
    choice::<Formatter>(object, "formatterMode")?;
    choice::<ProseWrap>(object, "formatterProseWrap")?;
    choice::<Trigger>(object, "exportPdf")?;
    if object
        .get("systemFonts")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err("systemFonts: choose true or false".into());
    }
    for key in ["formatterPrintWidth", "formatterIndentSize"] {
        if object.get(key).is_some_and(|value| {
            !value
                .as_u64()
                .is_some_and(|number| number <= u32::MAX as u64)
        }) {
            return Err(format!("{key}: use a non-negative whole number"));
        }
    }
    if let Some(args) = object.get("typstExtraArgs") {
        compiler_input_arguments(args)?;
    }
    if let Some(lint) = object.get("lint") {
        let lint = lint
            .as_object()
            .ok_or("lint: use an object, e.g. {\"enabled\":true}")?;
        if lint.get("enabled").is_some_and(|value| !value.is_boolean()) {
            return Err("lint.enabled: choose true or false".into());
        }
        choice::<Trigger>(lint, "when").map_err(|message| format!("lint.{message}"))?;
    }
    Ok(())
}

/// Only template inputs may precede the app's designated entry path.
pub fn compiler_input_arguments(value: &Value) -> Result<Vec<Value>, String> {
    let error =
        "typstExtraArgs: use --input=KEY=VALUE options only; the app supplies the entry document";
    let arguments = value.as_array().ok_or(error)?;
    let mut arguments = arguments.iter();
    let mut inputs = Vec::new();
    while let Some(argument) = arguments.next() {
        let argument = argument.as_str().ok_or(error)?;
        let input = if argument == "--input" {
            arguments.next().and_then(Value::as_str).ok_or(error)?
        } else {
            argument.strip_prefix("--input=").ok_or(error)?
        };
        if input.contains('\0')
            || !input
                .split_once('=')
                .is_some_and(|(key, _)| !key.is_empty())
        {
            return Err(error.into());
        }
        inputs.push(json!(format!("--input={input}")));
    }
    Ok(inputs)
}
