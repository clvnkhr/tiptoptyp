//! Canonical bytes and displayed pixels retain their build provenance together.
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ArtifactKey {
    pub revision: u64,
    pub generation: u64,
}
impl ArtifactKey {
    pub const fn unversioned(revision: u64) -> Self {
        Self {
            revision,
            generation: 0,
        }
    }
}
enum Content {
    Pdf(Arc<[u8]>),
    Image,
}
struct Artifact {
    key: ArtifactKey,
    content: Content,
}
#[derive(Default)]
enum Canonical {
    #[default]
    Missing,
    Current(Artifact),
    Stale(Artifact),
}
struct Raster<T> {
    key: ArtifactKey,
    pages: Vec<T>,
}
enum RasterState<T> {
    Missing,
    Rendering(Option<Raster<T>>),
    Ready(Raster<T>),
    Failed {
        key: ArtifactKey,
        error: String,
        displayed: Option<Raster<T>>,
    },
}
impl<T> RasterState<T> {
    fn displayed(&self) -> Option<&Raster<T>> {
        match self {
            Self::Ready(raster) => Some(raster),
            Self::Rendering(old) | Self::Failed { displayed: old, .. } => old.as_ref(),
            Self::Missing => None,
        }
    }
    fn displayed_mut(&mut self) -> Option<&mut Raster<T>> {
        match self {
            Self::Ready(raster) => Some(raster),
            Self::Rendering(old) | Self::Failed { displayed: old, .. } => old.as_mut(),
            Self::Missing => None,
        }
    }
    fn into_displayed(self) -> Option<Raster<T>> {
        match self {
            Self::Ready(raster) => Some(raster),
            Self::Rendering(old) | Self::Failed { displayed: old, .. } => old,
            Self::Missing => None,
        }
    }
}
pub struct PreviewContent<T> {
    canonical: Canonical,
    raster: RasterState<T>,
}
impl<T> Default for PreviewContent<T> {
    fn default() -> Self {
        Self {
            canonical: Canonical::Missing,
            raster: RasterState::Missing,
        }
    }
}
impl<T> PreviewContent<T> {
    pub fn artifact_key(&self) -> Option<ArtifactKey> {
        if let Canonical::Current(artifact) = &self.canonical {
            Some(artifact.key)
        } else {
            None
        }
    }
    pub fn pdf(&self) -> Option<&Arc<[u8]>> {
        match &self.canonical {
            Canonical::Current(Artifact {
                content: Content::Pdf(pdf),
                ..
            })
            | Canonical::Stale(Artifact {
                content: Content::Pdf(pdf),
                ..
            }) => Some(pdf),
            _ => None,
        }
    }
    pub fn raster_key(&self) -> Option<ArtifactKey> {
        self.raster.displayed().map(|raster| raster.key)
    }
    pub fn pages(&self) -> &[T] {
        self.raster.displayed().map_or(&[], |raster| &raster.pages)
    }
    pub fn pages_mut(&mut self) -> &mut [T] {
        self.raster
            .displayed_mut()
            .map_or(&mut [], |raster| &mut raster.pages)
    }
    pub fn error(&self) -> Option<&str> {
        if let RasterState::Failed { key, error, .. } = &self.raster {
            (self.artifact_key() == Some(*key)).then_some(error)
        } else {
            None
        }
    }
    pub fn accept_artifact(&mut self, key: ArtifactKey, pdf: Arc<[u8]>) {
        self.canonical = Canonical::Current(Artifact {
            key,
            content: Content::Pdf(pdf),
        });
        let displayed = std::mem::replace(&mut self.raster, RasterState::Missing).into_displayed();
        self.raster = RasterState::Rendering(displayed);
    }
    pub fn accept_raster(&mut self, key: ArtifactKey, pages: Vec<T>) -> bool {
        if self.artifact_key() != Some(key) {
            return false;
        }
        self.raster = RasterState::Ready(Raster { key, pages });
        true
    }
    pub fn fail_raster(&mut self, key: ArtifactKey, error: String) -> bool {
        if self.artifact_key() != Some(key) {
            return false;
        }
        let displayed = std::mem::replace(&mut self.raster, RasterState::Missing).into_displayed();
        self.raster = RasterState::Failed {
            key,
            error,
            displayed,
        };
        true
    }
    pub fn replace_asset(&mut self, key: ArtifactKey, pdf: Option<Arc<[u8]>>, pages: Vec<T>) {
        let content = pdf.map_or(Content::Image, Content::Pdf);
        self.canonical = Canonical::Current(Artifact { key, content });
        self.raster = RasterState::Ready(Raster { key, pages });
    }
    pub fn invalidate(&mut self) {
        self.canonical = match std::mem::take(&mut self.canonical) {
            Canonical::Current(artifact) => Canonical::Stale(artifact),
            other => other,
        };
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
    pub fn rebind_revision(&mut self, revision: u64) {
        match &mut self.canonical {
            Canonical::Current(artifact) | Canonical::Stale(artifact) => {
                artifact.key.revision = revision
            }
            Canonical::Missing => {}
        }
        if let Some(raster) = self.raster.displayed_mut() {
            raster.key.revision = revision;
        }
        if let RasterState::Failed { key, .. } = &mut self.raster {
            key.revision = revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dependency_rebuild_keeps_old_pixels_without_accepting_old_results() {
        let mut state = PreviewContent::default();
        let old = ArtifactKey {
            revision: 4,
            generation: 1,
        };
        let new = ArtifactKey {
            generation: 2,
            ..old
        };
        state.accept_artifact(old, Arc::from(&b"old"[..]));
        assert!(state.accept_raster(old, vec![10]));
        state.accept_artifact(new, Arc::from(&b"new"[..]));
        assert_eq!(state.pages(), &[10]);
        assert!(!state.accept_raster(old, vec![99]));
        assert!(!state.fail_raster(old, "late error".into()));
        assert!(state.fail_raster(new, "rasterizer failed".into()));
        assert_eq!(state.pdf().unwrap().as_ref(), b"new");
        assert_eq!(state.error(), Some("rasterizer failed"));
        assert!(state.accept_raster(new, vec![20]));
        assert_eq!(state.pages(), &[20]);
        assert_eq!(state.error(), None);
    }
    #[test]
    fn invalidation_retains_stale_pdf_until_replacement() {
        let mut state = PreviewContent::<usize>::default();
        let key = ArtifactKey::unversioned(1);
        state.accept_artifact(key, Arc::from(&b"pdf"[..]));
        state.invalidate();
        state.invalidate();
        assert_eq!(state.artifact_key(), None);
        assert!(state.pdf().is_some());
        assert!(!state.accept_raster(key, vec![1]));
    }
}
