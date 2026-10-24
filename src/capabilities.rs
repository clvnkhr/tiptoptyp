//! Cached application capabilities derived from resolved tools and live services.
//!
//! PDF parsing and rendering are built in. Deriving or reading a snapshot
//! performs no filesystem, environment or process work.

use crate::{
    document::DocumentKind, language_support::LanguageSupport, preview::ServiceState,
    toolchain::ToolResolution,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapabilitySnapshot {
    pub(crate) editing: ServiceState,
    pub(crate) lsp: ServiceState,
    pub(crate) interactive_preview: ServiceState,
    pub(crate) pdf_generation: ServiceState,
    pub(crate) pdf_rendering: ServiceState,
    pub(crate) link_extraction: ServiceState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapabilityInputs {
    pub(crate) document: DocumentKind,
    pub(crate) preview_document: DocumentKind,
    pub(crate) build_requirement: ServiceState,
    pub(crate) editor_tool: ToolResolution,
    pub(crate) interactive_tool: ToolResolution,
    pub(crate) lsp: ServiceState,
    pub(crate) interactive_preview: ServiceState,
    pub(crate) pdf_generation: ServiceState,
    pub(crate) pdf_rendering: ServiceState,
    pub(crate) interactive_preview_supported: bool,
}

#[derive(Debug)]
pub(crate) struct CapabilityCache {
    cached: Option<(CapabilityInputs, CapabilitySnapshot)>,
    #[cfg(test)]
    derivations: usize,
}

impl CapabilityCache {
    pub(crate) fn discover() -> Self {
        Self {
            cached: None,
            #[cfg(test)]
            derivations: 0,
        }
    }
    pub(crate) fn invalidate(&mut self) {
        self.cached = None;
    }
    pub(crate) fn refresh_tools(&mut self) {
        self.invalidate();
    }
    pub(crate) fn snapshot(&mut self, inputs: CapabilityInputs) -> CapabilitySnapshot {
        if let Some((cached_inputs, snapshot)) = self.cached.as_ref()
            && cached_inputs == &inputs
        {
            return snapshot.clone();
        }

        let snapshot = derive_snapshot(&inputs);
        #[cfg(test)]
        {
            self.derivations += 1;
        }
        self.cached = Some((inputs, snapshot.clone()));
        snapshot
    }
}

fn derive_snapshot(inputs: &CapabilityInputs) -> CapabilitySnapshot {
    let editor = LanguageSupport::for_document(inputs.document);
    let preview = LanguageSupport::for_document(inputs.preview_document);
    CapabilitySnapshot {
        editing: ServiceState::Ready(
            "Built-in source editing and syntax parsing are available".to_owned(),
        ),
        lsp: if editor.language_service.is_some() {
            require_tool(&inputs.editor_tool, "LSP", inputs.lsp.clone())
        } else {
            ServiceState::Unsupported(
                "No language service is implemented for this document type".to_owned(),
            )
        },
        interactive_preview: if !preview.interactive_preview {
            ServiceState::Unsupported(
                "This document type has no interactive preview service".to_owned(),
            )
        } else if !inputs.interactive_preview_supported {
            ServiceState::Unsupported(
                "Interactive preview is available on macOS and Windows".to_owned(),
            )
        } else {
            require_tool(
                &inputs.interactive_tool,
                "interactive preview",
                inputs.interactive_preview.clone(),
            )
        },
        pdf_generation: if preview.build.is_some() {
            require_build_requirement(&inputs.build_requirement, inputs.pdf_generation.clone())
        } else {
            ServiceState::Unsupported(
                "No PDF build engine is implemented for this document type".to_owned(),
            )
        },
        pdf_rendering: inputs.pdf_rendering.clone(),
        link_extraction: ServiceState::Ready("Built-in Rust PDF link extraction".into()),
    }
}

/// Cached compiler requirements, independent of the current build's progress.
/// The caller supplies discovery results; deriving this never checks a path.
pub(crate) fn build_requirement(
    document: DocumentKind,
    typst: &ToolResolution,
    tex: &crate::tex::settings::TexSettings,
    tectonic: &ToolResolution,
    system_engine_available: bool,
    latexmk_available: bool,
) -> ServiceState {
    use crate::tex::settings::{BuildDriver, BuildEngine};
    let available = || ServiceState::Ready("Compiler available".into());
    match document {
        DocumentKind::Typst => require_tool(typst, "PDF generation", available()),
        DocumentKind::Tex if !tex.build_enabled => {
            ServiceState::Disabled("TeX builds are disabled".into())
        }
        DocumentKind::Tex if tex.build_engine == BuildEngine::Tectonic => {
            require_tool(tectonic, "PDF generation", available())
        }
        DocumentKind::Tex if !system_engine_available => ServiceState::Failed(format!(
            "PDF generation is unavailable: {} was not found",
            tex.build_engine.label()
        )),
        DocumentKind::Tex if tex.build_driver == BuildDriver::Latexmk && !latexmk_available => {
            ServiceState::Failed("PDF generation is unavailable: latexmk was not found".into())
        }
        DocumentKind::Tex => available(),
        DocumentKind::Text | DocumentKind::Pdf | DocumentKind::Image => {
            ServiceState::Unsupported("This file has no PDF build engine".into())
        }
    }
}

fn require_build_requirement(requirement: &ServiceState, progress: ServiceState) -> ServiceState {
    if matches!(
        progress,
        ServiceState::Disabled(_) | ServiceState::Unsupported(_)
    ) || matches!(requirement, ServiceState::Ready(_))
    {
        progress
    } else {
        requirement.clone()
    }
}

fn require_tool(
    resolution: &ToolResolution,
    capability: &str,
    available_state: ServiceState,
) -> ServiceState {
    if matches!(
        available_state,
        ServiceState::Disabled(_) | ServiceState::Unsupported(_)
    ) || resolution.is_available()
    {
        available_state
    } else {
        let reason = resolution
            .fallback_reason
            .as_deref()
            .unwrap_or("The required tool is unavailable");
        ServiceState::Failed(format!("{capability} is unavailable: {reason}"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::{ToolKind, ToolOrigin};
    use std::path::PathBuf;

    fn tool(kind: ToolKind, available: bool) -> ToolResolution {
        ToolResolution {
            bundled_program: None,
            command: Default::default(),
            kind,
            program: PathBuf::from(kind.binary_name()),
            origin: if available {
                ToolOrigin::Bundled
            } else {
                ToolOrigin::Missing
            },
            fallback_reason: (!available).then(|| format!("{} is missing", kind.label())),
        }
    }

    fn inputs() -> CapabilityInputs {
        CapabilityInputs {
            document: DocumentKind::Typst,
            preview_document: DocumentKind::Typst,
            build_requirement: ServiceState::Ready("Compiler available".into()),
            editor_tool: tool(ToolKind::Tinymist, true),
            interactive_tool: tool(ToolKind::Tinymist, true),
            lsp: ServiceState::Ready("LSP connected".into()),
            interactive_preview: ServiceState::Ready("Preview connected".into()),
            pdf_generation: ServiceState::Ready("PDF ready".into()),
            pdf_rendering: ServiceState::Ready("Pages ready".into()),
            interactive_preview_supported: true,
        }
    }

    #[test]
    fn built_in_pdf_capabilities_do_not_depend_on_path() {
        let mut cache = CapabilityCache::discover();
        let mut input = inputs();
        input.build_requirement = ServiceState::Failed("Typst is missing".into());
        let state = cache.snapshot(input.clone());
        assert!(state.pdf_rendering.is_ready());
        assert!(state.link_extraction.is_ready());
        assert!(matches!(state.pdf_generation, ServiceState::Failed(_)));
        assert_eq!(cache.snapshot(input), state);
        assert_eq!(cache.derivations, 1);
        cache.refresh_tools();
        cache.snapshot(inputs());
        assert_eq!(cache.derivations, 2);
    }

    #[test]
    fn selected_system_tex_engine_does_not_require_typst_or_tectonic() {
        use crate::tex::settings::{BuildDriver, BuildEngine, TexSettings};
        let missing_typst = tool(ToolKind::Typst, false);
        let missing_tectonic = tool(ToolKind::Tectonic, false);
        for engine in [
            BuildEngine::PdfLatex,
            BuildEngine::XeLatex,
            BuildEngine::LuaLatex,
        ] {
            for driver in [BuildDriver::Direct, BuildDriver::Latexmk] {
                let tex = TexSettings {
                    build_engine: engine,
                    build_driver: driver,
                    ..Default::default()
                };
                let requirement = build_requirement(
                    DocumentKind::Tex,
                    &missing_typst,
                    &tex,
                    &missing_tectonic,
                    true,
                    true,
                );
                assert!(
                    requirement.is_ready(),
                    "{engine:?} {driver:?}: {requirement:?}"
                );
                let mut input = inputs();
                input.document = DocumentKind::Tex;
                input.preview_document = DocumentKind::Tex;
                input.build_requirement = requirement;
                input.editor_tool = tool(ToolKind::Texlab, true);
                input.interactive_tool = missing_typst.clone();
                let snapshot = derive_snapshot(&input);
                assert_eq!(snapshot.pdf_generation, input.pdf_generation);
                assert!(matches!(
                    snapshot.interactive_preview,
                    ServiceState::Unsupported(_)
                ));
            }
        }
    }

    #[test]
    fn cached_tex_requirements_distinguish_disabled_missing_engine_and_missing_driver() {
        use crate::tex::settings::{BuildDriver, BuildEngine, TexSettings};
        let typst = tool(ToolKind::Typst, true);
        let tectonic = tool(ToolKind::Tectonic, false);
        let mut tex = TexSettings {
            build_engine: BuildEngine::XeLatex,
            ..Default::default()
        };
        let project = |tex: &TexSettings, engine, driver| {
            build_requirement(DocumentKind::Tex, &typst, tex, &tectonic, engine, driver)
        };
        assert!(
            matches!(project(&tex, false, true), ServiceState::Failed(message) if message.contains("XeLaTeX"))
        );
        assert!(
            matches!(project(&tex, true, false), ServiceState::Failed(message) if message.contains("latexmk"))
        );
        tex.build_driver = BuildDriver::Direct;
        assert!(project(&tex, true, false).is_ready());
        tex.build_enabled = false;
        assert!(matches!(
            project(&tex, false, false),
            ServiceState::Disabled(_)
        ));
        tex.build_enabled = true;
        tex.build_engine = BuildEngine::Tectonic;
        assert!(
            matches!(project(&tex, true, true), ServiceState::Failed(message) if message.contains("Tectonic"))
        );
        assert!(
            build_requirement(DocumentKind::Typst, &typst, &tex, &tectonic, false, false)
                .is_ready()
        );
        for document in [DocumentKind::Text, DocumentKind::Pdf, DocumentKind::Image] {
            assert!(matches!(
                build_requirement(document, &typst, &tex, &tectonic, true, true),
                ServiceState::Unsupported(_)
            ));
        }
    }

    #[test]
    fn compiler_progress_and_disabled_state_remain_authoritative_after_requirements() {
        let ready = ServiceState::Ready("Compiler available".into());
        for progress in [
            ServiceState::Starting("Building".into()),
            ServiceState::Ready("Preview ready".into()),
            ServiceState::Degraded("The document did not build".into()),
        ] {
            assert_eq!(
                require_build_requirement(&ready, progress.clone()),
                progress
            );
        }
        let missing = ServiceState::Failed("XeLaTeX is missing".into());
        assert_eq!(require_build_requirement(&missing, ready), missing);
        let paused = ServiceState::Disabled("Preview updates paused".into());
        assert_eq!(require_build_requirement(&missing, paused.clone()), paused);
    }
}
