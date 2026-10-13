# Mathematical vocabulary

`mathematics.txt` supplements Harper's curated English dictionary for both Typst
and TeX. It contains 515 explicit spellings (about 5.9 KB), including common
plural forms. It is compiled into the app and requires no download or new
dependency. The merged dictionary is built once on first use by the writing
worker; parsing, spelling and spelling suggestions use the same dictionary.
Existing Harper entries retain their grammatical and dialect metadata.

The initial candidates came from the `Wikidata Label` column of
[MathGloss's database](https://github.com/MathGloss/MathGloss/blob/b8f659605486f80f2816515f525af2c395c711fa/data/database.csv),
commit `b8f659605486f80f2816515f525af2c395c711fa`, retrieved on 30 September 2026.
Its 4,814 concept labels yielded 1,591 distinct lowercase ASCII tokens of at
least four letters. Checking these against Harper 2.11.0 left 332 missing
spellings. These were reviewed before inclusion: broken fragments from accented
names (such as `hler`), unrelated words and the typo `propogator` were removed.
Dialect-dependent forms such as `barycenter` and `fibered` were excluded.

Additional reviewed terms cover gaps in that source, including `nonautonomous`,
`eigenfunction`, `quasilinear`, `pseudodifferential`, `parametrix`, `nonuniqueness`
and `coercivity`. Countable mathematical concepts have explicit plural forms;
there is no general prefix/suffix exemption that could accept a new typo.
Reviewed compounds include `well-posedness`, `ill-posedness`, `self-adjoint`
and `divergence-free`. Harper normally splits at hyphens; the prose adapters
recognize only complete, contiguous dictionary compounds before linting. ASCII,
Unicode hyphen (U+2010) and non-breaking hyphen (U+2011) variants are supported.
En/em dashes, spaces, masked code, and unknown prefixes/suffixes never join words.
For example, `well-posedniss` remains a spelling error; bare `posedness` has not
been added as a workaround.

The additional operator/PDE vocabulary includes attested uses of
[well-/ill-posedness](https://arxiv.org/abs/1709.01109),
[hypercontractivity](https://arxiv.org/abs/1203.1280),
[hypocoercivity](https://arxiv.org/abs/2307.08280),
[paradifferential operators and paraproducts](https://arxiv.org/abs/2410.00813),
[quasinormed spaces](https://arxiv.org/abs/2402.01250),
[accretivity](https://arxiv.org/abs/1905.04306),
[dissipativity](https://arxiv.org/abs/2104.10072), and
[sectoriality](https://arxiv.org/abs/1609.02650).

This is a supplement to Harper's much larger base dictionary, not an exhaustive
glossary of mathematics. Both unknown spelling and repeated-word checks remain
active around mathematical vocabulary.

The upstream collection is MIT licensed, and the underlying Wikidata structured
labels are [CC0](https://www.wikidata.org/wiki/Wikidata:Licensing). Attribution,
modification details and the upstream licence are in
[`docs/licenses/mathgloss-MIT.txt`](../../docs/licenses/mathgloss-MIT.txt) and
included in the packaged third-party notices.

To extend the list, add only attested prose words and their valid inflections,
keep the file lowercase, sorted and duplicate-free, and run the writing tests.
Do not import all words from a general scientific dictionary: rare words and
name fragments can conceal ordinary misspellings. Do not add a spelling already
classified by Harper as belonging to a particular English dialect.
