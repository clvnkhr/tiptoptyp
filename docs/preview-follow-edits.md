# Following source edits

**Settings → Preview backend → Follow edits in preview** is enabled by default.
When editing Typst in Split view, the Tinymist preview scrolls to the edited
caret position after successful compilation and a 200 ms pause in typing.
The setting is saved and also defaults on when absent from existing settings.
Turn it off to keep browsing the preview independently while editing.

Typing, undo/redo, completion and source-editing commands share the document
transaction path. The final caret or pending selection is captured once after
the edit; moving the cursor alone does not cause or retarget an automatic jump.
The jump uses the same canonical source mapping as manual preview navigation,
including miTeX views and edited source files beside a designated preview.
An explicit preview jump supersedes a pending automatic jump.

Hidden and paused previews do not follow edits. Changing the active document,
revision, Tinymist session, backend or toggle discards pending work. Automatic
following does not focus the editor or open the preview from Code view.
PDF.js and raster previews lack source mapping and keep their existing behavior.
Source that produces no rendered location, such as a comment, may have no jump
target. Tinymist compile reports are unversioned; the adapter admits only reports
for the current session/preview entry received after the edit and waits through
compiling/error states.

The scheduler stores one small pending request, replacing it on each edit. It
does not copy source, create workers, poll compilation or repaint while idle.
Only an accepted compiled edit schedules a bounded debounce wakeup and uses the
existing asynchronous navigation command. No material performance impact is
expected; this is not a measured native performance or visual-verification claim.

Deterministic coverage: `cargo test preview_follow`, the settings renderer's
semantic checkbox test, and settings search tests. These cover persistence,
independent settings edits, debounce/coalescing, slow/failed compilation, stale
documents/sessions, cursor-only movement, focus, and disabled/paused/hidden modes.
