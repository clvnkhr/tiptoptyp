#!/usr/bin/env python3
"""Build-time only: freeze the pinned English Moses rules and parity cases.

Run with the reference Liquid environment; generated data is used by Rust,
without shipping or executing Python in the application.
"""
from pathlib import Path
import importlib.metadata
import json
import shutil
import sys

from sacremoses import MosesTokenizer, MosesDetokenizer
from sacremoses.util import _CJKChars_ranges
from tokenizers import Tokenizer

from grammar_compare.models import normalize_word_apostrophes

ROOT = Path(__file__).resolve().parents[1]


def main():
    if importlib.metadata.version("sacremoses") != "0.2.0":
        raise SystemExit("Use the pinned sacremoses 0.2.0 reference environment.")
    tokenizer = Tokenizer.from_file(sys.argv[1])
    output = ROOT / "assets/liquid"
    output.mkdir(parents=True, exist_ok=True)
    moses = MosesTokenizer(lang="en")
    detokenizer = MosesDetokenizer(lang="en")
    data = dict(version=1, source="sacremoses 0.2.0 English; MIT",
                alnum=moses.IsAlnum, alpha=moses.IsAlpha,
                numbers=moses.IsN, lower=moses.IsLower,
                currency=detokenizer.IsSc,
                prefixes=moses.NONBREAKING_PREFIXES,
                numeric_prefixes=moses.NUMERIC_ONLY_PREFIXES,
                cjk_ranges=_CJKChars_ranges)
    (output / "moses-en.json").write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n")
    distribution = importlib.metadata.distribution("sacremoses")
    license_path = next(p for p in distribution.files if str(p).endswith("licenses/LICENSE"))
    shutil.copyfile(distribution.locate_file(license_path), output / "SACREMOSES-LICENSE")
    texts = []
    for relative in ("scripts/grammar_compare/corpus.json", "scripts/grammar_compare/arxiv-corpus.json",
                     "docs/grammar-comparison/2026-10-20/harper/corpus.json"):
        path = ROOT / relative
        if path.is_file():
            texts.extend(case["text"] for case in json.loads(path.read_text())["cases"])
    texts.extend([
        "She go to school every day.", "Tao's result is correct.", "Tao’s result is correct.",
        "Barbatoʼs and Romito’s results are correct.", "The Jones' house is here.",
        "We can't prove this; it isn't obvious.", "'quoted words' and \"double quotes\".",
        "The result costs $5,300.50 (approximately).", "Dr. Tao and Mr. Jones work on this.",
        "No. 5, Art. 3, pp. 10–12.", "The value of x is 1,234 and y is 3.14.",
        "Well-posedness and nonautonomous semigroups.", "One... two.. three.....", "a,b,c,d,e",
        "We say: a/b, a_b, a+b = c.", "A  sentence\twith\nspaces.", "中文 字符; 한글 test.",
        "e.g. this case. Next Case.", "'s 'a' 1990's 123'4 a'b'c aʼbʼc",
        "The Γ-function is holomorphic; α ≠ β.", "Éléments of naïve set theory.",
    ])
    cases = []
    for text in dict.fromkeys(texts):
        tokens = moses.tokenize(normalize_word_apostrophes(text), escape=False)
        prepared = " ".join(tokens)
        cases.append(dict(text=text, prepared=prepared,
                          ids=[tokenizer.token_to_id("<|startoftext|>")] + tokenizer.encode(prepared, add_special_tokens=False).ids,
                          detokenized=detokenizer.detokenize(tokens, unescape=False)))
    (output / "text-cases.json").write_text(json.dumps(cases, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(dict(cases=len(cases), rules_bytes=(output / "moses-en.json").stat().st_size)))


if __name__ == "__main__":
    main()
