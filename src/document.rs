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
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase);

        if extension.as_deref() == Some("typ") {
            return utf8_document(bytes, Self::Typst, path);
        }
        if extension.as_deref() == Some("pdf") || bytes.starts_with(b"%PDF-") {
            return Ok(Self::Pdf);
        }
        if extension
            .as_deref()
            .is_some_and(is_supported_image_extension)
            || image::guess_format(bytes).is_ok()
        {
            return Ok(Self::Image);
        }
        if extension.as_deref().is_some_and(is_text_extension) || looks_like_text(bytes) {
            return utf8_document(bytes, Self::Text, path);
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
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
            .is_some_and(|extension| {
                extension == "typ"
                    || extension == "pdf"
                    || is_text_extension(extension)
                    || is_supported_image_extension(extension)
            })
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

fn is_supported_image_extension(extension: &str) -> bool {
    matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tif" | "tiff"
    )
}

fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "markdown"
            | "rs"
            | "toml"
            | "json"
            | "jsonc"
            | "yaml"
            | "yml"
            | "xml"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "py"
            | "rb"
            | "go"
            | "java"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "sql"
            | "csv"
            | "tsv"
            | "ini"
            | "cfg"
            | "conf"
            | "log"
            | "tex"
            | "bib"
    )
}

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
}
