#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod liquid;
mod ltex;
mod macos;
mod payloads;
mod pdfium;
mod profile;

use std::{
    env,
    ffi::{OsStr, OsString},
    fs,
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
};

const MANIFEST: &str = include_str!("../../toolchain/manifest.tsv");
const TEX_LICENSES: &str = include_str!("../../toolchain/tex-licenses.tsv");
const TINYMIST_LICENSE_SHA256: &str =
    "a9f29769fd3a7ee2976e6e161a93e16461fa305c088c4806242e50ec8ef86bce";
const TYPST_LICENSE_SHA256: &str =
    "62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a";
const TYPST_NOTICE_SHA256: &str =
    "1778244777547c281b6f5fa9fc0c18ab21f8d4491c803f64e09046800f5fcb26";
const PACKAGE_TARGET_ENV: &str = "TIPTOPTYP_PACKAGE_TARGET";
const MACOS_APP_ICON: &str = "tiptoptyp.icns";
const PRODUCTION_MACOS_APP_BUNDLE: &str = "tiptoptyp.app";
// One registry feeds both aggregate notices and packaged-resource validation.
const FONT_LICENSES: &[(&str, &str)] = &[
    (
        "assets/fonts/notoemoji/OFL.txt",
        "licenses/NotoEmoji-OFL.txt",
    ),
    (
        "assets/fonts/notosanssymbols/OFL.txt",
        "licenses/NotoSansSymbols-OFL.txt",
    ),
    (
        "assets/fonts/notosansmath/OFL.txt",
        "licenses/NotoSansMath-OFL.txt",
    ),
    (
        "assets/fonts/notosanssymbols2/OFL.txt",
        "licenses/NotoSansSymbols2-OFL.txt",
    ),
    (
        "assets/fonts/notosanshebrew/OFL.txt",
        "licenses/NotoSansHebrew-OFL.txt",
    ),
    ("assets/fonts/notosans/OFL.txt", "licenses/NotoSans-OFL.txt"),
    ("assets/fonts/hack/LICENSE.md", "licenses/Hack-LICENSE.md"),
    (
        "assets/fonts/nerd/LICENSE.md",
        "licenses/NerdFont-LICENSE.md",
    ),
];
const HACK_LICENSE_SHA256: &str =
    "1f61bb7c790c59b4b0ecdf304628b94e42ae4c8020094a8c3da73381ab212623";

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
            "tectonic" => format!(
                "https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%40{}/{}",
                self.version, self.archive
            ),
            "texlab" => format!(
                "https://github.com/latex-lsp/texlab/releases/download/v{}/{}",
                self.version, self.archive
            ),
            "badness" => format!(
                "https://github.com/jolars/badness/releases/download/v{}/{}",
                self.version, self.archive
            ),
            "tex-fmt" => format!(
                "https://github.com/WGUNDERWOOD/tex-fmt/releases/download/v{}/{}",
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
    let mut production = false;
    let mut liquid_source = None;
    while let Some(argument) = arguments.next() {
        if argument == "--target" {
            target = Some(
                arguments
                    .next()
                    .ok_or_else(|| "--target requires a target triple".to_owned())?,
            );
        } else if argument == "--source" {
            liquid_source = Some(PathBuf::from(
                arguments
                    .next()
                    .ok_or("--source requires an input folder")?,
            ));
        } else if argument == "--prod" {
            production = true;
        } else {
            return Err(format!("unknown argument: {argument}"));
        }
    }

    if liquid_source.is_some() && !matches!(command.as_str(), "prepare-liquid" | "ensure-liquid") {
        return Err("--source is only available for prepare-liquid or ensure-liquid".into());
    }
    match command.as_str() {
        "prepare-liquid" => {
            liquid::prepare(&liquid_source.ok_or("prepare-liquid requires --source PATH")?)
        }
        "ensure-liquid" => liquid::ensure_prepared(liquid_source.as_deref()),
        "stage-liquid" => {
            liquid::preflight()?;
            liquid::stage()
        }
        "fetch-ltex" => ltex::stage(&package_target(target)?),
        "fetch-pdfium" => pdfium::fetch(&target_or_host(target)?),
        "fetch-sidecars" => {
            let target = target_or_host(target)?;
            fetch_sidecars(&target)
        }
        "package-build" => {
            let target = package_target(target)?;
            let slices = macos::targets(&target);
            for slice in &slices {
                fetch_sidecars(slice)?;
                pdfium::fetch(slice)?;
            }
            // A cross-only package also prepares the host's development renderer
            // and aggregate notices. Universal builds already fetched that slice.
            let host = host_target()?;
            if !slices.contains(&host.as_str()) {
                pdfium::fetch(&host)?;
            }
            ltex::stage(&target)?;
            generate_third_party_notices()?;
            payloads::stage(&target)?;
            if target.ends_with("apple-darwin") {
                macos::build_release(&target, production)
            } else {
                let root = repository_root();
                let mut cargo = Command::new("cargo");
                cargo
                    .args(["build", "--locked", "--release", "--target", &target])
                    .arg("--target-dir")
                    .arg(root.join("target"));
                if production {
                    cargo.args(["--features", "production"]);
                }
                run_status(&mut cargo, "release build")?;
                let name = if target.contains("windows") {
                    "tiptoptyp.exe"
                } else {
                    "tiptoptyp"
                };
                fs::create_dir_all(root.join("target/release"))
                    .map_err(|error| error.to_string())?;
                fs::copy(
                    root.join("target").join(target).join("release").join(name),
                    root.join("target/release").join(name),
                )
                .map(|_| ())
                .map_err(|error| error.to_string())
            }
        }
        "verify-package" => {
            if production {
                return Err("verify-package always verifies the production app bundle".to_owned());
            }
            verify_package(&package_target(target)?)
        }
        "generate-notices" => generate_third_party_notices(),
        "help" | "--help" | "-h" => {
            println!(
                "tiptoptyp tasks\n\n  profile --help\n  fetch-sidecars [--target TRIPLE]\n  fetch-pdfium [--target TRIPLE]\n  fetch-ltex [--target TRIPLE] (bundle LTeX+ with Java for the package targets)\n  prepare-liquid --source PATH (compress reviewed raw model and native runtimes)\n  ensure-liquid [--source PATH] (recover prepared assets from a staged or installed package)\n  stage-liquid (verify and stage the prepared native model package)\n  package-build [--target TRIPLE] [--prod] (macOS: universal by default)\n  verify-package [--target TRIPLE]\n  generate-notices\n\n{PACKAGE_TARGET_ENV} supplies the target to cargo-packager's hook."
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
        root.join("toolchain/licenses/tectonic-LICENSE"),
        root.join("toolchain/licenses/texlab-LICENSE"),
        root.join("toolchain/licenses/badness-LICENSE"),
        root.join("toolchain/licenses/tex-fmt-LICENSE"),
        root.join("assets/handwriting/LICENSE"),
        root.join("assets/handwriting/UNICODEIT-LICENSE"),
        root.join("assets/handwriting/HYPERREF-LICENSE"),
        root.join("assets/handwriting/DETEXIFY-LICENSE"),
        root.join("docs/licenses/ghostty-MIT.txt"),
        root.join("docs/licenses/simdutf-MIT.txt"),
        root.join("docs/licenses/highway-BSD3.txt"),
        root.join("docs/licenses/uucode-MIT.md"),
        root.join("docs/licenses/utf8-decoder-MIT.txt"),
        root.join("docs/licenses/Unicode-3.0.txt"),
        root.join("docs/licenses/zig-MIT.txt"),
        root.join("docs/licenses/mathgloss-MIT.txt"),
        root.join("assets/liquid/SACREMOSES-LICENSE"),
        root.join("toolchain/ltex/LICENSE.md"),
        root.join("toolchain/ltex/ACKNOWLEDGMENTS.md"),
        root.join("toolchain/ltex/PROVENANCE.json"),
    ]
    .into_iter()
    .chain(FONT_LICENSES.iter().map(|(source, _)| root.join(source)))
    {
        let name = path
            .strip_prefix(&root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("could not read bundled notice {name}: {error}"))?;
        bundled_notices.push_str(&format!("\n===== {name} =====\n\n{text}\n"));
    }
    let pdfium = pdfium::bundle_directory(&root, &host_target()?);
    let mut pdfium_licenses = vec![pdfium.join("LICENSE"), pdfium.join("PROVENANCE.txt")];
    for entry in fs::read_dir(pdfium.join("licenses"))
        .map_err(|e| format!("Fetch PDFium before generating notices: {e}"))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_file() {
            pdfium_licenses.push(path);
        }
    }
    pdfium_licenses.sort();
    for path in pdfium_licenses {
        let bytes = fs::read(&path).map_err(|e| e.to_string())?;
        let text = match String::from_utf8(bytes) {
            Ok(text) => text,
            // The pinned PDFium FreeType notice contains Latin-1 copyright signs.
            // Keep the original file in the bundle; transcode only the aggregate.
            Err(error) if path.file_name() == Some(OsStr::new("freetype.txt")) => {
                error.into_bytes().into_iter().map(char::from).collect()
            }
            Err(error) => return Err(format!("Invalid UTF-8 notice {}: {error}", path.display())),
        };
        bundled_notices.push_str(&format!(
            "\n===== {} =====\n{text}\n",
            path.strip_prefix(&root).unwrap().display()
        ));
    }
    let output = format!(
        "THIRD-PARTY NOTICES\nThis file contains notices for third-party code and assets distributed by tiptoptyp.\nThe Rust dependency section is generated by cargo-about during packaging.\n\n===== BUNDLED THEME NOTICES =====\n{themes}\n{bundled_notices}\n===== RUST DEPENDENCY NOTICES =====\n{cargo_notices}"
    );
    // Upstream licenses use mixed line endings and trailing spaces. Normalize
    // whitespace in the aggregate without changing their text or source files.
    let output = output
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
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
    let app = root
        .join("target/release")
        .join(PRODUCTION_MACOS_APP_BUNDLE);
    let executable_dir = app.join("Contents/MacOS");
    let resources = app.join("Contents/Resources");
    require_file(&executable_dir.join("tiptoptyp"))?;
    require_file(&resources.join(MACOS_APP_ICON))?;
    verify_bundle_icon_reference(&app.join("Contents/Info.plist"), MACOS_APP_ICON)?;
    verify_hash(&resources.join("LICENSE"), &sha256(&root.join("LICENSE"))?)?;
    macos::verify_architectures(&executable_dir.join("tiptoptyp"), target)?;
    payloads::verify(&resources, target)?;
    verify_liquid_is_on_demand(&resources)?;
    ltex::verify(&resources, target)?;
    verify_hash(
        &resources.join("licenses/Sacremoses-MIT.txt"),
        &sha256(&root.join("assets/liquid/SACREMOSES-LICENSE"))?,
    )?;
    for slice in macos::targets(target) {
        require_file(
            &resources
                .join("pdfium")
                .join(format!("PROVENANCE-{slice}.txt")),
        )?;
        for tool in [
            "typst", "tinymist", "tectonic", "texlab", "badness", "tex-fmt",
        ] {
            require_file(
                &resources
                    .join("toolchain-provenance")
                    .join(format!("{tool}-{slice}.txt")),
            )?;
        }
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
    for (file, _, hash) in tex_licenses()? {
        verify_hash(&resources.join("licenses").join(file), hash)?;
    }
    verify_font_licenses(&root, &resources)?;
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

    let host = host_target()?;
    let runtime_cache = tempfile::tempdir().map_err(|error| error.to_string())?;
    for slice in macos::targets(target) {
        let translated = slice == "x86_64-apple-darwin" && host == "aarch64-apple-darwin";
        if slice != host
            && (!translated
                || !Command::new("arch")
                    .args(["-x86_64", "/usr/bin/true"])
                    .status()
                    .is_ok_and(|status| status.success()))
        {
            println!(
                "runtime execution unavailable for {slice}; architecture, payload and signature checks passed"
            );
            continue;
        }
        for check in ["--check-runtime", "--check-toolchain"] {
            let mut command = if translated {
                let mut command = Command::new("arch");
                command.arg("-x86_64").arg(executable_dir.join("tiptoptyp"));
                command
            } else {
                Command::new(executable_dir.join("tiptoptyp"))
            };
            command
                .arg(check)
                .env("HOME", runtime_cache.path())
                .env("CFFIXED_USER_HOME", runtime_cache.path());
            run_status(&mut command, &format!("checking packaged {slice} {check}"))?;
        }
    }

    println!("verified {}", app.display());
    Ok(())
}

fn verify_liquid_is_on_demand(resources: &Path) -> Result<(), String> {
    let liquid = resources.join("liquid");
    match fs::symlink_metadata(&liquid) {
        Ok(_) => Err(format!(
            "package unexpectedly contains on-demand Liquid assets: {}",
            liquid.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not inspect {}: {error}", liquid.display())),
    }
}

fn verify_font_licenses(root: &Path, resources: &Path) -> Result<(), String> {
    // Both source and package must carry the genuine pinned Hack notice.
    verify_hash(
        &root.join("assets/fonts/hack/LICENSE.md"),
        HACK_LICENSE_SHA256,
    )?;
    for (source, destination) in FONT_LICENSES {
        verify_hash(&resources.join(destination), &sha256(&root.join(source))?)?;
    }
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

fn package_target(explicit: Option<String>) -> Result<String, String> {
    if let Some(target) = selected_package_target(explicit, env::var_os(PACKAGE_TARGET_ENV)) {
        return Ok(target);
    }
    Ok(macos::default_target(&host_target()?).to_owned())
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
    if artifacts.len() != 6
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
    for (file, url, hash) in tex_licenses()? {
        let path = licenses.join(file);
        if !path.is_file() || sha256(&path)? != hash {
            download(url, &path)?;
        }
        verify_hash(&path, hash)?;
    }
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
        if !matches!(
            fields[0],
            "typst" | "tinymist" | "tectonic" | "texlab" | "badness" | "tex-fmt"
        ) {
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

fn tex_licenses() -> Result<Vec<(&'static str, &'static str, &'static str)>, String> {
    TEX_LICENSES
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
        .map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            match fields.as_slice() {
                [_tool, file, url, hash]
                    if !file.contains('/')
                        && !file.contains("..")
                        && url.starts_with("https://raw.githubusercontent.com/")
                        && hash.len() == 64
                        && hash.bytes().all(|b| b.is_ascii_hexdigit()) =>
                {
                    Ok((*file, *url, *hash))
                }
                _ => Err("Invalid TeX license manifest".into()),
            }
        })
        .collect()
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
    fn both_packaging_configurations_bundle_ltex_and_leave_liquid_on_demand() {
        for config in [
            include_str!("../../Cargo.toml"),
            include_str!("../../packager-prod.toml"),
        ] {
            assert!(config.contains(r#"src = "toolchain/ltex", target = "ltex""#));
            assert!(!config.contains(r#"src = "toolchain/liquid""#));
        }
    }

    #[test]
    fn on_demand_liquid_verification_rejects_stale_payloads() {
        let resources = tempfile::tempdir().unwrap();
        verify_liquid_is_on_demand(resources.path()).unwrap();
        let liquid = resources.path().join("liquid");
        fs::create_dir(&liquid).unwrap();
        assert!(
            verify_liquid_is_on_demand(resources.path())
                .unwrap_err()
                .contains("on-demand Liquid assets")
        );
        fs::remove_dir(&liquid).unwrap();
        fs::write(&liquid, "stale payload").unwrap();
        assert!(verify_liquid_is_on_demand(resources.path()).is_err());
        fs::remove_file(&liquid).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(resources.path().join("missing"), &liquid).unwrap();
            assert!(verify_liquid_is_on_demand(resources.path()).is_err());
        }
    }

    #[test]
    fn font_notice_registry_covers_all_bundled_fonts_and_package_resources() {
        use std::collections::BTreeSet;
        let root = repository_root();
        let actual = fs::read_dir(root.join("assets/fonts"))
            .unwrap()
            .flat_map(|entry| {
                let directory = entry.unwrap().path();
                ["OFL.txt", "LICENSE.md"].map(|name| directory.join(name))
            })
            .filter(|path| path.is_file())
            .map(|path| path.strip_prefix(&root).unwrap().to_owned())
            .collect::<BTreeSet<_>>();
        let registered = FONT_LICENSES
            .iter()
            .map(|(source, _)| PathBuf::from(source))
            .collect::<BTreeSet<_>>();
        assert_eq!(
            actual, registered,
            "a bundled font is missing from notices or verification"
        );
        for manifest in [
            include_str!("../../Cargo.toml"),
            include_str!("../../packager-prod.toml"),
        ] {
            for (source, destination) in FONT_LICENSES {
                assert!(
                    manifest.contains(&format!(r#"src = "{source}", target = "{destination}""#))
                );
            }
        }
        verify_hash(
            &root.join("assets/fonts/hack/LICENSE.md"),
            HACK_LICENSE_SHA256,
        )
        .unwrap();
    }

    #[test]
    fn font_package_verification_rejects_missing_and_changed_hack_notice() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let root = repository_root();
        let scratch =
            env::temp_dir().join(format!("tiptoptyp-font-package-{}", std::process::id()));
        fs::create_dir(&scratch).unwrap();
        let scratch = Scratch(scratch);
        fs::create_dir(scratch.0.join("licenses")).unwrap();
        for (source, destination) in FONT_LICENSES {
            fs::copy(root.join(source), scratch.0.join(destination)).unwrap();
        }
        verify_font_licenses(&root, &scratch.0).unwrap();
        let hack = scratch.0.join("licenses/Hack-LICENSE.md");
        fs::remove_file(&hack).unwrap();
        assert!(
            verify_font_licenses(&root, &scratch.0)
                .unwrap_err()
                .contains("Hack-LICENSE.md")
        );
        fs::write(&hack, "incorrect notice").unwrap();
        assert!(
            verify_font_licenses(&root, &scratch.0)
                .unwrap_err()
                .contains("SHA-256 mismatch")
        );
    }

    #[test]
    fn manifest_has_six_valid_artifacts_for_every_target() {
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
            assert_eq!(matching.len(), 6, "{target}");
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
