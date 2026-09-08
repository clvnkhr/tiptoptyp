
pub fn audit_probe() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("source");
    let copied = fixture.path().join("copied");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("changed.typ"), "old").unwrap();
    fs::write(source.join("deleted.typ"), "old").unwrap();
    copy_directory(&source, &copied).unwrap();
    fs::write(source.join("changed.typ"), "new").unwrap();
    fs::remove_file(source.join("deleted.typ")).unwrap();
    fs::write(source.join("added.typ"), "new").unwrap();
    mirror_entry(&source, &copied, fs::metadata(&source).unwrap().file_type()).unwrap();
    println!("copied changed.typ = {:?}; deleted.typ still exists = {}; added.typ exists = {}", fs::read_to_string(copied.join("changed.typ")).unwrap(), copied.join("deleted.typ").exists(), copied.join("added.typ").exists());
}

pub fn race_probe() {
    let mut failures = 0;
    for _ in 0..100 {
        let project = tempfile::tempdir().unwrap();
        let project_path = project.path().to_path_buf();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let handles: Vec<_> = (0..8).map(|_| {
            let root = project_path.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || { barrier.wait(); PrivateWorkspace::open(root).map(|_| ()) })
        }).collect();
        for handle in handles {
            if let Err(e) = handle.join().unwrap() {
                failures += 1;
                if failures == 1 { println!("first concurrent open failure = {e}"); }
            }
        }
    }
    println!("concurrent open failures = {failures}/800");
}
