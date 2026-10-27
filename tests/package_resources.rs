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
    assert_eq!(
        production.get("LICENSE").map(String::as_str),
        Some("LICENSE"),
        "packaged apps must include the first-party license"
    );
    assert!(root.join("LICENSE").is_file());
    assert!(
        production
            .values()
            .any(|target| target == "licenses/NotoSans-OFL.txt")
    );
    assert_eq!(
        production
            .get("assets/liquid/SACREMOSES-LICENSE")
            .map(String::as_str),
        Some("licenses/Sacremoses-MIT.txt"),
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
fn first_party_packages_resolve_to_the_project_license() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let license = root.join("LICENSE").canonicalize().unwrap();
    for manifest in [
        "Cargo.toml",
        "core/Cargo.toml",
        "xtask/Cargo.toml",
        "scripts/liquid-native-probe/Cargo.toml",
    ] {
        let path = root.join(manifest);
        let value: toml::Value = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let relative = value["package"]["license-file"]
            .as_str()
            .unwrap_or_else(|| panic!("{manifest} omits its license"));
        assert_eq!(
            path.parent()
                .unwrap()
                .join(relative)
                .canonicalize()
                .unwrap(),
            license,
            "{manifest} must use the first-party license"
        );
        assert!(value["package"].get("license").is_none());
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
            ("toolchain/ltex", "ltex"),
            ("toolchain/pdfium-notices", "pdfium"),
        ] {
            assert_eq!(resources.get(source).map(String::as_str), Some(target));
        }
        assert!(
            !resources.contains_key("toolchain/liquid")
                && !resources.values().any(|target| target == "liquid"),
            "Liquid's model and native runtimes must be installed on demand"
        );
        for source in resources.keys() {
            assert!(
                !source.starts_with("toolchain/bin")
                    && !source.starts_with("toolchain/pdfium-targets"),
                "uncompressed native runtime duplicated in package resources: {source}"
            );
            assert!(
                ![
                    "ttf", "otf", "onnx", "pt", "bin", "dylib", "dll", "so", "exe"
                ]
                .iter()
                .any(|extension| Path::new(source)
                    .extension()
                    .is_some_and(|value| value == *extension)),
                "shared assets and native runtimes belong in the compressed trees: {source}"
            );
        }
    }
}

#[test]
fn liquid_measurements_use_the_production_engine_and_optimized_profile() {
    let app: toml::Value = toml::from_str(include_str!("../Cargo.toml")).unwrap();
    let probe: toml::Value =
        toml::from_str(include_str!("../scripts/liquid-native-probe/Cargo.toml")).unwrap();
    let app_lock: toml::Value = toml::from_str(include_str!("../Cargo.lock")).unwrap();
    let probe_lock: toml::Value =
        toml::from_str(include_str!("../scripts/liquid-native-probe/Cargo.lock")).unwrap();
    for name in ["tokenizers", "nix", "sha2"] {
        let version = |lock: &toml::Value| {
            lock["package"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|package| package["name"].as_str() == Some(name))
                .map(|package| package["version"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            version(&app_lock).len(),
            1,
            "missing production runtime: {name}"
        );
        assert_eq!(
            version(&app_lock),
            version(&probe_lock),
            "measurement runtime drifted: {name}"
        );
        let dependency = |manifest: &toml::Value| {
            manifest["dependencies"]
                .get(name)
                .or_else(|| {
                    manifest
                        .get("target")
                        .and_then(|target| target.get("cfg(unix)"))
                        .and_then(|target| target.get("dependencies"))
                        .and_then(|dependencies| dependencies.get(name))
                })
                .cloned()
                .expect("production runtime dependency")
        };
        assert_eq!(
            dependency(&app),
            dependency(&probe),
            "measurement dependency options drifted: {name}"
        );
    }
    let task: toml::Value = toml::from_str(include_str!("../xtask/Cargo.toml")).unwrap();
    assert_eq!(
        app["dependencies"]["sha2"], task["dependencies"]["sha2"],
        "package verification must use the same hardware hashing implementation"
    );
    assert!(
        app["dependencies"]["sha2"]["features"]
            .as_array()
            .unwrap()
            .iter()
            .any(|feature| feature.as_str() == Some("asm"))
    );
    for manifest in [&app, &probe, &task] {
        assert_eq!(
            manifest["profile"]["dev"]["package"]["sha2"]["opt-level"].as_integer(),
            Some(3),
            "model verification must remain usable during cargo run"
        );
    }
    for setting in ["lto", "codegen-units", "strip"] {
        assert_eq!(
            app["profile"]["release"][setting], probe["profile"]["release"][setting],
            "unmatched optimized profile: {setting}"
        );
    }
}
