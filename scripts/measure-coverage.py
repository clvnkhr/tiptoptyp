#!/usr/bin/env python3
"""Measure Rust workspace coverage; optionally exercise real preview/TeX tools.

Install cargo-llvm-cov and the matching llvm-tools-preview component first.
Reports are local artifacts under .tiptoptyp/coverage/latest, never app assets.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tomllib


ROOT = Path(__file__).resolve().parent.parent
EXCLUDE = r"/(vendor|tests|rustlib)/|/\.cargo/registry/|build\.rs$|/src/app/(.*tests|cross_owner_test)\.rs$|/src/app/preview_controls/e2e\.rs$"
PREVIEW_TESTS = [
    "app::preview_controls::e2e::",
    "app::pdfium_view::tests::native_view_retains_pixels_during_updates_and_errors",
    "pdfium::tests::native_thumbnails_preserve_pixels_rotation_white_background_and_cancellation",
    "pdfium::tests::native_worker_reuses_documents_rejects_stale_and_survives_errors",
    "tinymist::tests::real_",
    "app::tests::real_tinymist_hover_targets_respect_math_syntax_boundaries",
    "compiler::tests::persistent_watcher_compiles_errors_and_recovers",
    "editor_features::table::tests::table_generated_spans_and_markdown_compile_with_typst",
    "app::tests::projected_application_compiles_real_inline_and_block_output",
    "tex::tests::real_tex_services_format_complete_hover_and_lint",
]
TEX_TESTS = [
    "compiler::tex::tests::installed_engines_build_and_synctex_round_trip_after_private_mirror_is_removed",
    "compiler::tex::tests::real_tectonic_builds_unsaved_source_relative_inputs_errors_and_recovers",
]


def output(*command):
    return subprocess.check_output(command, cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--real-tools", action="store_true", help="include pinned Typst/Tinymist and bundled PDFium tests")
    parser.add_argument("--tex-distribution", action="store_true", help="also require all three system TeX engines, SyncTeX and pinned TeX sidecars")
    args = parser.parse_args()
    environment = os.environ.copy()
    channel = tomllib.loads((ROOT / "rust-toolchain.toml").read_text())["toolchain"]["channel"]
    host = next(line.removeprefix("host: ") for line in output("rustc", "-vV").splitlines() if line.startswith("host: "))
    if args.real_tools:
        for kind in ["TYPST", "TINYMIST"]:
            variable = f"TIPTOPTYP_TEST_{kind}"
            environment.setdefault(variable, str(ROOT / "toolchain/bin" / f"{kind.lower()}-{host}"))
            if not os.access(environment[variable], os.X_OK):
                parser.error(f"Missing {kind}; fetch sidecars or set {variable}")
    # Homebrew rustc and rustup can coexist. The compiler's sysroot may not
    # contain tools installed by rustup even when their LLVM versions match.
    sysroots = [Path(output("rustc", "--print", "sysroot"))]
    if not all(environment.get(name) for name in ["LLVM_COV", "LLVM_PROFDATA"]):
        sysroots.append(Path(output("rustup", "run", channel, "rustc", "--print", "sysroot")))
    for variable, binary in [("LLVM_COV", "llvm-cov"), ("LLVM_PROFDATA", "llvm-profdata")]:
        if variable not in environment:
            candidates = [root / "lib/rustlib" / host / "bin" / binary for root in sysroots]
            environment[variable] = str(next((path for path in candidates if path.is_file()), candidates[0]))
            if not Path(environment[variable]).is_file():
                parser.error(f"Missing {binary}; run rustup component add llvm-tools-preview --toolchain {channel}")
    destination = ROOT / ".tiptoptyp/coverage/latest"
    destination.mkdir(parents=True, exist_ok=True)
    commands = []

    def run(*arguments):
        command = ["cargo", "llvm-cov", *arguments]
        commands.append(command)
        subprocess.run(command, cwd=ROOT, env=environment, check=True)

    def summary(name):
        target = destination / name
        run("report", "--workspace", "--json", "--summary-only", "--ignore-filename-regex", EXCLUDE, "--output-path", str(target))
        return json.loads(target.read_text())["data"][0]

    # Explicitly clear profiles. --no-report deliberately retains profiles,
    # so subsequent invocations accumulate
    # into this exact instrumented build, never mixing an old run's counters.
    run("clean", "--workspace")
    run("--workspace", "--locked", "--no-report", "--no-fail-fast")
    standard = summary("standard.json")
    filters = (PREVIEW_TESTS if args.real_tools else []) + (TEX_TESTS if args.tex_distribution else [])
    for test in filters:
        run("test", "--locked", "--bin", "tiptoptyp", "--no-report", test, "--", "--ignored", "--test-threads=1")
    final = summary("coverage.json")
    run("report", "--workspace", "--html", "--ignore-filename-regex", EXCLUDE, "--output-dir", str(destination))

    def normalize(data):
        return {"totals": data["totals"], "files": {
            str(Path(item["filename"]).relative_to(ROOT)): item["summary"] for item in data["files"]
        }}

    report = {
        "revision": output("git", "rev-parse", "HEAD"),
        "working_tree_modified": bool(output("git", "status", "--porcelain")),
        "platform": platform.platform(),
        "rustc": output("rustc", "-vV"),
        "cargo_llvm_cov": output("cargo", "llvm-cov", "--version"),
        "source_sha256": hashlib.sha256(b"".join(
            str(Path(item["filename"]).relative_to(ROOT)).encode() + b"\0" + Path(item["filename"]).read_bytes()
            for item in sorted(final["files"], key=lambda item: item["filename"])
        )).hexdigest(),
        "scope": "Rust application and core workspace; inline unit tests included. Standalone test files, vendor code and build script excluded. Native GUI, JavaScript and external binaries are not instrumented. Branch coverage is unavailable on this stable toolchain.",
        "extra_tests": filters,
        "commands": commands,
        "standard": normalize(standard),
        "with_extra_tests": normalize(final),
    }
    (destination / "summary.json").write_text(json.dumps(report, indent=2) + "\n")
    print(f"Line coverage: {final['totals']['lines']['percent']:.2f}%")
    print(f"Report: {destination / 'html/index.html'}")


if __name__ == "__main__":
    main()
