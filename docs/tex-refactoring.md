# TeX command refactoring

Right-click a TeX command and choose **Inline command…** or **Inline all uses in
this file…**. For extraction, select an expression and choose **Extract selection
to command…**, then enter a new command name. Every action opens a source diff.
Accepting applies the complete proposal as one Undo step; Cancel changes nothing.
The original macro definition remains in place after inlining.

The review owns the source document, stable tab, revision and exact source
snapshot. Switching documents or editing the source invalidates that proposal.
An old review cannot apply to a newer source or another tab of the same file.

## Supported syntax

The first implementation handles literal local `\newcommand`, `\renewcommand`
and simple undelimited `\def` definitions with zero to nine consecutive
parameters. LaTeX optional first arguments are supported at the macro call.
Definitions must be unambiguous, unconditional and at document scope. A
`\renewcommand` or `\def` target must be defined after document initialization
and used before any subsequent environment boundary: existing commands can be
reset by begin-document hooks or environment setup. A fresh `\newcommand` can
cross those boundaries within the supported subset. Inlining
uses the definition's token body, substitutes argument tokens with only their
original outer argument braces removed, and preserves every other group.

All uses means direct uses in the current file, outside comments, verbatim,
definitions and other stored-token contexts. It does not edit an included file
or expand a use hidden inside another definition. Nested overlapping uses must
be inlined separately, starting with the inner use when its context is safe.

Extraction requires a complete literal `\begin{document} … \end{document}` file.
The new `\newcommand` goes immediately before `\begin{document}`. The selected
expression must contain complete tokens and balanced groups and remain within
one math/prose context. Names use ASCII letters, optionally preceded by `\`.
Existing local names and known standard commands are rejected. `\newcommand`
also reports any collision supplied by an external package rather than silently
using an existing, different definition. Inserted lines preserve CRLF files.

## Why expansions are not automatically grouped

Given `\newcommand{\term}{xy}`, `\term_i` expands to `xy_i`: the subscript
belongs to `y`. Replacing it with `{xy}_i` would change the mathematics. The same
rule matters inside an unbraced superscript and for a parameterized macro such
as `\newcommand{\square}[1]{#1^2}`. Inlining `\square{ab}` produces `ab^2`,
matching the original macro; it does not invent `{ab}^2`.

The serializer inserts an ignored space between a control word and following
letters when necessary, including Unicode letters. It consumes whitespace that
TeX already discarded after a macro name. It never uses an empty group or
`\relax` as a separator, because both can change argument or script binding.
Comments remove their endline and indentation according to TeX's input state.
Every candidate is re-tokenized and compared with the intended token delta.
Caller comments and `ttt`/`fmt`/`diag` suppression markers are retained in order.
If a comment lies inside an invocation/selection or its original trivia cannot
be preserved with identical tokens, refactoring is refused rather than deleting
or moving the annotation.

## Conservative boundaries

TeX is programmable. A literal source scan cannot establish arbitrary package
code, catcodes, macro mutation or token-reader behavior. The engine therefore
supports an explicit subset of ordinary LaTeX renderers and standard math/text
environments and reports a reason when a proposal cannot be proved safe within
that subset. A refusal leaves the source intact; it never falls back to a text
replacement or silently adds braces.

Refactoring is refused for:

- Local shadowing, repeated definitions, protected/robust or conditional
  definitions, delimited `\def` parameters and unsupported declaration forms.
- An included file, package or auxiliary-file reader between a definition and
  its use, since it can change the command's meaning.
- Hidden definitions, imports, environment wrappers, assignments, hooks or
  token-inspection effects in stored local macro bodies and optional defaults.
  Unknown control words and symbols in those bodies are also refused.
- Unknown executing commands or environments whose argument or execution
  behavior is unavailable from the supported subset.
- Raw token-reading fields such as labels, paths and `\detokenize`, moving
  arguments, optional `[…]` fields, and unbraced arguments to other commands.
  For example, `\textbf\term` is not interchangeable with `\textbf xy`.
- Optional-reader lookahead: replacing the next macro could manufacture a new
  `[…]` field that was not present before expansion. Modulo commands and other
  argument consumers follow the same braced/unbraced rules.
- Accent arguments are always opaque, including braced arguments to text and
  math accents. The pinned compiler demonstrated that `\'{\term}` with
  `\term` expanding to `ab` has different output from `\'{ab}`. Inlining a
  command whose complete body is `\'{a}` or `\hat{ab}` is supported because
  the accent then receives the same literal argument tokens.
- Catcode changes, aliases, conditional execution, `\csname`, expansion
  primitives, explicit control spaces, Unicode control-word names and raw `^^`
  input translation. `^^` is rejected before even comment/literal handling,
  because TeX performs that translation before tokenization.
- Token joins that cannot be serialized with identical ordinary TeX tokens,
  such as a literal space immediately after a control-word token.
- Recursive expansion, overlapping all-use edits, and proposals exceeding the
  document, token, byte or work budgets.

The limits are 4 MiB of source and 500,000 tokens, with separate bounded
expansion/output/work budgets. Planning happens only on an explicit request.
Definition and blocked-context lookups use sorted interval indexes, and all-use
output is constructed in one forward pass. There is no per-frame source scan,
background worker or idle repaint loop.

## Verification

`src/tex/refactor/tests.rs` exercises script binding, argument substitution,
optional defaults, comments/indentation, control-word joins, Unicode boundaries,
accents, token-reading fields, hidden mutation, snapshot rejection and CRLF.
The app tests exercise the review lifecycle, exact ownership and one-step Undo.

The opt-in `pinned_tex_expansion_metrics_and_pixels_survive_refactoring` test
independently compiles before/after documents with the pinned Tectonic engine.
It compares actual TeX expansion strings where fully expandable, hbox widths,
heights and depths, and every rasterized RGB pixel on the resulting page. It
covers macro arguments, unbraced and braced scripts, control-word boundaries,
comment indentation, optional defaults, complete accent applications and
extraction, and asserts refusal of the known unsafe accent-argument case. This provides
an independent check of the lexical model rather than only comparing the model
with itself.

```sh
TIPTOPTYP_REFACTOR_TECTONIC="$PWD/toolchain/bin/tectonic-aarch64-apple-darwin" \
TIPTOPTYP_REFACTOR_PDFTOPPM="$(command -v pdftoppm)" \
cargo test tex::refactor::tests::compiler::pinned_tex_expansion_metrics_and_pixels_survive_refactoring -- --ignored
```

The compiler run uses only the locally cached pinned TeX bundle. Missing tools
or bundles are an unavailable integration check, not evidence of equivalence.
