# UI snapshot gallery

`latest/` contains the current app-only framebuffer captures used for visual
review. Filenames are deterministic and include the actual framebuffer target,
validated QA scene, bundled theme, and any ordered color transforms. Regenerate
the complete gallery from the repository root with:

```sh
scripts/capture-theme-gallery.sh
```

Ordinary timestamped captures remain private under `.tiptoptyp/screenshots`.
Only deliberate `--ui-screenshot-latest` captures belong here.

The maintained matrix contains exactly 68 decoded PNGs:

- the ready main editor/preview scene for every bundled light and dark theme;
- all custom themed popup, tooltip, modal, context-menu, window, panel, and
  find/replace families in representative Catppuccin Latte and Mocha themes,
  including both independent Settings theme pickers, the Settings-local
  tooltip, and all three app-owned modal layouts;
- deterministic diagnostics and compiling-preview lifecycle states;
- inversion-then-hue-shift examples for both the main window and an elevated
  menu viewport.

Each capture is removed from its stable slot before the app launches, so a
stale image cannot conceal a missing write. The script restores that slot if
the command or PNG validation fails. Only a successful full default run prunes
obsolete `*.png` slots; environment-subset runs never prune, and `.gitkeep` or
other documentation files are never touched.

When ImageMagick is available, validation also checks all four outer corners of
every elevated popup, diagnostic, modal, rename, and workspace capture for full
transparency, and checks that the four Settings states differ pixelwise. These
are semantic smoke checks, not brittle fixed image hashes.

Inspect the deterministic filename manifest or validate the checked-in images
without launching the app with:

```sh
scripts/capture-theme-gallery.sh --print-manifest
scripts/capture-theme-gallery.sh --validate-latest
```

The release app is built once and then launched directly for each capture. A
45-second watchdog prevents a missing child viewport from hanging the matrix;
set `TIPTOPTYP_UI_GALLERY_CAPTURE_TIMEOUT_SECONDS` to an integer from 1 through
3600 when a slower machine needs a different limit. A timed-out capture fails
the run and restores the previous image in that stable slot.

Native operating-system file/folder pickers are intentionally absent because
they do not belong to tiptoptyp's framebuffer. See
[`../ui-qa-screenshots.md`](../ui-qa-screenshots.md) for the full scene and
target contract.
