//! One non-destructive visibility mask shared by every diagnostics consumer.
use super::*;
use crate::diagnostic_suppression::Suppression;
use std::collections::{HashMap, HashSet};

struct Source {
    key: DocumentKey,
    kind: DocumentKind,
    path: Option<PathBuf>,
    suppression: Suppression,
    line_starts: Vec<usize>,
}
impl Source {
    fn new(document: &DocumentSession) -> Self {
        let source = document.source();
        let line_starts = if source.contains("ttt:") {
            std::iter::once(0)
                .chain(
                    source
                        .chars()
                        .enumerate()
                        .filter_map(|(i, ch)| (ch == '\n').then_some(i + 1)),
                )
                .collect()
        } else {
            Vec::new()
        };
        Self {
            key: document.key(),
            kind: document.kind(),
            path: document.path().clone(),
            suppression: Suppression::parse(
                source,
                source_kind(document.kind(), document.path().as_deref()),
            ),
            line_starts,
        }
    }
    fn matches(&self, document: &DocumentSession) -> bool {
        self.key == document.key() && self.kind == document.kind() && self.path == *document.path()
    }
    fn suppresses_char(&self, index: usize) -> bool {
        let line = self.line_starts.partition_point(|&start| start <= index);
        self.suppression.contains_line(line)
    }
}

#[derive(Default)]
pub(super) struct State {
    pub(super) generation: u64,
    pub(super) suppressed: Vec<bool>,
    raw_generation: Option<u64>,
    main_path: Option<PathBuf>,
    sources: HashMap<u64, Source>,
    external: HashMap<PathBuf, Option<Suppression>>,
    external_epoch: u64,
    external_needed: Vec<PathBuf>,
    external_job: LatestJob<(u64, HashMap<PathBuf, Option<Suppression>>)>,
    invalidated: bool,
    #[cfg(test)]
    rebuilds: usize,
}
impl State {
    pub(super) fn visible_count(&self) -> usize {
        self.suppressed.iter().filter(|&&hidden| !hidden).count()
    }
    #[cfg(all(feature = "desktop-ui-tests", target_os = "macos"))]
    pub(super) fn is_suppressed(&self, index: usize) -> bool {
        self.suppressed.get(index).copied().unwrap_or(false)
    }
    pub(super) fn suppresses_char(&self, tab: Option<u64>, index: usize) -> bool {
        tab.and_then(|tab| self.sources.get(&tab))
            .is_some_and(|s| s.suppresses_char(index))
    }
    pub(super) fn invalidate_files(&mut self) {
        self.invalidated |= !self.external.is_empty() || self.external_job.is_running();
    }
}

fn source_kind(kind: DocumentKind, path: Option<&Path>) -> DocumentKind {
    if kind != DocumentKind::Text {
        return kind;
    }
    match path
        .and_then(Path::extension)
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("typ") => DocumentKind::Typst,
        Some("tex" | "sty" | "cls" | "bib") => DocumentKind::Tex,
        _ => kind,
    }
}
fn read_external(paths: Vec<PathBuf>) -> HashMap<PathBuf, Option<Suppression>> {
    paths
        .into_iter()
        .map(|path| {
            let kind = source_kind(DocumentKind::Text, Some(&path));
            let suppression = matches!(kind, DocumentKind::Typst | DocumentKind::Tex)
                .then(|| {
                    std::fs::read_to_string(&path)
                        .ok()
                        .map(|source| Suppression::parse(&source, kind))
                })
                .flatten();
            (path, suppression)
        })
        .collect()
}

enum Target {
    Open(u64),
    External(PathBuf),
    Unknown,
}

impl EditorApp {
    pub(super) fn diagnostic_presentation(&self) -> (DocumentKey, u64, u64) {
        (
            self.document().key(),
            self.preview.diagnostics_generation,
            self.diagnostic_regions.generation,
        )
    }
    /// Sources are parsed once per revision, and file targets once per batch.
    /// Unopened includes are read off-thread, with at most one reader at a time.
    pub(super) fn prepare_diagnostic_visibility(&mut self, context: Option<&egui::Context>) {
        let mut state = std::mem::take(&mut self.diagnostic_regions);
        let mut external_changed = false;
        if let LatestJobPoll::Ready((epoch, results)) = state.external_job.poll()
            && epoch == state.external_epoch
        {
            state.external.extend(results);
            external_changed = true;
        }
        let main_path = self.preview_document_path();
        let published = state.raw_generation != Some(self.preview.diagnostics_generation);
        let changed = published
            || state.invalidated
            || state.main_path.as_ref() != Some(&main_path)
            || state.sources.len() != self.tabs.len()
            || self.tabs.ids().any(|id| {
                self.document_for_tab(id).is_some_and(|doc| {
                    !state
                        .sources
                        .get(&id)
                        .is_some_and(|source| source.matches(doc))
                })
            });
        if changed || external_changed {
            state
                .sources
                .retain(|id, _| self.document_for_tab(*id).is_some());
            for id in self.tabs.ids() {
                if let Some(document) = self.document_for_tab(id)
                    && !state
                        .sources
                        .get(&id)
                        .is_some_and(|source| source.matches(document))
                {
                    state.sources.insert(id, Source::new(document));
                }
            }
            if published || state.invalidated {
                // A new publication/file event may describe changed disk bytes.
                // Advance acceptance even while the previous reader finishes.
                state.external_epoch = state.external_epoch.wrapping_add(1);
            }
            // Preserve a queued reread across unrelated source edits while
            // an obsolete reader is still finishing.
            let queued: HashSet<_> = state.external_needed.drain(..).collect();
            let mut targets = HashMap::new();
            let mut external_paths = HashSet::new();
            state.suppressed.clear();
            for diagnostic in self
                .preview
                .diagnostics
                .iter()
                .chain(&self.preview.editor_diagnostics)
            {
                let hidden = diagnostic.location.is_some_and(|location| {
                    let target = targets.entry(diagnostic.source.clone()).or_insert_with(|| {
                        self.diagnostic_target_path(diagnostic)
                            .map_or(Target::Unknown, |path| {
                                self.source_tab_for_path(&path)
                                    .map_or_else(|| Target::External(path.clone()), Target::Open)
                            })
                    });
                    match target {
                        Target::Open(id) => state
                            .sources
                            .get(id)
                            .is_some_and(|source| source.suppression.contains_line(location.line)),
                        Target::External(path) => {
                            external_paths.insert(path.clone());
                            state
                                .external
                                .get(path)
                                .and_then(Option::as_ref)
                                .is_some_and(|source| source.contains_line(location.line))
                        }
                        Target::Unknown => false,
                    }
                });
                state.suppressed.push(hidden);
            }
            state
                .external
                .retain(|path, _| external_paths.contains(path));
            state.external_needed = external_paths
                .into_iter()
                .filter(|path| {
                    queued.contains(path)
                        || published
                        || state.invalidated
                        || !state.external.contains_key(path)
                })
                .collect();
            state.raw_generation = Some(self.preview.diagnostics_generation);
            state.main_path = Some(main_path);
            state.invalidated = false;
            state.generation = state.generation.wrapping_add(1);
            #[cfg(test)]
            {
                state.rebuilds += 1;
            }
        }
        if !state.external_job.is_running()
            && !state.external_needed.is_empty()
            && let Some(context) = context
        {
            let paths = std::mem::take(&mut state.external_needed);
            let epoch = state.external_epoch;
            // Failure leaves unknown files visible; never spin retrying on idle.
            let _ =
                state
                    .external_job
                    .start_and_repaint("diagnostic-regions", context, move || {
                        Ok((epoch, read_external(paths)))
                    });
        }
        self.diagnostic_regions = state;
    }
}

#[cfg(test)]
mod tests;
