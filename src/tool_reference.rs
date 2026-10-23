//! Small, compiled-in configuration references. Opening these does no I/O,
//! tool discovery, parsing of documents, or process/network work.
use crate::toolchain::ToolKind;

mod catalog;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ReferenceId {
    Typst,
    Tinymist,
    Tectonic,
    Texlab,
    Badness,
    TexFmt,
    Ltex,
    Harper,
    Liquid,
    TexEngine,
    Latexmk,
    Bibtex,
    Biber,
    Latexdiff,
    Texdoc,
}
impl ReferenceId {
    pub(crate) const ALL: [Self; 15] = [
        Self::Typst,
        Self::Tinymist,
        Self::Tectonic,
        Self::Texlab,
        Self::Badness,
        Self::TexFmt,
        Self::Ltex,
        Self::Harper,
        Self::Liquid,
        Self::TexEngine,
        Self::Latexmk,
        Self::Bibtex,
        Self::Biber,
        Self::Latexdiff,
        Self::Texdoc,
    ];
    pub(crate) fn from_kind(kind: ToolKind) -> Self {
        match kind {
            ToolKind::Typst => Self::Typst,
            ToolKind::Tinymist => Self::Tinymist,
            ToolKind::Tectonic => Self::Tectonic,
            ToolKind::Texlab => Self::Texlab,
            ToolKind::Badness => Self::Badness,
            ToolKind::TexFmt => Self::TexFmt,
        }
    }
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Typst => "typst",
            Self::Tinymist => "tinymist",
            Self::Tectonic => "tectonic",
            Self::Texlab => "texlab",
            Self::Badness => "badness",
            Self::TexFmt => "tex-fmt",
            Self::Ltex => "ltex",
            Self::Harper => "harper",
            Self::Liquid => "liquid",
            Self::TexEngine => "tex-engine",
            Self::Latexmk => "latexmk",
            Self::Bibtex => "bibtex",
            Self::Biber => "biber",
            Self::Latexdiff => "latexdiff",
            Self::Texdoc => "texdoc",
        }
    }
}

pub(crate) struct Example {
    pub(crate) label: &'static str,
    pub(crate) syntax: &'static str,
    /// Syntax identifier accepted by the shared code editor.
    pub(crate) language: &'static str,
    pub(crate) explanation: &'static str,
}
pub(crate) struct Source {
    pub(crate) title: &'static str,
    pub(crate) url: &'static str,
}
#[derive(Clone, Copy)]
pub(crate) struct Reference {
    pub(crate) id: ReferenceId,
    pub(crate) title: &'static str,
    pub(crate) reviewed_version: &'static str,
    pub(crate) summary: &'static str,
    pub(crate) examples: &'static [Example],
    pub(crate) notes: &'static [&'static str],
    pub(crate) sources: &'static [Source],
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ReviewStatus {
    Reviewed,
    NeedsReview { current: String },
    Unverified,
}

pub(crate) fn reference(id: ReferenceId) -> &'static Reference {
    // The fixed catalogue contains only immutable text, not tool instances.
    &catalog::REFERENCES[id as usize]
}
impl Reference {
    /// Current build pins are independent of the version this text reviewed.
    /// In particular, never infer a custom/system executable's running version.
    pub(crate) fn status(&self, verified_pinned: bool) -> ReviewStatus {
        if !verified_pinned {
            return ReviewStatus::Unverified;
        }
        review_status(self.reviewed_version, self.pinned_version().as_deref())
    }
    pub(crate) fn status_label(&self, verified_pinned: bool) -> String {
        match self.status(verified_pinned) {
            ReviewStatus::Reviewed => "Matches this build’s pinned version".into(),
            ReviewStatus::NeedsReview { current } => format!("Current build pin: {current}"),
            ReviewStatus::Unverified => "Custom/system version unverified".into(),
        }
    }
    pub(crate) fn warning(&self, verified_pinned: bool) -> Option<String> {
        match self.status(verified_pinned) {
            ReviewStatus::NeedsReview { current } => Some(format!(
                "This reference was reviewed for {}. The build now pins {current}; review these options before using them.",
                self.reviewed_version
            )),
            _ => None,
        }
    }
    pub(crate) fn pinned_version(&self) -> Option<String> {
        let kind = match self.id {
            ReferenceId::Typst => Some(ToolKind::Typst),
            ReferenceId::Tinymist => Some(ToolKind::Tinymist),
            ReferenceId::Tectonic => Some(ToolKind::Tectonic),
            ReferenceId::Texlab => Some(ToolKind::Texlab),
            ReferenceId::Badness => Some(ToolKind::Badness),
            ReferenceId::TexFmt => Some(ToolKind::TexFmt),
            _ => None,
        };
        if let Some(kind) = kind {
            return Some(kind.bundled_version().into());
        }
        match self.id {
            ReferenceId::Ltex => Some(format!(
                "{} / {}",
                crate::ltex_resources::manifest::VERSION,
                crate::ltex_resources::manifest::JDK_DIRECTORY
            )),
            ReferenceId::Harper => Some(env!("TIPTOPTYP_HARPER_VERSION").to_owned()),
            ReferenceId::Liquid => {
                use crate::liquid_runtime::manifest;
                Some(format!(
                    "{}@{}; ONNX arm64 {} / x64 {}; format {}",
                    manifest::MODEL_ID,
                    manifest::UPSTREAM_REVISION,
                    manifest::runtime_version("aarch64-apple-darwin").unwrap_or("unknown"),
                    manifest::runtime_version("x86_64-apple-darwin").unwrap_or("unknown"),
                    manifest::FORMAT
                ))
            }
            _ => None,
        }
    }
}
fn review_status(reviewed: &str, current: Option<&str>) -> ReviewStatus {
    match current {
        Some(current) if current == reviewed => ReviewStatus::Reviewed,
        Some(current) => ReviewStatus::NeedsReview {
            current: current.into(),
        },
        None => ReviewStatus::Unverified,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_configurable_integration_has_one_local_reference() {
        let mut keys = std::collections::BTreeSet::new();
        for id in ReferenceId::ALL {
            let entry = reference(id);
            assert_eq!(entry.id, id);
            assert!(keys.insert(id.key()));
            assert!(
                !entry.title.is_empty()
                    && !entry.summary.is_empty()
                    && !entry.reviewed_version.is_empty()
            );
            assert!(
                !entry.examples.is_empty() && !entry.notes.is_empty() && !entry.sources.is_empty()
            );
            for example in entry.examples {
                assert!(
                    !example.label.is_empty()
                        && !example.syntax.is_empty()
                        && !example.language.is_empty()
                        && !example.explanation.is_empty()
                );
            }
            for source in entry.sources {
                assert!(!source.title.is_empty() && source.url.starts_with("https://"));
            }
        }
        for kind in [
            ToolKind::Typst,
            ToolKind::Tinymist,
            ToolKind::Tectonic,
            ToolKind::Texlab,
            ToolKind::Badness,
            ToolKind::TexFmt,
        ] {
            assert_eq!(reference(ReferenceId::from_kind(kind)).title, kind.label());
        }
    }
    #[test]
    fn literal_json_examples_are_complete_parseable_objects() {
        for id in ReferenceId::ALL {
            for example in reference(id).examples {
                if example.language == "json" {
                    let value: serde_json::Value = serde_json::from_str(example.syntax)
                        .unwrap_or_else(|error| {
                            panic!("{} / {}: {error}", id.key(), example.label)
                        });
                    assert!(value.is_object(), "{} / {}", id.key(), example.label);
                }
            }
        }
    }
    #[test]
    fn changed_pins_require_reference_review_and_keep_a_runtime_warning() {
        assert_eq!(review_status("1.0", Some("1.0")), ReviewStatus::Reviewed);
        assert_eq!(
            review_status("1.0", Some("2.0")),
            ReviewStatus::NeedsReview {
                current: "2.0".into()
            }
        );
        assert_eq!(review_status("1.0", None), ReviewStatus::Unverified);
        let outdated = Reference {
            reviewed_version: "not-the-bundled-version",
            ..*reference(ReferenceId::Typst)
        };
        assert!(matches!(
            outdated.status(true),
            ReviewStatus::NeedsReview { .. }
        ));
        assert!(
            outdated
                .warning(true)
                .unwrap()
                .contains("review these options")
        );
        assert_eq!(outdated.status(false), ReviewStatus::Unverified);
        assert!(outdated.warning(false).is_none());
    }
    #[test]
    fn catalogue_has_been_reviewed_against_every_current_pin() {
        for id in [
            ReferenceId::Typst,
            ReferenceId::Tinymist,
            ReferenceId::Tectonic,
            ReferenceId::Texlab,
            ReferenceId::Badness,
            ReferenceId::TexFmt,
            ReferenceId::Ltex,
            ReferenceId::Harper,
            ReferenceId::Liquid,
        ] {
            assert_eq!(
                reference(id).status(true),
                ReviewStatus::Reviewed,
                "{} version changed: review and update its offline reference",
                id.key()
            );
        }
    }
    #[test]
    fn custom_and_system_tools_never_claim_a_verified_bundled_version() {
        for id in ReferenceId::ALL {
            assert_eq!(reference(id).status(false), ReviewStatus::Unverified);
        }
        for id in [
            ReferenceId::TexEngine,
            ReferenceId::Latexmk,
            ReferenceId::Bibtex,
            ReferenceId::Biber,
            ReferenceId::Latexdiff,
            ReferenceId::Texdoc,
        ] {
            assert_eq!(reference(id).pinned_version(), None);
            assert_eq!(reference(id).status(true), ReviewStatus::Unverified);
        }
    }
    #[test]
    fn harper_runtime_stamp_matches_the_resolved_library_not_a_semver_lower_bound() {
        let lock = include_str!("../Cargo.lock");
        let package = lock
            .split("[[package]]")
            .find(|package| package.lines().any(|line| line == "name = \"harper-core\""))
            .expect("resolved harper-core package");
        let resolved = package
            .lines()
            .find_map(|line| line.strip_prefix("version = \""))
            .and_then(|line| line.strip_suffix('"'))
            .expect("resolved Harper version");
        assert_eq!(
            env!("TIPTOPTYP_HARPER_VERSION"),
            resolved,
            "build.rs must stamp the resolved Harper version for runtime drift warnings"
        );
    }
}
