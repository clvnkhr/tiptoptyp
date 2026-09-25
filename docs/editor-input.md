# Editor input preferences

Settings → Editor → **Tab inserts** chooses 1–16 spaces (two by default), or a
literal tab character with **Spaces** unchecked. Shift+Tab removes one indentation
unit at the beginning of the current line. This does not configure external
formatters or rewrite existing files. Pasted tabs remain literal tabs.

**Use ASCII punctuation when typing** is off by default. Turning it on converts
full-width ASCII punctuation and `。` in committed keyboard input. It preserves
letters, preedit composition, and pasted content. The setting affects the source
editor only, including Typst and TeX documents. It cannot undo a substitution
already performed by the operating system's input method before delivery.

The source editor regression exercises typing, configurable Tab/Shift+Tab and
paste through the real editor widget. Separate tests cover IME event boundaries,
settings actions and persistence. These are deterministic tests; a native CJK
keyboard session has not been verified. Work is bounded to incoming text; there
are no new workers, idle repaints, or whole-document transformations.
