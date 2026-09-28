use crate::settings::ToolPreference;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BuildEngine {
    #[default]
    Tectonic,
    PdfLatex,
    XeLatex,
    LuaLatex,
}

impl BuildEngine {
    pub(crate) const ALL: [Self; 4] = [
        Self::Tectonic,
        Self::PdfLatex,
        Self::XeLatex,
        Self::LuaLatex,
    ];
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Tectonic => "Tectonic",
            Self::PdfLatex => "pdfLaTeX",
            Self::XeLatex => "XeLaTeX",
            Self::LuaLatex => "LuaLaTeX",
        }
    }
    pub(crate) fn executable(self) -> &'static str {
        match self {
            Self::Tectonic => "tectonic",
            Self::PdfLatex => "pdflatex",
            Self::XeLatex => "xelatex",
            Self::LuaLatex => "lualatex",
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Formatter {
    #[default]
    Badness,
    TexFmt,
    Disabled,
}
impl Formatter {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Badness => "Badness",
            Self::TexFmt => "tex-fmt",
            Self::Disabled => "Disabled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TexSettings {
    pub(crate) build_enabled: bool,
    pub(crate) build_engine: BuildEngine,
    pub(crate) only_cached: bool,
    /// Emit a SyncTeX map for source/preview navigation.
    pub(crate) synctex: bool,
    pub(crate) texlab_enabled: bool,
    pub(crate) completion: bool,
    pub(crate) hover: bool,
    pub(crate) diagnostics: bool,
    pub(crate) formatter: Formatter,
    pub(crate) lint: bool,
    pub(crate) embedded_diagnostics: bool,
    pub(crate) embedded_formatting: bool,
    pub(crate) tectonic: ToolPreference,
    pub(crate) texlab: ToolPreference,
    pub(crate) badness: ToolPreference,
    pub(crate) tex_fmt: ToolPreference,
    /// Badness/TexLab diagnostic codes to hide, entered as a comma-separated
    /// list in Settings. Matching is case-insensitive and ignores quotes.
    pub(crate) texlab_ignored_codes: Vec<String>,
    pub(crate) badness_ignored_codes: Vec<String>,
    pub(crate) texlab_configuration: serde_json::Value,
    pub(crate) badness_configuration: serde_json::Value,
}
impl Default for TexSettings {
    fn default() -> Self {
        Self {
            build_enabled: true,
            build_engine: BuildEngine::Tectonic,
            only_cached: false,
            synctex: true,
            texlab_enabled: true,
            completion: true,
            hover: true,
            diagnostics: true,
            formatter: Formatter::Badness,
            lint: true,
            embedded_diagnostics: true,
            embedded_formatting: true,
            tectonic: ToolPreference::default(),
            texlab: ToolPreference::default(),
            badness: ToolPreference::default(),
            tex_fmt: ToolPreference::default(),
            texlab_ignored_codes: Vec::new(),
            badness_ignored_codes: Vec::new(),
            texlab_configuration: serde_json::json!({"texlab": {"build":{"onSave":false,"forwardSearchAfter":false},"chktex":{"onOpenAndSave":false,"onEdit":false},"latexFormatter":"none","bibtexFormatter":"none","hover":{"symbols":"glyph"}}}),
            badness_configuration: serde_json::json!({}),
        }
    }
}
impl TexSettings {
    pub(crate) fn needs_badness(&self) -> bool {
        self.lint || self.formatter == Formatter::Badness
    }

    pub(crate) fn ignores_diagnostic_code(&self, provider: Option<&str>, code: &str) -> bool {
        let normalized = code.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
        let codes = match provider {
            Some(name) if name.eq_ignore_ascii_case("badness") => &self.badness_ignored_codes,
            Some(name) if name.eq_ignore_ascii_case("texlab") => &self.texlab_ignored_codes,
            _ => return false,
        };
        codes.iter().any(|candidate| {
            candidate
                .trim()
                .trim_matches(['"', '\''])
                .eq_ignore_ascii_case(&normalized)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_independent_badness_features() {
        let mut settings: TexSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, TexSettings::default());
        assert!(settings.build_enabled && settings.texlab_enabled && settings.lint);
        assert!(settings.synctex);
        settings.texlab_enabled = false;
        settings.build_enabled = false;
        settings.formatter = Formatter::TexFmt;
        assert!(
            settings.needs_badness(),
            "lint remains independent of formatting and intelligence"
        );
        settings.lint = false;
        assert!(!settings.needs_badness());
        settings.formatter = Formatter::Badness;
        assert!(
            settings.needs_badness(),
            "formatting remains available without linting"
        );
        settings.badness_ignored_codes = vec!["redundant-script-braces".into()];
        assert!(settings.ignores_diagnostic_code(Some("Badness"), "\"redundant-script-braces\""));
        assert!(!settings.ignores_diagnostic_code(Some("Badness"), "other-code"));
        assert!(!settings.ignores_diagnostic_code(Some("TexLab"), "redundant-script-braces"));
        assert!(!settings.ignores_diagnostic_code(None, "redundant-script-braces"));
        assert_eq!(
            serde_json::from_str::<TexSettings>(&serde_json::to_string(&settings).unwrap())
                .unwrap(),
            settings
        );
    }
}
