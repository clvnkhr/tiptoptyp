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
    pub(crate) build_tool: ToolResolution,
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
            require_tool(
                &inputs.build_tool,
                "PDF generation",
                inputs.pdf_generation.clone(),
            )
        } else {
            ServiceState::Unsupported(
                "No PDF build engine is implemented for this document type".to_owned(),
            )
        },
        pdf_rendering: inputs.pdf_rendering.clone(),
        link_extraction: ServiceState::Ready("Built-in Rust PDF link extraction".into()),
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
            build_tool: tool(ToolKind::Typst, true),
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
        input.build_tool = tool(ToolKind::Typst, false);
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
}
