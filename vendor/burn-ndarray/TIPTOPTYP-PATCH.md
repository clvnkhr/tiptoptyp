# Local burn-ndarray dependency patch

Source: crates.io burn-ndarray 0.19.1, the previously locked release.
Archive: https://crates.io/api/v1/crates/burn-ndarray/0.19.1/download
Registry archive SHA-256: `b78bcf4a3508043342f918e796dc79108b5f3252398403eb73952847e7683374`.
Upstream VCS commit: `a6da424feacf6d82d6a0e81db197310796267c2b`; path: `crates/burn-ndarray`.

Replace the unmaintained `paste` dependency with maintained `pastey` 0.2.3, updating the actual dependency name and Rust import paths. The macro remains named `paste!`, as provided by pastey. There is no compatibility alias. Runtime APIs, grammar rules, numerical kernels and tokenization behavior are otherwise unchanged.

Both normalized and original manifests are kept consistent. Published source, resources, tests and license files are retained. Upstream package lockfiles are omitted: these library patches are resolved by the app's root Cargo.lock. The VCS receipt is retained unchanged.

Validation targets: the app's default workspace tests compile and exercise the patched consumers. Root integration tests in `tests/harper_archive.rs` cover Unicode Weirpack round trips, dictionary metadata and legacy CP437 filenames for the Zip API adjustment. These tests run in normal CI without requiring upstream package dev-dependency graphs. The dependency policy must pass with no new advisory exceptions.
