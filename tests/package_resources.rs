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
        let license = entry.unwrap().path().join("OFL.txt");
        if license.is_file() {
            let source = license.strip_prefix(root).unwrap().to_str().unwrap();
            for (name, manifest) in [("Cargo.toml", &cargo), ("packager-prod.toml", &production)] {
                assert!(manifest.contains_key(source), "{name} omits {source}");
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
    for source in production
        .keys()
        .filter(|source| source.starts_with("assets/"))
    {
        assert!(root.join(source).exists(), "missing resource: {source}");
    }
}
