//! Implemented document services, independent of tool installation and UI state.
use tiptoptyp_core::document::{DocumentKind, TypesettingLanguage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BuildEngineKind {
    Typst,
}

impl BuildEngineKind {
    pub(crate) fn language(self) -> TypesettingLanguage {
        match self {
            Self::Typst => TypesettingLanguage::Typst,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LanguageServiceKind {
    Tinymist,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LanguageSupport {
    pub(crate) build: Option<BuildEngineKind>,
    pub(crate) language_service: Option<LanguageServiceKind>,
    pub(crate) interactive_preview: bool,
}

impl LanguageSupport {
    pub(crate) fn for_document(kind: DocumentKind) -> Self {
        match kind.typesetting_language() {
            Some(TypesettingLanguage::Typst) => Self {
                build: Some(BuildEngineKind::Typst),
                language_service: Some(LanguageServiceKind::Tinymist),
                interactive_preview: true,
            },
            // Native TeX editing is supported. A TeX build adapter and language
            // service are separate future capabilities, never Typst fallbacks.
            Some(TypesettingLanguage::Tex) | None => Self {
                build: None,
                language_service: None,
                interactive_preview: false,
            },
        }
    }
}

pub(crate) fn preview_root<'a>(
    active_root: &'a std::path::Path,
    preview: Option<(&'a std::path::Path, bool)>,
) -> &'a std::path::Path {
    preview
        .filter(|(_, can_build)| *can_build)
        .map_or(active_root, |(root, _)| root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_root_uses_only_a_compatible_designated_document() {
        let active = std::path::Path::new("/active");
        let preview = std::path::Path::new("/preview");
        assert_eq!(preview_root(active, None), active);
        assert_eq!(preview_root(active, Some((preview, false))), active);
        assert_eq!(preview_root(active, Some((preview, true))), preview);
    }

    #[test]
    fn native_tex_is_editable_but_cannot_use_typst_services() {
        assert!(DocumentKind::Tex.is_editable());
        assert!(!DocumentKind::Tex.preview_only());
        assert_eq!(
            DocumentKind::Tex.typesetting_language(),
            Some(TypesettingLanguage::Tex)
        );
        assert_ne!(BuildEngineKind::Typst.language(), TypesettingLanguage::Tex);
        for kind in [
            DocumentKind::Tex,
            DocumentKind::Text,
            DocumentKind::Pdf,
            DocumentKind::Image,
        ] {
            let support = LanguageSupport::for_document(kind);
            assert_eq!(support.build, None);
            assert_eq!(support.language_service, None);
            assert!(!support.interactive_preview);
        }
        let typst = LanguageSupport::for_document(DocumentKind::Typst);
        assert_eq!(typst.build, Some(BuildEngineKind::Typst));
        assert_eq!(typst.language_service, Some(LanguageServiceKind::Tinymist));
        assert!(typst.interactive_preview);
    }
}
