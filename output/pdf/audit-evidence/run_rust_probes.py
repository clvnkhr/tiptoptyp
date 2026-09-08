#!/usr/bin/env python3
"""Run audit probes in temporary directories against existing Cargo artifacts.

Run `cargo build` first. No dependency download, source edit, or GUI launch is
performed by this script. All fixtures and probe executables are temporary.
"""

import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
if not TARGET.is_absolute():
    TARGET = ROOT / TARGET
DEPS = TARGET / "debug" / "deps"
LOCKED = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]


def dependency(name):
    candidates = list(DEPS.glob(f"lib{name}-*.rlib"))
    package_name = name.replace("_", "-")
    versions = [package["version"] for package in LOCKED
                if package["name"] == package_name]
    # quick-xml has two versions in this checkout. The audited compiler uses
    # the newer direct dependency, not the transitive 0.38 dependency.
    version = max(versions, key=lambda value: tuple(map(int, value.split("."))))
    def matching_version(path):
        depfile = path.with_name(path.stem.removeprefix("lib") + ".d")
        return depfile.exists() and f"/{package_name}-{version}/" in depfile.read_text()
    candidates = [path for path in candidates if matching_version(path)]
    if not candidates:
        raise RuntimeError(f"Missing built dependency {name} {version}; run cargo build")
    return max(candidates, key=lambda path: path.stat().st_mtime)


def compile_probe(source, executable, dependencies):
    command = ["rustc", "--edition=2024", "-O", "-A", "dead_code",
               "-L", f"dependency={DEPS}"]
    for name in dependencies:
        command += ["--extern", f"{name}={dependency(name)}"]
    command += [str(source), "-o", str(executable)]
    subprocess.run(command, check=True)


def run(executable, *args, env=None):
    result = subprocess.run([str(executable), *map(str, args)], text=True,
                            capture_output=True, check=True, env=env)
    return result.stdout.strip()


def main():
    observations = {}
    with tempfile.TemporaryDirectory(prefix="tiptoptyp-audit-probes-") as folder:
        temporary = Path(folder)
        for name, dependencies in [
            ("search", ["regex_automata"]),
            ("alpha", ["ecolor"]),
            ("project_index", ["typst_syntax"]),
        ]:
            text = (HERE / f"{name}_probe.rs").read_text()
            if name != "alpha":
                text = re.sub(r'#\[path = "[^"]+"\]',
                              '#[path = ' + json.dumps(str(ROOT / "src" / f"{name}.rs")) + ']',
                              text, count=1)
            source = temporary / f"{name}.rs"
            source.write_text(text)
            executable = temporary / name
            compile_probe(source, executable, dependencies)
            if name == "project_index":
                fixture = temporary / "index-fixture"
                fixture.mkdir()
                observations[name] = run(executable, fixture.resolve())
            else:
                observations[name] = run(executable)

        for name in ["private_workspace", "compiler"]:
            text = (ROOT / "src" / f"{name}.rs").read_text()
            text += (HERE / f"{name}_probe_appendix.rs").read_text()
            (temporary / f"{name}_probe.rs").write_text(text)
        shutil.copy2(HERE / "private_workspace_driver.rs", temporary / "private_main.rs")
        executable = temporary / "private_probe"
        compile_probe(temporary / "private_main.rs", executable, ["tempfile"])
        observations["private_workspace"] = run(executable)

        shutil.copy2(HERE / "compiler_driver.rs", temporary / "compiler_main.rs")
        executable = temporary / "compiler_binary"
        compile_probe(temporary / "compiler_main.rs", executable,
                      ["eframe", "quick_xml", "image", "tempfile"])
        # A fresh real PDF gives render_pdf valid bytes; the child PATH then
        # deliberately contains no rasterizer. Typst is required for this probe.
        source = temporary / "fixture.typ"
        source.write_text("= Audit fixture\nCanonical PDF bytes exist.\n")
        pdf = temporary / "fixture.pdf"
        subprocess.run(["typst", "compile", str(source), str(pdf)], check=True)
        empty_path = temporary / "empty-path"
        empty_path.mkdir()
        observations["pdf_without_poppler"] = run(
            executable, pdf, env={**os.environ, "PATH": str(empty_path)})
    print(json.dumps(observations, indent=2))


if __name__ == "__main__":
    main()
