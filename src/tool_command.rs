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
            Tectonic => return validate_tectonic_arguments(&tokens),
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
        let program = crate::bundled_payload::prepare_program(command.get_program())?;
        let mut replacement = Command::new(program);
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
/// Tectonic's root options precede `compile`; operation paths and toggles are
/// supplied by the compiler adapter. Validate the pinned CLI without spawning it.
fn validate_tectonic_arguments(tokens: &[String]) -> Result<(), String> {
    fn error(message: impl std::fmt::Display) -> String {
        format!("Tectonic arguments: {message}. Reset command restores the defaults.")
    }
    if tokens.first().map(String::as_str) != Some("-X") {
        return Err(error(
            "must retain -X followed by optional root flags and compile",
        ));
    }
    let mut index = 1;
    let mut chatter = false;
    let mut color = false;
    loop {
        let token = tokens
            .get(index)
            .ok_or_else(|| error("retain the compile subcommand"))?;
        if token == "compile" {
            index += 1;
            break;
        }
        let (flag, inline) = token
            .split_once('=')
            .map_or((token.as_str(), None), |(flag, value)| (flag, Some(value)));
        let (seen, values): (&mut bool, &[&str]) = match flag {
            "--chatter" | "-c" => (&mut chatter, &["default", "minimal"]),
            "--color" => (&mut color, &["always", "auto", "never"]),
            _ => {
                return Err(error(format!(
                    "unsupported root option {token:?}; place compile options after compile"
                )));
            }
        };
        if *seen {
            return Err(error(format!("{flag} may only be supplied once")));
        }
        *seen = true;
        let value = if let Some(value) = inline {
            value
        } else {
            index += 1;
            tokens
                .get(index)
                .map(String::as_str)
                .ok_or_else(|| error(format!("{flag} needs a value")))?
        };
        if !values.contains(&value) {
            return Err(error(format!(
                "{flag} must be one of {}",
                values.join(", ")
            )));
        }
        index += 1;
    }
    while let Some(token) = tokens.get(index) {
        let (flag, inline) = token
            .split_once('=')
            .map_or((token.as_str(), None), |(flag, value)| (flag, Some(value)));
        match flag {
            "--chatter" | "-c" | "--color" => {
                return Err(error(format!("{flag} belongs before compile")));
            }
            "--" | "--outdir" | "-o" | "--outfmt" | "--pass" | "--synctex" | "--only-cached"
            | "-C" => {
                return Err(error(format!(
                    "{flag} is managed by the app; use the dedicated Settings controls"
                )));
            }
            "--untrusted" | "--keep-logs" | "--keep-intermediates" | "-k" | "--print" | "-p" => {
                if inline.is_some() {
                    return Err(error(format!("{flag} does not take a value")));
                }
            }
            "--format" | "-f" | "--bundle" | "-b" | "--hide" | "--makefile-rules" | "--reruns"
            | "-r" | "-Z" => {
                let value = if let Some(value) = inline {
                    value
                } else {
                    index += 1;
                    tokens
                        .get(index)
                        .map(String::as_str)
                        .ok_or_else(|| error(format!("{flag} needs a value")))?
                };
                if value.is_empty() || value.starts_with('-') {
                    return Err(error(format!("{flag} needs a nonempty value")));
                }
                if matches!(flag, "--reruns" | "-r") && value.parse::<u32>().is_err() {
                    return Err(error(format!("{flag} needs a nonnegative integer")));
                }
                if flag == "-Z"
                    && matches!(
                        value.split('=').next().unwrap_or(value),
                        "help"
                            | "shell-escape"
                            | "shell-escape-cwd"
                            | "continue-on-errors"
                            | "deterministic-mode"
                    )
                {
                    return Err(error(format!(
                        "-Z {value} conflicts with app-managed compilation and source navigation"
                    )));
                }
            }
            _ => {
                return Err(error(format!(
                    "unsupported compile option {token:?}; input/output paths are supplied by the app"
                )));
            }
        }
        index += 1;
    }
    Ok(())
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
            changed.arguments.push_str(if kind == Tectonic {
                " --reruns 2"
            } else {
                " --optional 'two words'"
            });
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
    fn tectonic_root_options_are_validated_in_their_actual_cli_position() {
        use crate::toolchain::ToolKind::Tectonic;
        for arguments in [
            Tectonic.default_arguments(),
            "-X compile --untrusted --print --reruns 2 --bundle '/tmp/TeX bundle.tar'",
            "-X --color=auto -c minimal compile --keep-logs",
            "-X --chatter=default --color never compile -Z min-crossrefs=3",
        ] {
            CommandCustomization {
                arguments: arguments.into(),
                ..Default::default()
            }
            .validate_for_tool(Tectonic)
            .unwrap_or_else(|error| panic!("{arguments}: {error}"));
        }
        for arguments in [
            "-X compile --chatter minimal",
            "-X compile --color never",
            "-X --chatter compile",
            "-X --chatter noisy compile",
            "-X --color invisible compile",
            "-X --color never --color always compile",
            "-X --print compile",
            "-X --color never build",
            "-X --chatter minimal",
            "-X --color compile",
            "-X -- compile",
            "-X compile --reruns nope",
            "-X compile --reruns",
            "-X compile --keep-logs=false",
            "-X compile --print main.tex",
            "-X compile --help",
        ] {
            assert!(
                CommandCustomization {
                    arguments: arguments.into(),
                    ..Default::default()
                }
                .validate_for_tool(Tectonic)
                .is_err(),
                "{arguments}"
            );
        }
    }

    #[test]
    fn tectonic_custom_options_cannot_redirect_or_disable_managed_build_outputs() {
        use crate::toolchain::ToolKind::Tectonic;
        for option in [
            "--outdir /tmp",
            "--outdir=/tmp",
            "-o/tmp",
            "-o /tmp",
            "--outfmt xdv",
            "--outfmt=aux",
            "--pass tex",
            "--pass=tex",
            "--synctex",
            "--only-cached",
            "-C",
            "-- main.tex",
            "-Z shell-escape",
            "-Z shell-escape-cwd=/tmp",
            "-Z continue-on-errors",
            "-Z deterministic-mode",
        ] {
            let arguments = format!("{} {option}", Tectonic.default_arguments());
            assert!(
                CommandCustomization {
                    arguments,
                    ..Default::default()
                }
                .validate_for_tool(Tectonic)
                .is_err(),
                "{option}"
            );
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
