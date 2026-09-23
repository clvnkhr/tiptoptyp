# Legacy text import

Files that fail strict UTF-8 validation are offered an explicit encoding preview.
Conversion opens a new unsaved document; the original bytes remain untouched.
A decoder error prevents import rather than inserting replacement characters.
A successful decode is not proof of the encoding, especially for mixed files.
This feature converts text bytes, not TeX package declarations: legacy CJK/font
commands may need editing before the converted source compiles with Tectonic.

Verified on 23 September 2026 against these primary sources:

- [CTAN CJK GB.tex](https://mirrors.ctan.org/language/chinese/CJK/cjk-4.8.5/examples/GB.tex): GB2312 excerpt `本常问问答集`.
- [CTAN CJK Big5.tex](https://mirrors.ctan.org/language/chinese/CJK/cjk-4.8.5/examples/Big5.tex): Big5 excerpt `本常問問答集`.
- [CTAN CJK SJIS.tex](https://mirrors.ctan.org/language/chinese/CJK/cjk-4.8.5/examples/SJIS.tex): Shift-JIS excerpt `この~FAQ~リスト`.
- [arXiv 0709.2497 source](https://arxiv.org/src/0709.2497): `comment_apl.tex` contains `141\x96--147`, which decodes as `141–--147` under Windows-1252. Conversion preserves the original double hyphen too.

Only short byte excerpts are included in unit tests, not whole third-party documents.
Tests also cover incomplete multibyte input, NUL-containing input, and a deliberately
mixed UTF-8/invalid-byte example. The latter demonstrates why the app never guesses
a whole-file encoding after a UTF-8 failure. The transcript's other arXiv cases and
corpus-wide statistics have not been independently verified.
