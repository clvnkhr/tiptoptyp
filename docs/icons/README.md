# Icon contact sheet

[Open the contact sheet](contact-sheet.png) at **100% zoom** to judge the small glyphs.

The sheet contains every icon in the shared `UiIcon` registry (including the
Explorer folder and file), plus both application identities. Each vector pair
uses a 14 px reference box and a 4× rendering with proportionally scaled strokes.
Explorer glyphs retain their 14×12 proportions. Actual controls can use smaller
boxes, such as the tab close button. App identities use their shipped 32 px and
128 px PNGs. Platform-provided controls and font/terminal glyphs are not app-owned
icon assets.

## Regenerate

```sh
scripts/capture-icon-sheet.sh
```

Requires the normal native macOS app build environment. The script renders the
`icons` QA scene with the production painters, forces one framebuffer pixel per
logical point, and copies the fresh 1400×900 capture from
`.tiptoptyp/screenshots/icon-audit/` to this directory. It does not capture the
desktop. `CARGO_TARGET_DIR` may point at an existing build cache.

The enum and inventory share one declaration, so new UI icons automatically
appear here. Regression tests cover finite ink, control-size bounds, and sheet
capacity. The scene is also registered as a targeted screenshot-gallery scene.

## Visual audit — 2026-09-24

Inspected native captures at 1× and 4× and refined:

- **Copy:** exposed back-page edges instead of two intersecting full outlines.
- **Fit Width:** complete, symmetric outward arrowheads with clear side rails.
- **Folder:** a flat tab instead of a roof-shaped peak.
- **File:** portrait proportions that distinguish a document from a text field.

The remaining glyphs and app identities retain their existing designs. The sheet
uses unclipped per-icon layers so strokes at the reference-box edge are visible.
There are no new dependencies or recurring runtime tasks. Production changes
only affect a handful of small vector paths; no material performance impact is
expected.

Validation: 1,241 tests, strict all-target Clippy, 15 xtask tests, formatting, and
the 23-image stable gallery. The final native contact sheet was inspected at
1400×900 pixels. Baseline and intermediate captures remain in the ignored
`.tiptoptyp/screenshots/icon-audit-*` directories.
