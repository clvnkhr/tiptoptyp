//! Frozen historical PDF content shown in the existing preview. This owner
//! keeps its private artifact alive without changing the active source or the
//! live preview controller; closing history simply drops the override.
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(super) struct Preview {
    pub(super) side: usize,
    pub(super) source: Arc<str>,
    pub(super) label: String,
    artifact: crate::version_render::RenderedVersion,
}
impl Preview {
    pub(super) fn path(&self) -> &Path {
        &self.artifact.path
    }
    pub(super) fn bytes(&self) -> &Arc<[u8]> {
        &self.artifact.pdf
    }
}
impl crate::worker::OperationSummary for Preview {
    fn completion_summary(&self) -> String {
        "Version PDF rendered".into()
    }
}

pub(super) fn render(
    request: crate::version_render::Request,
    side: usize,
    source: Arc<str>,
    label: String,
    cancel: &AtomicBool,
) -> Result<Preview, String> {
    if side > 1 || request.source != source.as_ref() {
        return Err("The selected version changed before rendering".into());
    }
    let artifact = crate::version_render::render(request, cancel)?;
    if cancel.load(Ordering::Acquire) {
        return Err("Version PDF rendering cancelled".into());
    }
    Ok(Preview {
        side,
        source,
        label,
        artifact,
    })
}
