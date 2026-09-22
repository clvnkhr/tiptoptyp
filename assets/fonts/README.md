# Unicode glyph fallbacks

These unmodified fonts are embedded in the application for missing-glyph
coverage behind the selected UI and code fonts. No runtime download or system
font installation is required. Each font directory includes its SIL Open Font
License. The binaries come from the pinned upstream paths below.

Source: https://github.com/google/fonts/tree/809e4d8b8d7e9364a914909bb777679606c178b8/ofl

- Noto Emoji: outline fallback coverage; macOS terminal color emoji use the system bitmap font.
- Noto Sans Math: mathematical operators and alphabets.
- Noto Sans Symbols: alchemical symbols and other symbols.
- Noto Sans Symbols 2: additional symbols and supplementary Unicode blocks.
- Noto Sans Hebrew: Hebrew letters used in symbol completions.

Pinned Google Fonts revision: `809e4d8b8d7e9364a914909bb777679606c178b8`.

| Font | SHA-256 |
| --- | --- |
| `notoemoji/NotoEmoji[wght].ttf` | `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551` |
| `notosanssymbols/NotoSansSymbols[wght].ttf` | `f7e7e04b4a24b6c78893d50cbfd2b2f6cae49617ab047bfef668d252adb128f7` |
| `notosansmath/NotoSansMath-Regular.ttf` | `3f495fe933c06786e4d5f6d86b8ee70b6753a68ee3b9d87528726de0f6e2c47d` |
| `notosanssymbols2/NotoSansSymbols2-Regular.ttf` | `7d5fb73b7ca67a6798101741f5d280a3d016a56a197afcd4199dbb57b4b82a21` |
| `notosanshebrew/NotoSansHebrew[wdth,wght].ttf` | `7ef36a2c3593758cdb622e1bdef4f84523e92fbc3ccc667438dd80ff54c2de88` |

The package copies all five copyright/license notices to `Resources/licenses`
and this provenance record to `Resources/font-provenance.md`.
