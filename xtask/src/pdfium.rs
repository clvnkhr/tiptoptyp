//! Pinned native renderer; no runtime downloads or system-library fallback.
use super::*;

const VERSION: &str = "7881";
const TARGETS: &[(&str, &str, &str)] = &[
    (
        "aarch64-apple-darwin",
        "mac-arm64",
        "52e94ca5aa8847934330daf3f8150c190682c5ca93831468794f8b90d4392e40",
    ),
    (
        "x86_64-apple-darwin",
        "mac-x64",
        "6dedf83990e0e3d6b7c93c9e7589c5a126b0ae14b7464d76120cff7a26afb18b",
    ),
    (
        "x86_64-unknown-linux-gnu",
        "linux-x64",
        "1470e21b8b4a3b4ad7f85684e2da11d94f3b69a86d81dee11b9b6709d927ac1d",
    ),
    (
        "aarch64-unknown-linux-gnu",
        "linux-arm64",
        "ee7f7b7d5468958336a818c1cd580bdd20972846b7377b13f9a923d92d1d4674",
    ),
    (
        "x86_64-pc-windows-msvc",
        "win-x64",
        "73cc0de638ac2095e7445bf56a38200a5b7c7ca0e9f4ba144598f2457377ac08",
    ),
    (
        "aarch64-pc-windows-msvc",
        "win-arm64",
        "d3035d4d2cacac6ecd1a2ece197a3d702a1b2a58466276b9f870b8cb278a9d84",
    ),
];

pub(super) fn fetch(target: &str) -> Result<(), String> {
    let (_, platform, hash) = TARGETS
        .iter()
        .find(|row| row.0 == target)
        .ok_or_else(|| format!("PDFium has no pinned binary for {target}"))?;
    let root = repository_root();
    let cache = root.join("toolchain/cache");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let archive = cache.join(format!("pdfium-{platform}-{VERSION}.tgz"));
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium/{VERSION}/pdfium-{platform}.tgz"
    );
    if !archive.exists() || sha256(&archive)? != *hash {
        download(&url, &archive)?;
    }
    verify_hash(&archive, hash)?;
    let listing = run_output(
        Command::new("tar").arg("-tf").arg(&archive),
        "listing PDFium",
    )?;
    let listing = String::from_utf8(listing.stdout).map_err(|e| e.to_string())?;
    validate_archive_listing(&listing)?;
    let types = run_output(
        Command::new("tar").arg("-tvf").arg(&archive),
        "checking PDFium archive",
    )?;
    validate_archive_entry_types(&String::from_utf8_lossy(&types.stdout))?;
    let lib = if target.contains("apple") {
        "libpdfium.dylib"
    } else if target.contains("windows") {
        "pdfium.dll"
    } else {
        "libpdfium.so"
    };
    let member = find_unique_member(&listing, OsStr::new(lib))?;
    let bundle = root.join("toolchain/pdfium-bundle");
    // This directory contains generated artifacts only, for one package target.
    if bundle.exists() {
        fs::remove_dir_all(&bundle).map_err(|e| e.to_string())?;
    }
    fs::create_dir_all(bundle.join("licenses")).map_err(|e| e.to_string())?;
    extract_member_to_file(&archive, &member, &bundle.join(lib), false)?;
    for name in listing.lines().filter(|name| {
        *name == "LICENSE" || (name.starts_with("licenses/") && !name.ends_with('/'))
    }) {
        extract_member_to_file(&archive, name, &bundle.join(name), false)?;
    }
    fs::write(bundle.join("PROVENANCE.txt"), format!("PDFium chromium/{VERSION}\nTarget: {target}\nSource: {url}\nArchive SHA-256: {hash}\nBinding: pdfium-render 0.9.4, pdfium_7881\nNative and dependency licenses accompany this library.\n"))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pins_cover_supported_desktop_architectures() {
        assert_eq!(TARGETS.len(), 6);
        for (target, _, hash) in TARGETS {
            assert_eq!(TARGETS.iter().filter(|r| r.0 == *target).count(), 1);
            assert_eq!(hash.len(), 64);
            assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        }
    }
}
