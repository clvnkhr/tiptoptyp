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

The revised sheet contains 44 vector icons and both application identities.

- Checkmarks, chevrons, refresh arrows, magnifiers, eye outlines, and rounded
  window edges use continuous paths rather than separately rasterized segments.
  Separate paths remain for genuinely separate parts or branches, such as a
  pupil, crossed strokes, or the trash handle meeting its lid.
- Warning has an inset, rounded, explicitly closed triangle and a smaller dot.
- Trash now has a connected handle, lid, and tapered body; the spanner uses a
  rounded continuous outline. Copy and folder corners are rounded consistently.
- Directional chevrons, ticks, crosses, and plus/minus marks are about half their
  previous reference size. Hit targets are unchanged; already-small tab crosses
  are not shrunk twice.
- Diff uses two opposing replacement arrows. DiffAll adds a second arrowhead
  to each arrow; its base geometry remains identical. Neither icon shares a
  box with Stage/Unstage. Both appear side by side in the sheet. Replace All
  has an explicit accessible label.
- Compiling rotates a filled gear with a transparent axle hole. The idle gear
  remains an outline. Both variants appear in the sheet.

The sheet uses unclipped per-icon layers so strokes at the reference-box edge
are visible. No dependencies or recurring runtime tasks were added. The existing
visible-only 33 ms compilation animation is retained; geometry changes are
bounded to small vector paths and a 128-triangle gear ring.

Regression coverage checks finite ink and control bounds, single-path contours,
small-mark size and centering, the filled compilation state and transparent
axle, Replace All semantics/read-only behavior, and sheet capacity.

Baseline and intermediate captures remain in the ignored
`.tiptoptyp/screenshots/icon-audit-*` and `icon-revision-*` directories.

Validation: formatting and strict Clippy checks passed; 1,247 Rust tests and
15 tooling tests passed. The optimized native contact sheet was freshly captured
and inspected at 1×/4×. The complete 23-image gallery was regenerated and decoded;
the replacement controls were also inspected in their popup framebuffer.
