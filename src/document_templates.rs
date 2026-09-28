//! Editable, persisted starting points for new documents.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Language {
    Typst,
    Tex,
}
impl Language {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Typst => "typ",
            Self::Tex => "tex",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Template {
    pub(crate) name: String,
    pub(crate) language: Language,
    pub(crate) source: String,
}
pub(crate) fn defaults() -> Vec<Template> {
    [
        (
            "Typst — article",
            Language::Typst,
            include_str!("../manual-tests/templates/article.typ"),
        ),
        (
            "Typst — letter",
            Language::Typst,
            include_str!("../manual-tests/templates/letter.typ"),
        ),
        (
            "Typst — notes",
            Language::Typst,
            include_str!("../manual-tests/templates/notes.typ"),
        ),
        (
            "LaTeX — article",
            Language::Tex,
            include_str!("../manual-tests/templates/article.tex"),
        ),
        (
            "LaTeX — letter",
            Language::Tex,
            include_str!("../manual-tests/templates/letter.tex"),
        ),
    ]
    .into_iter()
    .map(|(name, language, source)| Template {
        name: name.into(),
        language,
        source: source.into(),
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_templates_roundtrip_and_reject_unknown_languages() {
        let templates = vec![Template {
            name: "My TeX".into(),
            language: Language::Tex,
            source: "custom source".into(),
        }];
        let encoded = serde_json::to_string(&templates).unwrap();
        assert_eq!(
            serde_json::from_str::<Vec<Template>>(&encoded).unwrap(),
            templates
        );
        let error = serde_json::from_str::<Vec<Template>>(&encoded.replace("tex", "python"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("typst") && error.contains("tex"), "{error}");
    }
}
