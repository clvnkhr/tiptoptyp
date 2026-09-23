# Encoding import fixtures

Open each `.tex` file in the Explorer. Choose the matching encoding, inspect the
preview against its UTF-8 `.expected.txt`, then open an unsaved converted copy.
The original file must remain byte-for-byte unchanged. Saving the copy writes UTF-8.

`mixed.tex` deliberately combines valid UTF-8 Chinese and an invalid isolated byte.
There is no correct whole-file fallback: cancel the preview and repair the source
separately. The app must not silently replace or mojibake the original.

These are original synthetic fixtures. The independent CTAN/arXiv byte-excerpt
regressions and source provenance are described in `docs/text-encoding.md`.
