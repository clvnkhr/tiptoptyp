use crate::settings::ToolPreference;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum BuildEngine {
    #[default]
    Tectonic,
    /// Reserved explicitly for a future system LaTeX/latexmk adapter.
    Latex,
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
    pub(crate) texlab_enabled: bool,
    pub(crate) completion: bool,
    pub(crate) hover: bool,
    pub(crate) diagnostics: bool,
    pub(crate) formatter: Formatter,
    pub(crate) lint: bool,
    pub(crate) tectonic: ToolPreference,
    pub(crate) texlab: ToolPreference,
    pub(crate) badness: ToolPreference,
    pub(crate) tex_fmt: ToolPreference,
}
impl Default for TexSettings {
    fn default() -> Self {
        Self {
            build_enabled: true,
            build_engine: BuildEngine::Tectonic,
            only_cached: false,
            texlab_enabled: true,
            completion: true,
            hover: true,
            diagnostics: true,
            formatter: Formatter::Badness,
            lint: true,
            tectonic: ToolPreference::default(),
            texlab: ToolPreference::default(),
            badness: ToolPreference::default(),
            tex_fmt: ToolPreference::default(),
        }
    }
}
impl TexSettings {
    pub(crate) fn needs_badness(&self) -> bool {
        self.lint || self.formatter == Formatter::Badness
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
        assert_eq!(
            serde_json::from_str::<TexSettings>(&serde_json::to_string(&settings).unwrap())
                .unwrap(),
            settings
        );
    }
}
