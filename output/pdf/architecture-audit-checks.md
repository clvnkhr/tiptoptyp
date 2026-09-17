# Architecture audit verification

Audit date: 17 September 2026.
Source HEAD: `27f644cf8752fa75731e9003350f26f245016d6e`.
Host: macOS 14.6.1, Apple M2 Max, arm64, Rust 1.96.0.

This report-only task did not modify production code or `todo.typ`. Existing
resource-audit artifacts and user edits were preserved. No new app profiling
or remote CI run was performed; the resource report remains the measurement
companion, with its own limitations and preserved evidence.

## Required checks

All completed successfully:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --no-fail-fast --quiet
cargo test --manifest-path xtask/Cargo.toml
```

The application binary suite reported 787 passed and eight ignored; the core
unit suite reported 59 passed and two ignored. Other integration/doc suites
passed. The separate xtask suite reported 13 passed. The opt-in native window
drag harness was skipped, as expected without its explicit environment flag.
This is not a claim of native desktop interaction coverage.

## Report checks

```sh
typst compile output/pdf/architecture-audit.typ output/pdf/architecture-audit.pdf
pdfinfo output/pdf/architecture-audit.pdf
pdftoppm -scale-to 1400 -png output/pdf/architecture-audit.pdf tmp/pdfs/architecture-audit/page
```

The PDF has 19 A4 pages. Every page was rendered and visually inspected;
modified pages were re-rendered and inspected after final revisions.
Temporary page renders were removed after inspection. The source and PDF
remain in `output/pdf/`.

## Scope of evidence

The report reviews first-party module/dependency structure, key source paths,
existing architecture notes, public state-machine contracts and the configured
delivery/test system. It distinguishes current source facts, structural risks
and proposed rewrites. It does not claim exhaustive auditing of every source
line, third-party implementation, security behavior or platform interaction.
