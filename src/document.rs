use std::path::Path;

/// How the currently selected file should be presented.
///
/// `ViewMode` remains the user's layout preference. Binary documents use
/// `preview_only` as a temporary presentation override, so opening a PDF or
/// image never silently changes that preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentKind {
    Typst,
    Text,
    Image,
    Pdf,
}

impl DocumentKind {
    pub fn detect(path: &Path, bytes: &[u8]) -> Result<Self, String> {
        // Content signatures take precedence over a misleading extension. In
        // particular, renaming an already-open PDF/image must never turn its
        // empty editor buffer into an editable text document.
        if bytes.starts_with(b"%PDF-") {
            return Ok(Self::Pdf);
        }
        if image::guess_format(bytes).is_ok() {
            return Ok(Self::Image);
        }

        match extension_kind(path) {
            Some(Self::Typst) => return utf8_document(bytes, Self::Typst, path),
            Some(Self::Text) => return utf8_document(bytes, Self::Text, path),
            Some(kind) => return Ok(kind),
            None if looks_like_text(bytes) => return Ok(Self::Text),
            None => {}
        }

        Err(format!(
            "{} is not a supported text, image, or PDF file",
            path.display()
        ))
    }

    pub fn is_editable(self) -> bool {
        matches!(self, Self::Typst | Self::Text)
    }

    pub fn is_typst(self) -> bool {
        self == Self::Typst
    }

    pub fn preview_only(self) -> bool {
        matches!(self, Self::Image | Self::Pdf)
    }

    pub fn supports_path(path: &Path) -> bool {
        extension_kind(path).is_some()
    }
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
            DocumentKind::detect(Path::new("main.TYP"), b"= Hello").unwrap(),
            DocumentKind::Typst
        );
        assert_eq!(
            DocumentKind::detect(Path::new("notes.MD"), b"# Hello").unwrap(),
            DocumentKind::Text
        );
        assert_eq!(
            DocumentKind::detect(Path::new("paper.PDF"), b"%PDF-1.7").unwrap(),
            DocumentKind::Pdf
        );
    }

    #[test]
    fn accepts_extensionless_utf8_but_rejects_binary_data() {
        assert_eq!(
            DocumentKind::detect(Path::new("README"), b"plain text").unwrap(),
            DocumentKind::Text
        );
        assert!(DocumentKind::detect(Path::new("blob"), b"\0\x01\x02").is_err());
    }

    #[test]
    fn pdf_magic_wins_without_an_extension() {
        assert_eq!(
            DocumentKind::detect(Path::new("download"), b"%PDF-2.0\n").unwrap(),
            DocumentKind::Pdf
        );
    }

    #[test]
    fn binary_magic_wins_after_renaming_to_a_text_extension() {
        assert_eq!(
            DocumentKind::detect(Path::new("renamed.typ"), b"%PDF-2.0\n").unwrap(),
            DocumentKind::Pdf
        );
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR";
        assert_eq!(
            DocumentKind::detect(Path::new("renamed.txt"), png).unwrap(),
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
            assert!(DocumentKind::supports_path(path));
            assert_eq!(
                DocumentKind::detect(path, b"plain utf-8 text").unwrap(),
                expected
            );
        }
        assert!(!DocumentKind::supports_path(Path::new("archive.zip")));
    }

    #[test]
    fn declared_text_files_still_require_valid_utf8() {
        let error = DocumentKind::detect(Path::new("broken.toml"), b"\xff\xfe").unwrap_err();
        assert!(error.contains("UTF-8"));
        assert!(error.contains("broken.toml"));
    }
}
