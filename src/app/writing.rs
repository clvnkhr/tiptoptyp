use super::*;
#[derive(Default)]
pub(super) struct WritingState {
    observed: Option<(DocumentKey, bool, bool)>,
    deadline: Option<Instant>,
    job: LatestJob<((DocumentKey, bool, bool), Vec<Diagnostic>)>,
    pub(super) diagnostics: Vec<Diagnostic>,
    pub(super) markers: Vec<usize>,
}
impl WritingState {
    pub(super) fn markers(&self, key: DocumentKey) -> &[usize] {
        if self.observed.is_some_and(|observed| observed.0 == key) {
            &self.markers
        } else {
            &[]
        }
    }
}
impl EditorApp {
    pub(super) fn update_writing_checks(&mut self, context: &egui::Context) {
        let key = self.document().key();
        let identity = (
            key,
            self.settings.english_grammar,
            self.settings.unicode_warnings,
        );
        if self.writing.observed != Some(identity) {
            self.writing.observed = Some(identity);
            self.writing.deadline = Some(Instant::now() + Duration::from_millis(600));
            self.writing.diagnostics.clear();
            self.writing.markers.clear();
            self.update_tex_diagnostics();
        }
        match self.writing.job.poll() {
            LatestJobPoll::Ready((completed, diagnostics))
                if Some(completed) == self.writing.observed =>
            {
                let mut starts = vec![0];
                for (index, ch) in self.document().source().chars().enumerate() {
                    if ch == '\n' {
                        starts.push(index + 1);
                    }
                }
                self.writing.markers = diagnostics
                    .iter()
                    .filter(|d| d.provider.as_deref() == Some("Unicode"))
                    .filter_map(|d| {
                        let location = d.location?;
                        Some(
                            starts.get(location.line.checked_sub(1)?)?
                                + location.column.saturating_sub(1),
                        )
                    })
                    .collect();
                self.writing.diagnostics = diagnostics;
                self.update_tex_diagnostics();
            }
            LatestJobPoll::Failed(error) => self.show_file_error(error),
            _ => {}
        }
        let Some(deadline) = self.writing.deadline else {
            return;
        };
        if !identity.1 && !identity.2 {
            self.writing.deadline = None;
            return;
        }
        let now = Instant::now();
        if now < deadline {
            context.request_repaint_after(deadline - now);
            return;
        }
        // Finish the previous job before launching another. The newest revision
        // replaces pending work; typing never creates concurrent grammar jobs.
        if self.writing.job.is_running() {
            return;
        }
        self.writing.deadline = None;
        let source = self.document().snapshot().source().to_owned();
        let kind = self.document().kind();
        let path = self
            .document()
            .path()
            .clone()
            .unwrap_or_else(|| self.tinymist_document_path());
        if source.len() > 2_000_000 {
            return;
        }
        if let Err(error) =
            self.writing
                .job
                .start_and_repaint("writing-checks", context, move || {
                    Ok((
                        identity,
                        crate::writing::check(&source, kind, &path, identity.1, identity.2),
                    ))
                })
        {
            self.show_file_error(error);
        }
    }
}
