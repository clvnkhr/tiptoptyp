# Local tokenizers dependency patch

Source: crates.io tokenizers 0.23.2, the previously locked release.
Archive: https://crates.io/api/v1/crates/tokenizers/0.23.2/download
Registry archive SHA-256: `7afbf6e88718afcc138bad01d6ccc3051dbbc3b2ce9793d8b8a3aeb610969cfc`.
Upstream VCS commit: `88a4498ad4ea1a9487b0a9b0ff881383fd5a06a3`; path: `tokenizers`.

Replace the unmaintained `paste` dependency with maintained `pastey` 0.2.3, updating the actual dependency name and Rust import paths. The macro remains named `paste!`, as provided by pastey. There is no compatibility alias. Runtime APIs, grammar rules, numerical kernels and tokenization behavior are otherwise unchanged.

Both normalized and original manifests are kept consistent. Published source, resources, tests and license files are retained. Upstream package lockfiles are omitted: these library patches are resolved by the app's root Cargo.lock. The VCS receipt is retained unchanged.

Validation targets: the app's default workspace tests compile and exercise the patched consumers. Root integration tests in `tests/harper_archive.rs` cover Unicode Weirpack round trips, dictionary metadata and legacy CP437 filenames for the Zip API adjustment. These tests run in normal CI without requiring upstream package dev-dependency graphs. The dependency policy must pass with no new advisory exceptions.
