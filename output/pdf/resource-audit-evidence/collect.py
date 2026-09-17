"""Read-only resource audit of an isolated, self-terminating app fixture.

Does not modify app sources/settings or operate existing user windows.
Generated workspaces and all measurements are retained beside this script.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[3]
BASE = Path(__file__).resolve().parent


def command(args, timeout=30):
    result = subprocess.run(args, text=True, capture_output=True, timeout=timeout)
    return {"command": args, "exit": result.returncode,
            "stdout": result.stdout, "stderr": result.stderr}


def save(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def process_table():
    text = command(["ps", "-axo", "pid=,ppid=,rss=,time=,comm="])["stdout"]
    rows = {}
    for line in text.splitlines():
        fields = line.split(None, 4)
        if len(fields) == 5:
            pid, ppid, rss, cpu, name = fields
            rows[int(pid)] = {"pid": int(pid), "ppid": int(ppid), "rss_kib": int(rss),
                              "cpu_time": cpu, "command": name}
    return rows


def descendants(rows, root):
    selected = {root}
    while True:
        expanded = selected | {pid for pid, row in rows.items() if row["ppid"] in selected}
        if expanded == selected:
            return [rows[pid] for pid in sorted(selected) if pid in rows]
        selected = expanded


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(scenario):
    stamp = datetime.datetime.now().strftime("%Y%m%d-%H%M%S")
    directory = BASE / f"{stamp}-{scenario}"
    directory.mkdir()
    workspace = directory / "workspace"
    workspace.mkdir()
    (workspace / "typst.toml").write_text("# Isolated resource-audit project.\n")
    source = ROOT / "docs/ui-snapshots/theme-fixture.typ"
    document = workspace / "main.typ"
    if scenario.startswith("pdf-"):
        count = int(scenario.split("-")[1])
        document.write_text('#set page(paper: "a4")\n' +
                            '\n#pagebreak()\n'.join(f'= Resource audit page {i+1}\nA fixed, simple page.' for i in range(count)))
        compile_result = command([str(ROOT / "toolchain/bin/typst-aarch64-apple-darwin"),
                                  "compile", str(document), str(workspace / "fixture.pdf")])
        save(directory / "fixture-compile.json", compile_result)
        if compile_result["exit"]:
            raise RuntimeError(compile_result)
        # Open the Typst source: the deterministic main scene waits for a
        # document preview, not an asset-tab PDF preview.
    elif scenario == "large":
        document.write_text("= Resource audit\n" + "\n".join(
            f"// line {i}: bounded profiling fixture; Unicode α β γ." for i in range(5000)))
    else:
        document.write_bytes(source.read_bytes())
    binary = ROOT / "target/profiling/tiptoptyp"
    metadata = {"scenario": scenario, "started": datetime.datetime.now().astimezone().isoformat(),
                "binary": str(binary), "binary_sha256": sha(binary), "fixture_sha256": sha(document),
                "build_command": "cargo build --locked --profile profiling --features profiling --bin tiptoptyp --config 'build.rustflags=[\"-C\", \"force-frame-pointers=yes\"]'",
                "warmup_seconds": 8, "measurement_seconds": 10,
                "source_revision": command(["git", "-C", str(ROOT), "rev-parse", "HEAD"])["stdout"].strip(),
                "source_status": command(["git", "-C", str(ROOT), "status", "--short"])["stdout"],
                "source_hashes": {str(p.relative_to(ROOT)): sha(p) for base in [ROOT / "src", ROOT / "core/src"] for p in base.rglob("*.rs")},
                "host": command(["sw_vers"]), "cpu": command(["sysctl", "-n", "machdep.cpu.brand_string", "hw.memsize"]),
                "rustc": command(["rustc", "-Vv"]),
                "tool_hashes": {p.name: sha(p) for p in [ROOT / "toolchain/bin/typst-aarch64-apple-darwin", ROOT / "toolchain/bin/tinymist-aarch64-apple-darwin"]}}
    env = {k: v for k, v in os.environ.items() if not k.startswith(("TIPTOPTYP_", "GIT_"))}
    env.update({"TIPTOPTYP_PROFILE_DIR": str(directory), "TIPTOPTYP_PROFILE_WARMUP": "8",
                "TIPTOPTYP_PROFILE_SECONDS": "10", "TIPTOPTYP_PROFILE_MULTI_WINDOW": str(int(scenario == "multi-window")),
                "TIPTOPTYP_PROFILE_NO_WINDOW": str(int(scenario == "no-window")),
                "TIPTOPTYP_TYPST": str(ROOT / "toolchain/bin/typst-aarch64-apple-darwin"),
                "TIPTOPTYP_TINYMIST": str(ROOT / "toolchain/bin/tinymist-aarch64-apple-darwin"),
                "GIT_CEILING_DIRECTORIES": str(directory)})
    args = [str(binary), "--ui-theme", "catppuccin-latte", "--ui-snapshot-scene",
            "settings-window" if scenario == "settings" else "main", str(document)]
    metadata["app_command"] = args
    before = process_table()
    with (directory / "app.log").open("w") as log:
        app = subprocess.Popen(args, cwd=workspace, env=env, stdout=log, stderr=subprocess.STDOUT,
                               start_new_session=True)
        metadata["pid"] = app.pid
        save(directory / "metadata.json", metadata)
        print(f"START {scenario} pid={app.pid} directory={directory}", flush=True)
        try:
            deadline = time.monotonic() + 120
            while not (directory / "ready").exists():
                if app.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("app did not reach capture-ready; see app.log")
                time.sleep(0.2)
            time.sleep(8)
            rows = process_table()
            family = descendants(rows, app.pid)
            candidates = [row for pid, row in rows.items() if pid not in before and "WebKit" in row["command"]]
            save(directory / "family-start.json", family)
            save(directory / "unattributed-new-webkit.json", candidates)
            pids = [row["pid"] for row in family]
            save(directory / "footprint.json", command(["/usr/bin/footprint", "--noCategories", "-f", "bytes"] +
                 [arg for pid in pids for arg in ["-p", str(pid)]]))
            save(directory / "vmmap.json", command(["/usr/bin/vmmap", "-summary", str(app.pid)]))
            start = time.monotonic()
            save(directory / "cpu-start.json", descendants(process_table(), app.pid))
            sampled = command(["/usr/bin/sample", str(app.pid), "4", "1", "-file", str(directory / "cpu.sample.txt")], timeout=15)
            save(directory / "sampler.json", sampled)
            save(directory / "cpu-end.json", descendants(process_table(), app.pid))
            save(directory / "cpu-interval.json", {"seconds": time.monotonic() - start,
                 "note": "brackets ps, four-second native sample and sample analysis; includes profiler perturbation"})
            app.wait(timeout=35)
            metadata["exit_code"] = app.returncode
        except Exception as error:
            metadata["error"] = str(error)
            if app.poll() is None:
                os.killpg(app.pid, signal.SIGTERM)
                try:
                    app.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(app.pid, signal.SIGKILL)
                    app.wait()
        finally:
            save(directory / "metadata.json", metadata)
    print(f"END {scenario} exit={metadata.get('exit_code')} error={metadata.get('error')}", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("scenario", choices=["main", "settings", "large", "multi-window", "no-window", "pdf-1", "pdf-20"])
    run(parser.parse_args().scenario)
