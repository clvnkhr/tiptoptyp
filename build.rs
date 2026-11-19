#[path = "build_support/build_identity.rs"]
mod build_identity;
#[path = "build_support/locked_versions.rs"]
mod locked_versions;
#[path = "build_support/tinymist_configuration.rs"]
mod tinymist_configuration;
#[path = "build_support/tool_versions.rs"]
mod tool_versions;

fn main() {
    emit_build_identity();
    let lock = std::fs::read_to_string("Cargo.lock").expect("read dependency lockfile");
    let harper = locked_versions::package_version(&lock, "harper-core")
        .expect("one resolved Harper version");
    println!("cargo:rustc-env=TIPTOPTYP_HARPER_VERSION={harper}");
    println!("cargo:rerun-if-changed=toolchain/manifest.tsv");
    println!("cargo:rerun-if-changed=build_support/tool_versions.rs");
    println!("cargo:rerun-if-changed=build_support/tinymist_configuration.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest =
        std::fs::read_to_string("toolchain/manifest.tsv").expect("read bundled toolchain manifest");
    let versions = tool_versions::versions(&manifest).expect("valid bundled toolchain versions");
    let tinymist_version = versions["tinymist"];
    assert!(
        tinymist_configuration::supports_version(tinymist_version),
        "Tinymist {tinymist_version} has no reviewed settings contract (reviewed: {}). Review build_support/tinymist_configuration.rs before updating the bundled server",
        tinymist_configuration::REVIEWED_VERSION,
    );
    tinymist_configuration::validate(&tinymist_configuration::defaults())
        .expect("built-in Tinymist defaults violate their configuration contract");
    for (tool, version) in versions {
        println!(
            "cargo:rustc-env=TIPTOPTYP_BUNDLED_{}_VERSION={version}",
            tool.to_ascii_uppercase().replace('-', "_")
        );
    }
}

fn emit_build_identity() {
    // Observe source inputs and the current HEAD without mutating Git's index.
    // Never inspect Git or the filesystem from a UI frame.
    let root = std::path::Path::new(".");
    for path in build_identity::watch_paths(root) {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("build clock after Unix epoch")
        .as_secs();
    let identity = build_identity::build_id(root, stamp);
    println!("cargo:rustc-env=TIPTOPTYP_BUILD_ID={identity}");
}
