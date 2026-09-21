//! PDF decoding shared by compiled documents, opened assets, and thumbnails.
//! Callers own artifact identity, scheduling, cancellation, and canonical export bytes.
use crate::{private_workspace::PrivateWorkspace, process::finish_reader_with_timeout};
use quick_xml::{
    Reader, XmlVersion,
    events::{BytesStart, Event},
};
use std::{
    collections::BTreeMap,
    fs,
    io::{BufReader, Read, Seek, SeekFrom},
    path::Path,
    process::{Command, Stdio},
    thread,
    time::Duration,
};

const PIPE_READER_SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(250);
const PDFINFO_TIMEOUT: Duration = Duration::from_secs(60);
const PDFINFO_OUTPUT_LIMIT: u64 = 4 * 1024 * 1024;
/// Resolution used only by the native recovery viewer. The primary Tinymist
/// viewer is vector-based. 144 DPI keeps the fallback crisp at 100% on a 2×
/// display without making every incremental build excessively expensive.
pub const PREVIEW_DPI: f32 = 144.0;
const MAX_PDF_PAGES: usize = 10_000;
#[derive(Debug)]
pub struct PreviewPage {
    pub size: [usize; 2],
    pub rgba: Vec<u8>,
    pub links: Vec<PreviewLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfPageMetadata {
    /// Page dimensions in the fallback preview's 144-DPI layout space.
    pub(crate) size: [usize; 2],
    pub(crate) links: Vec<PreviewLink>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PdfDocumentCatalog {
    pub(crate) pages: Vec<PdfPageMetadata>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreviewLink {
    /// Link rectangle normalized to the page's width and height.
    pub rect: [f32; 4],
    pub target: String,
}

/// Inspect page geometry separately from decoded pixels. This keeps long PDFs
/// cheap to open and gives the viewport enough information to request only the
/// pages it can display. Link extraction remains best-effort.
pub(crate) fn inspect_pdf(
    pdf: &[u8],
    project_root: &Path,
    cancelled: impl FnMut() -> bool,
) -> Result<PdfDocumentCatalog, String> {
    inspect_pdf_with_program(pdf, project_root, Path::new("pdfinfo"), cancelled)
}

pub(crate) fn inspect_pdf_with_program(
    pdf: &[u8],
    project_root: &Path,
    inspector: &Path,
    mut cancelled: impl FnMut() -> bool,
) -> Result<PdfDocumentCatalog, String> {
    let private = PrivateWorkspace::open(project_root).map_err(|error| {
        format!(
            "Could not prepare private PDF-inspection storage in {}: {error}",
            project_root.display()
        )
    })?;
    let inspect_dir = private
        .temp_dir("metadata-")
        .map_err(|error| format!("Could not create a private PDF-inspection directory: {error}"))?;
    let snapshot_path = inspect_dir.path().join("snapshot.pdf");
    fs::write(&snapshot_path, pdf)
        .map_err(|error| format!("Could not stage the PDF metadata snapshot: {error}"))?;
    if cancelled() {
        return Err("PDF inspection was superseded by a newer artifact".to_owned());
    }
    let mut stdout = tempfile::tempfile()
        .map_err(|error| format!("Could not create PDF metadata output storage: {error}"))?;
    let mut stderr = tempfile::tempfile()
        .map_err(|error| format!("Could not create PDF metadata error storage: {error}"))?;
    let child = Command::new(inspector)
        .arg("-f")
        .arg("1")
        .arg("-l")
        .arg(MAX_PDF_PAGES.to_string())
        .arg("-box")
        .arg(&snapshot_path)
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|error| error.to_string())?)
        .stderr(stderr.try_clone().map_err(|error| error.to_string())?)
        .spawn()
        .map_err(pdfinfo_command_error)?;
    let status = crate::process::wait(child, PDFINFO_TIMEOUT, || {
        if cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "PDF inspection superseded",
            ));
        }
        if stdout.metadata()?.len() > PDFINFO_OUTPUT_LIMIT
            || stderr.metadata()?.len() > PDFINFO_OUTPUT_LIMIT
        {
            return Err(std::io::Error::other(
                "PDF metadata output exceeds the 4 MiB limit",
            ));
        }
        Ok(())
    })
    .map_err(|error| match error.kind() {
        std::io::ErrorKind::Interrupted => {
            "PDF inspection was superseded by a newer artifact".to_owned()
        }
        std::io::ErrorKind::TimedOut => "PDF metadata inspection timed out".to_owned(),
        _ => format!("Could not wait for PDF metadata inspection: {error}"),
    })?;
    let read_output = |file: &mut fs::File| -> Result<Vec<u8>, String> {
        file.seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        file.take(PDFINFO_OUTPUT_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        if bytes.len() as u64 > PDFINFO_OUTPUT_LIMIT {
            return Err("PDF metadata output exceeds the 4 MiB limit".to_owned());
        }
        Ok(bytes)
    };
    let output = read_output(&mut stdout)?;
    let errors = read_output(&mut stderr)?;
    if !status.success() {
        return Err(format!(
            "PDF metadata inspection failed: {}",
            String::from_utf8_lossy(&errors).trim()
        ));
    }
    let mut catalog = parse_pdfinfo(&String::from_utf8_lossy(&output))?;
    let mut links = extract_pdf_links(&snapshot_path, inspect_dir.path(), &mut cancelled);
    for (index, page) in catalog.pages.iter_mut().enumerate() {
        page.links = links.get_mut(index).map(std::mem::take).unwrap_or_default();
    }
    Ok(catalog)
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
    FirstPage {
        max_dimension: u32,
    },
    PageRange {
        /// Inclusive, zero-based page range.
        first: usize,
        last: usize,
        dpi: u32,
    },
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
        PdfRasterMode::PageRange { first, last, dpi } => {
            if first > last {
                return Err("The requested PDF page range is empty".to_owned());
            }
            render_command
                .arg("-f")
                .arg(first.saturating_add(1).to_string())
                .arg("-l")
                .arg(last.saturating_add(1).to_string())
                .arg("-r")
                .arg(dpi.max(1).to_string())
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

    let mut pages = Vec::with_capacity(page_paths.len());
    for page_path in page_paths {
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
            links: Vec::new(),
        });
    }

    Ok(pages)
}

fn parse_pdfinfo(output: &str) -> Result<PdfDocumentCatalog, String> {
    let mut page_count = None;
    let mut sizes = BTreeMap::<usize, [f32; 2]>::new();
    let mut rotations = BTreeMap::<usize, i32>::new();
    let mut generic_size = None;
    let mut generic_rotation = 0;
    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Pages:") {
            page_count = value.trim().parse::<usize>().ok();
            continue;
        }
        if let Some((page, value)) = numbered_pdfinfo_value(line, "size:") {
            if let Some(size) = parse_pdf_points(value) {
                sizes.insert(page, size);
            }
            continue;
        }
        if let Some((page, value)) = numbered_pdfinfo_value(line, "rot:") {
            if let Ok(rotation) = value.trim().parse::<i32>() {
                rotations.insert(page, rotation);
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("Page size:") {
            generic_size = parse_pdf_points(value);
        } else if let Some(value) = line.strip_prefix("Page rot:") {
            generic_rotation = value.trim().parse().unwrap_or(0);
        }
    }
    let page_count = page_count.ok_or_else(|| "pdfinfo did not report a page count".to_owned())?;
    if page_count == 0 {
        return Err("The PDF contains no pages".to_owned());
    }
    if page_count > MAX_PDF_PAGES {
        return Err(format!(
            "The PDF contains {page_count} pages; the preview limit is {MAX_PDF_PAGES}"
        ));
    }
    let fallback = generic_size
        .or_else(|| sizes.values().next().copied())
        .ok_or_else(|| "pdfinfo did not report page dimensions for the PDF preview".to_owned())?;
    let pages = (0..page_count)
        .map(|index| {
            let page_number = index + 1;
            let mut points = sizes.get(&page_number).copied().unwrap_or(fallback);
            let rotation = rotations
                .get(&page_number)
                .copied()
                .unwrap_or(generic_rotation)
                .rem_euclid(360);
            if rotation == 90 || rotation == 270 {
                points.swap(0, 1);
            }
            PdfPageMetadata {
                size: points_to_preview_pixels(points),
                links: Vec::new(),
            }
        })
        .collect();
    Ok(PdfDocumentCatalog { pages })
}

fn numbered_pdfinfo_value<'a>(line: &'a str, field: &str) -> Option<(usize, &'a str)> {
    let rest = line.strip_prefix("Page ")?;
    let (number, value) = rest.split_once(field)?;
    Some((number.trim().parse().ok()?, value))
}

fn parse_pdf_points(value: &str) -> Option<[f32; 2]> {
    let mut fields = value.split_whitespace();
    let width = fields.next()?.parse::<f32>().ok()?;
    if fields.next()? != "x" {
        return None;
    }
    let height = fields.next()?.parse::<f32>().ok()?;
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then_some([width, height])
}

fn points_to_preview_pixels([width, height]: [f32; 2]) -> [usize; 2] {
    let scale = PREVIEW_DPI / 72.0;
    [
        (width * scale).round().max(1.0) as usize,
        (height * scale).round().max(1.0) as usize,
    ]
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
                "page" => {
                    let number = xml_attr(&element, "number")
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(pages.len() + 1)
                        .saturating_sub(1);
                    let width = xml_attr(&element, "width")
                        .and_then(|value| value.parse::<f32>().ok())
                        .unwrap_or(0.0);
                    let height = xml_attr(&element, "height")
                        .and_then(|value| value.parse::<f32>().ok())
                        .unwrap_or(0.0);
                    if pages.len() <= number {
                        pages.resize_with(number + 1, Vec::new);
                    }
                    current_page = Some(number);
                    page_size = [width, height];
                }
                "text" => {
                    let left = xml_f32_attr(&element, "left");
                    let top = xml_f32_attr(&element, "top");
                    let width = xml_f32_attr(&element, "width");
                    let height = xml_f32_attr(&element, "height");
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
                "a" => {
                    if let (Some(page), Some(rect), Some(target)) =
                        (current_page, text_rect, xml_attr(&element, "href"))
                    {
                        pages[page].push(PreviewLink {
                            rect,
                            target: normalize_pdftohtml_target(&target, generated_stem),
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::End(element)) => match element.name().as_ref() {
                "text" => text_rect = None,
                "page" => {
                    current_page = None;
                    page_size = [0.0; 2];
                }
                _ => {}
            },
            Ok(Event::Empty(element)) if element.name().as_ref() == "page" => {
                let number = xml_attr(&element, "number")
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

fn xml_attr(element: &BytesStart<'_>, name: &str) -> Option<String> {
    element
        .attributes()
        .flatten()
        .find(|attribute| attribute.key.as_ref() == name)
        .and_then(|attribute| attribute.normalized_value(XmlVersion::Implicit1_0).ok())
        .map(|value| value.into_owned())
}

fn xml_f32_attr(element: &BytesStart<'_>, name: &str) -> Option<f32> {
    xml_attr(element, name)?.parse().ok()
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

fn pdfinfo_command_error(error: std::io::Error) -> String {
    if error.kind() == std::io::ErrorKind::NotFound {
        "Poppler was not found. Install `pdfinfo` (usually provided by the `poppler` package) and make sure it is on PATH."
            .to_owned()
    } else {
        format!("Could not start `pdfinfo`: {error}")
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

    #[test]
    fn pdfinfo_catalog_preserves_all_page_sizes_and_rotation_without_pixels() {
        let catalog = parse_pdfinfo(
            "Pages: 3\nPage    1 size: 612 x 792 pts\nPage    1 rot: 0\nPage    2 size: 300 x 500 pts\nPage    2 rot: 90\nPage    3 size: 400 x 200 pts\nPage    3 rot: 0\n",
        )
        .unwrap();
        assert_eq!(catalog.pages.len(), 3);
        assert_eq!(catalog.pages[0].size, [1224, 1584]);
        assert_eq!(catalog.pages[1].size, [1000, 600]);
        assert_eq!(catalog.pages[2].size, [800, 400]);
        assert!(catalog.pages.iter().all(|page| page.links.is_empty()));
    }

    #[cfg(unix)]
    #[test]
    fn pdfinfo_process_is_cancelled_and_reaped() {
        use std::os::unix::fs::PermissionsExt;

        let project = tempfile::tempdir().unwrap();
        let inspector = project.path().join("slow-pdfinfo");
        fs::write(&inspector, "#!/bin/sh\nexec sleep 30\n").unwrap();
        fs::set_permissions(&inspector, fs::Permissions::from_mode(0o700)).unwrap();
        let mut polls = 0;
        let started = Instant::now();
        let error = inspect_pdf_with_program(
            b"%PDF-cancellation-fixture",
            project.path(),
            &inspector,
            || {
                polls += 1;
                polls > 1
            },
        )
        .unwrap_err();

        assert!(error.contains("superseded"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "cancelled pdfinfo was not reaped promptly"
        );
    }

    #[cfg(unix)]
    #[test]
    fn page_range_rasterization_preserves_snapshot_pixels_and_cancellation() {
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
                "[ \"$1\" = -png ] && [ \"$2\" = -f ] && [ \"$3\" = 2 ] || exit 21\n",
                "[ \"$4\" = -l ] && [ \"$5\" = 3 ] && [ \"$6\" = -r ] && [ \"$7\" = 144 ] || exit 22\n",
                "cmp \"$8\" \"${0%/*}/expected.pdf\" || exit 23\n",
                "cp \"${0%/*}/fixture.png\" \"$9-3.png\"\n",
                "cp \"${0%/*}/fixture.png\" \"$9-2.png\"\n",
            ),
        )
        .unwrap();
        fs::set_permissions(&rasterizer, fs::Permissions::from_mode(0o700)).unwrap();
        let pages = rasterize_pdf_with_program(
            bytes,
            project.path(),
            &rasterizer,
            PdfRasterMode::PageRange {
                first: 1,
                last: 2,
                dpi: 144,
            },
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
            PdfRasterMode::PageRange {
                first: 1,
                last: 2,
                dpi: 144,
            },
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
