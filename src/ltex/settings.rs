use super::SOURCE_LIMIT;
use crate::tool_command::CommandCustomization;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LtexSettings {
    pub(crate) enabled: bool,
    pub(crate) executable: String,
    pub(crate) command: CommandCustomization,
    /// LTeX settings themselves, without a surrounding `ltex` key.
    pub(crate) configuration: Value,
}
impl Default for LtexSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            executable: "ltex-ls-plus".into(),
            command: CommandCustomization::default(),
            configuration: json!({
                "language":"en-GB", "checkFrequency":"manual", "completionEnabled":false,
                "additionalRules":{"enablePickyRules":false}, "ltex-ls":{"logLevel":"warning"}
            }),
        }
    }
}
impl LtexSettings {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.executable.trim().is_empty() || self.executable.contains('\0') {
            return Err("LTeX+ executable must be a nonempty program name or path".into());
        }
        self.command
            .validate()
            .map_err(|e| format!("LTeX+ command: {e}"))?;
        let config = self
            .configuration
            .as_object()
            .ok_or("LTeX+ configuration must be a JSON object")?;
        if serde_json::to_vec(&self.configuration)
            .map_err(|e| e.to_string())?
            .len()
            > SOURCE_LIMIT
        {
            return Err("LTeX+ configuration must be smaller than 2 MiB".into());
        }
        if config.contains_key("ltex") || config.keys().any(|key| key.starts_with("ltex.")) {
            return Err(
                "LTeX+ configuration contains settings directly, without the ltex prefix".into(),
            );
        }
        if let Some(value) = config.get("checkFrequency")
            && value != "manual"
        {
            return Err(
                "LTeX+ checkFrequency must be \"manual\"; the editor schedules live checks safely"
                    .into(),
            );
        }
        if let Some(value) = config.get("language")
            && !value.as_str().is_some_and(supported_language)
        {
            return Err(
                "LTeX+ language must be a supported code such as en-GB, en-US, de-DE, or auto"
                    .into(),
            );
        }
        if let Some(value) = config.get("enabled")
            && !(value.is_boolean()
                || value.as_array().is_some_and(|values| {
                    values
                        .iter()
                        .all(|v| matches!(v.as_str(), Some("latex" | "typst")))
                }))
        {
            return Err(
                "LTeX+ enabled must be true, false, or an array containing latex and/or typst"
                    .into(),
            );
        }
        for name in [
            "dictionary",
            "disabledRules",
            "enabledRules",
            "hiddenFalsePositives",
        ] {
            if let Some(value) = config.get(name)
                && !value.as_object().is_some_and(|values| {
                    values.values().all(|list| {
                        list.as_array()
                            .is_some_and(|list| list.iter().all(Value::is_string))
                    })
                })
            {
                return Err(format!(
                    "LTeX+ {name} must map language codes to arrays of strings"
                ));
            }
        }
        if let Some(value) = config.get("diagnosticSeverity")
            && !(severity_value(value)
                || value
                    .as_object()
                    .is_some_and(|values| values.values().all(severity_value)))
        {
            return Err("LTeX+ diagnosticSeverity must be error, warning, information, hint, or an object mapping rules to those severities".into());
        }
        for name in [
            "completionEnabled",
            "clearDiagnosticsWhenClosingFile",
            "paragraphCacheEnabled",
        ] {
            if config.get(name).is_some_and(|v| !v.is_boolean()) {
                return Err(format!("LTeX+ {name} must be true or false"));
            }
        }
        if let Some(value) = config.get("languageToolHttpServerUri")
            && !value.as_str().is_some_and(|v| {
                v.is_empty()
                    || url::Url::parse(v)
                        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host().is_some())
            })
        {
            return Err("LTeX+ languageToolHttpServerUri must be empty or an http(s) URL".into());
        }
        for name in [
            "sentenceCacheSize",
            "paragraphCacheTtlMinutes",
            "maxRequestSize",
        ] {
            if config.get(name).is_some_and(|v| {
                v.as_u64()
                    .is_none_or(|n| n > i32::MAX as u64 || (name == "maxRequestSize" && n == 0))
            }) {
                return Err(format!(
                    "LTeX+ {name} must be a nonnegative integer{}",
                    if name == "maxRequestSize" {
                        " greater than zero"
                    } else {
                        ""
                    }
                ));
            }
        }
        if let Some(latex) = config.get("latex") {
            let latex = latex.as_object().ok_or("LTeX+ latex must be an object")?;
            for (field, allowed) in [
                (
                    "commands",
                    &["default", "ignore", "dummy", "pluralDummy", "vowelDummy"][..],
                ),
                ("environments", &["default", "ignore"][..]),
            ] {
                if let Some(values) = latex.get(field)
                    && !values.as_object().is_some_and(|values| {
                        values.values().all(|value| {
                            value.as_str().is_some_and(|value| allowed.contains(&value))
                        })
                    })
                {
                    return Err(format!(
                        "LTeX+ latex.{field} values must be {}",
                        allowed.join(", ")
                    ));
                }
            }
        }
        if let Some(server) = config.get("ltex-ls") {
            let server = server
                .as_object()
                .ok_or("LTeX+ ltex-ls must be an object")?;
            if server.get("logLevel").is_some_and(|v| {
                !matches!(
                    v.as_str(),
                    Some("severe" | "warning" | "info" | "config" | "fine" | "finer" | "finest")
                )
            }) {
                return Err("LTeX+ ltex-ls.logLevel must be severe, warning, info, config, fine, finer, or finest".into());
            }
        }
        if let Some(additional) = config.get("additionalRules") {
            let additional = additional
                .as_object()
                .ok_or("LTeX+ additionalRules must be an object")?;
            if additional
                .get("enablePickyRules")
                .is_some_and(|v| !v.is_boolean())
            {
                return Err("LTeX+ additionalRules.enablePickyRules must be true or false".into());
            }
            if additional.get("motherTongue").is_some_and(|v| {
                !v.as_str()
                    .is_some_and(|v| v.is_empty() || supported_language(v))
            }) {
                return Err(
                    "LTeX+ additionalRules.motherTongue must be empty or a supported language code"
                        .into(),
                );
            }
            if additional
                .get("languageModel")
                .is_some_and(|v| !v.is_string())
            {
                return Err(
                    "LTeX+ additionalRules.languageModel must be a directory string".into(),
                );
            }
        }
        Ok(())
    }
    pub(super) fn effective_configuration(&self) -> Value {
        let mut settings = self.configuration.clone();
        settings["checkFrequency"] = "manual".into();
        settings
    }
}

fn severity_value(value: &Value) -> bool {
    matches!(
        value.as_str(),
        Some("error" | "warning" | "information" | "hint")
    )
}
/// Canonical languages advertised by LTeX+18.7; reject typos rather than silently
/// falling back to the server's English default. Premium-only nb is accepted for
/// an explicitly configured remote checker.
fn supported_language(value: &str) -> bool {
    matches!(
        value,
        "auto"
            | "ar"
            | "ast-ES"
            | "be-BY"
            | "br-FR"
            | "ca-ES"
            | "ca-ES-balear"
            | "ca-ES-valencia"
            | "crh-UA"
            | "da-DK"
            | "de-AT"
            | "de-CH"
            | "de-DE"
            | "de-DE-x-simple-language"
            | "el-GR"
            | "en-AU"
            | "en-CA"
            | "en-GB"
            | "en-NZ"
            | "en-US"
            | "en-ZA"
            | "eo"
            | "es-AR"
            | "es-ES"
            | "fa-IR"
            | "fr-BE"
            | "fr-CA"
            | "fr-CH"
            | "fr-FR"
            | "ga-IE"
            | "gl-ES"
            | "it-IT"
            | "ja-JP"
            | "km-KH"
            | "nb"
            | "nl-BE"
            | "nl-NL"
            | "pl-PL"
            | "pt-AO"
            | "pt-BR"
            | "pt-MZ"
            | "pt-PT"
            | "ro-RO"
            | "ru-RU"
            | "sk-SK"
            | "sl-SI"
            | "sv-SE"
            | "ta-IN"
            | "tl-PH"
            | "uk-UA"
            | "zh-CN"
    )
}
