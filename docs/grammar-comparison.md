# Comparing local grammar models

This developer tool runs the same reviewed prose through Liquid AI's spellchecker,
GECToR and CoEdIT. It writes a local HTML comparison and machine-readable results;
it does not enable a model in the editor or apply corrections to documents.

The first question is whether each model catches real grammar errors without
rewriting correct mathematical prose. Release date, model size and one successful
example are insufficient to choose a checker.

## Manual Liquid checks in the editor

Select prose in a Typst or TeX document, then right-click → **Check selection
with Liquid**. The complete selected source is shown before and after, with
changes highlighted using the Git diff style chosen in Settings (Unified or Side
by Side). **Apply** makes one undoable edit; **Cancel** leaves the source alone. Only the captured selection
is eligible, and editing/replacing its document invalidates the review. The
source-aware projection protects equations, commands, links and omitted markup;
a proposal crossing an ambiguous boundary is rejected as a whole. Writing settings
choose a protected placeholder or separate fragments around math/code/links, and
whether recognized formatting bodies are checked. The review can disclose the
exact submitted text; see [input settings](settings-customization.md#optional-liquid-grammar).

The first check automatically verifies and expands the Liquid model and native
ONNX Runtime helper already bundled with the app. There is no install button,
Python environment, external grammar service or model download. One shared ONNX
graph/tokenizer serves both architectures; each uses its matching native CPU
runtime. Liquid needs macOS 14 on Apple Silicon and macOS 12 on Intel; the editor
itself keeps its existing macOS minimum. Writing settings expose the optional
model/cache directory and 1–8 CPU threads. An empty directory setting uses the
persistent app-data folder. The model loads only for an explicit selected-text
request, never at app startup.

Preparation and correction are local and offline. All document windows share one lazy
native model worker and serialize setup/checks. Compressed and expanded model
files have SHA256 checksums, with integrity checked before loading. Background
work is tracked through shutdown, and the loaded model remains resident for
subsequent selections. This optional manual action remains independent of
background Harper checks. The universal bundle includes one compressed model/tokenizer and both native
runtimes; only the matching runtime is prepared for use. See the
[deployment measurements](performance.md) for storage and inference memory costs.

Each request permits at most 16,384 selected characters, 64 prose fragments and
4,096 projected characters per paragraph. The current tagger has a 128-model-token
limit per fragment/refinement pass; long passages receive a shorter-selection
error rather than silent truncation. Proposals may include spelling, punctuation
and casing as well as grammar. Review is necessary: the comparison below records
changes to correct prose too. Model quality has not been improved or fine-tuned
by this integration.

## Recorded comparison — 20 October 2026

Open the [side-by-side report](grammar-comparison/2026-10-20/report.html) or inspect
[the complete outputs and measurements](grammar-comparison/2026-10-20/results.json).
[GECToR reference packing and output checks](grammar-comparison/2026-10-20/reference-parity.json)
are retained alongside them.

The run completed all 192 case/model combinations with no missing predictions or
truncation. Environment: Apple M2 Max, macOS 14.6.1, Python 3.12.14, CPU FP32,
four threads, one model per process, three measurements per case after two
warmups. Weights were already cached; downloads are excluded. Inference latency
includes preprocessing and correction passes, and does not include model loading.

| Model/configuration | Exact fixes / 32 errors | Correct prose changed / 32 controls | Median inference | p95 inference |
| --- | ---: | ---: | ---: | ---: |
| Liquid 350M, tagger only | 23 | 1 | 233 ms | 459 ms |
| Official GECToR-2024 RoBERTa-large | 18 | 0 | 103 ms | 222 ms |
| Community CoEdIT Small | 19 | 3 | 76 ms | 115 ms |

Liquid matched more answer-key corrections, including the missing article in
“Let T be bounded operator on this space.” GECToR preserved every correct control
at its published precision settings. CoEdIT Small was fastest in this reference
configuration, but missed all four plain article cases.

Important qualifications:

- One CoEdIT change is merely `Tao’s` becoming `Tao's`. The strict protection test
  flags this as changed protected text; it is **not a misspelt name or damaged
  equation**. The other two changes rewrite the same display-math boundary example
  represented once as TeX and once as Typst.
- Tagger preprocessing normalises word-internal curly apostrophes for Moses and
  restores their source glyph afterward. CoEdIT receives and returns its native
  text. Thus that one cosmetic edit is not a fair basis for a model-quality claim.
- Liquid's single changed control adds “of” to “The norm MATH is finite.” This
  involves the artificial math placeholder, not ordinary plain prose. Both Liquid
  and GECToR preserved all 22 plain-prose controls.
- All three exactly matched only 3 of the 8 projected-math error references.
  Liquid produced three other edits in this group; GECToR produced two. These need
  human interpretation and do not count as exact fixes. For example, Liquid's
  “There are two distinct solutions” is grammatical but differs from the reference
  “There exist two distinct solutions.” Do not equate every unmatched reference
  with an invalid grammatical correction.
- These configurations are not identical operating points: GECToR uses its paper's
  `min_error_probability=0.65` and keep bias `0.1`; Liquid uses `0.5` and `0.0`
  without its optional reranker; CoEdIT uses greedy generation. The report records
  all of these. None was tuned on this corpus.
- These are **unquantized reference runtimes**, not future deployment costs.
  Peak process RSS, including model loading and the runtime, was approximately
  2.38 GiB for Liquid, 3.06 GiB for GECToR and 0.63 GiB for CoEdIT. Measurements
  from this ARM Mac do not establish Intel performance.

This supports testing Liquid further for recall and GECToR for conservatism. It
does not justify enabling either by default yet: the math projection and a larger,
independent set of correct mathematical paragraphs need evaluation first.

## Expanded arXiv run — 20 October 2026

The [expanded side-by-side report](grammar-comparison/2026-10-20/arxiv/report.html)
and [raw outputs](grammar-comparison/2026-10-20/arxiv/results.json) cover all
**94 author-first paper observations**, plus the original 64 baseline cases:
158 inputs, 152 distinct text strings and 474 case/model combinations. Each
prediction was measured three times after two warmups. There were no failed or
truncated requests, and every repeated output was deterministic.

The [checked-in expanded corpus](../scripts/grammar_compare/arxiv-corpus.json)
contains 56 self-contained paper sentences with minimal references prepared before
inference. The other 38 passages—35 excerpts and 3 interpretation-dependent full
sentences—are unscored research observations. They do not enter correction,
miss, or false-alarm denominators. Paper citations and locations remain attached
to every case. The 94 paper candidates contain known errors, so false alarms
still come from the 32 correct baseline controls, not this all-error paper set.

| Model/configuration | Exact references | Grammar complete after assistant review | Correct controls changed | arXiv median inference |
| --- | ---: | ---: | ---: | ---: |
| Liquid 350M, tagger only | 18/56 | 26/56 | 1/32 | 546 ms |
| Official GECToR-2024 RoBERTa-large | 19/56 | 27/56 | 0/32 | 170 ms |
| Community CoEdIT Small | 8/56 | 9/56 | 3/32 | 240 ms |

Exact references are deliberately strict and nonexhaustive. The
[qualitative review](grammar-comparison/2026-10-20/arxiv/qualitative-review.json)
judges additional grammatical variants without changing the answer key. It is
**assistant review, not blinded independent human annotation**. The two taggers
are essentially tied on this small challenge set. Their additional accepted
repairs often change formula spacing: 8 of Liquid's and 5 of GECToR's otherwise
complete alternatives have protected-typography flags. A grammatical repair
with detached accents or subscripts is not a safe editor insertion.

CoEdIT Small's review found 15 outputs with substantive mathematical symbol or
operator loss. Some other flags in the report are only typography, including
flattened mathematical fonts or straight/curly apostrophes; protected-text flags
must be inspected rather than all described as changed mathematical claims.
The ambiguous numbered-region convention remains uncertain for all three models.
The hardest larger constructions remain poorly handled by all three.

This run intentionally preserves the collected normalized mathematical glyphs;
they are **not** hidden behind the original corpus's `MATH` placeholder. Model
quality and math preservation therefore both affect the result. These inputs
are not output from the application's parser and do not validate its extraction
or source mapping. A next integration evaluation should protect mathematical
spans before inference and test correct real mathematical paragraphs as well.

All 192 earlier baseline outputs reproduced exactly. Model revisions, CPU FP32,
four threads, warmup, repeat count and model options match the earlier run on
the same Apple M2 Max/macOS 14.6.1/Python 3.12.14 environment. Weights were cached;
downloads and loading are excluded from inference timing. The table's timing
is for the 94 arXiv observations, not just the 56 scored ones. New paragraphs
and baseline cases have different lengths: do not compare an overall expanded
median with the earlier 64-case median as a performance regression. Runtime
metadata and separate cohort timing are retained in the results. No Rust build
or test suite ran concurrently with these inference measurements.

Reproduce the expanded run with the same environment and cached weights:

```sh
.tiptoptyp/grammar-comparison/venv/bin/python scripts/compare-grammar.py \
  --corpus scripts/grammar_compare/arxiv-corpus.json \
  --models liquid gector coedit \
  --output .tiptoptyp/grammar-comparison/arxiv-new-run \
  --cache-dir /private/tmp/tiptoptyp-grammar-models \
  --threads 4 --repeats 3 --warmup 2
```

## Production Harper inputs — 20 October 2026

The [Harper-interface comparison](grammar-comparison/2026-10-20/harper/report.html)
shows Liquid, GECToR, the current Harper rules and a spelling-only Harper group
side by side. The [complete outputs](grammar-comparison/2026-10-20/harper/results.json)
include diagnostics, suggestion lists and source-edit rejection reasons.
[Verification and timing scopes](grammar-comparison/2026-10-20/harper/verification.json),
[deployment measurements](grammar-comparison/2026-10-20/harper/deployment.json),
and the compressed [raw token export](grammar-comparison/2026-10-20/harper/token-export.json.gz)
retain the supporting evidence.
This is comparison tooling; no model is enabled in the editor and no documents
are corrected automatically.

### What was actually passed in

Harper consumes original source characters plus a masked token stream, rather
than a plain string with equations removed. The opt-in Rust probe replays the
**actual production cache paragraphs**, dictionary normalization and final Harper
`Document` tokens. It asserts that reconstructed diagnostics exactly match the
ones the app publishes. The token export includes sentence boundaries, admitted
prose, hidden source ranges and global Unicode scalar offsets.

ML models require text. The adapter copies admitted prose, collapses whitespace,
omits known formatting wrappers and serializes opaque objects as protected `x`.
Display-equation punctuation stays in the sentence. This is an explicit, lossy
serialization of the same admitted tokens, **not a claim that Harper receives
this string**. An `x` does not retain a formula's meaning or number; closing TeX
syntax can also produce an opaque `x`. These artifacts can affect model output.

The checked-in [source fixtures](../scripts/grammar_compare/harper-fixtures.json)
contain 120 original documents: 40 generated boundary probes, 56 word/context
probes, four short original arXiv TeX excerpts, 18 existing source-backed cases,
and two complete existing manual-test documents. They produce **139 admitted
blocks, 108 distinct strings, 57 error references and 82 unchanged controls**.
Two opaque-only documents are excluded, rather than counted as successful checks.
Paired TeX/Typst cases and fragmented source documents are not independent
observations. This is a developer challenge set, not held-out accuracy research.
The 94 normalized paper observations in the earlier run are not falsely presented
as original TeX; only the four explicitly sourced excerpts have that provenance.

### Corrections and costs

All 278 ML input/model combinations completed; all three repeated predictions
per input were identical. Reference answers and alternatives were fixed before
inference. CPU FP32, four threads, batch size one, two warmups, three measurements,
Apple M2 Max, macOS 14.6.1, Python 3.12.14; no Rust compilation or test suite ran
concurrently. Models, revisions and thresholds match the earlier comparison.

| Checker | Exact references /57 | Unchanged inputs modified /82 | Median / p95 timed check |
| --- | ---: | ---: | ---: |
| Liquid 350M, tagger only | 42 | 13 | 322 / 894 ms |
| GECToR-2024 RoBERTa-large | 31 | 2 | 185 / 406 ms |
| Harper curated, first compatible suggestions | 25 | 6 | See timing scopes below |
| Harper spelling-only, first compatible suggestions | 16 | 3 | See timing scopes below |

Harper currently shows diagnostics and alternatives. For this table only, the
adapter chooses each shown lint's first compatible suggestion in priority order.
This is **not** an app autofix policy, and a useful second suggestion does not
count as an exact first-suggestion repair. Exact references are nonexhaustive:
two Liquid alternatives singularize a subject instead of fixing the verb and
need author review for intended multiplicity. Other nonmatching outputs include
malformed fragments and invented technical words. Such observations are assistant
review, not blinded human annotation; the [28 annotated observations](grammar-comparison/2026-10-20/harper/qualitative-review.json) preserve the reasoning.

Liquid finds more references but is too eager for a quiet grammar-only default.
It changes `ill-posedness` to `illposedness`, and removes the article from “We
minimise the energy.” GECToR is more conservative but still makes an optional
article deletion. Both repair all four original arXiv excerpts; four selected
known-error excerpts are far too few to establish paper-level accuracy.

| Reference runtime cost | Liquid | GECToR |
| --- | ---: | ---: |
| Weight artifact, decimal units | 713 MB | 1.442 GB |
| Required cached model/config/tokenizer files | 718 MB | 1.449 GB |
| Cached load, excluding imports and downloads | 4.44 s | 12.08 s |
| Python/runtime import | 1.26 s | 0.90 s |
| Peak worker RSS including runtime/model | 2,425.5 MiB | 2,895.6 MiB |

The published Liquid weights are FP16 and the GECToR checkpoint is FP32; both
were executed in FP32 here. Their file-size ratio is partly a storage-precision
difference, not twice as many model parameters.

Harper already ships in the app, so it needs no additional model download. Its
memory is not zero; the export test process is not an isolated Harper memory
measurement. Harper's fresh-Checker timing includes parser work, configuration,
cache misses, linting and diagnostic filtering, with a process-resident dictionary.
Its unchanged-cache-hit timings are separate. Spelling-only warm timings use
already-built `Document`s and a reused suggestion cache, excluding parsing,
tagging and filtering. A fresh spelling group is much more expensive to construct;
do not interpret its warm linter figure as end-to-end spelling latency or a
measured speedup over the whole current integration. The median fresh-Checker
check on single-paragraph inputs was 8.73 ms (p95 14.56 ms); across original
documents, the unchanged-cache-hit median was 0.017 ms. The spelling-only warm
linter median was 0.025 ms versus 7.26 ms with a newly configured group. The
first process document, including dictionary initialization, took 535 ms. These
scopes differ; raw samples and contracts are retained in the export and
verification files.

### Isolated bits versus context

The paired cohort deliberately tests the same 28 words alone and in sentences.
Eight are actual typos; the rest are legitimate mathematics, names, UK spelling
or real words whose misuse is visible only in a sentence.

| Isolated input checker | Exact first repairs /8 typos | Valid words modified /20 |
| --- | ---: | ---: |
| Liquid | 5 | 4 |
| GECToR | 2 | 1 |
| Harper spelling-only | 6 | 0 |

Harper actually detects **all eight** isolated typos. For `misspelld`, its first
suggestion is `misspell`, while `misspelled` is second; for `estmates`, `estates`
precedes `estimates`. This exposes a ranking problem, not a missing spelling rule.
Liquid expands valid standalone `form` to `formulation` and `their` to `There is`.
GECToR capitalizes standalone `there`. In the six contextual real-word confusables,
Liquid matches all six references, GECToR and Harper grammar match three each,
and Harper spelling matches none.

A useful product split would therefore be **Off / Spelling / Grammar**, with
Harper spelling for short labels, captions and fragments, and optional ML grammar
for contextual paragraphs after a pause or on demand. Preserve our syntax
admission, dialect, compounds, possessives, names and personal dictionary.
A future word-only dictionary path could avoid Harper's full Document tagging,
but that optimization has not been implemented or measured here. Merely filtering
ML output to spelling edits still incurs the full forward-pass cost; making
one ML request per word loses context and is not a performance solution.

### Source safety and integration findings

Seven of Liquid's 64 changed proposals are rejected by the conservative source
map guard; all 33 GECToR changes are geometrically mappable. Harper curated and
spelling each have one joined-word proposal rejected. Reasons include edits
across omitted formatting syntax, and insertions with ambiguous adjacency to
math or a fragment edge. Zero generic `x`-count flags does not mean zero source
risks. The guard leaves source unchanged if any edit is rejected. Passing it
proves coordinates, **not** grammatical correctness, TeX/Typst syntax safety or
preservation of mathematical meaning. No source file is written.

The frozen export also exposed extraction limitations in the interface used
for this comparison. A subsequent fix now preserves sentence context through
known inline Typst formatting bodies, including `#emph`, `#strong` and `#text`,
while keeping original diagnostic coordinates and actual paragraph boundaries.
The frozen model outputs below have not been rerun with that fix. The later
manual Liquid action above uses the corrected adapter; background ML checking
remains disabled. The remaining limitations are unchanged:

- In the frozen pre-fix export, Typst `These #emph[estimates] is uniform.` becomes `These`, `estimates`, and
  `is uniform.` in three cache blocks. The upstream content-block isolation
  discards the subject context and can cause false capitalization warnings.
  TeX emphasis remains in one sentence. This Typst extraction issue is now fixed
  in the production Harper adapter.
- An unused TeX `\newcommand` replacement body is admitted as prose, so technical
  tokens can receive spelling warnings.
- TeX text inside math is admitted; Typst `text(...)` inside an equation is wholly
  masked, and therefore cannot be corrected by any checker using these inputs.
- Unknown Typst function positional strings are not necessarily technical noise:
  the fixture function returns its string, so admitting it can be correct.

Display-math commas, semicolons, colons and periods continue the surrounding
prose in the inspected fixtures. The parser preserved mathematical compounds as word units; all four checkers
preserved the selected Tao/Barbato/Romito possessive/name controls. Those successes do
not remove the remaining extraction and math-text coverage gaps above.

### Native Liquid export and packaging

The editor runs Liquid's tagger in a persistent, private native **ONNX Runtime**
helper. This is Microsoft's C/C++ engine, distinct from the earlier Tract
experiment. The portable graph and tokenizer are bundled once with two thin CPU
runtimes: ONNX Runtime 1.31.0 for Apple Silicon (macOS 14+) and 1.19.2 for Intel
(the library itself needs macOS 11; our helper keeps the app's macOS 12 floor).
Intel 1.20.1 requires macOS 13.3 and 1.23.2 requires 13.4, so those newer Intel
binaries would raise the existing deployment requirement. No Python interpreter,
Python packages, remote model code or optional Liquid reranker is shipped or
executed. Preparation is automatic on the first selected-text check and stays
off the UI thread.

Original trained weights are retained exactly: FP16-representable initializers
are compacted only when the FP32 → FP16 → FP32 round trip preserves every bit;
other constants remain FP32. Explicit casts restore FP32 CPU arithmetic. This is
not lossy quantization. The graph is approximately 714 MB and the tokenizer
4.7 MB before compression. The reviewed model matched all 268 canonical inputs
and all 427 refinement passes' best edit IDs and applied edits against the
original PyTorch reference; maximum detection-probability difference was
0.00000594. Both native helpers also passed all 427 raw passes, 136 dynamic
length/transition checks and eight invalid-input/recovery checks. Intel was
executed under Rosetta; this does not establish physical Intel performance.
Actual deployment speed, cold-load time, storage and memory are recorded in the
[performance report](performance.md).

To reproduce the model export, create a disposable Python 3.12 environment on
Apple Silicon. Python/PyTorch are **build-time reference tools only**; neither is
installed by the editor or needed on users' Macs. The explicit reference setup
below downloads checksum-pinned upstream model files:

```sh
python3.12 -m venv /private/tmp/liquid-onnx-build
/private/tmp/liquid-onnx-build/bin/python -m pip install -r scripts/liquid-onnx-requirements.txt
/private/tmp/liquid-onnx-build/bin/python scripts/liquid-grammar-worker.py \
  --cache-dir /private/tmp/liquid-reference --install
/private/tmp/liquid-onnx-build/bin/python scripts/export-liquid-onnx.py \
  --cache-dir /private/tmp/liquid-reference \
  --output-dir /private/tmp/liquid-onnx-raw --export
/private/tmp/liquid-onnx-build/bin/python scripts/export-liquid-onnx.py \
  --cache-dir /private/tmp/liquid-reference \
  --output-dir /private/tmp/liquid-onnx-raw --verify
python3 scripts/build-liquid-onnx.py \
  --source /private/tmp/liquid-onnx-raw \
  --work-dir /private/tmp/liquid-onnx-vendor
cargo xtask prepare-liquid --source /private/tmp/liquid-onnx-raw
cargo xtask stage-liquid
```

Export and verification are offline. The builder runs on Apple Silicon with
Rosetta and Xcode tools, pins official native ONNX Runtime archives by SHA256,
builds the checked-in helper for each architecture, normalizes install names and
RPATHs, strips/signs the two-file closures and smoke-tests both loaded-model
helpers. It includes each archive's complete licence and third-party notices;
there are no Python wheels or environments in the payload. The ARM archive has
a publisher-provided digest; the older Intel release's pin is the locally
observed hash of the reviewed official HTTPS download. `--offline` reuses the
checksum-verified archives already in the developer work directory.

`prepare-liquid` verifies the raw model/tokenizer, exact canonical 268-input /
427-pass parity and native provenance before compressing the payload.
`stage-liquid` checks compressed and expanded files and licences before staging
under `toolchain/liquid`. The strict raw tree has ten files; build-only export
and parity receipts stay in a sibling `OUTPUT-evidence` folder. These generated
payloads are ignored by Git. Rebuilding from prepared assets requires neither
Python nor model downloads. See the scripts' `--help` for the same commands.

The earlier [Tract source/evidence archive](performance-results/liquid-tract-2026-10-20-source.tar.gz)
and [native PyTorch source/evidence archive](performance-results/liquid-pytorch-2026-10-20-source.tar.gz)
remain comparison experiments. Their inactive runtime/tooling has been removed;
old format-1 and format-2 payloads are rejected by current packaging. Tract and
ONNX Runtime are different engines and their timings must not be interchanged.

Quantization remains a separate experiment: it can change corrections and must
be revalidated. Compression reduces distribution/storage, not inference RAM.
Liquid's [LFM Open License](https://www.liquid.ai/lfm-license), ONNX Runtime's MIT
licence, complete third-party notices and conversion provenance ship with the
payload. GECToR remains a comparison-only model; its
[reference code is Apache-2.0](https://github.com/grammarly/gector/blob/master/LICENSE),
but a separate redistribution licence for its
[2024 checkpoint](https://github.com/grammarly/pillars-of-gec) was not found.

### Reproduce the Harper-interface run

```sh
python3 scripts/compare-harper-grammar.py request \
  --output /private/tmp/harper-request.json --samples 3
TIPTOPTYP_GRAMMAR_PROBE_INPUT=/private/tmp/harper-request.json \
TIPTOPTYP_GRAMMAR_PROBE_OUTPUT=/private/tmp/harper-export.json \
  cargo test --release --bin tiptoptyp \
  writing::cache::probe::export_production_harper_fragments \
  -- --ignored --exact --nocapture --test-threads=1
python3 scripts/compare-harper-grammar.py prepare \
  --fixtures scripts/grammar_compare/harper-fixtures.json \
  --probe /private/tmp/harper-export.json \
  --corpus .tiptoptyp/grammar-comparison/harper-new/corpus.json \
  --sidecar .tiptoptyp/grammar-comparison/harper-new/sidecar.json
.tiptoptyp/grammar-comparison/venv/bin/python scripts/compare-grammar.py \
  --corpus .tiptoptyp/grammar-comparison/harper-new/corpus.json \
  --models liquid gector --threads 4 --repeats 3 --warmup 2 \
  --cache-dir /private/tmp/tiptoptyp-grammar-models \
  --output .tiptoptyp/grammar-comparison/harper-new-ml
python3 scripts/compare-harper-grammar.py finish \
  --results .tiptoptyp/grammar-comparison/harper-new-ml/results.json \
  --sidecar .tiptoptyp/grammar-comparison/harper-new/sidecar.json \
  --output .tiptoptyp/grammar-comparison/harper-new-report
```

Build before measuring; for timing use the compiled test binary directly so no
compiler runs concurrently. `request` includes expected and accepted source
variants; `prepare` verifies source equality, scalar geometry and paragraph
alignment; `finish` verifies corpus identity before attaching Harper outputs.
The exporter is test-only and introduces no runtime work, UI changes or model
payload into the app. The deterministic Python and Rust tests cover those adapter
contracts; no native interaction or screenshot evidence is claimed.

## Run it

Validate the checked-in corpus without downloading models or installing Python
packages:

```sh
python3 scripts/compare-grammar.py --validate
```

Create an isolated environment and install the model runner dependencies. The
reference run uses Python 3.12; model packages are separate from the editor:

```sh
python3.12 -m venv .tiptoptyp/grammar-comparison/venv
.tiptoptyp/grammar-comparison/venv/bin/python -m pip install -r scripts/grammar_compare/requirements.txt
```

Run all three models on CPU, with a shared model cache outside the repository:

```sh
.tiptoptyp/grammar-comparison/venv/bin/python scripts/compare-grammar.py \
  --models liquid gector coedit \
  --output .tiptoptyp/grammar-comparison/run-20261020 \
  --cache-dir /private/tmp/tiptoptyp-grammar-models \
  --threads 4 --repeats 3 --warmup 2
```

The first run downloads model files and may take considerably longer than later
runs. Downloads and model loading must not be interpreted as per-sentence latency.
Use a new output directory when comparing configurations so the earlier evidence
remains available. The runner's model metadata, options and runtime measurements
are part of the result, not an assumption based on the model's name.

Open `report.html` in the output directory. The corresponding `results.json` and
`per-worker/*.json` retain the observations for inspection. A failed or unavailable
model is reported as such; it is not a successful unchanged prediction.

For a quick first run, select one model and a few cases:

```sh
.tiptoptyp/grammar-comparison/venv/bin/python scripts/compare-grammar.py \
  --models coedit --limit 8 \
  --output .tiptoptyp/grammar-comparison/coedit-smoke
```

Use `--category article` or `--case tex-display-clause` to investigate a specific
kind of error. Rebuild an HTML report from saved results without loading models:

```sh
python3 scripts/compare-grammar.py \
  --report-only .tiptoptyp/grammar-comparison/run-20261020/results.json \
  --output .tiptoptyp/grammar-comparison/review
```

## What the corpus measures

`corpus.json` contains **64 manually reviewed examples**: 32 should remain exactly
unchanged and 32 require corrections. There are 46 plain-prose examples, 10 TeX
examples and 8 Typst examples. Paired TeX/Typst cases sometimes share the same
projected input: there are **58 unique input sentences**, not 64 independent
observations. Required edits cover articles, agreement, verb
forms, number, possessives and repeated words. Correct examples deliberately
include British spelling, mathematical compounds, names, straight and curly
apostrophes, numbers and legitimate word repetition.

This is a small developer regression and comparison set. It is not held out from
our design decisions, is not a representative scientific benchmark, and does not
establish general accuracy. Do not tune on these examples and then report the same
examples as independent evidence of model quality. If we fine-tune a model, keep
an independently collected evaluation set separate from training and development.

Every model receives the case's `text`. For TeX and Typst cases, `source` shows the
original markup for human inspection; it is never passed to the model. These
`text` fields are manually reviewed prose projections, not output from the app's
parser. They join prose around inline/display math, retain sentence punctuation
from equations, and retain natural language inside math text commands. They
therefore compare models under a controlled projection and **do not prove that
our existing TeX/Typst extraction or source mapping works**.

A formula is replaced with the protected literal `MATH`. This placeholder is not
normal mathematical English and may itself affect predictions. It also loses
formula meaning, including singular/plural information. The corpus avoids asking
models to infer agreement from the hidden formula and includes plain-language
controls for key boundary cases. Inspect plain and projected results separately;
a placeholder failure alone is not proof that a model cannot handle the original
sentence with a better projection.

The answer key favours the smallest required correction. A few explicit
`accepted` alternatives allow equally grammatical forms such as `proved` and
`proven`. Exact matching is deliberately strict: a different valid correction can
still miss the answer key. Review disagreements before classifying them as bugs.
British-to-American spelling conversion and optional stylistic rewrites are
unwanted changes for this comparison.

## Reading the results

Evaluate the measures together:

- **Preservation of correct prose:** how often a model leaves unchanged examples
  alone. This is essential for a quiet background checker.
- **Exact corrections:** how often a model matches a reviewed target for an
  erroneous example. A partial correction or extra rewrite needs inspection.
- **Protected text:** whether names, numbers, references or math markers changed.
  These checks flag damage rather than making a model inherently safe.
- **Latency:** compare warm runs under the same thread count, machine, model
  revision and settings. Keep cold loading separate. This sentence-level set is
  not an editor responsiveness or long-document benchmark.

A single accuracy percentage hides the difference between a model that preserves
all prose but catches nothing and one that fixes errors by rewriting everything.
These sentence-level scores are not token-level precision/recall or ERRANT/M2
benchmark scores. They must not be compared directly with published GEC scores.
Check the individual outputs, especially the unchanged cases, before selecting a
model. Re-run any promising configuration on longer real mathematical paragraphs
and on the app's actual extracted prose before integrating it.

## Editing the corpus

The top-level object has `version: 1`, a `description`, and a `cases` array. Each
scored case has a unique `id`, a `category`, `text`, `expected`, and `format` (`plain`,
`tex` or `typst`). Optional fields are:

- `accepted`: additional complete corrected sentences accepted by the answer key.
- `source`: original markup, for display only.
- `protected`: exact values that must retain spelling, occurrence count and order;
  word-like values also require lexical boundaries.
- `notes`: an explanation of the boundary or intended interpretation.

Omit `expected` only for an explicitly unscored research passage; nonempty
`notes` must explain the limitation, and `accepted` is forbidden. Unscored
outputs remain visible but cannot count as fixes, misses or false alarms.
`cohort` separates baseline and paper results; `kind` distinguishes sentences
from excerpts; `citation` retains a safe HTTP(S) source URL, title and location.

Use `expected == text` for a genuinely correct sentence. Required targets and
accepted alternatives must preserve protected strings. Prefer clear grammatical
errors over disputed usage; add a note if an example has a non-obvious purpose.
Re-run `--validate` after editing. Adding an example also changes the denominator,
so retain the corpus and run metadata with any reported comparison.

The model implementations and weights have separate licences and constraints.
Inspect the pinned adapter metadata and upstream terms before redistributing
weights; this opt-in comparison does not add them to the app bundle.

Upstream references: [Liquid AI's encoder release](https://www.liquid.ai/blog/lfm2-5-encoders),
[Liquid spellchecker](https://huggingface.co/LiquidAI/LFM2.5-Encoder-350M-Spellchecker),
[GECToR](https://github.com/grammarly/gector), and
[CoEdIT](https://huggingface.co/grammarly/coedit-large).

## Harness checks

The fast regression suite uses no ML packages or weights:

```sh
python3 -m unittest discover -s scripts -p 'test_grammar*.py'
```

It covers reference validation, accepted targets, protected words, missing/failed
predictions, reporting, verified downloads and per-model worker behaviour. Real
inference is a separate opt-in check. The editor's runtime, app size, normal
startup and UI behaviour are unchanged by this developer tool.
