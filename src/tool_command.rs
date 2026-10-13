//! Explicit argv customization. No shell, interpolation or terminal is implicit.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ffi::OsString, process::Command};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CommandCustomization {
    /// Literal argv, replacing the retired generated-argument template field.
    /// Shell-style quoting splits values, but never evaluates shell syntax.
    #[serde(rename = "argv")]
    pub(crate) arguments: String,
    /// JSON object, kept as text so partially edited settings are not lost.
    pub(crate) environment: String,
    /// Empty preserves the operation's working directory.
    pub(crate) directory: String,
}
impl Default for CommandCustomization {
    fn default() -> Self {
        Self {
            arguments: String::new(),
            environment: "{}".into(),
            directory: String::new(),
        }
    }
}
impl CommandCustomization {
    pub(crate) fn for_tool(kind: crate::toolchain::ToolKind) -> Self {
        Self {
            arguments: kind.default_arguments().into(),
            ..Default::default()
        }
    }

    /// Validate literal argument syntax before saving.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let tokens = shlex::split(&self.arguments).ok_or("Arguments contain an unmatched quote")?;
        for token in tokens {
            if token.contains('\0') {
                return Err("Arguments cannot contain NUL".into());
            }
            if token == "{args}" || token.starts_with("{arg:") {
                return Err("Argument placeholders are not supported. Use the tool's literal default arguments; Reset command restores them.".into());
            }
        }
        let env: BTreeMap<String, String> = serde_json::from_str(&self.environment)
            .map_err(|e| format!("Environment must be a JSON object of strings: {e}"))?;
        if env
            .iter()
            .any(|(k, v)| k.is_empty() || k.contains(['=', '\0']) || v.contains('\0'))
        {
            return Err("Invalid environment variable name or value".into());
        }
        if !self.directory.is_empty()
            && (!std::path::Path::new(&self.directory).is_absolute()
                || !std::path::Path::new(&self.directory).is_dir())
        {
            return Err("Working directory must be blank or an existing absolute directory".into());
        }
        Ok(())
    }
    /// These few options are part of the app's watch, stdio and PDF contracts.
    /// Optional formatting, font and compiler options remain tool-validated.
    pub(crate) fn validate_for_tool(&self, kind: crate::toolchain::ToolKind) -> Result<(), String> {
        self.validate()?;
        let tokens = shlex::split(&self.arguments).ok_or("Arguments contain an unmatched quote")?;
        use crate::toolchain::ToolKind::*;
        let flags = &tokens[..tokens
            .iter()
            .position(|token| token == "--")
            .unwrap_or(tokens.len())];
        let valid = match kind {
            Typst => {
                tokens.first().is_some_and(|token| token == "watch")
                    && flags.iter().enumerate().any(|(index, token)| {
                        token == "--diagnostic-format=short"
                            || (token == "--diagnostic-format"
                                && flags.get(index + 1).is_some_and(|value| value == "short"))
                    })
            }
            Tinymist | Badness => tokens.first().is_some_and(|token| token == "lsp"),
            Tectonic => tokens.starts_with(&["-X".into(), "compile".into()]),
            TexFmt => ["--stdin", "--quiet"]
                .iter()
                .all(|flag| flags.iter().any(|token| token == flag)),
            Texlab => true,
        };
        if valid {
            Ok(())
        } else {
            Err(format!(
                "{} arguments must retain {} for the app's {}. Reset command restores the defaults.",
                kind.label(),
                match kind {
                    Typst => "watch --diagnostic-format short",
                    Tinymist | Badness => "lsp",
                    Tectonic => "-X compile",
                    TexFmt => "--stdin --quiet",
                    Texlab => unreachable!(),
                },
                match kind {
                    Typst => "incremental PDF builds and diagnostics",
                    Tinymist | Badness | Texlab => "language-server connection",
                    Tectonic => "PDF builds",
                    TexFmt => "stdin/stdout formatting",
                }
            ))
        }
    }

    /// Editable literal arguments precede app-managed paths and operation flags.
    /// Call before attaching stdio; preserves native paths without stringifying them.
    pub(crate) fn apply(&self, command: &mut Command) -> std::io::Result<()> {
        let invalid = |message: &str| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_owned())
        };
        self.validate().map_err(|message| invalid(&message))?;
        let tokens = shlex::split(&self.arguments)
            .ok_or_else(|| invalid("Command arguments have an unmatched quote"))?;
        let managed: Vec<OsString> = command.get_args().map(Into::into).collect();
        let mut replacement = Command::new(command.get_program());
        if let Some(dir) = command.get_current_dir() {
            replacement.current_dir(dir);
        }
        for (name, value) in command.get_envs() {
            if let Some(value) = value {
                replacement.env(name, value);
            } else {
                replacement.env_remove(name);
            }
        }
        replacement.args(tokens).args(managed);
        let environment: BTreeMap<String, String> = serde_json::from_str(&self.environment)
            .map_err(|e| {
                invalid(&format!(
                    "Command environment must be a JSON object of strings: {e}"
                ))
            })?;
        for (name, value) in environment {
            if name.is_empty() || name.contains(['=', '\0']) || value.contains('\0') {
                return Err(invalid("Invalid command environment variable"));
            }
            replacement.env(name, value);
        }
        if !self.directory.trim().is_empty() {
            replacement.current_dir(&self.directory);
        }
        *command = replacement;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_defaults_are_literal_and_preserve_operation_arguments_once() {
        use crate::toolchain::ToolKind::*;
        for kind in [Typst, Tinymist, Tectonic, Texlab, Badness, TexFmt] {
            let options = CommandCustomization::for_tool(kind);
            options.validate_for_tool(kind).unwrap();
            let expected = shlex::split(kind.default_arguments()).unwrap();
            let mut command = Command::new(kind.binary_name());
            command.args(["managed input", "managed output"]);
            options.apply(&mut command).unwrap();
            let actual = command.get_args().collect::<Vec<_>>();
            assert_eq!(actual.len(), expected.len() + 2, "{kind:?}");
            assert_eq!(
                &actual[..expected.len()],
                expected
                    .iter()
                    .map(std::ffi::OsStr::new)
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                &actual[expected.len()..],
                ["managed input", "managed output"]
            );
            assert!(!options.arguments.contains('{'));
            let mut changed = options.clone();
            changed.arguments.push_str(" --optional 'two words'");
            changed.validate_for_tool(kind).unwrap();
            if kind != Texlab {
                changed.arguments.clear();
                let error = changed.validate_for_tool(kind).unwrap_err();
                assert!(
                    error.contains(kind.label()) && error.contains("Reset command"),
                    "{error}"
                );
            }
        }
    }

    #[test]
    fn positional_values_cannot_satisfy_protocol_flags() {
        use crate::toolchain::ToolKind::*;
        for (kind, arguments) in [
            (Typst, "watch -- --diagnostic-format short"),
            (TexFmt, "-- --stdin --quiet"),
        ] {
            assert!(
                CommandCustomization {
                    arguments: arguments.into(),
                    ..Default::default()
                }
                .validate_for_tool(kind)
                .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn managed_native_paths_and_environment_removals_are_preserved() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"/tmp/native-\xff.tex".to_vec());
        let mut command = Command::new("tool");
        command
            .arg(&path)
            .env_remove("APP_REMOVED")
            .env("APP_DEFAULT", "original");
        CommandCustomization {
            arguments: "--option value".into(),
            environment: r#"{"APP_DEFAULT":"changed"}"#.into(),
            ..Default::default()
        }
        .apply(&mut command)
        .unwrap();
        assert_eq!(command.get_args().last(), Some(path.as_os_str()));
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "APP_REMOVED" && value.is_none())
        );
        assert!(
            command.get_envs().any(|(key, value)| key == "APP_DEFAULT"
                && value == Some(std::ffi::OsStr::new("changed")))
        );
    }

    #[test]
    fn command_drafts_validate_without_executing() {
        let directory = tempfile::tempdir().unwrap();
        assert!(CommandCustomization::default().validate().is_ok());
        for draft in [
            CommandCustomization {
                arguments: "'unterminated".into(),
                ..Default::default()
            },
            CommandCustomization {
                arguments: "{arg:-1}".into(),
                ..Default::default()
            },
            CommandCustomization {
                environment: r#"{"RUST_LOG":42}"#.into(),
                ..Default::default()
            },
            CommandCustomization {
                environment: r#"{"bad=name":"x"}"#.into(),
                ..Default::default()
            },
            CommandCustomization {
                directory: "~/papers".into(),
                ..Default::default()
            },
        ] {
            assert!(draft.validate().is_err(), "{draft:?}");
        }
        assert!(
            CommandCustomization {
                directory: directory.path().display().to_string(),
                ..Default::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn quoted_flags_and_shell_syntax_are_literal() {
        let mut cmd = Command::new("typst");
        cmd.args(["source with spaces.typ", "output.pdf"])
            .current_dir("/tmp");
        let options = CommandCustomization {
            arguments: "watch --features 'a b' '$(touch /tmp/never)'".into(),
            environment: r#"{"MODE":"test"}"#.into(),
            ..Default::default()
        };
        options.apply(&mut cmd).unwrap();
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                "watch",
                "--features",
                "a b",
                "$(touch /tmp/never)",
                "source with spaces.typ",
                "output.pdf"
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(std::path::Path::new("/tmp")));
        assert_eq!(
            cmd.get_envs().next().unwrap().1,
            Some(std::ffi::OsStr::new("test"))
        );
    }
    #[test]
    fn literal_options_preserve_managed_paths_and_reject_placeholders() {
        let mut cmd = Command::new("wrapper");
        cmd.args(["input", "output"]);
        let mut options = CommandCustomization {
            arguments: "compile".into(),
            ..Default::default()
        };
        options.apply(&mut cmd).unwrap();
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["compile", "input", "output"]
        );
        for invalid in ["'unfinished", "{args}", "{arg:1}"] {
            options.arguments = invalid.into();
            assert!(options.apply(&mut cmd).is_err());
            assert_eq!(
                cmd.get_args().collect::<Vec<_>>(),
                ["compile", "input", "output"]
            );
        }
    }
}
