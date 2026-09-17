# Local TeX completion (item 168)

TeX command completion uses the existing editor completion popup and keyboard
controls. Typing a backslash or command prefix requests local suggestions;
explicit completion uses the same path even when Tinymist is unavailable.

Supported contexts are `tex`/`latex` raw blocks and the embedded literals already
recognized for MiTeX highlighting: `mi`, `mitex`, `mimath`, `mitex-convert`, and
`mitext`, including qualified names, named `input:` arguments, raw literals and
escaped Typst strings. Ordinary Typst, Markdown and unrelated raw languages keep
their existing completion behavior.

The built-in command vocabulary is ranked by math/text context, not restricted
to it. Math commands such as `\alpha` remain available in text-classified raw
blocks, and text commands remain available in math. Document macros and
context-appropriate commands win equally matching suggestions. It recognizes
dollar math, `\(...\)`, `\[...\]`, common math environments, and nested text
arguments such as `\text{...}`. Comments, inline verbatim and common verbatim
environments do not offer command suggestions. Completions replace the entire
command name, preserving its existing backslash and surrounding text. Unicode
and Typst string escapes retain their original source coordinates.

Document-local definitions are indexed across recognized TeX fragments. The
supported declarations include `\newcommand`, `\renewcommand`,
`\providecommand`, `\DeclareRobustCommand`, the `\NewDocumentCommand` family,
`\DeclareMathOperator`, `\def`/`\gdef`/`\edef`/`\xdef`, and `\let`.
Suggestions are lexical discoveries, not proof that a macro is available at
runtime: this does not expand macros, resolve TeX group scope, read imported
files/packages, interpret catcode changes, or execute TeX. It inserts command
names, not argument templates, and does not complete environment names.
The built-in vocabulary is intentionally finite rather than a complete package
catalog. Definition syntax follows the
[LaTeX author guide](https://www.latex-project.org/help/documentation/usrguide.pdf).

## Follow-ups 171–172: suggestions and block Enter

The initial math/text filter was too restrictive: a tagged TeX block starts in
text mode, so it hid `\alpha` and other math commands unless a math delimiter
had already been recognized. Both vocabularies are now offered. Context changes
the ordering of equally matching candidates, while prefix/fuzzy matching still
comes first. Comments and verbatim exclusions remain unchanged.

With automatic delimiter pairing enabled, Enter after an opening fence of at
least three backticks and an optional language creates a blank body line and a
matching closing fence. The body and closing fence retain the opening line's
indentation. This also works in a MiTeX argument. Existing closing fences are
reused, not duplicated, and typing backticks individually produces the same
result as an already present opening fence.

Enter between an empty Typst dollar pair creates an indented body and puts the
closing dollar on its own line. The body uses the existing leading indentation
plus two spaces, matching Typst's formatter indentation. Both operations leave
the caret in the body and are a single document edit for undo/redo. An open
completion popup does not consume this block-opening Enter; Tab still accepts
its selected suggestion.

Syntax-context checks exclude comments, strings, existing raw contents and
closing fences. Paste/IME input and disabled pairing remain verbatim; character
limits are respected. Pure text/caret and actual TextEdit event tests verify
these behaviors without requiring new screenshot fixtures. Ordinary Enter does
not invoke syntax parsing. No new idle work, timers, workers or per-frame scans
were added; no material overall performance impact is expected. The earlier
microbenchmark below describes the initial item-168 vocabulary, not a new
measurement of this follow-up.

## Performance and checks

The shared incremental Typst syntax tree feeds a lazy, revision-keyed TeX index.
Unchanged requests reuse it; fragment and command lookup use binary search. No
language server, worker, filesystem lookup, font reset, timer or idle repaint was
added. Work on a changed document remains proportional to its syntax/TeX content;
this is not an incremental TeX parser. Source-to-LSP conversion and candidate
construction still have costs on each request.
Raw blocks use direct offsets rather than allocating a per-byte offset table;
only escaped strings need that mapping.

Deterministic tests cover context exclusions, nested modes, document macros,
Unicode/escaped-string replacement, application without Tinymist, and index
reuse over 100 requests with invalidation after a revision change. Existing
completion filtering caps visible candidates at 200.

An opt-in headless microbenchmark compares rebuilding the TeX index on each
request with reusing it, under one release profile and identical fixture:

```sh
cargo test --release --bin tiptoptyp profile_tex_index_reuse -- --ignored --nocapture
```

Both paths exclude Typst parsing, use 1,001 fragments, warm up with ten requests,
and measure 100 requests. This isolates cache overhead, not full typing latency
or native-popup performance. No cross-platform latency guarantee is implied.

Validation environment: 2026-09-16, Apple M2 Max, macOS 14.6.1 (23G93),
`aarch64-apple-darwin`, rustc 1.96.0 (`ac68faa20`, LLVM 22.1.2).
The fixture is 52,012 ASCII bytes and the benchmark excludes viewport/theme
rendering. The final optimized run measured 106.092458 ms for 100 rebuild-per-query
requests versus 32.373292 ms for 100 cached requests (approximately 1.061 versus
0.324 ms/request). This is a comparison of two paths in the same final binary,
not a before/after measurement of the whole application. It does not measure
cold startup, editing-time syntax updates, popup rendering or idle CPU.
Measured test binary: `target/release/deps/tiptoptyp-ba39cdf5a1adca55`, SHA-256
`5e8a23aba3b1546aa80d6ad084ed47a0c515b70b4760dd6afc73e21e085dd450`.
Formatting, strict Clippy, all ordinary tests (714 application tests)
and all 13 xtask tests pass. No completion-popup appearance changed; the existing
popup is reused, with local edit application covered by the application test.
