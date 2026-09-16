# Keyboard coverage (todo 176)

There are 76 configurable application actions, each with a collision-free
default on macOS and other platforms. Open **Settings → Keyboard shortcuts**
or press Primary+Alt+, to search, change, disable or reset bindings. Primary
means Command on macOS and Control elsewhere; Alt means Option on macOS.
Existing saved overrides still take precedence, including intentionally
disabled actions. Native menu equivalents use the same effective bindings.

This item covers application/document/workspace commands, not a separate
global chord for every Settings checkbox or every per-item dialog button.

New bindings (modifier order is interchangeable):

| Action | Default |
| --- | --- |
| Keyboard shortcuts | Primary+Alt+, |
| Toggle miTeX notation | Primary+Alt+M |
| Use active tab for preview | Primary+Alt+P |
| Toggle fold at caret | Primary+Alt+L |
| Collapse all folds | Primary+Alt+Shift+L |
| Expand all folds | Primary+Alt+Shift+E |
| Next match | Primary+G |
| Previous match | Primary+Shift+G |
| Replace current match | Primary+Alt+Enter |
| Replace all matches | Primary+Alt+Shift+Enter |
| Toggle case-sensitive search | Primary+Alt+C |
| Toggle regular expressions | Primary+Alt+X |
| Previous preview page | Primary+PageUp |
| Next preview page | Primary+PageDown |
| Toggle fit preview width | Primary+Alt+W |
| Search Explorer | Primary+Alt+E |
| Refresh workspace | Primary+Alt+Shift+R |
| Status history | Primary+Alt+H |
| Toggle line wrapping | Primary+Alt+Z |
| Toggle line numbers | Primary+Alt+N |
| Toggle sticky context | Primary+Alt+T |
| Reset interface scale | Primary+0 |
| Window color | Primary+Alt+B |
| File menu | Primary+Alt+F1 |
| Edit menu | Primary+Alt+F2 |
| View menu | Primary+Alt+F3 |

Previously unassigned commands now also have defaults: Rename uses F2,
Packages uses Primary+Alt+Shift+P, Git uses Primary+Alt+G, and Reveal in preview
uses Primary+Shift+J. Fullscreen uses Control+Command+F on macOS and F11 on
other platforms. All of the original file, edit, build, window, tab and hunk
bindings remain available.

## Routing and availability

New actions resolve the most-specific binding against the whole catalog
before dispatch: for example Packages must not become Use active tab for
preview. Commands are consumed once, respect current bindings and do not
fall through into text editing. Document actions do not run behind child
windows, a file transaction or a pending process close. Shortcut-editor access
and interface-scale reset remain usable from child windows.

- Preview-source selection applies only to Typst tabs. Re-selecting the
  explicit preview source does not restart Tinymist or compile again.
- Folding requires a Typst editor with line numbers enabled. The nearest
  enclosing fold is toggled; collapsing moves a hidden caret to its visible
  header. Folding never changes the source or undo history.
- Find navigation uses the current query. With no query it opens Find.
  Replace shortcuts first expose hidden Find/Replace fields; a subsequent
  invocation performs the visible replacement through the same edit path as
  the buttons. Case and regular-expression toggles invalidate search results.
- Page navigation and fit-width apply to the raster/PDF/image viewer. When
  an asset tab is active, these controls affect that asset, not pinned Typst
  output. The interactive web preview retains its own controls.
- Explorer search opens the panel and requests focus once. Display toggles
  use the normal persisted/shared Settings update path.

New dispatch only examines actual key events, with a fixed-size action
catalog. No idle repaint timer, background worker or per-frame disk I/O was
added. Folding prepares cached source structure only on command invocation.
These changes are not expected to have a material performance impact; no
unrelated benchmark was used to claim a speedup.

Regression tests cover catalog uniqueness and defaults on both platforms,
effective overrides, cross-action chord specificity, modal/empty guards,
Find/Replace safety, fold/caret/source invariants, and bounded independent
asset page navigation. See also [tab and gutter verification](tabs-and-git-hunks.md).
