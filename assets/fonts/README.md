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
- Noto Sans: Latin extensions and IPA, including ʟ.
- Noto Sans Hebrew: Hebrew letters used in symbol completions.
- Hack: terminal regular/bold/italic/bold-italic faces with shared advances.

Pinned Google Fonts revision: `809e4d8b8d7e9364a914909bb777679606c178b8`.

Hack release: `v3.003` from https://github.com/source-foundry/Hack/releases/tag/v3.003.

| Font | SHA-256 |
| --- | --- |
| `notoemoji/NotoEmoji[wght].ttf` | `de6c18832938afc99caf132b39d6a30a19bac7f2e812e28db2535b4608d27551` |
| `notosanssymbols/NotoSansSymbols[wght].ttf` | `f7e7e04b4a24b6c78893d50cbfd2b2f6cae49617ab047bfef668d252adb128f7` |
| `notosansmath/NotoSansMath-Regular.ttf` | `3f495fe933c06786e4d5f6d86b8ee70b6753a68ee3b9d87528726de0f6e2c47d` |
| `notosanssymbols2/NotoSansSymbols2-Regular.ttf` | `7d5fb73b7ca67a6798101741f5d280a3d016a56a197afcd4199dbb57b4b82a21` |
| `notosanshebrew/NotoSansHebrew[wdth,wght].ttf` | `7ef36a2c3593758cdb622e1bdef4f84523e92fbc3ccc667438dd80ff54c2de88` |
| `notosans/NotoSans[wdth,wght].ttf` | `bfb7bb691513f12e734dc346c03a03f784912432d7e3fa8e56efcf906fe86b3d` |
| `hack/Hack-Regular.ttf` | `15f55cc0c85a2988d2b4b3a8cdb5d77fdfbaf319e1bb5309d725db9818fb7125` |
| `hack/Hack-Bold.ttf` | `5bbf531eff7f8a0c2559c9a0656718e2828a012a9b1f60b5f54006d59a4de8d4` |
| `hack/Hack-Italic.ttf` | `096fb67a2b85f3c866e9cb3e965b27c2c10b977315f4d3d7f095674be35091c1` |
| `hack/Hack-BoldItalic.ttf` | `64f74a079700b7dfe128551a1e28875d5ba980971e55f5e0f0596e37bdc6a6bc` |

The package copies all seven copyright/license notices to `Resources/licenses`
and this provenance record to `Resources/font-provenance.md`.
