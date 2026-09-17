#[path = "../build_support/tool_versions.rs"]
mod tool_versions;

const VALID: &str = "# tool target asset version archive hash\n\
typst target-a asset-a 1.2.3 archive hash\n\
typst target-b asset-b 1.2.3 archive hash\n\
tinymist target-a asset-a 4.5.6 archive hash\n";

#[test]
fn runtime_labels_are_generated_from_the_packaging_manifest() {
    let versions = tool_versions::versions(include_str!("../toolchain/manifest.tsv")).unwrap();
    assert_eq!(versions["typst"], env!("TIPTOPTYP_BUNDLED_TYPST_VERSION"));
    assert_eq!(
        versions["tinymist"],
        env!("TIPTOPTYP_BUNDLED_TINYMIST_VERSION")
    );
}

#[test]
fn all_targets_must_agree_on_the_tool_version() {
    assert_eq!(tool_versions::versions(VALID).unwrap()["typst"], "1.2.3");
    let mismatch = VALID.replacen("asset-b 1.2.3", "asset-b 9.9.9", 1);
    assert!(
        tool_versions::versions(&mismatch)
            .unwrap_err()
            .contains("versions disagree")
    );
}

#[test]
fn invalid_manifest_cannot_generate_runtime_metadata() {
    for (manifest, reason) in [
        ("typst missing fields", "six fields"),
        ("", "missing manifest tool"),
        ("other target asset 1 archive hash", "unknown manifest tool"),
        (
            "typst target asset 1 archive hash",
            "missing manifest tool tinymist",
        ),
    ] {
        assert!(
            tool_versions::versions(manifest)
                .unwrap_err()
                .contains(reason)
        );
    }
    let duplicate = format!("{VALID}typst target-a asset-a 1.2.3 archive hash\n");
    assert!(
        tool_versions::versions(&duplicate)
            .unwrap_err()
            .contains("duplicate manifest target")
    );
}
