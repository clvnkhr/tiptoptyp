# Git diff editor

Hover a Git gutter marker for a compact read-only hunk comparison. Click it to
open the full-document comparison in the existing code pane, with the cursor
at that change. Previous/next navigate changes, wrapping around the document.
Escape or Close returns to the regular editor without moving the pane divider.
Temporary menus and the Find popup leave the comparison open; its button tips
use the same native tooltip host as the regular editor, above the preview.

The code pane’s right edge overlays a narrow, click-through overview of all
Git changes: green additions, blue modifications and red deletions. It marks
off-screen changes without reserving width or changing text wrapping.

Double-click a text file in the Git subpanel to open this full-document editor.
An already-open tab keeps its unsaved buffer and undo history. Removed, binary
and special files retain the read-only Git comparison.

The left column shows the baseline before the document's changes; the right
column edits the current document directly. There is no separate draft, Apply
button or diff/edit switch. All lines remain visible. Edits, Undo and Redo
refresh the comparison immediately and use the document's existing history,
auto-save and preview update flow. The baseline stays fixed during the session.
Reopen the view after changing the repository baseline outside the app.

Line backgrounds and foregrounds mark removed/added text; stronger backgrounds
mark changes within paired lines. Unchanged text keeps normal syntax colors,
rainbow brackets and configured definition/text decorations. A line's gutter
revert restores its baseline text, or removes a newly added line, as an undoable
source edit. Stage/unstage are enabled after the repository worker has refreshed
the selected hunk, avoiding actions on obsolete ranges.

## Alignment and bounded work

The in-memory line comparison trims common prefixes/suffixes, then runs a
bounded Myers search. Its trace holds at most four million entries and examines
at most 512 edit depths. If this limit is reached, the unmatched middle is
represented as a coarse replacement: every source line is still present and
reversible. No Git process or temporary file is needed for each keystroke.

Both syntax galleys are laid out at the same column width. Each matched pair
occupies the larger of its two wrapped heights; unmatched lines reserve space
in the other column. These gaps change glyph positions, not the editable
string or its character indices. One scroll area keeps the columns together.
The model and both laid-out galleys are reused until source, width, scale,
settings or style changes. Caches are bounded to the current editor view.
