//! Cached application capabilities derived from resolved tools and live services.
//!
//! Optional host-tool discovery is deliberately confined to construction and
//! explicit refresh. Deriving or reading a snapshot performs no filesystem,
//! environment or process work, so Settings may request it during rendering.

use crate::{
    preview::ServiceState,
    toolchain::{PathProgramResolution, ToolResolution, resolve_path_program},
};

const RASTERIZER: &str = "pdftoppm";
const LINK_EXTRACTOR: &str = "pdftohtml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapabilitySnapshot {
    pub(crate) editing: ServiceState,
    pub(crate) lsp: ServiceState,
    pub(crate) interactive_preview: ServiceState,
    pub(crate) pdf_generation: ServiceState,
    pub(crate) rasterization: ServiceState,
    pub(crate) link_extraction: ServiceState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CapabilityInputs {
    pub(crate) typst: ToolResolution,
    pub(crate) tinymist: ToolResolution,
    pub(crate) lsp: ServiceState,
    pub(crate) interactive_preview: ServiceState,
    pub(crate) pdf_generation: ServiceState,
    pub(crate) rasterization: ServiceState,
    pub(crate) interactive_preview_supported: bool,
}

#[derive(Debug)]
pub(crate) struct CapabilityCache {
    rasterizer: PathProgramResolution,
    link_extractor: PathProgramResolution,
    cached: Option<(CapabilityInputs, CapabilitySnapshot)>,
    #[cfg(test)]
    derivations: usize,
}

impl CapabilityCache {
    pub(crate) fn discover() -> Self {
        Self::discover_with(resolve_path_program)
    }

    fn discover_with(mut resolve: impl FnMut(&'static str) -> PathProgramResolution) -> Self {
        Self {
            rasterizer: resolve(RASTERIZER),
            link_extractor: resolve(LINK_EXTRACTOR),
            cached: None,
            #[cfg(test)]
            derivations: 0,
        }
    }

    /// Invalidates derived state without repeating host-tool discovery.
    pub(crate) fn invalidate(&mut self) {
        self.cached = None;
    }

    /// Repeats optional host-tool discovery only for an explicit tool refresh.
    pub(crate) fn refresh_tools(&mut self) {
        self.refresh_with(resolve_path_program);
    }

    fn refresh_with(&mut self, mut resolve: impl FnMut(&'static str) -> PathProgramResolution) {
        self.rasterizer = resolve(RASTERIZER);
        self.link_extractor = resolve(LINK_EXTRACTOR);
        self.invalidate();
    }

    pub(crate) fn snapshot(&mut self, inputs: CapabilityInputs) -> CapabilitySnapshot {
        if let Some((cached_inputs, snapshot)) = self.cached.as_ref()
            && cached_inputs == &inputs
        {
            return snapshot.clone();
        }

        let snapshot = derive_snapshot(&inputs, &self.rasterizer, &self.link_extractor);
        #[cfg(test)]
        {
            self.derivations += 1;
        }
        self.cached = Some((inputs, snapshot.clone()));
        snapshot
    }
}

fn derive_snapshot(
    inputs: &CapabilityInputs,
    rasterizer: &PathProgramResolution,
    link_extractor: &PathProgramResolution,
) -> CapabilitySnapshot {
    CapabilitySnapshot {
        editing: ServiceState::Ready(
            "Built-in source editing and syntax parsing are available".to_owned(),
        ),
        lsp: require_tool(&inputs.tinymist, "LSP", inputs.lsp.clone()),
        interactive_preview: if !inputs.interactive_preview_supported {
            ServiceState::Unsupported(
                "Interactive preview is available on macOS and Windows".to_owned(),
            )
        } else {
            require_tool(
                &inputs.tinymist,
                "interactive preview",
                inputs.interactive_preview.clone(),
            )
        },
        pdf_generation: require_tool(
            &inputs.typst,
            "PDF generation",
            inputs.pdf_generation.clone(),
        ),
        rasterization: require_path_program(
            rasterizer,
            "PDF rasterization",
            inputs.rasterization.clone(),
        ),
        link_extraction: path_program_state(link_extractor, "PDF link extraction"),
    }
}

fn require_tool(
    resolution: &ToolResolution,
    capability: &str,
    available_state: ServiceState,
) -> ServiceState {
    if resolution.is_available() {
        available_state
    } else {
        let reason = resolution
            .fallback_reason
            .as_deref()
            .unwrap_or("The required tool is unavailable");
        ServiceState::Failed(format!("{capability} is unavailable: {reason}"))
    }
}

fn require_path_program(
    resolution: &PathProgramResolution,
    capability: &str,
    available_state: ServiceState,
) -> ServiceState {
    if resolution.is_available() {
        available_state
    } else {
        missing_path_program(resolution, capability)
    }
}

fn path_program_state(resolution: &PathProgramResolution, capability: &str) -> ServiceState {
    match resolution.program() {
        Some(program) => ServiceState::Ready(format!(
            "{capability} uses `{}` at {}",
            resolution.binary(),
            program.display()
        )),
        None => missing_path_program(resolution, capability),
    }
}

fn missing_path_program(resolution: &PathProgramResolution, capability: &str) -> ServiceState {
    ServiceState::Failed(format!(
        "{capability} is unavailable because `{}` was not found on PATH",
        resolution.binary()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::toolchain::{ToolKind, ToolOrigin};
    use std::{cell::Cell, path::PathBuf};

    fn tool(kind: ToolKind, available: bool) -> ToolResolution {
        ToolResolution {
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
            typst: tool(ToolKind::Typst, true),
            tinymist: tool(ToolKind::Tinymist, true),
            lsp: ServiceState::Ready("LSP connected".into()),
            interactive_preview: ServiceState::Ready("Preview connected".into()),
            pdf_generation: ServiceState::Ready("PDF ready".into()),
            rasterization: ServiceState::Ready("Pages ready".into()),
            interactive_preview_supported: true,
        }
    }

    #[test]
    fn partial_availability_is_reported_per_capability() {
        let mut cache = CapabilityCache::discover_with(|binary| match binary {
            RASTERIZER => PathProgramResolution::from_program(binary, None),
            LINK_EXTRACTOR => {
                PathProgramResolution::from_program(binary, Some(PathBuf::from("/tools/pdftohtml")))
            }
            _ => unreachable!(),
        });
        let mut inputs = inputs();
        inputs.typst = tool(ToolKind::Typst, false);
        inputs.interactive_preview_supported = false;

        let snapshot = cache.snapshot(inputs);

        assert!(snapshot.editing.is_ready());
        assert!(snapshot.lsp.is_ready());
        assert!(matches!(
            snapshot.interactive_preview,
            ServiceState::Unsupported(_)
        ));
        assert!(matches!(snapshot.pdf_generation, ServiceState::Failed(_)));
        assert!(matches!(snapshot.rasterization, ServiceState::Failed(_)));
        assert!(snapshot.link_extraction.is_ready());
    }

    #[test]
    fn snapshots_do_not_probe_and_invalidation_is_distinct_from_refresh() {
        let probes = Cell::new(0);
        let resolver = |binary| {
            probes.set(probes.get() + 1);
            PathProgramResolution::from_program(
                binary,
                Some(PathBuf::from(format!("/tools/{binary}"))),
            )
        };
        let mut cache = CapabilityCache::discover_with(resolver);
        assert_eq!(probes.get(), 2);

        let current = inputs();
        assert!(cache.snapshot(current.clone()).editing.is_ready());
        assert!(cache.snapshot(current.clone()).editing.is_ready());
        assert_eq!(cache.derivations, 1);
        assert_eq!(probes.get(), 2, "render-time reads must not probe PATH");

        cache.invalidate();
        cache.snapshot(current.clone());
        assert_eq!(cache.derivations, 2);
        assert_eq!(
            probes.get(),
            2,
            "preference invalidation is derivation-only"
        );

        cache.refresh_with(resolver);
        assert_eq!(probes.get(), 4, "explicit refresh re-probes optional tools");
        cache.snapshot(current);
        assert_eq!(cache.derivations, 3);
    }
}
