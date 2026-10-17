use std::{collections::BTreeMap, path::Path};

fn resources(value: &toml::Value) -> BTreeMap<String, String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["src"].as_str().unwrap().to_owned(),
                entry["target"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn production_and_cargo_packages_include_all_font_licenses_and_the_same_resources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cargo: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let production: toml::Value = toml::from_str(include_str!("../packager-prod.toml")).unwrap();
    let cargo = resources(&cargo["package"]["metadata"]["packager"]["resources"]);
    let production = resources(&production["resources"]);
    for entry in std::fs::read_dir(root.join("assets/fonts")).unwrap() {
        let directory = entry.unwrap().path();
        for name in ["OFL.txt", "LICENSE.md"] {
            let license = directory.join(name);
            if license.is_file() {
                let source = license.strip_prefix(root).unwrap().to_str().unwrap();
                for (name, manifest) in
                    [("Cargo.toml", &cargo), ("packager-prod.toml", &production)]
                {
                    assert!(manifest.contains_key(source), "{name} omits {source}");
                }
            }
        }
    }
    assert_eq!(
        cargo, production,
        "production and Cargo resource lists diverged"
    );
    assert!(
        production
            .values()
            .any(|target| target == "licenses/NotoSans-OFL.txt")
    );
    assert_eq!(
        production
            .get("assets/fonts/hack/LICENSE.md")
            .map(String::as_str),
        Some("licenses/Hack-LICENSE.md"),
    );
    assert!(
        production
            .values()
            .all(|target| target != "licenses/Hack-OFL.txt")
    );
    for source in production
        .keys()
        .filter(|source| source.starts_with("assets/"))
    {
        assert!(root.join(source).exists(), "missing resource: {source}");
    }
}

#[test]
fn macos_production_defaults_to_universal_and_canonical_release_output() {
    let cargo: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let production: toml::Value = toml::from_str(include_str!("../packager-prod.toml")).unwrap();
    let metadata = &cargo["package"]["metadata"]["packager"];
    assert_eq!(
        production["target-triple"].as_str(),
        Some("universal-apple-darwin")
    );
    // Cargo's shared metadata must not force a macOS target on Linux/Windows.
    assert!(metadata.get("target-triple").is_none());
    for manifest in [metadata, &production] {
        assert_eq!(manifest["out-dir"].as_str(), Some("target/release"));
        assert_eq!(manifest["binaries-dir"].as_str(), Some("target/release"));
        assert_eq!(
            manifest["macos"]["minimum-system-version"].as_str(),
            Some("12.0"),
            "the app's minimum must include the bundled PDFium requirement"
        );
    }
}

#[test]
fn packages_ship_compressed_payload_trees_without_duplicate_raw_binaries_or_assets() {
    let cargo: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let production: toml::Value = toml::from_str(include_str!("../packager-prod.toml")).unwrap();
    for manifest in [&cargo["package"]["metadata"]["packager"], &production] {
        assert!(
            manifest.get("external-binaries").is_none(),
            "external sidecars duplicate the compressed architecture-specific payloads"
        );
        let resources = resources(&manifest["resources"]);
        for (source, target) in [
            ("toolchain/payloads", "toolchain"),
            ("toolchain/shared-assets", "assets"),
            ("toolchain/pdfium-notices", "pdfium"),
        ] {
            assert_eq!(resources.get(source).map(String::as_str), Some(target));
        }
        for source in resources.keys() {
            assert!(
                !source.starts_with("toolchain/bin")
                    && !source.starts_with("toolchain/pdfium-bundle")
                    && !source.starts_with("toolchain/pdfium-targets"),
                "uncompressed native runtime duplicated in package resources: {source}"
            );
            assert!(
                !["ttf", "otf", "onnx", "bin", "dylib", "dll", "so", "exe"]
                    .iter()
                    .any(|extension| Path::new(source)
                        .extension()
                        .is_some_and(|value| value == *extension)),
                "shared assets and native runtimes belong in the compressed trees: {source}"
            );
        }
    }
}
