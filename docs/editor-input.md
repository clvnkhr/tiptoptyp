# Editor input preferences

Settings → Editor → **Tab inserts** chooses 1–16 spaces (two by default), or a
literal tab character with **Spaces** unchecked. Shift+Tab removes one indentation
unit at the beginning of the current line. This does not configure external
formatters or rewrite existing files. Pasted tabs remain literal tabs.

**Convert CJK punctuation to ASCII when typing** is off by default. Turning it on converts
full-width ASCII punctuation and `。` in committed keyboard input. It preserves
letters, preedit composition, and pasted content. The setting affects the source
editor only, including Typst and TeX documents. It cannot undo a substitution
already performed by the operating system's input method before delivery.

The source editor regression exercises typing, configurable Tab/Shift+Tab and
paste through the real editor widget. Separate tests cover IME event boundaries,
settings actions and persistence. Geometry regressions cover long, scrolled and
wrapped source, plus shared code fields. Native platform output forwards the
caret rectangle as the input-method candidate area, rather than the full text
widget, which may extend below the screen in a long document.

The real-app `--journey ime` checks the rectangle AppKit actually reads, Pinyin
composition and conversion, interface scaling, and both punctuation preferences.
It requires an unlocked Mac with an already-enabled Simplified Pinyin input
source and restores the original input source and preferences afterward. See
[desktop journeys](desktop-ui-journeys.md). Work is bounded to incoming text;
there are no new workers, idle repaints, or whole-document transformations.
