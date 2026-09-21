#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod profile;

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

const MANIFEST: &str = include_str!("../../toolchain/manifest.tsv");
const TINYMIST_LICENSE_SHA256: &str =
    "a9f29769fd3a7ee2976e6e161a93e16461fa305c088c4806242e50ec8ef86bce";
const TYPST_LICENSE_SHA256: &str =
    "62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a";
const TYPST_NOTICE_SHA256: &str =
    "1778244777547c281b6f5fa9fc0c18ab21f8d4491c803f64e09046800f5fcb26";
const PACKAGE_TARGET_ENV: &str = "TIPTOPTYP_PACKAGE_TARGET";
const MACOS_APP_ICON: &str = "tiptoptyp.icns";
const MACOS_APP_BUNDLE: &str = "tiptoptyp Dev.app";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Artifact {
    tool: String,
    app_target: String,
    asset_target: String,
    version: String,
    archive: String,
    sha256: String,
}

impl Artifact {
    fn url(&self) -> String {
        match self.tool.as_str() {
            "typst" => format!(
                "https://github.com/typst/typst/releases/download/v{}/{}",
                self.version, self.archive
            ),
            "tinymist" => format!(
                "https://github.com/Myriad-Dreamin/tinymist/releases/download/v{}/{}",
                self.version, self.archive
            ),
            _ => unreachable!("manifest parser rejects unknown tools"),
        }
    }

    fn staged_name(&self) -> String {
        if self.app_target.contains("windows") {
            format!("{}-{}.exe", self.tool, self.app_target)
        } else {
            format!("{}-{}", self.tool, self.app_target)
        }
    }

    fn extracted_name(&self) -> String {
        if self.asset_target.contains("windows") {
            format!("{}.exe", self.tool)
        } else {
            self.tool.clone()
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("xtask: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut arguments = env::args().skip(1);
    let command = arguments.next().unwrap_or_else(|| "help".to_owned());
    if command == "profile" {
        return profile::run(arguments.collect());
    }
    let mut target = None;
    while let Some(argument) = arguments.next() {
        if argument == "--target" {
            target = Some(
                arguments
                    .next()
                    .ok_or_else(|| "--target requires a target triple".to_owned())?,
            );
        } else {
            return Err(format!("unknown argument: {argument}"));
        }
    }

    match command.as_str() {
        "fetch-sidecars" => {
            let target = target_or_host(target)?;
            fetch_sidecars(&target)
        }
        "package-build" => {
            let target = target_or_host(target)?;
            fetch_sidecars(&target)?;
            generate_third_party_notices()?;
            let mut cargo = Command::new("cargo");
            cargo.arg("build").arg("--release");
            if target != host_target()? {
                cargo.arg("--target").arg(&target);
            }
            run_status(&mut cargo, "release build")
        }
        "verify-package" => {
            let target = target_or_host(target)?;
            verify_package(&target)
        }
        "generate-notices" => generate_third_party_notices(),
        "help" | "--help" | "-h" => {
            println!(
                "tiptoptyp tasks\n\n  profile --help\n  fetch-sidecars [--target TRIPLE]\n  package-build [--target TRIPLE]\n  verify-package [--target TRIPLE]\n  generate-notices\n\n{PACKAGE_TARGET_ENV} supplies the target to cargo-packager's hook."
            );
            Ok(())
        }
        _ => Err(format!("unknown xtask: {command}")),
    }
}

fn generate_third_party_notices() -> Result<(), String> {
    let root = repository_root();
    let generated = root.join("target/third-party-cargo-about.txt");
    if let Some(parent) = generated.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("could not create {}: {error}", parent.display()))?;
    }
    let output = run_output(
        Command::new("cargo")
            .arg("about")
            .arg("generate")
            .arg("--manifest-path")
            .arg(root.join("Cargo.toml"))
            .arg(root.join("about.hbs")),
        "generating Rust dependency notices with cargo-about",
    )?;
    fs::write(&generated, output.stdout)
        .map_err(|error| format!("could not write {}: {error}", generated.display()))?;
    let themes = fs::read_to_string(root.join("docs/theme-sources.md"))
        .map_err(|error| format!("could not read bundled theme notices: {error}"))?;
    let cargo_notices = fs::read_to_string(&generated)
        .map_err(|error| format!("could not read generated dependency notices: {error}"))?;
    let mut bundled_notices = String::new();
    for path in [
        root.join("toolchain/licenses/tinymist-LICENSE"),
        root.join("toolchain/licenses/typst-LICENSE"),
        root.join("toolchain/licenses/typst-NOTICE"),
        root.join("assets/fonts/notosanssymbols/OFL.txt"),
        root.join("assets/fonts/notosansmath/OFL.txt"),
        root.join("assets/fonts/notosanssymbols2/OFL.txt"),
        root.join("assets/fonts/notosanshebrew/OFL.txt"),
    ] {
        let name = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("could not read bundled notice {name}: {error}"))?;
        bundled_notices.push_str(&format!("\n===== {name} =====\n\n{text}\n"));
    }
    let output = format!(
        "THIRD-PARTY NOTICES\nThis file contains notices for third-party code and assets distributed by tiptoptyp.\nThe Rust dependency section is generated by cargo-about during packaging.\n\n===== BUNDLED THEME NOTICES =====\n{themes}\n{bundled_notices}\n===== RUST DEPENDENCY NOTICES =====\n{cargo_notices}"
    );
    fs::write(root.join("THIRD_PARTY_NOTICES"), output)
        .map_err(|error| format!("could not write THIRD_PARTY_NOTICES: {error}"))?;
    println!("generated {}", root.join("THIRD_PARTY_NOTICES").display());
    Ok(())
}

fn verify_package(target: &str) -> Result<(), String> {
    if !target.ends_with("apple-darwin") {
        return Err("verify-package currently supports macOS app bundles".to_owned());
    }
    let root = repository_root();
    let app = if target == host_target()? {
        root.join("target/release").join(MACOS_APP_BUNDLE)
    } else {
        root.join("target")
            .join(target)
            .join("release")
            .join(MACOS_APP_BUNDLE)
    };
    let executable_dir = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    require_file(&executable_dir.join("tiptoptyp"))?;
    require_file(&resources.join(MACOS_APP_ICON))?;
    verify_bundle_icon_reference(&app.join("Contents/Info.plist"), MACOS_APP_ICON)?;

    let artifacts = parse_manifest(MANIFEST)?;
    for artifact in artifacts
        .iter()
        .filter(|artifact| artifact.app_target == target)
    {
        let executable = executable_dir.join(&artifact.tool);
        require_file(&executable)?;
        let output = run_output(
            Command::new(&executable).arg("--version"),
            &format!("checking packaged {}", artifact.tool),
        )?;
        let reported = String::from_utf8_lossy(&output.stdout);
        if !reported.contains(&artifact.version) {
            return Err(format!(
                "packaged {} reported {reported:?}, expected {}",
                artifact.tool, artifact.version
            ));
        }
        require_file(
            &resources
                .join("toolchain-provenance")
                .join(format!("{}-{target}.txt", artifact.tool)),
        )?;
    }

    verify_hash(
        &resources.join("licenses/tinymist-LICENSE"),
        TINYMIST_LICENSE_SHA256,
    )?;
    verify_hash(
        &resources.join("licenses/typst-LICENSE"),
        TYPST_LICENSE_SHA256,
    )?;
    verify_hash(
        &resources.join("licenses/typst-NOTICE"),
        TYPST_NOTICE_SHA256,
    )?;
    for (directory, name) in [
        ("notosanssymbols", "NotoSansSymbols"),
        ("notosansmath", "NotoSansMath"),
        ("notosanssymbols2", "NotoSansSymbols2"),
        ("notosanshebrew", "NotoSansHebrew"),
    ] {
        verify_hash(
            &resources.join(format!("licenses/{name}-OFL.txt")),
            &sha256(&root.join("assets/fonts").join(directory).join("OFL.txt"))?,
        )?;
    }
    verify_hash(
        &resources.join("font-provenance.md"),
        &sha256(&root.join("assets/fonts/README.md"))?,
    )?;

    #[cfg(target_os = "macos")]
    run_status(
        Command::new("codesign")
            .arg("--verify")
            .arg("--deep")
            .arg("--strict")
            .arg("--verbose=2")
            .arg(&app),
        "verifying the packaged app signature",
    )?;

    println!("verified {}", app.display());
    Ok(())
}

fn require_file(path: &Path) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("package is missing {}", path.display()))
    }
}

fn verify_bundle_icon_reference(info_plist: &Path, expected: &str) -> Result<(), String> {
    let contents = fs::read_to_string(info_plist)
        .map_err(|error| format!("could not read {}: {error}", info_plist.display()))?;
    if bundle_icon_reference(&contents, expected) {
        Ok(())
    } else if !contents.contains("<key>CFBundleIconFile</key>") {
        Err(format!(
            "{} does not declare CFBundleIconFile",
            info_plist.display()
        ))
    } else {
        Err(format!(
            "{} does not reference {expected} as CFBundleIconFile",
            info_plist.display()
        ))
    }
}

fn bundle_icon_reference(info_plist: &str, expected: &str) -> bool {
    let Some(after_key) = info_plist
        .split_once("<key>CFBundleIconFile</key>")
        .map(|(_, tail)| tail)
    else {
        return false;
    };
    let value_scope = after_key.split("<key>").next().unwrap_or(after_key);
    let expected_value = format!("<string>{expected}</string>");
    value_scope.contains(&expected_value)
}

fn verify_hash(path: &Path, expected: &str) -> Result<(), String> {
    require_file(path)?;
    let actual = sha256(path)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 mismatch for {}: expected {expected}, got {actual}",
            path.display()
        ))
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask is inside the repository")
        .to_path_buf()
}

fn host_target() -> Result<String, String> {
    let output = run_output(
        Command::new("rustc").arg("-vV"),
        "querying the Rust host target",
    )?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV did not report a host target".to_owned())
}

fn target_or_host(explicit: Option<String>) -> Result<String, String> {
    selected_package_target(explicit, env::var_os(PACKAGE_TARGET_ENV))
        .map(Ok)
        .unwrap_or_else(host_target)
}

fn selected_package_target(
    explicit: Option<String>,
    environment: Option<OsString>,
) -> Option<String> {
    explicit.or_else(|| environment.map(|target| target.to_string_lossy().into_owned()))
}

fn fetch_sidecars(target: &str) -> Result<(), String> {
    let artifacts = parse_manifest(MANIFEST)?
        .into_iter()
        .filter(|artifact| artifact.app_target == target)
        .collect::<Vec<_>>();
    if artifacts.len() != 2
        || !artifacts.iter().any(|artifact| artifact.tool == "typst")
        || !artifacts.iter().any(|artifact| artifact.tool == "tinymist")
    {
        return Err(format!(
            "the pinned toolchain does not support {target}; see toolchain/manifest.tsv"
        ));
    }

    let root = repository_root();
    let cache = root.join("toolchain/cache");
    let binaries = root.join("toolchain/bin");
    let licenses = root.join("toolchain/licenses");
    let provenance = root.join("toolchain/provenance");
    for directory in [&cache, &binaries, &licenses, &provenance] {
        fs::create_dir_all(directory)
            .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    }

    let native = target == host_target()?;
    for artifact in &artifacts {
        fetch_artifact(artifact, &cache, &binaries, &licenses, &provenance, native)?;
    }
    fetch_tinymist_license(&licenses)?;
    Ok(())
}

fn fetch_artifact(
    artifact: &Artifact,
    cache: &Path,
    binaries: &Path,
    licenses: &Path,
    provenance: &Path,
    verify_version: bool,
) -> Result<(), String> {
    let archive = cache.join(&artifact.archive);
    if !archive.is_file() || sha256(&archive)? != artifact.sha256 {
        download(&artifact.url(), &archive)?;
    }
    let actual_hash = sha256(&archive)?;
    if actual_hash != artifact.sha256 {
        return Err(format!(
            "SHA-256 mismatch for {}: expected {}, got {}",
            artifact.archive, artifact.sha256, actual_hash
        ));
    }

    let listing = run_output(
        Command::new("tar").arg("-tf").arg(&archive),
        &format!("listing {}", artifact.archive),
    )?;
    let listing = String::from_utf8(listing.stdout)
        .map_err(|_| format!("{} contains non-UTF-8 member names", artifact.archive))?;
    validate_archive_listing(&listing)?;
    let verbose_listing = run_output(
        Command::new("tar").arg("-tvf").arg(&archive),
        &format!("inspecting entry types in {}", artifact.archive),
    )?;
    let verbose_listing = String::from_utf8(verbose_listing.stdout)
        .map_err(|_| format!("{} has non-UTF-8 entry metadata", artifact.archive))?;
    validate_archive_entry_types(&verbose_listing)?;

    let expected_name = artifact.extracted_name();
    let executable_member = find_unique_member(&listing, OsStr::new(&expected_name))?;
    let staged = binaries.join(artifact.staged_name());
    extract_member_to_file(&archive, &executable_member, &staged, true)?;

    if artifact.tool == "typst" {
        let license = licenses.join("typst-LICENSE");
        let license_member = find_unique_member(&listing, OsStr::new("LICENSE"))?;
        extract_member_to_file(&archive, &license_member, &license, false)?;
        verify_hash(&license, TYPST_LICENSE_SHA256)?;

        let notice = licenses.join("typst-NOTICE");
        let notice_member = find_unique_member(&listing, OsStr::new("NOTICE"))?;
        extract_member_to_file(&archive, &notice_member, &notice, false)?;
        verify_hash(&notice, TYPST_NOTICE_SHA256)?;
    }

    let version_output = if verify_version {
        let output = run_output(
            Command::new(&staged).arg("--version"),
            &format!("checking the staged {} version", artifact.tool),
        )?;
        let output = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !output.contains(&artifact.version) {
            return Err(format!(
                "{} reported {output:?}, expected version {}",
                staged.display(),
                artifact.version
            ));
        }
        output
    } else {
        "not executed (cross-target artifact)".to_owned()
    };

    let record = provenance.join(format!("{}-{}.txt", artifact.tool, artifact.app_target));
    fs::write(
        &record,
        format!(
            "tool={}\nversion={}\ntarget={}\nasset_target={}\nurl={}\nsha256={}\nversion_output={}\n",
            artifact.tool,
            artifact.version,
            artifact.app_target,
            artifact.asset_target,
            artifact.url(),
            artifact.sha256,
            version_output
        ),
    )
    .map_err(|error| format!("could not write {}: {error}", record.display()))?;

    println!(
        "staged {} {} for {}",
        artifact.tool, artifact.version, artifact.app_target
    );
    Ok(())
}

fn parse_manifest(input: &str) -> Result<Vec<Artifact>, String> {
    let mut artifacts = Vec::new();
    for (index, line) in input.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if fields.len() != 6 {
            return Err(format!(
                "toolchain manifest line {} has {} fields, expected 6",
                index + 1,
                fields.len()
            ));
        }
        if !matches!(fields[0], "typst" | "tinymist") {
            return Err(format!("unknown tool {} on line {}", fields[0], index + 1));
        }
        if fields[5].len() != 64 || !fields[5].bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!("invalid SHA-256 on line {}", index + 1));
        }
        artifacts.push(Artifact {
            tool: fields[0].to_owned(),
            app_target: fields[1].to_owned(),
            asset_target: fields[2].to_owned(),
            version: fields[3].to_owned(),
            archive: fields[4].to_owned(),
            sha256: fields[5].to_ascii_lowercase(),
        });
    }
    Ok(artifacts)
}

fn validate_archive_listing(listing: &str) -> Result<(), String> {
    for member in listing.lines().filter(|line| !line.trim().is_empty()) {
        let path = Path::new(member);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(format!("unsafe archive member: {member}"));
        }
    }
    Ok(())
}

fn validate_archive_entry_types(listing: &str) -> Result<(), String> {
    for entry in listing.lines().filter(|line| !line.trim().is_empty()) {
        match entry.as_bytes().first() {
            Some(b'-' | b'd') => {}
            Some(kind) => {
                return Err(format!(
                    "unsafe archive entry type {:?}: {entry}",
                    char::from(*kind)
                ));
            }
            None => unreachable!("empty lines were filtered"),
        }
    }
    Ok(())
}

fn download(url: &str, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension("download");
    println!("downloading {url}");
    run_status(
        Command::new("curl")
            .arg("--proto")
            .arg("=https")
            .arg("--tlsv1.2")
            .arg("--fail")
            .arg("--location")
            .arg("--silent")
            .arg("--show-error")
            .arg("--output")
            .arg(&temporary)
            .arg(url),
        "downloading a pinned release artifact",
    )?;
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("could not replace {}: {error}", destination.display()))?;
    }
    fs::rename(&temporary, destination).map_err(|error| {
        format!(
            "could not move {} to {}: {error}",
            temporary.display(),
            destination.display()
        )
    })
}

fn fetch_tinymist_license(licenses: &Path) -> Result<(), String> {
    let destination = licenses.join("tinymist-LICENSE");
    if !destination.is_file() || sha256(&destination)? != TINYMIST_LICENSE_SHA256 {
        download(
            "https://raw.githubusercontent.com/Myriad-Dreamin/tinymist/v0.15.2/LICENSE",
            &destination,
        )?;
    }
    let actual = sha256(&destination)?;
    if actual == TINYMIST_LICENSE_SHA256 {
        Ok(())
    } else {
        Err(format!(
            "SHA-256 mismatch for Tinymist LICENSE: expected {TINYMIST_LICENSE_SHA256}, got {actual}"
        ))
    }
}

fn sha256(path: &Path) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    let output = run_output(
        Command::new("shasum").arg("-a").arg("256").arg(path),
        "calculating SHA-256",
    )?;
    #[cfg(all(unix, not(target_os = "macos")))]
    let output = run_output(Command::new("sha256sum").arg(path), "calculating SHA-256")?;
    #[cfg(windows)]
    let output = run_output(
        Command::new("certutil")
            .arg("-hashfile")
            .arg(path)
            .arg("SHA256"),
        "calculating SHA-256",
    )?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .split_whitespace()
        .map(|field| field.replace(' ', ""))
        .find(|field| field.len() == 64 && field.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|hash| hash.to_ascii_lowercase())
        .ok_or_else(|| format!("could not parse SHA-256 output for {}", path.display()))
}

fn find_unique_member(listing: &str, name: &OsStr) -> Result<String, String> {
    let matches = listing
        .lines()
        .filter(|member| !member.ends_with('/'))
        .filter(|member| Path::new(member).file_name() == Some(name))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err(format!(
            "archive did not contain the expected file {}",
            name.to_string_lossy()
        )),
        _ => Err(format!(
            "archive contained multiple files named {}",
            name.to_string_lossy()
        )),
    }
}

/// Stream one exact allowlisted archive member to an owned file. `tar` never
/// gets an extraction directory, so links and special entries cannot write
/// elsewhere in the filesystem even if a future archive contains them.
fn extract_member_to_file(
    archive: &Path,
    member: &str,
    destination: &Path,
    executable: bool,
) -> Result<(), String> {
    let file_name = destination
        .file_name()
        .unwrap_or_else(|| OsStr::new("artifact"))
        .to_string_lossy();
    let temporary =
        destination.with_file_name(format!(".{file_name}.staging-{}", std::process::id()));
    let file = fs::File::create(&temporary)
        .map_err(|error| format!("could not create {}: {error}", temporary.display()))?;
    let output = Command::new("tar")
        .arg("-xOf")
        .arg(archive)
        .arg("--")
        .arg(member)
        .stdout(Stdio::from(file))
        .output()
        .map_err(|error| format!("could not extract {member}: {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "extracting {member} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let metadata = fs::metadata(&temporary)
        .map_err(|error| format!("could not inspect {}: {error}", temporary.display()))?;
    if metadata.len() == 0 {
        let _ = fs::remove_file(&temporary);
        return Err(format!("archive member {member} was empty"));
    }
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&temporary, permissions)
            .map_err(|error| format!("could not chmod {}: {error}", temporary.display()))?;
    }
    if destination.exists() {
        fs::remove_file(destination)
            .map_err(|error| format!("could not replace {}: {error}", destination.display()))?;
    }
    fs::rename(&temporary, destination).map_err(|error| {
        format!(
            "could not move {} to {}: {error}",
            temporary.display(),
            destination.display()
        )
    })
}

fn run_status(command: &mut Command, purpose: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("could not start {purpose}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{purpose} failed with {status}"))
    }
}

fn run_output(command: &mut Command, purpose: &str) -> Result<Output, String> {
    let output = command
        .output()
        .map_err(|error| format!("could not start {purpose}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(format!(
            "{purpose} failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_has_two_valid_artifacts_for_every_target() {
        let artifacts = parse_manifest(MANIFEST).unwrap();
        let mut targets = artifacts
            .iter()
            .map(|artifact| artifact.app_target.as_str())
            .collect::<Vec<_>>();
        targets.sort_unstable();
        targets.dedup();
        assert_eq!(targets.len(), 6);
        for target in targets {
            let matching = artifacts
                .iter()
                .filter(|artifact| artifact.app_target == target)
                .collect::<Vec<_>>();
            assert_eq!(matching.len(), 2, "{target}");
            assert!(matching.iter().any(|artifact| artifact.tool == "typst"));
            assert!(matching.iter().any(|artifact| artifact.tool == "tinymist"));
        }
    }

    #[test]
    fn archive_listing_rejects_traversal_and_absolute_paths() {
        assert!(validate_archive_listing("safe/bin/typst\nLICENSE\n").is_ok());
        assert!(validate_archive_listing("../escape\n").is_err());
        assert!(validate_archive_listing("safe/../../escape\n").is_err());
        assert!(validate_archive_listing("/absolute\n").is_err());
    }

    #[test]
    fn archive_metadata_rejects_links_and_special_entries() {
        assert!(
            validate_archive_entry_types(
                "drwxr-xr-x  0 user group 0 Jan 1 00:00 release/\n-rwxr-xr-x  0 user group 1 Jan 1 00:00 release/typst\n"
            )
            .is_ok()
        );
        assert!(
            validate_archive_entry_types(
                "lrwxr-xr-x  0 user group 0 Jan 1 00:00 release/typst -> ../../escape\n"
            )
            .is_err()
        );
        assert!(
            validate_archive_entry_types(
                "hrwxr-xr-x  0 user group 0 Jan 1 00:00 release/typst link to escape\n"
            )
            .is_err()
        );
        assert!(
            validate_archive_entry_types("prw-r--r--  0 user group 0 Jan 1 00:00 release/pipe\n")
                .is_err()
        );
    }

    #[test]
    fn archive_members_are_exactly_allowlisted_and_must_be_unique() {
        let listing = "release/bin/typst\nrelease/LICENSE\n";
        assert_eq!(
            find_unique_member(listing, OsStr::new("typst")).unwrap(),
            "release/bin/typst"
        );
        assert!(find_unique_member(listing, OsStr::new("NOTICE")).is_err());
        assert!(find_unique_member("one/typst\ntwo/typst\n", OsStr::new("typst")).is_err());
        assert!(find_unique_member("directory/typst/\n", OsStr::new("typst")).is_err());
    }

    #[test]
    fn parser_rejects_bad_hashes_and_unknown_tools() {
        assert!(parse_manifest("typst target asset 1 archive not-a-hash").is_err());
        assert!(
            parse_manifest(&format!("other target asset 1 archive {}", "a".repeat(64))).is_err()
        );
    }

    #[test]
    fn package_target_prefers_explicit_then_tiptoptyp_environment() {
        let environment = Some(OsString::from("environment-target"));
        assert_eq!(
            selected_package_target(Some("explicit-target".to_owned()), environment.clone(),)
                .as_deref(),
            Some("explicit-target")
        );
        assert_eq!(
            selected_package_target(None, environment).as_deref(),
            Some("environment-target")
        );
    }

    #[test]
    fn macos_bundle_icon_reference_is_exact_and_scoped_to_its_key() {
        let valid = "<key>CFBundleIconFile</key>\n<string>tiptoptyp.icns</string>\n<key>CFBundleName</key>\n<string>tiptoptyp</string>";
        assert!(bundle_icon_reference(valid, MACOS_APP_ICON));

        let stale = "<key>CFBundleIconFile</key>\n<string>old.icns</string>\n<key>Note</key>\n<string>tiptoptyp.icns</string>";
        assert!(!bundle_icon_reference(stale, MACOS_APP_ICON));
        assert!(!bundle_icon_reference(
            "<string>tiptoptyp.icns</string>",
            MACOS_APP_ICON
        ));
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        let directory =
            env::temp_dir().join(format!("tiptoptyp-xtask-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("abc");
        fs::write(&path, b"abc").unwrap();
        assert_eq!(
            sha256(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
