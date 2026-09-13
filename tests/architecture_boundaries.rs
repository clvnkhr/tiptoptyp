//! These are dependency rules, not behavioral substitutes for adapter tests.
use std::{
    fs,
    path::{Path, PathBuf},
};

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(rust_files(&path));
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
    files
}

#[test]
fn core_has_no_platform_dependencies_or_effect_apis() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: toml::Value = fs::read_to_string(root.join("core/Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let dependencies = manifest["dependencies"].as_table().unwrap();
    assert_eq!(
        dependencies.keys().map(String::as_str).collect::<Vec<_>>(),
        ["serde"]
    );
    for path in rust_files(&root.join("core/src")) {
        let source = fs::read_to_string(&path).unwrap();
        // Tokenize identifiers/punctuation so whitespace cannot bypass the rule.
        // Deliberately conservative: effects named in comments also merit review.
        let compact: String = source.chars().filter(|c| !c.is_whitespace()).collect();
        for forbidden in [
            "std::fs",
            "std::process",
            "std::thread",
            "std::env",
            "Instant::now(",
            "SystemTime::now(",
            "Command::new(",
            "include_bytes!(",
        ] {
            assert!(
                !compact.contains(forbidden),
                "{} uses forbidden core effect {forbidden}",
                path.display()
            );
        }
        // Grouped `use std::{fs, ...}` imports must not evade qualified-path checks.
        for token in source.split(|c: char| !c.is_alphanumeric() && c != '_') {
            assert!(
                !["fs", "process", "thread", "env", "eframe", "wry", "rfd"].contains(&token),
                "{} contains platform identifier {token}",
                path.display()
            );
        }
    }
}

#[test]
fn viewport_creation_and_font_installation_stay_in_rendering_adapters() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for path in rust_files(&root) {
        let relative = path.strip_prefix(&root).unwrap().to_str().unwrap();
        let compact: String = fs::read_to_string(&path)
            .unwrap()
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        if compact.contains(".show_viewport_immediate(") {
            assert_eq!(
                relative, "viewport_fonts.rs",
                "Immediate viewports must synchronize the shared font atlas"
            );
        }
        if compact.contains(".set_fonts(") {
            assert!(
                ["theme.rs", "viewport_fonts.rs", "font_preview.rs"].contains(&relative),
                "{relative} bypasses context font ownership"
            );
        }
    }
}
