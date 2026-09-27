//! Window-owned TeX service admission and editor transactions.
use super::*;
use crate::tex::{Event, Identity, Provider, RequestKind, Snapshot};

impl EditorApp {
    pub(super) fn sync_tex(&mut self, context: &egui::Context) {
        let embedded = self.document().config().is_some();
        if (self.document().kind() != DocumentKind::Tex && !embedded)
            || self.snapshot_scene.is_some()
            || !self.lifecycle.allows_document_work()
        {
            self.embedded_tex = None;
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
        let same_document = self
            .tex_service
            .identity()
            .is_some_and(|s| crate::diagnostics::same_document(s.key, key));
        let path = if embedded {
            root.join(".tiptoptyp")
                .join(format!("embedded-{:?}-{}.tex", key.owner, key.epoch))
        } else {
            self.document().path().clone().unwrap_or_else(|| {
                root.join(".tiptoptyp")
                    .join(format!("untitled-{:?}-{}.tex", key.owner, key.epoch))
            })
        };
        let Ok(uri) = url::Url::from_file_path(path) else {
            return;
        };
        // A new revision makes existing results stale, but they remain useful
        // until a service supplies a replacement. Changing documents or explicitly
        // disabling a provider retires its results.
        if !same_document {
            self.tex_diagnostics = Default::default();
            self.update_tex_diagnostics();
        }
        let mut retired = false;
        for (index, enabled) in [
            self.settings.tex.texlab_enabled
                && self.settings.tex.diagnostics
                && (!embedded || self.settings.tex.embedded_diagnostics),
            self.settings.tex.lint && (!embedded || self.settings.tex.embedded_diagnostics),
        ]
        .into_iter()
        .enumerate()
        {
            if !enabled && !self.tex_diagnostics[index].is_empty() {
                self.tex_diagnostics[index].clear();
                retired = true;
            }
        }
        if retired {
            self.update_tex_diagnostics();
        }
        self.embedded_tex = if embedded {
            self.prepare_editor_source_data();
            Some(self.editor_data.embedded_tex_projection())
        } else {
            None
        };
        self.tex_service.synchronize(
            Snapshot {
                generation: Generation(0),
                key,
                uri: uri.into(),
                source: Arc::from(
                    self.embedded_tex
                        .as_ref()
                        .map_or(self.document().source().as_str(), |projection| {
                            projection.source.as_str()
                        }),
                ),
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
                    if provider == Provider::Texlab {
                        self.activity.completion = None;
                        self.activity.hover = None;
                    }
                    if provider == Provider::Badness && self.format_request_key.is_some() {
                        self.format_request_key = None;
                        self.activity.format_error = Some(message.clone());
                    }
                    // A provider failure is not a successful empty diagnostic response.
                    // Keep its last results while the activity panel reports the failure.
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
                    let diagnostics = if let Some(projection) = &self.embedded_tex {
                        diagnostics
                            .into_iter()
                            .filter_map(|mut diagnostic| {
                                if !self.settings.tex.embedded_diagnostics {
                                    return None;
                                }
                                diagnostic.range = projection
                                    .map_range(self.document().source(), &diagnostic.range)?;
                                Some(diagnostic)
                            })
                            .collect()
                    } else {
                        diagnostics
                    };
                    self.activity.diagnostics[1 + provider_index(provider)] =
                        Some(self.document().key());
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
                            diagnostic.provider = Some(provider.label().into());
                            diagnostic
                        })
                        .collect();
                    self.update_tex_diagnostics();
                }
                Event::Formatted { request, edits } if self.embedded_tex.is_some() => {
                    self.receive_embedded_format(context, request.identity.key, edits);
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
                    if self.activity.hover == Some(token) {
                        self.activity.hover = None;
                    }
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
                    match request.kind {
                        RequestKind::Hover { token, .. } if self.activity.hover == Some(token) => {
                            self.activity.hover = None;
                            self.activity.intelligence_error = Some(message.clone());
                        }
                        RequestKind::Completion { token, .. }
                            if self.activity.completion == Some(token) =>
                        {
                            self.activity.completion = None;
                            self.activity.intelligence_error = Some(message.clone());
                        }
                        _ => {}
                    }
                    if matches!(request.kind, RequestKind::Format) {
                        self.activity.format_error = Some(message.clone());
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
        let ready = (self.document().kind() == DocumentKind::Tex || self.embedded_tex.is_some())
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
            .chain(self.writing.diagnostics.iter())
            .filter(|diagnostic| {
                diagnostic_code(diagnostic)
                    .is_none_or(|code| !self.settings.tex.ignores_diagnostic_code(&code))
            })
            .cloned()
            .collect();
        normalize_diagnostics(&mut diagnostics);
        self.preview.editor_diagnostics = diagnostics;
        self.mark_diagnostics_changed();
    }
    fn receive_embedded_format(
        &mut self,
        context: &egui::Context,
        key: DocumentKey,
        edits: Option<Vec<LspTextEdit>>,
    ) {
        if self.format_request_key != Some(key) || self.document().key() != key {
            return;
        }
        self.format_request_key = None;
        let save_after_format = self.manual_format_revision == Some(key.revision);
        let result = (|| -> Result<(), String> {
            let projection = self.embedded_tex.as_ref().ok_or("Math source changed")?;
            let edits = edits.unwrap_or_default();
            let formatted = tiptoptyp_core::text::apply_text_edits(
                &projection.source,
                &edits,
                [tiptoptyp_core::text::ScalarOffset::new(0); 2],
            )?;
            let edits = projection.formatted_edits(self.document().source(), &formatted.text)?;
            let snapshot = self.editor_snapshot(context);
            let applied = tiptoptyp_core::text::apply_text_edits(
                self.document().source(),
                &edits,
                [
                    snapshot.cursor.primary.index.0,
                    snapshot.cursor.secondary.index.0,
                ]
                .map(tiptoptyp_core::text::ScalarOffset::new),
            )?;
            self.document_mut()
                .edit(snapshot.cursor, |source| *source = applied.text);
            self.pending_editor_selection = Some(EditorSelection::Focus(
                applied.mapped_offsets[1].get()..applied.mapped_offsets[0].get(),
            ));
            self.mark_edited();
            if save_after_format && let Some(path) = self.document().path().clone() {
                self.save_to_with_intent(path, SaveIntent::Explicit, context);
            }
            Ok(())
        })();
        self.manual_format_revision = None;
        if let Err(error) = result {
            self.activity.format_error = Some(error.clone());
            self.show_file_error(error);
        }
    }

    pub(super) fn request_tex_format(&mut self) {
        self.activity.format_error = None;
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
                self.activity.format_error = Some(message.clone());
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
