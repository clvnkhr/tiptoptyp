# Local harper-core dependency patch

Source: crates.io harper-core 2.13.0.
Archive: https://crates.io/api/v1/crates/harper-core/2.13.0/download
Registry archive SHA-256: `1d88acd51ecd32aa6a7fcf1fd9f858bbbe507a935bcf31ddd833b454ae804a69`.
Upstream VCS commit: `f4ebc61d77df1fc00b6aa8bc604e91af3534dad1`; path: `harper-core`.

Replace the unmaintained `paste` dependency with maintained `pastey` 0.2.3, updating the actual dependency name and Rust import paths. The macro remains named `paste!`, as provided by pastey. There is no compatibility alias. Beyond upstream 2.13.0's changes, runtime APIs, grammar rules, numerical kernels and tokenization behavior are unchanged by this dependency patch.

Zip is upgraded from 8.6.0 to 9.1.2, fixing RUSTSEC-2026-0367, RUSTSEC-2026-0368 and RUSTSEC-2026-0369. Its filename API now returns a Result; the in-memory Weirpack reader handles it with `?`. Unicode and legacy CP437 filenames remain covered. Archive extraction is not introduced.

Both normalized and original manifests are kept consistent. Published source, resources, tests and license files are retained. Upstream package lockfiles are omitted: these library patches are resolved by the app's root Cargo.lock. The VCS receipt is retained unchanged.

Validation targets: the app's default workspace tests compile and exercise the patched consumers. Root integration tests in `tests/harper_archive.rs` cover Unicode Weirpack round trips, dictionary metadata and legacy CP437 filenames for the Zip API adjustment. These tests run in normal CI without requiring upstream package dev-dependency graphs. The dependency policy must pass with no new advisory exceptions.

License: Apache-2.0. The published crate declares this license but omits its text; LICENSE is verified against the upstream repository at the recorded VCS commit: https://raw.githubusercontent.com/Automattic/harper/f4ebc61d77df1fc00b6aa8bc604e91af3534dad1/LICENSE.
