# Architecture performance verification — 2026-09-17

The measured workloads show no material regression after the architecture work,
except a missing-import indexing regression found and fixed during this review.
This is local evidence, not a guarantee for every workload or platform.

## Comparison and reproducibility

Baseline: `27f644cf8752fa75731e9003350f26f245016d6e`.
Before fix: `45f11162aec1bfa3d256d747d8884b4745804b60` plus the working-tree
item 207 completeness changes. Fixed: the same sources with per-analysis parent
path normalization caching. Native measurements precede item 202's subsequent
behavior-preserving protocol extraction; that extraction is covered by wire,
borrowed-payload, fake-server, and real-server tests, not a new native timing run.

Environment: Apple M2 Max, arm64, macOS 14.6.1, Rust 1.96.0. Matched optimized
`profiling` builds used `--features profiling` and
`RUSTFLAGS=-Cforce-frame-pointers=yes`, including matching integration-test
feature unification. No builds ran during timed workloads. The user's existing
release app remained open, so these are not isolated laboratory measurements.

Local retained evidence: `.tiptoptyp/profiles/architecture-review.0ysn8Z/`.
`provenance.txt` records commands, binary hashes, toolchain manifest hash, and
run conditions. The directory retains binaries, symbols, and raw probe output.
The baseline worktree is `/tmp/tiptoptyp-architecture-baseline.OeiF44`.

Run the opt-in headless probe with:

```sh
RUSTFLAGS=-Cforce-frame-pointers=yes cargo test --profile profiling --features profiling --test architecture_cost architecture_cost_probe -- --ignored --nocapture
```

Fixtures are created before timing. Each case has five warmups and seven
batches (20 indexing operations or 1,000 framing operations per batch).
The table gives the median of three process medians; process order was reversed
in the middle trial. There are no wall-time CI assertions.

## Headless costs

Microseconds per operation; indexing reads real fixture files.

| Case | Baseline | Before fix | Fixed |
| --- | ---: | ---: | ---: |
| Index 1 file | 27.09 | 23.81 | 25.62 |
| Index 64 files | 1328.48 | 1300.94 | 1303.75 |
| Index 256 files | 5369.39 | 5156.79 | 5092.50 |
| Index 300 files, capped at 256 | 5459.70 | 5485.29 | 5420.99 |
| Index root with 63 missing imports | 636.10 | 1006.49 | 636.68 |
| Read 128-byte LSP payload | 0.782 | 0.781 | 0.783 |
| Write 128-byte LSP payload | 0.335 | 0.417 | 0.395 |
| Read 32-KiB LSP payload | 22.43 | 22.73 | 22.41 |
| Write 32-KiB LSP payload | 11.33 | 11.25 | 11.29 |

The tiny-write aggregate differs by about 0.06 microseconds. Baseline process
medians span 0.331–0.401 microseconds and fixed medians span 0.358–0.399;
the ranges overlap and the serializer body is unchanged. This does not establish
a meaningful UI regression, but should not be reported as literally identical.

Missing siblings previously canonicalized their common parent repeatedly.
The fix caches parent resolutions within one analysis, capped at 256 entries;
existing-file canonicalization still happens first to preserve symlink handling.
A deterministic test checks 64 missing siblings require 65 canonicalization
attempts, verifies the cache cap, and verifies a fresh analysis starts empty.
There is no persistent filesystem cache or idle work.

## Native steady-state workloads

Matched isolated QA fixtures used Catppuccin Latte, 6-second warmup and 6-second
measurement, with macOS CPU sampling at 1 ms. Corresponding fresh framebuffers
were inspected: main 2800×1770; Settings 1000×1120. These establish comparable
fixtures, not composed desktop/native-child geometry.

| Workload | Baseline | Compared version |
| --- | --- | --- |
| Main: process CPU advance | 0.07 s | 0.06 s, before fix |
| Main: sampled footprint | 435.0 MB | 425.5 MB, before fix |
| Four windows: editor passes / total span | 398 / 140.09 ms | 405 / 139.27 ms, fixed |
| Four windows: process CPU advance | 0.42 s | 0.41 s, fixed |
| Four windows: sampled footprint | 736.3 MB | 730.5 MB, fixed |

Sampled Settings runs showed approximately one-second inclusive wall-time
outliers in **both** versions despite little process CPU advance. A reverse-order
repeat with sampling disabled (8-second warmup and measurement for both) had
no such outlier: baseline 148 editor passes / 59.43 ms, fixed 146 / 54.23 ms;
both advanced process CPU by 0.21 seconds. Settings itself ran eight passes in
each, totaling 11.75 versus 12.00 ms. Treat the sampled outliers as inconclusive,
not evidence of either a regression or a speedup.

Run directories under `.tiptoptyp/profiles/`, in baseline/comparison order:

- Main: `1789668037827-80468-main-0`, `1789668089374-80795-main-0`.
- Settings sampled: `1789668316375-82237-settings-0`, `1789668331732-82526-settings-0`.
- Four windows: `1789668347287-82798-multi-window-0`, `1789668363086-83169-multi-window-0`.
- Settings unsampled: `1789668438220-83865-settings-0`, `1789668418963-83566-settings-0`.

All runs completed with zero dropped scopes. Inclusive spans are wall time,
not CPU time, and must not be added across nested scopes. Measurements exclude
child-process and GPU accounting, allocation profiling, cold startup,
typing/scroll latency, long-running leak detection, and exhaustive Git actions.
Those remain separate workloads, not claims established by this review.
