use harper_core::weirpack::{Weirpack, WeirpackManifest};

#[test]
fn archive_round_trip_preserves_unicode_rules_and_dictionary_metadata() {
    let mut manifest = WeirpackManifest::new();
    manifest.set_author("Test Author");
    manifest.set_version("0.1.0");
    manifest.set_description("Test pack");
    manifest.set_license("MIT");

    let mut pack = Weirpack::new(manifest);
    pack.add_rule("ExampleRule", "expr main test");
    pack.add_rule("ΛRule", "expr main unicode");
    pack.dictionary = Some("example\n".into());
    pack.annotations = Some("{}".into());

    let bytes = pack.to_bytes().expect("serialize weirpack");
    let parsed = Weirpack::from_bytes(&bytes).expect("deserialize weirpack");

    assert_eq!(parsed.manifest.author().unwrap(), "Test Author");
    assert_eq!(parsed.manifest.version().unwrap(), "0.1.0");
    assert_eq!(parsed.manifest.description().unwrap(), "Test pack");
    assert_eq!(parsed.manifest.license().unwrap(), "MIT");
    assert_eq!(parsed.rules.get("ExampleRule").unwrap(), "expr main test");
    assert_eq!(parsed.rules.get("ΛRule").unwrap(), "expr main unicode");
    assert_eq!(parsed.dictionary.as_deref(), Some("example\n"));
    assert_eq!(parsed.annotations.as_deref(), Some("{}"));
}

#[test]
fn archive_decodes_legacy_cp437_names_without_lossy_replacement() {
    let mut manifest = WeirpackManifest::new();
    manifest.set_author("Test Author");
    manifest.set_version("0.1.0");
    manifest.set_description("Legacy filename fixture");
    manifest.set_license("MIT");
    let mut pack = Weirpack::new(manifest);
    pack.add_rule("ΛRule", "expr main unicode");
    let mut bytes = pack.to_bytes().expect("serialize weirpack");
    let name = "ΛRule.weir".as_bytes();
    let offsets = bytes
        .windows(name.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == name).then_some(offset))
        .collect::<Vec<_>>();
    // ZIP falls back to CP437 when names are not valid UTF-8. Keep the local
    // and central names identical, with unchanged sizes and file contents.
    assert_eq!(offsets.len(), 2);
    for offset in offsets {
        bytes[offset] = 0x82; // é in CP437
        bytes[offset + 1] = b'A';
    }
    let parsed = Weirpack::from_bytes(&bytes).expect("decode legacy filename");
    assert_eq!(parsed.rules.len(), 1);
    assert_eq!(parsed.rules.get("éARule").unwrap(), "expr main unicode");
}
