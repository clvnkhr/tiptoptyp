//! Offline PDF.js assets and one immutable PDF snapshot per preview surface.
//! A capability URL on loopback serves only these resources, never workspace files.
use std::{
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use include_dir::{Dir, include_dir};
use serde::Serialize;
use tiny_http::{Header, Method, Response, Server, StatusCode};

static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/assets/pdfjs");
const HOST_SCRIPT: &[u8] = include_bytes!("pdfjs/host.js");
const HOST_STYLE: &[u8] = include_bytes!("pdfjs/host.css");

#[derive(Clone, Serialize)]
pub(crate) struct DocumentState {
    pub(crate) document: u64,
    pub(crate) revision: u64,
    pub(crate) url: String,
    pub(crate) filename: String,
    pub(crate) dark: bool,
}

struct Snapshot {
    path: PathBuf,
    pdf: Arc<[u8]>,
    state: DocumentState,
}

pub(crate) struct PdfJsServer {
    server: Arc<Server>,
    worker: Option<thread::JoinHandle<()>>,
    snapshot: Arc<Mutex<Option<Snapshot>>>,
    base: String,
    revision: u64,
    document: u64,
}

impl PdfJsServer {
    pub(crate) fn start() -> Result<Self, String> {
        let mut secret = [0_u8; 24];
        getrandom::fill(&mut secret).map_err(|e| format!("PDF.js URL token: {e}"))?;
        let token: String = secret.iter().map(|byte| format!("{byte:02x}")).collect();
        let server = Arc::new(Server::http("127.0.0.1:0").map_err(|e| e.to_string())?);
        let address = server.server_addr().to_string();
        let prefix = format!("/{token}/");
        let base = format!("http://{address}{prefix}");
        let snapshot = Arc::new(Mutex::new(None));
        let worker_server = server.clone();
        let worker_snapshot = snapshot.clone();
        let worker = thread::Builder::new()
            .name("tiptoptyp-pdfjs".into())
            .spawn(move || {
                // recv blocks until a request or Drop::unblock; no idle polling.
                while let Ok(request) = worker_server.recv() {
                    let host = request.headers().iter().find(|h| h.field.equiv("Host"));
                    let origin = request.headers().iter().find(|h| h.field.equiv("Origin"));
                    let allowed_origin = format!("http://{address}");
                    let allowed = host.is_some_and(|h| h.value.as_str() == address)
                        && origin.is_none_or(|h| h.value.as_str() == allowed_origin)
                        && matches!(request.method(), Method::Get | Method::Head);
                    let response = if allowed {
                        resource(request.url(), &prefix, &worker_snapshot)
                    } else {
                        reply(403, "text/plain", Arc::from(&b"Forbidden"[..]))
                    };
                    let _ = request.respond(response);
                }
            })
            .map_err(|e| format!("Could not start PDF.js asset server: {e}"))?;
        Ok(Self {
            server,
            worker: Some(worker),
            snapshot,
            base,
            revision: 0,
            document: 0,
        })
    }

    pub(crate) fn viewer_url(&self) -> String {
        format!("{}web/viewer.html", self.base)
    }

    /// Arc identity makes unchanged frames O(1), even for very large PDFs.
    /// Invalidation keeps the previous snapshot; only accepted bytes replace it.
    pub(crate) fn publish(&mut self, path: &Path, pdf: Arc<[u8]>, dark: bool) -> bool {
        let mut snapshot = self.snapshot.lock().expect("PDF.js snapshot lock");
        let same_document = snapshot.as_ref().is_some_and(|old| old.path == path);
        if let Some(old) = snapshot.as_mut()
            && same_document
            && Arc::ptr_eq(&old.pdf, &pdf)
        {
            let changed = old.state.dark != dark;
            old.state.dark = dark;
            return changed;
        }
        self.revision += 1;
        if !same_document {
            self.document += 1;
        }
        *snapshot = Some(Snapshot {
            path: path.to_owned(),
            pdf,
            state: DocumentState {
                document: self.document,
                revision: self.revision,
                url: format!("{}document.pdf?revision={}", self.base, self.revision),
                filename: path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
                    + ".pdf",
                dark,
            },
        });
        true
    }
}

impl Drop for PdfJsServer {
    fn drop(&mut self) {
        self.server.unblock();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

type HttpResponse = Response<Cursor<Arc<[u8]>>>;

fn reply(status: u16, mime: &str, bytes: Arc<[u8]>) -> HttpResponse {
    let len = bytes.len();
    let headers = [
        ("Content-Type", mime),
        ("Cache-Control", "no-store"),
        ("X-Content-Type-Options", "nosniff"),
        ("Referrer-Policy", "no-referrer"),
        ("Cross-Origin-Resource-Policy", "same-origin"),
        ("Content-Security-Policy", "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; font-src 'self' blob: data:; object-src 'none'; frame-ancestors 'none'; base-uri 'self'"),
    ].into_iter().map(|(name, value)| Header::from_bytes(name, value).expect("static HTTP header")).collect();
    Response::new(
        StatusCode(status),
        headers,
        Cursor::new(bytes),
        Some(len),
        None,
    )
}

fn resource(url: &str, prefix: &str, snapshot: &Mutex<Option<Snapshot>>) -> HttpResponse {
    let Some(path) = url.strip_prefix(prefix) else {
        return reply(404, "text/plain", Arc::from(&b"Not found"[..]));
    };
    if path == "state.json" {
        let snapshot = snapshot.lock().expect("PDF.js snapshot lock");
        let state = snapshot.as_ref().map(|s| &s.state);
        return reply(
            200,
            "application/json",
            serde_json::to_vec(&state).unwrap().into(),
        );
    }
    if path.starts_with("document.pdf?") {
        let snapshot = snapshot.lock().expect("PDF.js snapshot lock");
        if let Some(snapshot) = snapshot.as_ref()
            && path == format!("document.pdf?revision={}", snapshot.state.revision)
        {
            return reply(200, "application/pdf", snapshot.pdf.clone());
        }
        // A stale request must never receive bytes from a newer build.
        return reply(409, "text/plain", Arc::from(&b"Superseded PDF"[..]));
    }
    if path == "host.js" {
        return reply(200, "text/javascript", HOST_SCRIPT.into());
    }
    if path == "host.css" {
        return reply(200, "text/css", HOST_STYLE.into());
    }
    if let Some(file) = ASSETS.get_file(path) {
        if path == "web/viewer.html" {
            let html = std::str::from_utf8(file.contents()).expect("upstream viewer HTML");
            let html = html.replace("<head>", "<head>\n<script src=\"../host.js\"></script>\n<link rel=\"stylesheet\" href=\"../host.css\">");
            return reply(200, "text/html; charset=utf-8", html.into_bytes().into());
        }
        let mime = match Path::new(path).extension().and_then(|e| e.to_str()) {
            Some("mjs" | "js") => "text/javascript",
            Some("css") => "text/css",
            Some("json") => "application/json",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("wasm") => "application/wasm",
            Some("ftl" | "txt") => "text/plain; charset=utf-8",
            _ => "application/octet-stream",
        };
        return reply(200, mime, file.contents().into());
    }
    reply(404, "text/plain", Arc::from(&b"Not found"[..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_frames_reuse_bytes_and_rebuilds_reject_stale_urls() {
        let mut server = PdfJsServer::start().unwrap();
        let pdf: Arc<[u8]> = b"first PDF".as_slice().into();
        assert!(server.publish(Path::new("a.typ"), pdf.clone(), false));
        for _ in 0..100 {
            assert!(!server.publish(Path::new("a.typ"), pdf.clone(), false));
        }
        assert_eq!(server.revision, 1);
        assert!(server.publish(Path::new("a.typ"), pdf, true));
        assert_eq!(server.revision, 1);
        assert!(server.publish(Path::new("a.typ"), b"new PDF".as_slice().into(), true));
        assert_eq!((server.document, server.revision), (1, 2));
        assert_eq!(
            resource("/x/document.pdf?revision=1", "/x/", &server.snapshot).status_code(),
            StatusCode(409)
        );
        let body = ureq::get(format!("{}document.pdf?revision=2", server.base))
            .call()
            .unwrap()
            .body_mut()
            .read_to_vec()
            .unwrap();
        assert_eq!(body, b"new PDF");
        server.publish(Path::new("b.typ"), b"other PDF".as_slice().into(), false);
        assert_eq!(server.document, 2);
    }

    #[test]
    fn serves_offline_viewer_and_denies_unknown_files_and_wrong_capabilities() {
        let snapshot = Mutex::new(None);
        for path in [
            "web/viewer.html",
            "web/viewer.mjs",
            "build/pdf.mjs",
            "build/pdf.worker.mjs",
            "host.js",
            "host.css",
        ] {
            assert_eq!(
                resource(&format!("/secret/{path}"), "/secret/", &snapshot).status_code(),
                StatusCode(200),
                "{path}"
            );
        }
        for path in [
            "/web/viewer.html",
            "/wrong/state.json",
            "/secret/../../Cargo.toml",
            "/secret/%2e%2e/Cargo.toml",
            "/secret/Cargo.toml",
        ] {
            assert_eq!(
                resource(path, "/secret/", &snapshot).status_code(),
                StatusCode(404),
                "{path}"
            );
        }
    }

    /// Driven by scripts/check-pdfjs.mjs. No test-only HTTP mutation endpoint.
    #[test]
    #[ignore = "requires the opt-in PDF.js browser fixture driver"]
    fn browser_fixture() {
        use std::io::{BufRead as _, Write as _};
        let Some(path) = std::env::var_os("TIPTOPTYP_PDFJS_FIXTURE") else {
            return;
        };
        let path = PathBuf::from(path);
        let original = std::fs::read(&path).unwrap();
        let mut server = PdfJsServer::start().unwrap();
        server.publish(&path, original.clone().into(), false);
        println!("{}", serde_json::json!({"url": server.viewer_url()}));
        std::io::stdout().flush().unwrap();
        for line in std::io::stdin().lock().lines() {
            match line.unwrap().as_str() {
                "quit" => break,
                "reload" => {
                    server.publish(&path, original.clone().into(), false);
                }
                "switch" => {
                    server.publish(
                        &path.with_file_name("other.pdf"),
                        original.clone().into(),
                        false,
                    );
                }
                "short" => {
                    let short = std::env::var_os("TIPTOPTYP_PDFJS_SHORT_FIXTURE").unwrap();
                    server.publish(&path, std::fs::read(short).unwrap().into(), false);
                }
                "dark" => {
                    let pdf = server
                        .snapshot
                        .lock()
                        .unwrap()
                        .as_ref()
                        .unwrap()
                        .pdf
                        .clone();
                    server.publish(&path, pdf, true);
                }
                command => panic!("Unknown fixture command: {command}"),
            }
            println!("{}", serde_json::json!({"published": server.revision}));
            std::io::stdout().flush().unwrap();
        }
    }
}
