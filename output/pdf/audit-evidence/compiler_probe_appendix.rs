
pub fn audit_probe(pdf_path: &Path) {
    let project = tempfile::tempdir().unwrap();
    let bytes = fs::read(pdf_path).unwrap();
    let path = project.path().join("preview.pdf");
    fs::write(&path, &bytes).unwrap();
    let shutdown = AtomicBool::new(false);
    let latest = AtomicU64::new(1);
    let result = render_pdf(&path, project.path(), String::new(), 1, &shutdown, &latest);
    println!("input PDF bytes = {}; result = {}", bytes.len(), match result { Ok(_) => "success".to_owned(), Err(error) => format!("error: {error}") });
}
