use eframe::egui::text::CCursorRange;
use std::path::Path;
pub(crate) use tiptoptyp_core::document::{DocumentKey, DocumentKind, DocumentSnapshot};
pub(crate) type DocumentSession = tiptoptyp_core::document::DocumentSession<CCursorRange>;
pub(crate) type EditorSnapshot = tiptoptyp_core::document::EditorSnapshot<CCursorRange>;
pub(crate) fn detect_document(path: &Path, bytes: &[u8]) -> Result<DocumentKind, String> {
    // Content signatures take precedence over a misleading extension. In
    // particular, renaming an already-open PDF/image must never turn its
    // empty editor buffer into an editable text document.
    if bytes.starts_with(b"%PDF-") {
        return Ok(DocumentKind::Pdf);
    }
    if image::guess_format(bytes).is_ok() {
        return Ok(DocumentKind::Image);
    }

    match extension_kind(path) {
        Some(DocumentKind::Typst) => return utf8_document(bytes, DocumentKind::Typst, path),
        Some(DocumentKind::Text) => return utf8_document(bytes, DocumentKind::Text, path),
        Some(kind) => return Ok(kind),
        None if looks_like_text(bytes) => return Ok(DocumentKind::Text),
        None => {}
    }

    Err(format!(
        "{} is not a supported text, image, or PDF file",
        path.display()
    ))
}

pub(crate) fn supports_path(path: &Path) -> bool {
    extension_kind(path).is_some()
}
pub(crate) fn preview_kind_for_path(path: &Path) -> Option<DocumentKind> {
    extension_kind(path).filter(|kind| kind.preview_only())
}
fn utf8_document(bytes: &[u8], kind: DocumentKind, path: &Path) -> Result<DocumentKind, String> {
    std::str::from_utf8(bytes)
        .map(|_| kind)
        .map_err(|error| format!("Could not open {} as UTF-8 text: {error}", path.display()))
}
fn looks_like_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}
fn extension_kind(path: &Path) -> Option<DocumentKind> {
    let extension = path.extension()?.to_str()?;
    if extension.eq_ignore_ascii_case("typ") {
        Some(DocumentKind::Typst)
    } else if extension.eq_ignore_ascii_case("pdf") {
        Some(DocumentKind::Pdf)
    } else if IMAGE_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        Some(DocumentKind::Image)
    } else if TEXT_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
    {
        Some(DocumentKind::Text)
    } else {
        None
    }
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff",
];

const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "markdown", "rs", "toml", "json", "jsonc", "yaml", "yml", "xml", "html", "htm",
    "css", "scss", "js", "jsx", "ts", "tsx", "py", "rb", "go", "java", "c", "h", "cc", "cpp",
    "hpp", "sh", "bash", "zsh", "fish", "sql", "csv", "tsv", "ini", "cfg", "conf", "log", "tex",
    "bib",
];

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn recognizes_supported_document_kinds_case_insensitively() {
        assert_eq!(
            detect_document(Path::new("main.TYP"), b"= Hello").unwrap(),
            DocumentKind::Typst
        );
        assert_eq!(
            detect_document(Path::new("notes.MD"), b"# Hello").unwrap(),
            DocumentKind::Text
        );
        assert_eq!(
            detect_document(Path::new("paper.PDF"), b"%PDF-1.7").unwrap(),
            DocumentKind::Pdf
        );
    }

    #[test]
    fn accepts_extensionless_utf8_but_rejects_binary_data() {
        assert_eq!(
            detect_document(Path::new("README"), b"plain text").unwrap(),
            DocumentKind::Text
        );
        assert!(detect_document(Path::new("blob"), b"\0\x01\x02").is_err());
    }

    #[test]
    fn pdf_magic_wins_without_an_extension() {
        assert_eq!(
            detect_document(Path::new("download"), b"%PDF-2.0\n").unwrap(),
            DocumentKind::Pdf
        );
    }

    #[test]
    fn binary_magic_wins_after_renaming_to_a_text_extension() {
        assert_eq!(
            detect_document(Path::new("renamed.typ"), b"%PDF-2.0\n").unwrap(),
            DocumentKind::Pdf
        );
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(
            detect_document(Path::new("renamed.txt"), png).unwrap(),
            DocumentKind::Image
        );
    }

    #[test]
    fn advertised_paths_and_detection_share_one_case_insensitive_extension_policy() {
        for (path, expected) in [
            ("main.TyP", DocumentKind::Typst),
            ("notes.JsOnC", DocumentKind::Text),
            ("photo.WeBp", DocumentKind::Image),
            ("paper.PdF", DocumentKind::Pdf),
        ] {
            let path = Path::new(path);
            assert!(supports_path(path));
            assert_eq!(
                detect_document(path, b"plain utf-8 text").unwrap(),
                expected
            );
        }
        assert!(!supports_path(Path::new("archive.zip")));
    }

    #[test]
    fn hover_preview_classification_excludes_editable_files() {
        assert_eq!(
            preview_kind_for_path(Path::new("photo.JPEG")),
            Some(DocumentKind::Image)
        );
        assert_eq!(
            preview_kind_for_path(Path::new("paper.PDF")),
            Some(DocumentKind::Pdf)
        );
        assert_eq!(preview_kind_for_path(Path::new("chapter.typ")), None);
    }

    #[test]
    fn declared_text_files_still_require_valid_utf8() {
        let error = detect_document(Path::new("broken.toml"), b"\xff\xfe").unwrap_err();
        assert!(error.contains("UTF-8"));
        assert!(error.contains("broken.toml"));
    }
}
