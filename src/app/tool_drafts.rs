//! Rebase in-progress tool edits over explicit changes to committed settings.
//!
//! An external change wins only where its committed value changed. Unrelated
//! draft fields survive, including partially typed command/environment text.
use crate::{ltex::LtexSettings, tool_command::CommandCustomization};
use serde::Serialize;
use serde_json::Value;

pub(super) fn rebase_command(
    base: &CommandCustomization,
    current: &CommandCustomization,
    mut draft: CommandCustomization,
) -> CommandCustomization {
    if base.arguments != current.arguments {
        draft.arguments = current.arguments.clone();
    }
    if base.environment != current.environment {
        draft.environment = current.environment.clone();
    }
    if base.directory != current.directory {
        draft.directory = current.directory.clone();
    }
    draft
}

pub(super) fn rebase_ltex(
    base: &LtexSettings,
    current: &LtexSettings,
    mut draft: LtexSettings,
) -> LtexSettings {
    if base.enabled != current.enabled {
        draft.enabled = current.enabled;
    }
    if base.follow_document_language != current.follow_document_language {
        draft.follow_document_language = current.follow_document_language;
    }
    if base.mode != current.mode {
        draft.mode = current.mode;
    }
    if base.executable != current.executable {
        draft.executable = current.executable.clone();
    }
    draft.command = rebase_command(&base.command, &current.command, draft.command);
    overlay_changed_keys(
        &base.configuration,
        &current.configuration,
        &mut draft.configuration,
    );
    draft
}

/// Keep malformed/oversized JSON exactly as typed. It remains unapplied until
/// the user fixes it and explicitly applies the editor, just like any draft.
/// When no committed value changed, skip parsing/serialization and retain its
/// formatting as well as its contents.
pub(super) fn rebase_json<T: Serialize + PartialEq>(base: &T, current: &T, text: String) -> String {
    if base == current || text.len() > 2_000_000 {
        return text;
    }
    let Ok(mut draft) = serde_json::from_str::<Value>(&text) else {
        return text;
    };
    let base = serde_json::to_value(base).expect("tool settings serialize to JSON");
    let current = serde_json::to_value(current).expect("tool settings serialize to JSON");
    // A parseable scalar/array is still an invalid draft in an object editor.
    // Preserve it for correction instead of regenerating the entire document.
    if current.is_object() && !draft.is_object() {
        return text;
    }
    overlay_changed_keys(&base, &current, &mut draft);
    serde_json::to_string_pretty(&draft).expect("JSON values serialize")
}

fn overlay_changed_keys(base: &Value, current: &Value, draft: &mut Value) {
    if base == current {
        return;
    }
    if let (Some(base), Some(current), Some(draft)) =
        (base.as_object(), current.as_object(), draft.as_object_mut())
    {
        for key in base.keys() {
            if !current.contains_key(key) {
                draft.remove(key);
            }
        }
        for (key, value) in current {
            match (base.get(key), draft.get_mut(key)) {
                (Some(old), Some(edited)) => overlay_changed_keys(old, value, edited),
                (Some(old), None) if old == value => {}
                _ => {
                    draft.insert(key.clone(), value.clone());
                }
            }
        }
    } else {
        *draft = current.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::ToolMode;
    use serde_json::json;

    #[test]
    fn command_preset_replaces_only_arguments_and_keeps_unfinished_environment_and_path() {
        let base = CommandCustomization::default();
        let mut current = base.clone();
        current.arguments = "--verbose".into();
        let mut draft = base.clone();
        draft.arguments = "--old-argument".into();
        draft.environment = "{\"JAVA_OPTS\":\"-Xmx".into();
        draft.directory = "/custom/unfinished path".into();
        let rebased = rebase_command(&base, &current, draft.clone());
        assert_eq!(rebased.arguments, current.arguments);
        assert_eq!(rebased.environment, draft.environment);
        assert_eq!(rebased.directory, draft.directory);
        assert!(rebased.validate().is_err());
    }

    #[test]
    fn unchanged_command_preserves_every_draft_field_and_external_changes_win_conflicts() {
        let base = CommandCustomization::default();
        let draft = CommandCustomization {
            arguments: "--draft".into(),
            environment: "draft environment".into(),
            directory: "/draft".into(),
        };
        assert_eq!(rebase_command(&base, &base, draft.clone()), draft);
        let current = CommandCustomization {
            arguments: "--external".into(),
            environment: "{}".into(),
            directory: "/external".into(),
        };
        let rebased = rebase_command(&base, &current, draft.clone());
        assert_eq!(rebased.arguments, current.arguments);
        // The factory environment is also "{}", so it did not change.
        assert_eq!(rebased.environment, draft.environment);
        assert_eq!(rebased.directory, current.directory);
    }

    #[test]
    fn ltex_preset_retains_unapplied_executable_language_dictionary_and_nested_config_edits() {
        let base = LtexSettings::default();
        let mut current = base.clone();
        current.configuration["disabledRules"] = json!({});
        current.configuration["ltex-ls"]["logLevel"] = json!("fine");
        let mut draft = base.clone();
        draft.mode = ToolMode::Custom;
        draft.executable = "/custom/ltex".into();
        draft.follow_document_language = false;
        draft.configuration["language"] = json!("de-DE");
        draft.configuration["dictionary"] = json!({"de-DE":["Customwort"]});
        draft.configuration["ltex-ls"]["path"] = json!("/custom/language-tool");
        draft.command.environment = "{\"JAVA_OPTS\":\"".into();
        let rebased = rebase_ltex(&base, &current, draft.clone());
        assert_eq!(rebased.mode, draft.mode);
        assert_eq!(rebased.executable, draft.executable);
        assert_eq!(
            rebased.follow_document_language,
            draft.follow_document_language
        );
        assert_eq!(rebased.command, draft.command);
        assert_eq!(
            rebased.configuration["language"],
            draft.configuration["language"]
        );
        assert_eq!(
            rebased.configuration["dictionary"],
            draft.configuration["dictionary"]
        );
        assert_eq!(
            rebased.configuration["ltex-ls"]["path"],
            draft.configuration["ltex-ls"]["path"]
        );
        assert_eq!(rebased.configuration["disabledRules"], json!({}));
        assert_eq!(rebased.configuration["ltex-ls"]["logLevel"], json!("fine"));
    }

    #[test]
    fn json_rebase_changes_preset_keys_without_losing_draft_added_or_removed_keys() {
        let base = json!({"tool":{"level":"warning","old":true,"keep":false},"dictionary":[]});
        let current = json!({"tool":{"level":"fine","new":true,"keep":false},"dictionary":[]});
        let text =
            r#"{"tool":{"level":"draft","old":false,"extra":42},"dictionary":["Semigroup"]}"#;
        let result: Value =
            serde_json::from_str(&rebase_json(&base, &current, text.into())).unwrap();
        assert_eq!(
            result,
            json!({"tool":{"level":"fine","new":true,"extra":42},"dictionary":["Semigroup"]})
        );
    }

    #[test]
    fn json_rebase_keeps_invalid_text_and_unchanged_formatting_exactly() {
        let base = json!({"level":"warning"});
        let current = json!({"level":"fine"});
        for text in ["{\"dictionary\": [", "[", "", "[]", "null", "false", "42"] {
            assert_eq!(rebase_json(&base, &current, text.into()), text);
        }
        let text = "{\n\t\"level\" : \"warning\"\n}";
        assert_eq!(rebase_json(&base, &base, text.into()), text);
        let oversized = " ".repeat(2_000_001);
        assert_eq!(rebase_json(&base, &current, oversized.clone()), oversized);
    }

    #[test]
    fn json_external_scalar_and_array_changes_replace_only_the_changed_value() {
        let base = json!({"rules":["old"],"mode":{"enabled":false},"other":true});
        let current = json!({"rules":[],"mode":false,"other":true});
        let text = r#"{"rules":["draft"],"mode":{"extra":"draft"},"other":false}"#;
        let result: Value =
            serde_json::from_str(&rebase_json(&base, &current, text.into())).unwrap();
        assert_eq!(result, json!({"rules":[],"mode":false,"other":false}));
    }
}
