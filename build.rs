#[path = "build_support/tool_versions.rs"]
mod tool_versions;

fn main() {
    println!("cargo:rerun-if-changed=toolchain/manifest.tsv");
    println!("cargo:rerun-if-changed=build_support/tool_versions.rs");
    println!("cargo:rerun-if-changed=build.rs");
    let manifest =
        std::fs::read_to_string("toolchain/manifest.tsv").expect("read bundled toolchain manifest");
    let versions = tool_versions::versions(&manifest).expect("valid bundled toolchain versions");
    for (tool, version) in versions {
        println!(
            "cargo:rustc-env=TIPTOPTYP_BUNDLED_{}_VERSION={version}",
            tool.to_ascii_uppercase()
        );
    }
}
