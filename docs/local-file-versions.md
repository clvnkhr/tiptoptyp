# Local file versions

On macOS, ordinary document saves now use Foundation's `NSFileVersion` revision
store. These are local system-managed revisions associated with the original
file, not Git commits or a second history directory maintained by the app.
Existing native versions are available too. History is unavailable on other
platforms; ordinary document saving remains available.

## Saving and retention

All revision and save I/O runs inside the existing background save worker.
The canonical destination lease spans baseline validation, staging, the atomic
replacement, and revision creation. `NSFileCoordinator` additionally coordinates
with participating macOS applications. It cannot lock out programs that ignore
file coordination.

The first overwrite preserves the exact original disk contents. Subsequent
ordinary autosaves coalesce native snapshots to at most one per minute, while
explicit saves preserve changed versions immediately. An unchanged version is
not copied again. A bounded cache of 128 last-write metadata records recognizes
the app's own writes; it stores no document text or backups. An unrecognized or
externally changed disk state is preserved before overwrite even if another
native version was saved recently. Evicting a metadata record can create an
extra safety snapshot, rather than dropping a required backup.

Existing-file replacement uses `NSFileManager`, preserving the revision
association and original metadata. A newly created file uses a no-clobber
commit. Directories and other non-file save destinations are rejected. If
coordination redirects to a moved file, the operation stops before disk
validation or mutation and asks the user to reopen the document.

The system manages revision storage. The app does not prune native versions or
promise permanent retention. Unsupported volumes and revision-store failures
produce a backup warning separately from the source-save result: a committed
file is not falsely marked unsaved. Conflict rejection happens before any new
revision is created. There is no private-history fallback.

## Revert To

File → Revert To opens a comparison in the code pane, keeping the sibling
preview available. The complete document is shown with normal source
highlighting and changes layered over it. Older content is on the left and
newer content on the right. Each column identifies its saved time explicitly
in UTC; an unsaved current buffer is labelled as current edits.

Each side can select a different native version or the current snapshot.
Comparisons are read-only. Opening a revision does not open its hidden system
path as an editable tab, change the current document, or create a Git change.
Version selection and whole-document diff computation run on workers.

Restore version first preserves the current canonical buffer, including unsaved
edits, as a native revision without replacing the disk file. If that backup
fails, restore stops. The owner, document key, and original path must still
match when the worker returns. The replacement is then applied as an undoable
source change and saved through the normal guarded save transaction. An
external disk conflict therefore remains a conflict, and later user edits or a
replacement document cannot receive a stale restore result.

## Historical PDF previews

Preview PDF renders the chosen side's captured source bytes with the configured
Typst or TeX compiler. The result is shown in the ordinary preview area as an
immutable PDF. It does not become an editable source tab or replace the live
preview controller's artifact. Closing history releases its override.

Only the selected source file is historical. Relative imports, included files,
images, fonts, package installations, compiler arguments, and environment come
from the current project and current configuration. This is not a historical
checkout of the whole project, so a version may render differently or fail if
its dependencies have changed.

The isolated compiler uses a private source mirror and produces an owned
artifact below `.tiptoptyp/version-pdf-*`. It does not replace the original
source or its adjacent PDF. Compiler processes are stopped before the immutable
PDF is published; dropping its owner removes the temporary artifact. Closing
history requests cancellation of a pending render. Render failures remain in
the comparison. Source-reveal actions are disabled for a historical PDF because
its locations do not describe the current buffer.

## Evidence and limits

Native disposable-file tests cover history surviving repeated atomic saves,
deduplication, autosave cadence, first overwrites with existing native history,
unsaved-buffer preservation, non-file rejection, conflict rejection, and
bounded ownership bookkeeping. A separate real Foundation probe verified
permissions, ownership, and a custom extended attribute survive replacement.
The coordinated-path guard has tests that prove a redirected path cannot
execute its operation.

UI interaction evidence and final integrated check results are recorded
separately; native storage tests do not prove the composed window's appearance
or focus behavior. Save-worker measurements are described in
[background-saves.md](background-saves.md).

Foundation references:
[NSFileVersion](https://developer.apple.com/documentation/foundation/nsfileversion),
[adding a coordinated version](https://developer.apple.com/documentation/foundation/nsfileversion/addofitem(at:withcontentsof:options:)),
[coordinating writes](https://developer.apple.com/documentation/foundation/nsfilecoordinator/coordinate(writingitemat:options:error:byaccessor:)),
and [metadata-preserving replacement](https://developer.apple.com/documentation/foundation/filemanager/replaceitemat(_:withitemat:backupitemname:options:)).
