//! Explicit argv customization. No shell, interpolation or terminal is implicit.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, ffi::OsString, process::Command};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CommandCustomization {
    /// Shell-style quoting splits arguments, but never evaluates shell syntax.
    pub(crate) arguments: String,
    /// JSON object, kept as text so partially edited settings are not lost.
    pub(crate) environment: String,
    /// Empty preserves the operation's working directory.
    pub(crate) directory: String,
}
impl Default for CommandCustomization {
    fn default() -> Self {
        Self {
            arguments: "{args}".into(),
            environment: "{}".into(),
            directory: String::new(),
        }
    }
}
impl CommandCustomization {
    /// Validate syntax before saving. Argument-index bounds depend on the operation.
    pub(crate) fn validate(&self) -> Result<(), String> {
        let tokens = shlex::split(&self.arguments).ok_or("Arguments contain an unmatched quote")?;
        for token in tokens {
            if token.contains('\0') {
                return Err("Arguments cannot contain NUL".into());
            }
            if token.starts_with("{arg:")
                && token
                    .strip_prefix("{arg:")
                    .and_then(|s| s.strip_suffix('}'))
                    .and_then(|s| s.parse::<usize>().ok())
                    .is_none()
            {
                return Err("Use {arg:N} with a non-negative argument index".into());
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
    /// Call after constructing default argv/env/cwd, before attaching stdio.
    pub(crate) fn apply(&self, command: &mut Command) -> std::io::Result<()> {
        let invalid = |message: &str| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, message.to_owned())
        };
        let tokens = shlex::split(&self.arguments)
            .ok_or_else(|| invalid("Command arguments have an unmatched quote"))?;
        let defaults: Vec<OsString> = command.get_args().map(Into::into).collect();
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
        for token in tokens {
            if token == "{args}" {
                replacement.args(&defaults);
            } else if let Some(index) = token
                .strip_prefix("{arg:")
                .and_then(|s| s.strip_suffix('}'))
            {
                let index: usize = index
                    .parse()
                    .map_err(|_| invalid("Invalid {arg:N} index"))?;
                replacement.arg(
                    defaults
                        .get(index)
                        .ok_or_else(|| invalid("Command argument index is out of range"))?,
                );
            } else {
                replacement.arg(token);
            }
        }
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
        cmd.args(["watch", "source with spaces.typ", "output.pdf"])
            .current_dir("/tmp");
        let options = CommandCustomization {
            arguments: "{args} --features 'a b' '$(touch /tmp/never)'".into(),
            environment: r#"{"MODE":"test"}"#.into(),
            ..Default::default()
        };
        options.apply(&mut cmd).unwrap();
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            [
                "watch",
                "source with spaces.typ",
                "output.pdf",
                "--features",
                "a b",
                "$(touch /tmp/never)"
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(std::path::Path::new("/tmp")));
        assert_eq!(
            cmd.get_envs().next().unwrap().1,
            Some(std::ffi::OsStr::new("test"))
        );
    }
    #[test]
    fn complete_replacement_and_invalid_settings() {
        let mut cmd = Command::new("wrapper");
        cmd.args(["watch", "input", "output"]);
        let mut options = CommandCustomization {
            arguments: "compile {arg:1} {arg:2}".into(),
            ..Default::default()
        };
        options.apply(&mut cmd).unwrap();
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            ["compile", "input", "output"]
        );
        options.arguments = "'unfinished".into();
        assert!(options.apply(&mut cmd).is_err());
    }
}
