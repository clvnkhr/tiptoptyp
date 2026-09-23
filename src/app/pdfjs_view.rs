//! PDF.js native surfaces for opened PDFs and the Typst PDF fallback.
use super::*;

#[derive(Default)]
pub(super) struct PdfJsView {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    native: Option<NativePdfJsView>,
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    error: Option<String>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
struct NativePdfJsView {
    // Drop the web view before stopping its asset server.
    webview: wry::WebView,
    server: crate::pdfjs::PdfJsServer,
    bounds: NativeRect,
    visible: bool,
    events: mpsc::Receiver<PdfJsEvent>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(serde::Deserialize)]
struct PdfJsEvent {
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

impl PdfJsView {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }

    pub(super) fn hide(&mut self) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(native) = &mut self.native
            && native.visible
        {
            let _ = native.webview.set_visible(false);
            native.visible = false;
        }
    }

    pub(super) fn zoom(&self, action: PreviewZoomAction) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        if let Some(native) = &self.native {
            let command = match action {
                PreviewZoomAction::In => "in",
                PreviewZoomAction::Out => "out",
                PreviewZoomAction::Reset => "reset",
            };
            let _ = native.webview.evaluate_script(&format!(
                "window.dispatchEvent(new CustomEvent('tiptoptyp-preview-zoom', {{detail: '{command}'}}))"
            ));
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = action;
    }
}

impl EditorApp {
    pub(super) fn pdfjs_preview_requested(&self) -> bool {
        self.preview_status_snapshot().effective_backend == crate::preview::PreviewBackend::PdfJs
    }

    pub(super) fn pdfjs_asset_requested(&self) -> bool {
        self.document().kind() == DocumentKind::Pdf
            && self.settings.preview_preference != PreviewPreference::Pdfium
            && cfg!(any(target_os = "macos", target_os = "windows"))
    }

    pub(super) fn clear_pdfjs_views(&mut self) {
        self.pdfjs_preview.clear();
        self.pdfjs_asset.clear();
        self.pdfium_preview = Default::default();
        self.pdfium_asset = Default::default();
    }

    pub(super) fn show_pdfjs_view(
        &mut self,
        ui: &mut egui::Ui,
        frame: Option<&eframe::Frame>,
        asset: bool,
    ) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let path = if asset {
                self.document().path().clone().unwrap_or_default()
            } else {
                self.preview_document_path()
            };
            let preview = if asset {
                &self.asset_preview
            } else {
                &self.preview
            };
            let Some(pdf) = preview.content.pdf().cloned() else {
                show_centered_preview_message(
                    ui,
                    "Waiting for a PDF…",
                    preview.status != PreviewStatus::Error,
                );
                return;
            };
            let dark = preview.render_dark();
            let view = if asset {
                &mut self.pdfjs_asset
            } else {
                &mut self.pdfjs_preview
            };
            let available = ui.available_rect_before_wrap();
            let clip = ui.clip_rect();
            let rect = clipped_preview_rect(available, clip);
            let Some(native_rect) = egui_rect_to_native(ui.ctx(), rect) else {
                view.hide();
                return;
            };
            trace_native_preview_bounds(ui.ctx(), available, clip, rect, native_rect);
            if view.native.is_none() && view.error.is_none() {
                if !may_create_window_webview(
                    self.window_host,
                    cfg!(target_os = "macos"),
                    ui.ctx().input(|i| i.viewport().focused),
                ) {
                    show_preview_transition(ui, true);
                    return;
                }
                let result = (|| {
                    let mut server = crate::pdfjs::PdfJsServer::start()?;
                    server.publish(&path, pdf.clone(), dark);
                    let url = server.viewer_url();
                    let navigation_url = url.clone();
                    let link_sender = self.web_link_sender.clone();
                    let popup_sender = link_sender.clone();
                    let repaint = crate::worker::RepaintTarget::current(ui.ctx());
                    let popup_repaint = repaint.clone();
                    let event_repaint = repaint.clone();
                    let (sender, events) = mpsc::channel();
                    let builder = wry::WebViewBuilder::new()
                        .with_url(&url)
                        .with_bounds(webview_bounds(native_rect))
                        .with_hotkeys_zoom(false)
                        .with_ipc_handler(move |request| {
                            if let Ok(event) = serde_json::from_str::<PdfJsEvent>(request.body())
                                && sender.send(event).is_ok()
                            {
                                event_repaint.request_repaint();
                            }
                        })
                        .with_navigation_handler(move |candidate| {
                            // Only the viewer itself and in-document fragments
                            // may navigate this child; links go through the app.
                            if internal_pdfjs_navigation(&navigation_url, &candidate) {
                                return true;
                            }
                            if safe_pdfjs_link(&candidate) && link_sender.send(candidate).is_ok() {
                                repaint.request_repaint();
                            }
                            false
                        })
                        .with_new_window_req_handler(move |candidate, _| {
                            if safe_pdfjs_link(&candidate) && popup_sender.send(candidate).is_ok() {
                                popup_repaint.request_repaint();
                            }
                            wry::NewWindowResponse::Deny
                        });
                    let webview = if self.window_host.is_root() {
                        let window = frame
                            .and_then(eframe::Frame::winit_window)
                            .ok_or("Waiting for the native window")?;
                        builder.build_as_child(window.as_ref())
                    } else {
                        let window = self
                            .native_window_parent
                            .as_ref()
                            .ok_or("Waiting for the document window")?;
                        builder.build_as_child(window)
                    }
                    .map_err(|e| e.to_string())?;
                    // PDF.js owns pinch/wheel zoom too, so reset acts on the
                    // same scale. Do not add a second WKWebView magnification.
                    Ok::<_, String>(NativePdfJsView {
                        webview,
                        server,
                        bounds: native_rect,
                        visible: true,
                        events,
                    })
                })();
                match result {
                    Ok(native) => view.native = Some(native),
                    Err(error) => view.error = Some(error),
                }
            }
            if let Some(native) = &mut view.native {
                while let Ok(event) = native.events.try_recv() {
                    match event.kind.as_str() {
                        "error" => view.error = Some(event.message),
                        "loaded" => view.error = None,
                        _ => {}
                    }
                }
                if native.server.publish(&path, pdf, dark) {
                    let _ = native
                        .webview
                        .evaluate_script("window.tiptoptypPdf?.refresh()");
                }
                if native.bounds != native_rect {
                    match native.webview.set_bounds(webview_bounds(native_rect)) {
                        Ok(()) => native.bounds = native_rect,
                        Err(error) => view.error = Some(error.to_string()),
                    }
                }
                if !native.visible && view.error.is_none() {
                    match native.webview.set_visible(true) {
                        Ok(()) => native.visible = true,
                        Err(error) => view.error = Some(error.to_string()),
                    }
                }
            }
            if let Some(error) = view.error.clone() {
                view.hide();
                ui.vertical_centered(|ui| {
                    ui.label("PDF.js preview could not load");
                    ui.add(egui::Label::new(error).wrap());
                    if crate::app::icons::action_button(ui, "Retry PDF.js").clicked() {
                        view.clear();
                        ui.ctx().request_repaint();
                    }
                });
            } else {
                ui.allocate_rect(rect, Sense::hover());
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = (frame, asset);
            show_centered_preview_message(
                ui,
                "PDF.js requires native web-view support (macOS or Windows).",
                false,
            );
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn webview_bounds(rect: NativeRect) -> wry::Rect {
    wry::Rect {
        position: wry::dpi::LogicalPosition::new(rect.left() as f64, rect.top() as f64).into(),
        size: wry::dpi::LogicalSize::new(
            rect.width().max(1.0) as f64,
            rect.height().max(1.0) as f64,
        )
        .into(),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn internal_pdfjs_navigation(viewer: &str, candidate: &str) -> bool {
    let candidate = candidate.split('#').next().unwrap_or(candidate);
    candidate == viewer
        || viewer
            .strip_suffix("viewer.html")
            .is_some_and(|base| candidate.strip_prefix(base) == Some("frame.html"))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn safe_pdfjs_link(target: &str) -> bool {
    url::Url::parse(target).is_ok_and(|url| matches!(url.scheme(), "http" | "https" | "mailto"))
}

#[cfg(all(test, any(target_os = "macos", target_os = "windows")))]
mod tests {
    use super::*;

    #[test]
    fn staging_frames_share_only_the_exact_capability_navigation() {
        let viewer = "http://127.0.0.1:1234/secret/web/viewer.html";
        for candidate in [
            viewer,
            "http://127.0.0.1:1234/secret/web/frame.html",
            "http://127.0.0.1:1234/secret/web/frame.html#page=2",
        ] {
            assert!(internal_pdfjs_navigation(viewer, candidate));
        }
        for candidate in [
            "http://127.0.0.1:1234/other/web/frame.html",
            "https://example.com/frame.html",
            "http://127.0.0.1:1234/secret/web/frame.html?other",
        ] {
            assert!(!internal_pdfjs_navigation(viewer, candidate));
        }
    }
    #[test]
    fn pdf_links_only_dispatch_supported_external_schemes() {
        for link in [
            "https://example.com/document",
            "http://example.com/",
            "mailto:editor@example.com",
        ] {
            assert!(safe_pdfjs_link(link));
        }
        for link in [
            "javascript:alert(1)",
            "data:text/html,hello",
            "file:///tmp/document",
            "#page=2",
            "blob:http://localhost/id",
        ] {
            assert!(!safe_pdfjs_link(link));
        }
    }
}
