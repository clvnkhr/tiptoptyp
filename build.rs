#[path = "build_support/tool_versions.rs"]
mod tool_versions;

fn main() {
    emit_build_identity();
    println!("cargo:rerun-if-changed=toolchain/manifest.tsv");
    println!("cargo:rerun-if-changed=build_support/tool_versions.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest =
        std::fs::read_to_string("toolchain/manifest.tsv").expect("read bundled toolchain manifest");
    let versions = tool_versions::versions(&manifest).expect("valid bundled toolchain versions");
    for (tool, version) in versions {
        println!(
            "cargo:rustc-env=TIPTOPTYP_BUNDLED_{}_VERSION={version}",
            tool.to_ascii_uppercase().replace('-', "_")
        );
    }
}

fn emit_build_identity() {
    // Refresh for code/assets and Git operations, including linked worktrees.
    // Never inspect Git or the filesystem from a UI frame.
    for path in [
        "src",
        "core/src",
        "core/Cargo.toml",
        "assets",
        "Cargo.toml",
        "Cargo.lock",
        "build_support",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    for name in ["HEAD", "index", "refs", "packed-refs"] {
        if let Some(path) = git(&["rev-parse", "--git-path", name])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let revision =
        git(&["rev-parse", "--short=12", "HEAD"]).unwrap_or_else(|| "unknown-revision".into());
    let dirty = match git(&["status", "--porcelain", "--untracked-files=normal"]) {
        Some(status) if status.is_empty() => "",
        Some(_) => "-dirty",
        None => "-unknown-state",
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("build clock after Unix epoch")
        .as_secs();
    println!("cargo:rustc-env=TIPTOPTYP_BUILD_ID={revision}{dirty}.{stamp}");
}

fn git(args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}
