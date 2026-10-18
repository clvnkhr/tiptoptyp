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
pub struct PreviewContent<T> {
    canonical: Canonical,
    raster: Option<Raster<T>>,
}
impl<T> Default for PreviewContent<T> {
    fn default() -> Self {
        Self {
            canonical: Canonical::Missing,
            raster: None,
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
        self.raster.as_ref().map(|raster| raster.key)
    }
    pub fn pages(&self) -> &[T] {
        self.raster.as_ref().map_or(&[], |raster| &raster.pages)
    }
    pub fn accept_artifact(&mut self, key: ArtifactKey, pdf: Arc<[u8]>) {
        self.canonical = Canonical::Current(Artifact {
            key,
            content: Content::Pdf(pdf),
        });
    }
    pub fn replace_asset(&mut self, key: ArtifactKey, pdf: Option<Arc<[u8]>>, pages: Vec<T>) {
        let content = pdf.map_or(Content::Image, Content::Pdf);
        self.canonical = Canonical::Current(Artifact { key, content });
        self.raster = Some(Raster { key, pages });
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
        if let Some(raster) = self.raster.as_mut() {
            raster.key.revision = revision;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dependency_rebuild_retains_static_pixels_without_promoting_their_provenance() {
        let mut state = PreviewContent::default();
        let old = ArtifactKey {
            revision: 4,
            generation: 1,
        };
        let new = ArtifactKey {
            generation: 2,
            ..old
        };
        state.replace_asset(old, Some(Arc::from(&b"old"[..])), vec![10]);
        state.accept_artifact(new, Arc::from(&b"new"[..]));
        assert_eq!(state.artifact_key(), Some(new));
        assert_eq!(state.raster_key(), Some(old));
        assert_eq!(state.pages(), &[10]);
        assert_eq!(state.pdf().unwrap().as_ref(), b"new");
        state.invalidate();
        assert_eq!(state.artifact_key(), None);
        assert_eq!(state.raster_key(), Some(old));
        assert_eq!(state.pages(), &[10]);
        state.replace_asset(new, None, vec![20]);
        assert_eq!(state.artifact_key(), Some(new));
        assert_eq!(state.raster_key(), Some(new));
        assert_eq!(state.pages(), &[20]);
        assert!(
            state.pdf().is_none(),
            "an image replacement removes old PDF bytes"
        );
        state.clear();
        assert_eq!(state.artifact_key(), None);
        assert_eq!(state.raster_key(), None);
        assert!(state.pages().is_empty());
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
    }
}
