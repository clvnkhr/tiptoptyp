#!/usr/bin/env python3
"""Build the pinned Python-free ONNX Runtime helpers for both macOS targets.

After export-liquid-onnx.py --export and --verify:
  python3 scripts/build-liquid-onnx.py --source /private/tmp/liquid-onnx-raw --work-dir /private/tmp/liquid-onnx-vendor
  cargo xtask prepare-liquid --source /private/tmp/liquid-onnx-raw
  cargo xtask stage-liquid

Requires Apple Silicon macOS, Rosetta, Xcode command-line tools and CMake. This
build-only script downloads SHA256-pinned official C/C++ runtime archives, not
Python packages. --offline requires the two archives in WORK/downloads. ARM 1.31
has a publisher SHA256; Intel 1.19.2's pin is locally observed from the official
HTTPS download because that older GitHub release publishes no digest. Actual
Mach-O floors are 14.0/11.0; helpers target 14.0/12.0. Newer Intel 1.20+ binaries
require macOS 13.3+, so 1.19.2 preserves the editor's existing Intel deployment floor.

The app bundles one compressed model/tokenizer plus two thin helper/library
pairs and full upstream notices. It unpacks the matching runtime automatically
on the first selected-text request. End users install no Python or model runtime.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import subprocess
import tempfile
import urllib.request
import tarfile

HERE = Path(__file__).resolve().parent
CANONICAL_FIXTURE_SHA256 = "3567a30d66d213ad47a3fbd7cbedad4974ea9b6e19037326da82ee5050159dc2"
DISTRIBUTIONS = {
    "aarch64-apple-darwin": {
        "arch": "arm64", "version": "1.31.0", "minimum_macos": [14, 0],
        "upstream_install_name": "@rpath/libonnxruntime.1.dylib",
        "archive": ("https://github.com/microsoft/onnxruntime/releases/download/v1.31.0/onnxruntime-osx-arm64-1.31.0.tgz", 36549009,
                    "1031b36dd3bdaa8d976a6130d9cf2a05e9be320b93ae804f8427b91dc3432afc"),
        "publisher_digest": True,
    },
    "x86_64-apple-darwin": {
        "arch": "x86_64", "version": "1.19.2", "minimum_macos": [12, 0],
        "upstream_install_name": "@rpath/libonnxruntime.1.19.2.dylib",
        "archive": ("https://github.com/microsoft/onnxruntime/releases/download/v1.19.2/onnxruntime-osx-x86_64-1.19.2.tgz", 9219426,
                    "6536e36d7ea92e32d53dad7ddd0fdf10be5b62d1dace85a13e1295ff81e9b5d4"),
        "publisher_digest": False,
    },
}

def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_json(path, value):
    temporary = Path(path).with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def download(pin, downloads, offline):
    url, size, sha = pin
    path = downloads / url.rsplit("/", 1)[1]
    if not path.exists():
        if offline:
            raise ValueError("Missing pinned offline download: " + path.name)
        temporary = path.with_suffix(path.suffix + ".partial")
        try:
            with urllib.request.urlopen(url, timeout=90) as source, temporary.open("wb") as output:
                remaining = size + 1
                while remaining:
                    chunk = source.read(min(1024 * 1024, remaining))
                    if not chunk:
                        break
                    output.write(chunk)
                    remaining -= len(chunk)
            if temporary.stat().st_size != size or digest(temporary) != sha:
                raise ValueError("Downloaded artifact checksum mismatch: " + path.name)
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    if path.stat().st_size != size or digest(path) != sha:
        raise ValueError("Cached artifact checksum mismatch: " + path.name)
    return path


def extract_vendor(path, destination):
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    with tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        if len(members) > 10000 or sum(max(0, row.size) for row in members) > 1024 ** 3:
            raise ValueError("Native archive expansion is too large")
        for row in members:
            name = PurePosixPath(row.name)
            if name.is_absolute() or ".." in name.parts:
                raise ValueError("Unsafe native archive member")
        archive.extractall(destination, filter="data")
    return destination / path.name.removesuffix(".tgz")


def create_notices(target, settings, vendor, destination):
    url, _, sha = settings["archive"]
    sections = ["ONNX Runtime " + settings["version"] + " native distribution notices for " + target,
                "Official archive: " + url, "Archive SHA256: " + sha]
    for name in ["LICENSE", "ThirdPartyNotices.txt"]:
        sections += ["\n===== " + name + " =====\n", (vendor / name).read_text()]
    destination.write_text("\n".join(sections) + "\n")


def command(*arguments, env=None):
    subprocess.run([str(value) for value in arguments], check=True, env=env)


def runtime_names():
    return ["liquid-onnx", "libonnxruntime.dylib"]


def native_rpaths(load_commands):
    lines = load_commands.splitlines()
    paths = []
    for index, line in enumerate(lines):
        if line.strip() == "cmd LC_RPATH":
            for candidate in lines[index + 1:index + 5]:
                if candidate.strip().startswith("path "):
                    paths.append(candidate.strip()[5:].split(" (offset", 1)[0])
                    break
    return paths


def minimum_os(load_commands):
    lines = load_commands.splitlines()
    for index, line in enumerate(lines):
        if line.strip() in ["cmd LC_BUILD_VERSION", "cmd LC_VERSION_MIN_MACOSX"]:
            for candidate in lines[index + 1:index + 8]:
                if candidate.strip().startswith(("minos ", "version ")):
                    return candidate.split()[1]
    raise ValueError("Native file has no macOS minimum-version command")


def version_tuple(value):
    return tuple((list(map(int, value.split("."))) + [0, 0])[:3])


def normalization_plan(name, names, own_id, dependencies, rpaths, upstream_install_name=None):
    """Remove developer paths without relying on a machine's DYLD environment."""
    plan = []
    if own_id and own_id != "@rpath/" + name:
        plan.append(("-id", "@rpath/" + name))
    needs_rpath = False
    for dependency in dependencies:
        if dependency == own_id or dependency.startswith(("/usr/lib/", "/System/Library/")):
            continue
        if dependency == upstream_install_name:
            plan.append(("-change", dependency, "@loader_path/libonnxruntime.dylib"))
            continue
        basename = PurePosixPath(dependency).name
        if basename not in names:
            raise ValueError("Native dependency lies outside the bundled runtime closure")
        if dependency not in ["@rpath/" + basename, "@loader_path/" + basename]:
            plan.append(("-change", dependency, "@loader_path/" + basename))
        elif dependency.startswith("@rpath/"):
            needs_rpath = True
    for entry in rpaths:
        if entry != "@loader_path":
            plan.append(("-delete_rpath", entry))
    if needs_rpath and "@loader_path" not in rpaths:
        plan.append(("-add_rpath", "@loader_path"))
    return plan


def inspect_native(path):
    def output(*args):
        return subprocess.check_output([str(value) for value in args], text=True)
    ids = output("/usr/bin/otool", "-D", path).splitlines()
    own_id = ids[1].strip() if len(ids) > 1 else None
    dependencies = [line.strip().split(" (compatibility", 1)[0]
                    for line in output("/usr/bin/otool", "-L", path).splitlines()[1:]
                    if line.startswith("\t")]
    load_commands = output("/usr/bin/otool", "-l", path)
    return {"own_id": own_id, "dependencies": dependencies,
            "rpaths": native_rpaths(load_commands), "minimum_macos": minimum_os(load_commands),
            "architecture": output("/usr/bin/lipo", "-archs", path).strip()}


def normalize_native(path, names, settings):
    before = inspect_native(path)
    for action in normalization_plan(path.name, names, before["own_id"],
                                     before["dependencies"], before["rpaths"], settings["upstream_install_name"]):
        command("/usr/bin/install_name_tool", *action, path)
    after = inspect_native(path)
    if normalization_plan(path.name, names, after["own_id"], after["dependencies"], after["rpaths"], settings["upstream_install_name"]):
        raise ValueError("Native dependency normalization did not converge")
    minimum = ".".join(map(str, settings["minimum_macos"]))
    if (after["architecture"] != settings["arch"]
            or version_tuple(after["minimum_macos"]) > version_tuple(minimum)
            or (path.name == "liquid-onnx" and version_tuple(after["minimum_macos"]) != version_tuple(minimum))):
        raise ValueError("Native architecture or minimum macOS does not match its target")
    return after


def validate_smoke(target, stdout):
    rows = [json.loads(line) for line in stdout.splitlines()]
    if len(rows) != 2:
        raise ValueError("Native helper smoke test returned an unexpected response count")
    ready, prediction = rows
    probabilities = prediction.get("error_probs", [])
    if (ready.get("ready") is not True or ready.get("protocol") != 1
            or ready.get("target") != target or ready.get("threads") != 4
            or prediction.get("id") != 1 or prediction.get("ok") is not True
            or prediction.get("best_tags") != [0] * 6 or len(probabilities) != 6
            or any(not math.isfinite(value) or not 0 <= value <= 1 for value in probabilities)):
        raise ValueError("Native helper failed its loaded-model smoke test")


def smoke_runtime(target, runtime, model):
    # No developer library paths or Python can participate in this smoke test.
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(("DYLD_", "PYTHON"))}
    environment["PATH"] = "/usr/bin:/bin"
    result = subprocess.run([str(runtime / "liquid-onnx"), str(model), "4"],
                            input="1 1 7580 12693 938 11534 1701\n", text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            check=True, timeout=60, env=environment)
    validate_smoke(target, result.stdout)


def build_runtime(target, settings, vendor, source, work):
    build = work / ("build-" + target)
    minimum = ".".join(map(str, settings["minimum_macos"]))
    environment = dict(os.environ, MACOSX_DEPLOYMENT_TARGET=minimum)
    command("cmake", "-S", HERE / "liquid-onnx-helper", "-B", build,
            "-DCMAKE_BUILD_TYPE=Release", "-DBUILD_TESTING=OFF",
            "-DCMAKE_CXX_COMPILER=/usr/bin/clang++", "-DONNXRUNTIME_ROOT=" + str(vendor),
            "-DCMAKE_OSX_ARCHITECTURES=" + settings["arch"],
            "-DCMAKE_OSX_DEPLOYMENT_TARGET=" + minimum, env=environment)
    command("cmake", "--build", build, "--config", "Release", "--parallel", "2", env=environment)
    runtime_root = source / "runtime"
    runtime_root.mkdir(exist_ok=True)
    destination = runtime_root / target
    if destination.exists():
        raise ValueError("Native destination already exists; remove it explicitly before rebuilding: " + str(destination))
    with tempfile.TemporaryDirectory(prefix=".liquid-native-", dir=runtime_root) as temporary:
        staged = Path(temporary)
        inspections = {}
        for name in runtime_names():
            original = build / name if name == "liquid-onnx" else vendor / "lib" / name
            shutil.copyfile(original, staged / name)
            (staged / name).chmod(0o755)
            inspections[name] = normalize_native(staged / name, runtime_names(), settings)
            command("/usr/bin/strip", "-S", "-x", staged / name)
            command("/usr/bin/codesign", "--force", "--sign", "-", "--timestamp=none", staged / name)
            command("/usr/bin/codesign", "--verify", "--strict", staged / name)
        smoke_runtime(target, staged, source / "liquid.onnx")
        staged.rename(destination)
    return destination, inspections


def validate_source(source):
    provenance = json.loads((source / "PROVENANCE.json").read_text())
    if provenance.get("format") != 3:
        raise ValueError("Native build requires the current format-3 source provenance")
    parity = provenance.get("parity", {})
    for name, key in [("liquid.onnx", "model_sha256"), ("tokenizer.json", "tokenizer_sha256")]:
        value = digest(source / name)
        if provenance.get(key) != value or parity.get(key) != value:
            raise ValueError("Shared source hashes differ from passing parity")
    shapes = parity.get("shape_boundaries", [])
    if (parity.get("matched", 0) != 268 or parity.get("compared_reference_steps", 0) != 427
            or parity.get("fixture_sha256") != CANONICAL_FIXTURE_SHA256
            or parity.get("mismatches") != []
            or parity.get("lossless_weights") is not True
            or not 0 <= parity.get("max_error_probability_delta", 1) <= 1e-4
            or [shape.get("sequence") for shape in shapes] != [1, 2, 8, 32, 127, 128]
            or not all(shape.get("tags_match") is True and 0 <= shape.get("max_error_probability_delta", 1) <= 1e-4
                       for shape in shapes)):
        raise ValueError("Native build requires passing canonical 268-case/427-pass correction parity")
    return provenance


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "arm64":
        parser.error("building and smoke-testing both helpers requires Apple Silicon macOS with Rosetta and Xcode tools")
    source, work = args.source.resolve(), args.work_dir.resolve()
    provenance = validate_source(source)
    downloads = work / "downloads"
    downloads.mkdir(parents=True, exist_ok=True)
    (source / "licenses").mkdir(exist_ok=True)
    runtimes = []
    for target, settings in DISTRIBUTIONS.items():
        archive = download(settings["archive"], downloads, args.offline)
        vendor = extract_vendor(archive, work / ("vendor-" + target))
        notices = source / "licenses" / (target + ".txt")
        create_notices(target, settings, vendor, notices)
        runtime, inspections = build_runtime(target, settings, vendor, source, work)
        files = [{"name": name, "bytes": (runtime / name).stat().st_size,
                  "sha256": digest(runtime / name)} for name in runtime_names()]
        runtimes.append({"target": target, "runtime_version": settings["version"],
                         "minimum_macos": settings["minimum_macos"],
                         "archive_url": settings["archive"][0], "archive_sha256": settings["archive"][2],
                         "archive_digest_source": "publisher SHA256" if settings["publisher_digest"] else "reviewed official HTTPS download; no upstream-published checksum",
                         "helper_sha256": digest(runtime / "liquid-onnx"),
                         "notices_sha256": digest(notices), "files": files,
                         "inspection": inspections, "loaded_model_smoke_passed": True,
                         "helper_source_sha256": digest(HERE / "liquid-onnx-helper/main.cpp"),
                         "helper_cmake_sha256": digest(HERE / "liquid-onnx-helper/CMakeLists.txt"),
                         "helper_protocol_sha256": digest(HERE / "liquid-onnx-helper/protocol.hpp"),
                         "builder_sha256": digest(__file__)})
    provenance["runtimes"] = runtimes
    write_json(source / "PROVENANCE.json", provenance)
    print("Native raw payload ready:", source)


if __name__ == "__main__":
    main()
