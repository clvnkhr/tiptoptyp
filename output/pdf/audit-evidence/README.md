# Audit evidence

The report source is `../repository-audit.typ`; its compiled counterpart is
`../repository-audit.pdf`. These files record an audit, not application fixes.

## Recorded checkout

- Commit: `10bd0295c066e7866a066c115f9e76e3a7f9be63`.
- `metrics.json` records SHA-256 hashes for all 32 measured Rust files.
- 38,090 physical Rust lines; 29,328 before inline test modules; 8,762 in
  trailing inline test modules or standalone integration-test files.
- 145 declared fields in `EditorApp`, including platform-conditional fields.
- Existing source edits were included, not overwritten. Their content was
  unchanged when the checkout advanced from `bb9fbf4` to the recorded commit.

## Checks

`fmt.log`, `clippy.log`, `test.log`, and `xtask-test.log` contain the required
checks. Empty `fmt.log` means a successful check with no output. The application
test command passed 376 executions and ignored two; xtask passed seven. Some
inline tests execute again in the integration binaries.

## Probes

Run from the repository with Python 3, Rust/Cargo build artifacts, and Typst:

```sh
python3 output/pdf/audit-evidence/run_rust_probes.py
python3 output/pdf/audit-evidence/gallery_rollback_probe.py
```

The Rust runner reuses existing `target/debug/deps` artifacts (or
`CARGO_TARGET_DIR`), selects rlibs for the newest locked version of each required
crate (checking Cargo depfiles), and compiles probe
executables in a temporary directory. If artifacts are absent or incompatible,
run `cargo build` before retrying. It does not edit application source. The
private-workspace/compiler probe appendices are appended to temporary copies of
the current modules; driver files alone are not standalone executables. The
alpha probe copies the exact audited helper and supplies a white background.
The other editor probes import the repository modules, with paths adjusted by
the runner. Probe sources preserve the original minimal experiments; they are
audit evidence, not maintained regression tests.

The original observed search times were 169 microseconds (case-sensitive) and
366.8 milliseconds (case-insensitive) for 50,000 `a` characters and a query of
999 `a`s plus `b`. The original concurrent-open probe failed 574 of 800 calls.
Reruns vary with scheduling/builds and are stored separately in
`rust-probes-rerun.json`; these numbers are not an application latency or user
failure-rate estimate.

The original PDF probe used an existing valid 20,138-byte PDF; the reusable
runner creates a fresh fixture, so its byte count differs. Both deliberately
remove Poppler from child PATH after the PDF exists.

`gallery-probe.log` records the exact transaction-function extraction and
fault-injected failure on the second move. The probe touches temporary files
only. `cli-probe.log` records the observed invalid-scene exit status. The
contradictory screenshot-option panic path, preference persistence path,
cross-volume write failure, and sidecar startup/liveness risks are static
findings and were not exercised against user state.

Final PDF pages were rendered with Poppler and visually inspected. No native
application screenshots or native composition QA were performed for this
report-only task.
