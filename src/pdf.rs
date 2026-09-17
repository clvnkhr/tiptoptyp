//! PDF decoding shared by compiled documents, opened assets, and thumbnails.
//! Callers own artifact identity, scheduling, cancellation, and canonical export bytes.
use crate::{private_workspace::PrivateWorkspace, process::finish_reader_with_timeout};
use quick_xml::{
    Decoder, Reader, XmlVersion,
    events::{BytesStart, Event},
};
use std::{
    fs,
    io::{BufReader, Read},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const PIPE_READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
/// Resolution used only by the native recovery viewer. The primary Tinymist
/// viewer is vector-based. 144 DPI keeps the fallback crisp at 100% on a 2×
/// display without making every incremental build excessively expensive.
pub const PREVIEW_DPI: f32 = 144.0;
#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub links: Vec<PreviewLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewLink {
    /// Link rectangle normalized to the page's width and height.
    pub rect: [f32; 4],
    pub target: String,
}

/// Rasterize already-snapshotted PDF bytes for either a Typst build or a PDF
/// opened directly from the project tree. The caller owns cancellation, which
/// lets the compiler and the asset loader discard obsolete long documents.
pub(crate) fn rasterize_pdf(
    pdf: &[u8],
    project_root: &Path,
    cancelled: impl FnMut() -> bool,
) -> Result<Vec<PreviewPage>, String> {
    rasterize_pdf_with_program(
        pdf,
        project_root,
        Path::new("pdftoppm"),
        PdfRasterMode::Document,
        cancelled,
    )
}

/// Render only the first page of a PDF, capped to `max_dimension` pixels on
/// its longest edge. Hover previews use this path so a large PDF cannot fill
/// the thumbnail cache with every page or full-resolution pixels.
pub(crate) fn rasterize_pdf_first_page(
    pdf: &[u8],
    project_root: &Path,
    max_dimension: u32,
    cancelled: impl FnMut() -> bool,
) -> Result<PreviewPage, String> {
    let mut pages = rasterize_pdf_with_program(
        pdf,
        project_root,
        Path::new("pdftoppm"),
        PdfRasterMode::FirstPage { max_dimension },
        cancelled,
    )?;
    pages
        .pop()
        .ok_or_else(|| "The PDF renderer produced no preview pages".to_owned())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PdfRasterMode {
    Document,
    FirstPage { max_dimension: u32 },
}

pub(crate) fn rasterize_pdf_with_program(
    pdf: &[u8],
    project_root: &Path,
    rasterizer: &Path,
    mode: PdfRasterMode,
    mut cancelled: impl FnMut() -> bool,
) -> Result<Vec<PreviewPage>, String> {
    let private = PrivateWorkspace::open(project_root).map_err(|error| {
        format!(
            "Could not prepare private PDF-rendering storage in {}: {error}",
            project_root.display()
        )
    })?;
    let render_dir = private
        .temp_dir("pages-")
        .map_err(|error| format!("Could not create a private page-rendering directory: {error}"))?;
    let snapshot_path = render_dir.path().join("snapshot.pdf");
    fs::write(&snapshot_path, pdf)
        .map_err(|error| format!("Could not stage the PDF preview: {error}"))?;
    let page_prefix = render_dir.path().join("page");
    let mut render_command = Command::new(rasterizer);
    render_command.arg("-png");
    match mode {
        PdfRasterMode::Document => {
            // Keep this argument order stable: release/testing wrappers may
            // treat the final two arguments as the input and output paths.
            render_command
                .arg("-r")
                .arg(PREVIEW_DPI.to_string())
                .arg(&snapshot_path)
                .arg(&page_prefix);
        }
        PdfRasterMode::FirstPage { max_dimension } => {
            render_command
                .arg("-f")
                .arg("1")
                .arg("-l")
                .arg("1")
                .arg("-scale-to")
                .arg(max_dimension.max(1).to_string())
                .arg(&snapshot_path)
                .arg(&page_prefix);
        }
    }
    let mut render_child = render_command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(rasterizer_command_error)?;

    let stderr = render_child
        .stderr
        .take()
        .ok_or_else(|| "Could not read output from `pdftoppm`".to_owned())?;
    let stderr_reader = match thread::Builder::new()
        .name("tiptoptyp-pdf-render-log".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = BufReader::new(stderr).read_to_end(&mut bytes);
            bytes
        }) {
        Ok(reader) => reader,
        Err(error) => {
            let _ = render_child.kill();
            let _ = render_child.wait();
            return Err(format!(
                "Could not start the PDF renderer log reader: {error}"
            ));
        }
    };

    let render_status = loop {
        if cancelled() {
            let _ = render_child.kill();
            let _ = render_child.wait();
            let _ = finish_reader_with_timeout(stderr_reader, PIPE_READER_SHUTDOWN_TIMEOUT);
            return Err("Preview rendering was superseded by a newer edit".to_owned());
        }
        match render_child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = render_child.kill();
                let _ = render_child.wait();
                let _ = finish_reader_with_timeout(stderr_reader, PIPE_READER_SHUTDOWN_TIMEOUT);
                return Err(format!("Could not wait for the PDF renderer: {error}"));
            }
        }
    };
    let render_stderr =
        finish_reader_with_timeout(stderr_reader, PIPE_READER_SHUTDOWN_TIMEOUT).unwrap_or_default();

    if !render_status.success() {
        let details = String::from_utf8_lossy(&render_stderr);
        return Err(format!("PDF preview rendering failed: {}", details.trim()));
    }

    let mut page_paths = fs::read_dir(render_dir.path())
        .map_err(|error| format!("Could not read rendered preview pages: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|extension| extension == "png")
                && path
                    .file_stem()
                    .is_some_and(|stem| stem.to_string_lossy().starts_with("page-"))
        })
        .collect::<Vec<_>>();
    page_paths.sort_by_key(|path| preview_page_number(path));
    if matches!(mode, PdfRasterMode::FirstPage { .. }) {
        // Honour the thumbnail memory contract even if a wrapper or older
        // Poppler binary ignores the requested page range.
        page_paths.truncate(1);
    }

    if page_paths.is_empty() {
        return Err("The PDF renderer produced no preview pages".to_owned());
    }

    // Link extraction is best-effort: raster rendering remains useful when an
    // older/minimal Poppler installation lacks `pdftohtml`.
    let mut page_links = match mode {
        PdfRasterMode::Document => {
            extract_pdf_links(&snapshot_path, render_dir.path(), &mut cancelled)
        }
        PdfRasterMode::FirstPage { .. } => Vec::new(),
    };
    let mut pages = Vec::with_capacity(page_paths.len());
    for (index, page_path) in page_paths.into_iter().enumerate() {
        if cancelled() {
            return Err("Preview decoding was superseded by a newer edit".to_owned());
        }
        let encoded = fs::read(&page_path)
            .map_err(|error| format!("Could not read a preview page: {error}"))?;
        let decoded = image::load_from_memory_with_format(&encoded, image::ImageFormat::Png)
            .map_err(|error| format!("Could not decode a preview page: {error}"))?
            .into_rgba8();
        let (width, height) = decoded.dimensions();
        pages.push(PreviewPage {
            size: [width as usize, height as usize],
            rgba: decoded.into_raw(),
            links: page_links
                .get_mut(index)
                .map(std::mem::take)
                .unwrap_or_default(),
        });
    }

    Ok(pages)
}

fn extract_pdf_links(
    snapshot_path: &Path,
    render_dir: &Path,
    cancelled: &mut impl FnMut() -> bool,
) -> Vec<Vec<PreviewLink>> {
    let xml_path = render_dir.join("links.xml");
    let child = match Command::new("pdftohtml")
        .arg("-xml")
        .arg("-hidden")
        .arg("-i")
        .arg("-q")
        .arg("-zoom")
        .arg("1")
        .arg(snapshot_path)
        .arg(&xml_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return Vec::new(),
    };

    let Ok(status) = crate::process::wait(child, Duration::from_secs(60), || {
        if cancelled() {
            Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "link extraction cancelled",
            ))
        } else {
            Ok(())
        }
    }) else {
        return Vec::new();
    };
    if !status.success() {
        return Vec::new();
    }

    let Ok(xml) = fs::read_to_string(&xml_path) else {
        return Vec::new();
    };
    let generated_stem = xml_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("links");
    parse_pdf_links(&xml, generated_stem).unwrap_or_default()
}

fn parse_pdf_links(xml: &str, generated_stem: &str) -> Result<Vec<Vec<PreviewLink>>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut pages = Vec::<Vec<PreviewLink>>::new();
    let mut current_page = None;
    let mut page_size = [0.0_f32; 2];
    let mut text_rect = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(element)) => match element.name().as_ref() {
                b"page" => {
                    let number = xml_attr(&element, b"number", reader.decoder())
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(pages.len() + 1)
                        .saturating_sub(1);
                    let width = xml_attr(&element, b"width", reader.decoder())
                        .and_then(|value| value.parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let height = xml_attr(&element, b"height", reader.decoder())
                        .and_then(|value| value.parse::<f32>().ok())
                        .unwrap_or(0.0);
                    if pages.len() <= number {
                        pages.resize_with(number + 1, Vec::new);
                    }
                    current_page = Some(number);
                    page_size = [width, height];
                }
                b"text" => {
                    let left = xml_f32_attr(&element, b"left", reader.decoder());
                    let top = xml_f32_attr(&element, b"top", reader.decoder());
                    let width = xml_f32_attr(&element, b"width", reader.decoder());
                    let height = xml_f32_attr(&element, b"height", reader.decoder());
                    text_rect = match (left, top, width, height) {
                        (Some(left), Some(top), Some(width), Some(height))
                            if page_size[0] > 0.0
                                && page_size[1] > 0.0
                                && width > 0.0
                                && height > 0.0 =>
                        {
                            Some([
                                (left / page_size[0]).clamp(0.0, 1.0),
                                (top / page_size[1]).clamp(0.0, 1.0),
                                ((left + width) / page_size[0]).clamp(0.0, 1.0),
                                ((top + height) / page_size[1]).clamp(0.0, 1.0),
                            ])
                        }
                        _ => None,
                    };
                }
                b"a" => {
                    if let (Some(page), Some(rect), Some(target)) = (
                        current_page,
                        text_rect,
                        xml_attr(&element, b"href", reader.decoder()),
                    ) {
                        pages[page].push(PreviewLink {
                            rect,
                            target: normalize_pdftohtml_target(&target, generated_stem),
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::End(element)) => match element.name().as_ref() {
                b"text" => text_rect = None,
                b"page" => {
                    current_page = None;
                    page_size = [0.0; 2];
                }
                _ => {}
            },
            Ok(Event::Empty(element)) if element.name().as_ref() == b"page" => {
                let number = xml_attr(&element, b"number", reader.decoder())
                    .and_then(|value| value.parse::<usize>().ok())
                    .unwrap_or(pages.len() + 1)
                    .saturating_sub(1);
                if pages.len() <= number {
                    pages.resize_with(number + 1, Vec::new);
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(error) => return Err(format!("Could not parse PDF links: {error}")),
        }
    }
    Ok(pages)
}

fn xml_attr(element: &BytesStart<'_>, name: &[u8], decoder: Decoder) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == name)
        .and_then(|attribute| {
            attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, decoder)
                .ok()
        })
        .map(|value| value.into_owned())
}

fn xml_f32_attr(element: &BytesStart<'_>, name: &[u8], decoder: Decoder) -> Option<f32> {
    xml_attr(element, name, decoder)?.parse().ok()
}

fn normalize_pdftohtml_target(target: &str, generated_stem: &str) -> String {
    let Some((path, fragment)) = target.rsplit_once('#') else {
        return target.to_owned();
    };
    let generated_internal = Path::new(path)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("html"))
        && Path::new(path)
            .file_stem()
            .is_some_and(|stem| stem == generated_stem);
    if generated_internal && fragment.parse::<usize>().is_ok() {
        format!("#page={fragment}")
    } else {
        target.to_owned()
    }
}

fn preview_page_number(path: &Path) -> u32 {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.strip_prefix("page-"))
        .and_then(|number| number.parse().ok())
        .unwrap_or(u32::MAX)
}

fn rasterizer_command_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "Poppler was not found. Install `pdftoppm` (usually provided by the `poppler` package) and make sure it is on PATH."
            .to_owned()
    } else {
        format!("Could not start `pdftoppm`: {error}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    #[test]
    fn preview_pages_sort_numerically() {
        assert_eq!(preview_page_number(Path::new("page-2.png")), 2);
        assert_eq!(preview_page_number(Path::new("page-11.png")), 11);
        assert_eq!(preview_page_number(Path::new("preview.pdf")), u32::MAX);
    }

    #[cfg(unix)]
    #[test]
    fn document_rasterization_preserves_snapshot_pixels_and_cancellation() {
        use std::os::unix::fs::PermissionsExt;
        let project = tempfile::tempdir().unwrap();
        let bytes = b"%PDF-canonical-export-byte-fixture";
        fs::write(project.path().join("expected.pdf"), bytes).unwrap();
        image::RgbaImage::from_pixel(2, 1, image::Rgba([12, 34, 56, 255]))
            .save(project.path().join("fixture.png"))
            .unwrap();
        let rasterizer = project.path().join("render");
        fs::write(
            &rasterizer,
            concat!(
                "#!/bin/sh\n",
                "[ \"$1\" = -png ] && [ \"$2\" = -r ] && [ \"$3\" = 144 ] || exit 21\n",
                "cmp \"$4\" \"${0%/*}/expected.pdf\" || exit 22\n",
                "cp \"${0%/*}/fixture.png\" \"$5-10.png\"\n",
                "cp \"${0%/*}/fixture.png\" \"$5-2.png\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&rasterizer, fs::Permissions::from_mode(0o700)).unwrap();
        let pages = rasterize_pdf_with_program(
            bytes,
            project.path(),
            &rasterizer,
            PdfRasterMode::Document,
            || false,
        )
        .unwrap();
        assert_eq!(pages.len(), 2);
        for page in pages {
            assert_eq!(page.size, [2, 1]);
            assert_eq!(page.rgba, [12, 34, 56, 255, 12, 34, 56, 255]);
        }
        assert_eq!(
            fs::read(project.path().join("expected.pdf")).unwrap(),
            bytes
        );
        let error = rasterize_pdf_with_program(
            bytes,
            project.path(),
            &rasterizer,
            PdfRasterMode::Document,
            || true,
        )
        .unwrap_err();
        assert!(error.contains("superseded"));
    }

    #[test]
    fn poppler_link_rectangles_are_normalized_and_internal_pages_are_preserved() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<pdf2xml>
  <page number="1" top="0" left="0" height="200" width="100">
    <text top="40" left="10" width="30" height="20"><a href="https://example.com/?a=1&amp;b=2">External</a></text>
    <text top="80" left="10" width="30" height="20"><a href="links.html#2">Internal</a></text>
  </page>
  <page number="2" top="0" left="0" height="200" width="100" />
</pdf2xml>"#;
        let pages = parse_pdf_links(xml, "links").unwrap();

        assert_eq!(pages.len(), 2);
        assert_eq!(pages[0][0].rect, [0.1, 0.2, 0.4, 0.3]);
        assert_eq!(pages[0][0].target, "https://example.com/?a=1&b=2");
        assert_eq!(pages[0][1].target, "#page=2");
        assert!(pages[1].is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn hover_pdf_rasterization_requests_one_bounded_page() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        let fixture_path = project.path().join("fixture.png");
        image::RgbaImage::from_pixel(2, 1, image::Rgba([12, 34, 56, 255]))
            .save(&fixture_path)
            .unwrap();
        let rasterizer = project.path().join("fake-thumbnail-pdftoppm");
        fs::write(
            &rasterizer,
            concat!(
                "#!/bin/sh\n",
                "[ \"$1\" = \"-png\" ] || exit 21\n",
                "[ \"$2\" = \"-f\" ] || exit 22\n",
                "[ \"$3\" = \"1\" ] || exit 23\n",
                "[ \"$4\" = \"-l\" ] || exit 24\n",
                "[ \"$5\" = \"1\" ] || exit 25\n",
                "[ \"$6\" = \"-scale-to\" ] || exit 26\n",
                "[ \"$7\" = \"333\" ] || exit 27\n",
                "cp \"${0%/*}/fixture.png\" \"${9}-1.png\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&rasterizer, fs::Permissions::from_mode(0o700)).unwrap();

        let pages = rasterize_pdf_with_program(
            b"%PDF-hover-fixture",
            project.path(),
            &rasterizer,
            PdfRasterMode::FirstPage { max_dimension: 333 },
            || false,
        )
        .unwrap();

        assert_eq!(pages.len(), 1);
        assert_eq!(pages[0].size, [2, 1]);
        assert!(pages[0].links.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn rasterizer_exit_does_not_wait_for_a_descendant_holding_stderr() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        image::RgbaImage::from_pixel(2, 1, image::Rgba([12, 34, 56, 255]))
            .save(project.path().join("fixture.png"))
            .unwrap();
        let rasterizer = project.path().join("inherited-stderr-pdftoppm");
        fs::write(
            &rasterizer,
            concat!(
                "#!/bin/sh\n",
                // The finite lifetime makes the fixture self-cleaning even if
                // the regression fails before its explicit kill below.
                "sleep 8 >&2 &\n",
                "printf '%s' \"$!\" > \"${0%/*}/descendant.pid\"\n",
                "cp \"${0%/*}/fixture.png\" \"${9}-1.png\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&rasterizer, fs::Permissions::from_mode(0o700)).unwrap();

        let started = Instant::now();
        let pages = rasterize_pdf_with_program(
            b"%PDF-inherited-stderr-fixture",
            project.path(),
            &rasterizer,
            PdfRasterMode::FirstPage { max_dimension: 333 },
            || false,
        )
        .unwrap();
        let elapsed = started.elapsed();

        let pid = fs::read_to_string(project.path().join("descendant.pid")).unwrap();
        let kill_status = std::process::Command::new("kill")
            .args(["-9", pid.trim()])
            .status()
            .unwrap();

        assert_eq!(pages.len(), 1);
        assert!(
            kill_status.success(),
            "could not terminate fixture pid {pid}"
        );
        assert!(
            elapsed < super::PIPE_READER_SHUTDOWN_TIMEOUT + Duration::from_secs(4),
            "rasterizer waited for a descendant-owned stderr pipe: {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn completed_rasterizer_stderr_is_drained_before_reporting_failure() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        let rasterizer = project.path().join("failing-pdftoppm");
        fs::write(
            &rasterizer,
            "#!/bin/sh\nprintf '%s\\n' 'distinct renderer failure' >&2\nexit 23\n",
        )
        .unwrap();
        fs::set_permissions(&rasterizer, fs::Permissions::from_mode(0o700)).unwrap();

        let error = rasterize_pdf_with_program(
            b"%PDF-failing-fixture",
            project.path(),
            &rasterizer,
            PdfRasterMode::FirstPage { max_dimension: 333 },
            || false,
        )
        .unwrap_err();

        assert!(error.contains("distinct renderer failure"), "{error}");
    }
}
