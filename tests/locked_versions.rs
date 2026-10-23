#[path = "../build_support/locked_versions.rs"]
mod locked_versions;

#[test]
fn compiled_harper_version_matches_the_actual_resolved_dependency() {
    assert_eq!(
        locked_versions::package_version(include_str!("../Cargo.lock"), "harper-core").unwrap(),
        env!("TIPTOPTYP_HARPER_VERSION")
    );
}

#[test]
fn missing_ambiguous_or_invalid_dependency_metadata_is_rejected() {
    for input in [
        "bad TOML =",
        "",
        "[[package]]\nname = \"other\"\nversion = \"1\"",
        "[[package]]\nname = \"harper-core\"",
        "[[package]]\nname = \"harper-core\"\nversion = \"1\"\n[[package]]\nname = \"harper-core\"\nversion = \"2\"",
    ] {
        assert!(locked_versions::package_version(input, "harper-core").is_err());
    }
}
