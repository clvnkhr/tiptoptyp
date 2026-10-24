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
/// Start in the active source language while preserving the user's template order.
/// Untyped documents and libraries without a match keep their first template.
pub(crate) fn initial_selection(templates: &[Template], language: Option<Language>) -> usize {
    language
        .and_then(|language| {
            templates
                .iter()
                .position(|template| template.language == language)
        })
        .unwrap_or(0)
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
    fn initial_template_matches_each_source_language_without_reordering_the_library() {
        let templates = defaults();
        let before = templates.clone();
        for language in [Language::Typst, Language::Tex] {
            let index = initial_selection(&templates, Some(language));
            assert_eq!(templates[index].language, language);
            assert_eq!(
                Some(index),
                templates
                    .iter()
                    .position(|template| template.language == language)
            );
        }
        assert_eq!(templates, before);
        assert_eq!(initial_selection(&templates, None), 0);
        let mut reordered = templates;
        reordered.reverse();
        for language in [Language::Typst, Language::Tex] {
            let index = initial_selection(&reordered, Some(language));
            assert_eq!(reordered[index].language, language);
            assert_eq!(
                Some(index),
                reordered
                    .iter()
                    .position(|template| template.language == language)
            );
        }
    }

    #[test]
    fn absent_language_and_empty_libraries_keep_the_available_first_choice() {
        let typst_only: Vec<_> = defaults()
            .into_iter()
            .filter(|template| template.language == Language::Typst)
            .collect();
        assert_eq!(initial_selection(&typst_only, Some(Language::Tex)), 0);
        let tex_only: Vec<_> = defaults()
            .into_iter()
            .filter(|template| template.language == Language::Tex)
            .collect();
        assert_eq!(initial_selection(&tex_only, Some(Language::Typst)), 0);
        assert_eq!(initial_selection(&[], Some(Language::Tex)), 0);
        assert_eq!(initial_selection(&[], None), 0);
    }

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
