//! Window-owned TeX service admission and editor transactions.
use super::*;
use crate::tex::{Event, Identity, Provider, RequestKind, Snapshot};

impl EditorApp {
    pub(super) fn sync_tex(&mut self, context: &egui::Context) {
        if self.document().kind() != DocumentKind::Tex
            || self.snapshot_scene.is_some()
            || !self.lifecycle.allows_document_work()
        {
            self.tex_service.stop();
            if self.tex_diagnostics.iter().any(|d| !d.is_empty()) {
                self.tex_diagnostics = Default::default();
                self.update_tex_diagnostics();
            }
            return;
        }
        let key = self.document().key();
        let root = self.project_root();
        if self.tex_service.identity().is_some_and(|s| {
            s.key == key
                && s.root == root
                && s.settings == self.settings.tex
                && s.tools == self.tex_tools
        }) {
            return;
        }
        let path = self.document().path().clone().unwrap_or_else(|| {
            root.join(".tiptoptyp")
                .join(format!("untitled-{:?}-{}.tex", key.owner, key.epoch))
        });
        let Ok(uri) = url::Url::from_file_path(path) else {
            return;
        };
        self.tex_diagnostics = Default::default();
        self.update_tex_diagnostics();
        self.tex_service.synchronize(
            Snapshot {
                generation: Generation(0),
                key,
                uri: uri.into(),
                source: Arc::from(self.document().source().as_str()),
                root,
                settings: self.settings.tex.clone(),
                tools: self.tex_tools.clone(),
            },
            crate::worker::RepaintTarget::current(context),
        );
    }
    pub(super) fn editor_lsp_identity(&self) -> (Option<Generation>, Option<&str>) {
        if self.document().kind() == DocumentKind::Tex {
            self.tex_service
                .identity()
                .map(|s| (Some(s.generation), Some(s.uri.as_str())))
                .unwrap_or_default()
        } else {
            (
                self.tinymist_sync.generation,
                self.tinymist_sync.current_uri.as_deref(),
            )
        }
    }
    pub(super) fn editor_lsp_ready(&self) -> bool {
        if self.document().kind() == DocumentKind::Tex {
            self.settings.tex.texlab_enabled && self.tex_service.texlab_ready
        } else {
            tinymist_language_features_ready(
                self.document().kind(),
                self.preview.connection.is_ready(),
                self.tinymist_sync.current_open,
            )
        }
    }
    pub(super) fn editor_accepts_reply(
        &self,
        reply: crate::tinymist_sync::ReplyIdentity<'_>,
        key: DocumentKey,
    ) -> bool {
        if self.document().kind() == DocumentKind::Tex {
            reply.version == revision_as_i32(key.revision)
                && self.tex_service.accepts(
                    &Identity {
                        generation: reply.generation,
                        key: reply.key,
                        uri: reply.uri.into(),
                    },
                    key,
                )
        } else {
            self.tinymist_sync.accepts_reply(reply, key)
        }
    }
    pub(super) fn receive_tex_events(&mut self, context: &egui::Context) {
        self.sync_tex(context);
        while let Some(event) = self.tex_service.try_recv() {
            if !self
                .tex_service
                .accepts(event.identity(), self.document().key())
            {
                continue;
            }
            match event {
                Event::Ready { .. } => {
                    let key = self.document().key();
                    let ready = self.settings.tex.formatter
                        != crate::tex::settings::Formatter::Badness
                        || self.tex_service.badness_ready;
                    if take_ready_format_handoff(&mut self.format_when_service_ready, key, ready) {
                        self.request_format_after_manual_save();
                    }
                }
                Event::Failed {
                    provider, message, ..
                } => {
                    self.tex_diagnostics[provider_index(provider)].clear();
                    self.update_tex_diagnostics();
                    self.notice = Some(Notice {
                        message,
                        kind: NoticeKind::Error,
                    });
                }
                Event::Diagnostics {
                    provider,
                    diagnostics,
                    ..
                } => {
                    let locations =
                        crate::tex::diagnostic_locations(self.document().source(), &diagnostics);
                    self.tex_diagnostics[provider_index(provider)] = diagnostics
                        .into_iter()
                        .zip(locations)
                        .map(|(d, location)| {
                            let mut diagnostic = tinymist_diagnostic(
                                d,
                                self.document()
                                    .path()
                                    .as_ref()
                                    .map_or(DiagnosticSource::Main, |p| {
                                        DiagnosticSource::File(p.clone())
                                    }),
                            );
                            diagnostic.location = location;
                            diagnostic.details.push(provider.label().into());
                            diagnostic
                        })
                        .collect();
                    self.update_tex_diagnostics();
                }
                Event::Formatted { request, edits } => self.receive_formatted_document(
                    context,
                    request.identity.generation,
                    &request.identity.uri,
                    revision_as_i32(request.identity.key.revision),
                    edits,
                ),
                Event::Completed {
                    request,
                    is_incomplete,
                    items,
                } => {
                    let RequestKind::Completion { token, .. } = request.kind else {
                        continue;
                    };
                    self.receive_editor_completions(
                        EditorCompletionResponse {
                            generation: request.identity.generation,
                            uri: request.identity.uri,
                            version: revision_as_i32(request.identity.key.revision),
                            request_token: token,
                            is_incomplete,
                            items,
                        },
                        context,
                    );
                }
                Event::Hovered { request, contents } => {
                    let RequestKind::Hover { token, .. } = request.kind else {
                        continue;
                    };
                    if let Some(hover) = &mut self.editor_hover
                        && hover.key == request.identity.key
                        && hover.accepts_response(
                            &request.identity.uri,
                            revision_as_i32(request.identity.key.revision),
                            token,
                        )
                    {
                        hover.detail = contents.map(Arc::from);
                    }
                }
                Event::RequestFailed { request, message } => {
                    if matches!(request.kind, RequestKind::Format) {
                        self.format_request_key = None;
                        self.manual_format_revision = None;
                    }
                    if matches!(request.kind, RequestKind::Completion { .. }) {
                        self.editor_completion = None;
                    }
                    self.notice = Some(Notice {
                        message,
                        kind: NoticeKind::Error,
                    });
                }
            }
        }
        let key = self.document().key();
        let ready = self.document().kind() == DocumentKind::Tex
            && self.tex_service.identity().is_some_and(|s| s.key == key)
            && match self.settings.tex.formatter {
                crate::tex::settings::Formatter::Badness => self.tex_service.badness_ready,
                crate::tex::settings::Formatter::TexFmt => self.tex_tools.tex_fmt.is_available(),
                crate::tex::settings::Formatter::Disabled => true,
            };
        if take_ready_format_handoff(&mut self.format_when_service_ready, key, ready) {
            self.request_format_after_manual_save();
        }
    }
    pub(super) fn update_tex_diagnostics(&mut self) {
        let mut diagnostics = self
            .preview
            .tinymist_diagnostics
            .iter()
            .chain(self.tex_diagnostics.iter().flatten())
            .cloned()
            .collect();
        normalize_diagnostics(&mut diagnostics);
        self.preview.editor_diagnostics = diagnostics;
        self.mark_diagnostics_changed();
    }
    pub(super) fn request_tex_format(&mut self) {
        // mark_edited runs before actions; refuse an out-of-date service snapshot.
        if self
            .tex_service
            .identity()
            .is_none_or(|s| s.key != self.document().key())
        {
            self.notice = Some(Notice {
                message: "TeX source is still synchronizing; try formatting again".into(),
                kind: NoticeKind::Info,
            });
            self.manual_format_revision = None;
            return;
        }
        match self.tex_service.request(RequestKind::Format) {
            Ok(()) => {
                self.format_request_key = Some(self.document().key());
                self.notice = Some(Notice {
                    message: format!("Formatting with {}…", self.settings.tex.formatter.label()),
                    kind: NoticeKind::Info,
                });
            }
            Err(message) => {
                self.format_request_key = None;
                self.manual_format_revision = None;
                self.notice = Some(Notice {
                    message,
                    kind: NoticeKind::Error,
                });
            }
        }
    }
}
fn provider_index(provider: Provider) -> usize {
    match provider {
        Provider::Texlab => 0,
        Provider::Badness => 1,
    }
}
