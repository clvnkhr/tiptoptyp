#!/usr/bin/env python3
"""Headless black-box verification of universal packaged payload caches."""
import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import stat
import subprocess
import sys
import tempfile
import time

TOOLS = {"typst", "tinymist", "tectonic", "texlab", "badness", "tex-fmt"}
LIBRARY = "libpdfium.dylib"
MODELS = {"detypify.onnx", "detexify-samples.bin"}
TARGETS = {"arm64": "aarch64-apple-darwin", "x86_64": "x86_64-apple-darwin"}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def command(arguments, *, environment=None, check=True, timeout=60):
    result = subprocess.run([str(value) for value in arguments], env=environment,
                            capture_output=True, text=True, timeout=timeout)
    require(len(result.stdout) + len(result.stderr) <= 16384,
            "Packaged runtime command produced unexpectedly large output")
    if check:
        require(result.returncode == 0,
                f"Command failed ({result.returncode}): {' '.join(map(str, arguments))}\n"
                f"{result.stdout[-2000:]}{result.stderr[-2000:]}")
    return result


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def cache_state(home):
    root = home / "Library/Caches/dev.tiptoptyp.editor/payloads"
    result = {}
    if root.exists():
        for path in root.rglob("*"):
            require(not path.is_symlink(), "Payload cache contains a symlink")
            metadata = path.stat()
            if path.is_dir():
                require(stat.S_IMODE(metadata.st_mode) == 0o700,
                        "Payload cache directory is not private")
                continue
            require(path.is_file(), "Payload cache contains an unexpected entry type")
            result[path.relative_to(root).as_posix()] = {
                "bytes": metadata.st_size, "mtime_ns": metadata.st_mtime_ns,
                "mode": stat.S_IMODE(metadata.st_mode),
            }
    return result


def expect_cache(state, expected):
    actual = set()
    for relative, metadata in state.items():
        parts = Path(relative).parts
        require(len(parts) == 3 and len(parts[1]) == 64
                and all(value in "0123456789abcdef" for value in parts[1]),
                f"Unexpected payload cache layout: {relative}")
        scope, _, name = parts
        require(name not in MODELS, "Handwriting models were eagerly expanded")
        mode = 0o600 if scope == "shared" or name == LIBRARY else 0o700
        require(metadata["mode"] == mode and metadata["bytes"] > 0,
                f"Incorrect payload cache permissions or size: {relative}")
        actual.add((scope, name))
    wanted = {(scope, name) for scope, names in expected.items() for name in names}
    require(actual == wanted and len(state) == len(wanted),
            f"Payload cache mismatch: missing={sorted(wanted - actual)}, "
            f"unexpected={sorted(actual - wanted)}, files={len(state)}")


def unchanged(before, after):
    require(before == after, "A cache-hit run rewrote, duplicated or removed payload files")


def probe(executable, architecture, home, argument):
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("TIPTOPTYP_", "DYLD_"))}
    environment.update(HOME=str(home), CFFIXED_USER_HOME=str(home),
                       PATH="/usr/bin:/bin:/usr/sbin:/sbin")
    start = time.perf_counter()
    result = command(["/usr/bin/arch", f"-{architecture}", executable, argument],
                     environment=environment)
    return {"seconds": round(time.perf_counter() - start, 6),
            "stdout": result.stdout.strip()}, cache_state(home)


def verify_native(home, state):
    root = home / "Library/Caches/dev.tiptoptyp.editor/payloads"
    for relative in state:
        scope = Path(relative).parts[0]
        if scope != "shared":
            expected = next(arch for arch, target in TARGETS.items() if target == scope)
            actual = command(["/usr/bin/lipo", "-archs", root / relative]).stdout.split()
            require(actual == [expected], f"Wrong cached payload architecture: {relative}")


def run(app):
    require(sys.platform == "darwin", "This packaged runtime journey requires macOS")
    executable = app / "Contents/MacOS/tiptoptyp"
    resources = app / "Contents/Resources"
    command(["/usr/bin/codesign", "--verify", "--deep", "--strict", app])
    architectures = command(["/usr/bin/lipo", "-archs", executable]).stdout.split()
    require(set(architectures) == set(TARGETS), "The packaged app is not universal")
    manifest = resources / "assets/shared/manifest.tsv"
    require(manifest.stat().st_size <= 65536, "Shared asset manifest is too large")
    names = {line.split("\t")[0] for line in manifest.read_text().splitlines()}
    fonts = {name for name in names if name.endswith(".ttf")}
    require(len(fonts) == 11 and names - fonts == MODELS,
            "Unexpected shared font/model registry")
    receipt = {"started_utc": datetime.now(timezone.utc).isoformat(),
               "timing_scope": "Headless CLI. First use has an empty payload cache; OS file cache is uncontrolled. Cache hits include process startup and per-process verification.",
               "app_version": command([executable, "--version"]).stdout.strip(),
               "executable_sha256": sha256(executable), "macos": platform.mac_ver()[0],
               "host_architecture": platform.machine(), "app_architectures": architectures,
               "timing_unit": "seconds", "runs": {}, "skipped": {}}
    with tempfile.TemporaryDirectory(prefix="tiptoptyp-payload-probe-") as directory:
        homes = {}
        for architecture, target in TARGETS.items():
            if command(["/usr/bin/arch", f"-{architecture}", "/usr/bin/true"],
                       check=False, timeout=10).returncode:
                receipt["skipped"][architecture] = "Architecture execution is unavailable (Rosetta may be absent)"
                continue
            home = Path(directory) / architecture
            home.mkdir(mode=0o700)
            homes[architecture] = home
            measurements = {}
            previous = {}
            for label, argument in [("tools_first_use", "--check-toolchain"),
                                    ("tools_cache_hit", "--check-toolchain"),
                                    ("runtime_first_use", "--check-runtime"),
                                    ("runtime_cache_hit", "--check-runtime")]:
                measured, state = probe(executable, architecture, home, argument)
                expected = {target: TOOLS}
                if label.startswith("runtime"):
                    expected.update({target: TOOLS | {LIBRARY}, "shared": fonts})
                expect_cache(state, expected)
                if label.endswith("cache_hit"):
                    unchanged(previous, state)
                measurements[label] = {**measured, "cache_files": state,
                                       "cache_bytes": sum(item["bytes"] for item in state.values())}
                previous = state
            verify_native(home, previous)
            receipt["runs"][architecture] = measurements
        require(homes, "Neither packaged architecture could execute")
        if set(homes) == set(TARGETS):
            home = homes["arm64"]
            before = cache_state(home)
            tools, _ = probe(executable, "x86_64", home, "--check-toolchain")
            runtime, after = probe(executable, "x86_64", home, "--check-runtime")
            expect_cache(after, {target: TOOLS | {LIBRARY} for target in TARGETS.values()}
                         | {"shared": fonts})
            unchanged(before, {key: after.get(key) for key in before})
            verify_native(home, after)
            receipt["cross_architecture_reuse"] = {"tools": tools, "runtime": runtime,
                                                   "cache_files": after, "file_count": len(after)}
    command(["/usr/bin/codesign", "--verify", "--deep", "--strict", app])
    require(sha256(executable) == receipt["executable_sha256"], "The packaged executable changed")
    receipt["bundle_signature_before_and_after"] = "passed"
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path,
                        default=Path(__file__).resolve().parents[1] / "target/release/tiptoptyp.app")
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    result = json.dumps(run(arguments.app.resolve()), indent=2) + "\n"
    if arguments.output:
        arguments.output.write_text(result)
    else:
        print(result, end="")


if __name__ == "__main__":
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"Packaged runtime journey failed: {error}", file=sys.stderr)
        sys.exit(1)
