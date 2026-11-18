# Headless Overleaf connection

The browser initially opens Overleaf login and can navigate ordinary HTTP/HTTPS
websites, with address and history controls. Selecting a verified Overleaf project
connects its native working copy; non-project pages carry no project authority.
Sign-in is done on Overleaf's own page. Its browser data store retains the signed-in
session; the app does not request, extract, log, or save a password or session
cookie. A retained working copy has an opaque browser-profile ID, so different
workspaces can use different Overleaf accounts. Reopening that workspace selects
the same browser-owned data store. On macOS 14 and later, WebKit's persistent
identified stores retain browser data across app restarts; these stores are separate
from Safari. On macOS 12/13, public WebKit APIs support only isolated in-memory
stores: the app retains those stores across reconnects and workspace switches
for the app's lifetime, and a restart can require signing in again. It never
silently falls back to sharing accounts between scoped workspaces. The unscoped
browser retains its separate general login profile. See
[WebKit's profile API](https://webkit.org/blog/14423/building-profiles-with-new-webkit-api/).
The app retains each scoped data store while it runs, including during offline
mode and workspace switches. Each new browser view receives a fresh configuration
and IPC controller, so retaining a login does not reuse an old project bridge.

Scoped WebKit data stores remain alive across browser replacement, offline
periods and workspace changes. Each new view gets a fresh configuration and
IPC controller; old views and their project bridge are not retained.

On Windows, WebView2 uses a stable user-owned profile under
`%LOCALAPPDATA%/dev.tiptoptyp.editor/overleaf-browser/<opaque-id>`, independent of
the app's installation path. The unscoped browser uses the parent folder. No
workspace address or account name is embedded in a browser-profile directory.
Overleaf and its sign-in provider control session expiry and any required
reauthentication. The app does not clear a profile on Disconnect or workspace
changes, and does not automate password-manager prompts, store
passwords in Settings, or export cookies between browsers. Files shown in
Explorer come from the connected project. Local files supply compiler input;
the sync journal keeps their last server-confirmed ancestor separately from
saved offline drafts.

On macOS, **Passwords…** opens Apple Passwords, with System Settings → Passwords
as the fallback on older systems. Unlock your saved Overleaf login there, then
copy and paste it into Overleaf's sign-in page. The app opens this UI on a
background worker and reports launch failures. It does not read the password or
clipboard, fill the form automatically, or share Safari's signed-in session.
Apple documents the saved-password locations in
[Find saved passwords and passkeys on your Mac](https://support.apple.com/en-ie/105115).

This is an integration with Overleaf's internal browser interfaces, not a public
Overleaf API. The interface may change. The bridge fails visibly when its expected
project tree or editor interface is absent. A successful fake-server or pure test
is not proof of compatibility with the user's signed-in Cloud project.

## Returning from sign-in

A project connection recognizes committed `/login`, `/project` dashboard, and
exact bound-project routes on that project's trusted origin. After sign-in returns
to either the dashboard or the original project, one automatic reconnect reopens
the original project using the same native browser profile, a new decoder inbox
and a new connection epoch. Coalesced login/dashboard/project redirects use the
same handoff; choosing a different project keeps the ordinary connection flow. Pending
pre-login messages and mirror receipts cannot authorize the new session. The
existing lease, document history, drafts and selections remain in place, and
editing stays locked until every fresh source and asset reaches disk.

This continuation reads no login fields and depends on no provider-specific DOM
selectors. Third-party SSO pages carry no native project authority. An explicit
address/history action cancels pending automatic return, and repeated redirects
without successful hydration stop with a short error instead of looping. The
controller owns this single attempt across native browser replacement. Only
complete verified hydration or an explicit Reconnect re-arms recovery. If a login
requires a password, passkey, CAPTCHA or verification step, the user completes it
on the site's own page.

## Browser and project authority

Opening the browser does not reconnect a project, cancel a pending save, or
allocate a mirror. Login, dashboard and external pages run without the project
bridge. Committed page navigation preserves an unsubmitted address while its
local-copy options are edited, even after the address field loses focus. An
accepted Go/Connect/history action or a real click back into the browser releases
that draft. A project selected on the official origin (or an
explicitly connected self-host origin) starts the normal storage/hydration flow;
ordinary websites cannot select a trusted project. Explicit canonical project
addresses expose local-copy options and Connect/Reconnect. Other addresses use Go
in the existing browser session. File and executable URL schemes and embedded URL
credentials are rejected.

Leaving a bound project permits browsing while keeping native managed source
locked. Re-entering the project requires fresh verified synchronization. Browser
cookies remain in the WebView profile when a project connection is rebuilt.

Explicit clicks on native address/storage controls own their keyboard events and
editing commands even if a page later autofocuses an HTML field. An unsubmitted
project address also survives moving from the address field to storage controls
or Passwords; a delayed login-page navigation cannot replace that draft. Accepted
Go/Connect or history actions commit the target, and a real click inside the
WebView releases the draft and keyboard ownership to the page. On macOS this is an
exact-window event monitor, removed before browser teardown; idle frames do not
poll or repeatedly reclaim focus.

WebKit popups may share the opener's configuration for browser sign-in, but
Overleaf IPC is explicitly restricted to the owning native WebView's main frame.
WebKit's sending-WebView identity and frame identity are checked before the body
is read or queued, followed by the exact origin/project check. A popup showing
the same project has no authority over its opener's native model. This does not
restrict ordinary browsing or SSO popups and needs no page-provided `opener`
heuristic. WebView2 uses its owning top-document event rather than subscribing to
child-frame message events.

These native identity checks use [WebKit message frame and WebView metadata](https://developer.apple.com/documentation/webkit/wkscriptmessage/frameinfo)
and [WebView2's top-document message event](https://learn.microsoft.com/en-us/dotnet/api/microsoft.web.webview2.core.corewebview2.webmessagereceived).

## Explorer file actions

The managed Explorer supports new text documents and folders, editable-document
duplication, rename, move and delete. These use authenticated, same-origin
Overleaf requests with its CSRF token, exact server IDs and validated names and
paths. Mirrors are written only after server tree/source confirmations. One
file operation owns the browser at a time; unknown outcomes are not retried
silently. Native source edits are held during that operation, and dirty originals
cannot be duplicated or deleted without saving their draft first.

A document copy first creates a fresh empty document ID, waits for its real
hosted editor state, then saves the downloaded original through the existing
OT/HTTP confirmation path. The original stays independent. Binary/folder copies
are not offered: both upstream upload and linked-file routes upsert by name and
provide no safe independent-copy endpoint. Rename, move and delete still apply
to those entries. The compact prompts validate names and current ownership before
sending; later server rejection remains visible.

## Incremental incoming updates

An incoming source snapshot is compared with the bound document and applied as a
same-session external revision. Unchanged source does not invalidate editor or
provider caches. Current selections are mapped through changed spans; stale
whole-buffer undo snapshots are retired so Undo cannot overwrite collaborators.
Clean MiTeX sources retain their projection when possible and atomically fall
back to canonical source when a remote edit cannot be projected. Dirty drafts
continue through the existing conflict-preserving reconciliation.

Language servers remain running. Tinymist, TeXlab and compatible TeX providers
receive worker-generated UTF-16 range changes when they advertise incremental
sync; full-sync servers retain their required format. Parked Typst documents also
receive changes. Published diagnostics and inline results remain visible until
current replacements arrive. Harper/LTeX still check complete prose context using
their normal debounce; this is not partial grammar inference. Actual pinned-file
renames use the existing preview-entry transition because its launch configuration
contains the file path; ordinary incoming edits do not restart that preview.

## Authoritative editing

The JavaScript bridge is injected before the editor loads. It observes the
project's actual Socket.IO client and reuses the editor's own CodeMirror view.
The official page loads a global Socket.IO script before the IDE bundle. That
script first assigns an empty `io` object, then populates `io.connect`; the bridge
intercepts the method assignment itself, synchronously. A setup timer cannot
reliably catch the first connection. The `Manager` method is intercepted in the
same way for Overleaf's alternate v2 shim.

The editor view is accessed through:

```js
const store = window.overleaf.unstable.store;
const id = store.get('editor.open_doc_id');
const view = store.get('editor.view');
view.dispatch({changes: {from, to, insert}});
```

These are real exposed editor interfaces, used by Overleaf's extensions. The app
checks the active document ID and exact base text before dispatch, sends one
minimal transaction, observes that OT update being sent and its matching own
`otUpdateApplied` server acknowledgement, then waits for Overleaf's `doc:saved`
event for that document. A delayed saved notification for an earlier operation
cannot acknowledge a new edit. The bridge counts actual outgoing OT sends per
document and accepts a saved event only when every observed send has a matching
server acknowledgement. Duplicate prior acknowledgement versions cannot clear a
newer outstanding send. Counters are capped, and removed document tracking is
pruned (except an uncertain pending native request). A retry with ambiguous
accounting may require reconnecting; it never becomes an assumed successful save.
After those signals, the bridge performs a fresh authenticated document download.
The native acknowledgement contains that revision-checked server text after any
collaborator transformations. A newer, debounced edit in the hosted view cannot
be included merely because an older `doc:saved` event arrived. Unchanged explicit
saves also confirm the server snapshot. Failed confirmation preserves the pending
request; newer server revisions invalidate and refresh an older confirmation.
Document responses must have the official `text/plain` type; HTML sign-in pages
and JSON error payloads cannot become authoritative source even with HTTP 200. An `applyOtUpdate` RPC callback alone is **not** an
acknowledgement that a change was saved. Optimistic browser edits are not published
as server-confirmed snapshots while an app write or browser OT update is pending.

The hosted editor handles both legacy `sharejs-text-ot` and `history-ot`, including
its tracked-change representation, UTF-16 positions, transforms, reordering,
deduplication and reconnect logic. We deliberately do not implement a second OT
engine. Omitting `supportsHistoryOT` does not request conversion to the older OT
format: the real-time service rejects a history-OT document in that case.

A file switch during a save is queued (only the last requested file is kept). A
save for a parked native tab first activates its official browser document, waits
for its new state, then checks the exact base. If safe activation fails, the local
edit is retained rather than dispatched to another file.

Opening uses Overleaf's stable file IDs, not labels or file-name matching. It
clicks the exact identified tree row, or uses the official `?doc=<id>` deep link
when that row is not mounted. A changed exposed document ID is insufficient: the
bridge also waits for the matching opened event and CodeMirror's new state.

Initial project startup never uses that deep link when the file tree is unmounted.
Overleaf mounts its editor from asynchronous chunks, often after the project socket
and store are ready. Reloading during that interval cancels the chunks, and
Overleaf consumes the document query before the next bootstrap, causing repeated
loads. Startup instead permits a mounted tree-row click or waits for the normal
editor/store notifications under the existing bounded watchdog. Explicit file opens
retain their deep-link fallback.

## Project cache and events

`joinProjectResponse` supplies project ID/name, permissions and the recursive
`rootFolder` tree. A folder contains `docs`, `fileRefs`, and `folders`; each entity
has an ID and name. The bridge accepts only safe relative names, at most 10,000
entries and 64 nested folder levels. A newer server that stops supplying this
expected tree needs an adapter; the app does not synthesize a partial authoritative
tree by scraping the rendered editor.

All text documents and binary assets are hydrated through the authenticated,
same-origin browser session, at most two downloads concurrently:

- `GET /project/<project>/doc/<document>/download`
- `GET /project/<project>/file/<file>`

The document download route returns plain source. Internally the document updater
converts history OT to ordinary lines unless the caller explicitly opts into the
history-OT representation. This avoids decoding either OT snapshot format in the
native cache.

Current Overleaf servers broadcast `otUpdateApplied` to the project room, so
HTTP hydration does not prevent background files from receiving live updates.
The official `joinDoc` acknowledgement advertises this through
`{canSkipLeaveDoc: true}`. This changed upstream on 17 September 2026. Older
servers send edits only to joined document rooms: the active document still
saves safely, but collaborator edits to other files require Refresh. The bridge
checks the capability and shows “Connected · refresh to sync other files on this
server” without marking the connection offline. It does not join every document
on the official editor socket: older servers use a single join/leave epoch, so
passive joins could cancel the editor's own document activation.

A remote `otUpdateApplied` invalidates any outstanding older download for that
document. Both active and nonactive documents are downloaded again. Ordinary `doc:changed`
events never publish CodeMirror text: those events can precede the browser
client's debounced OT send. Outside the explicit saved acknowledgement above,
only authenticated HTTP downloads publish authoritative document snapshots. Rename, move, create and delete
messages update the tree by ID. Downloads completed against an obsolete tree or
document revision are discarded and refreshed. There are no periodic project
polls after setup. Setup polling is bounded to 30 seconds; each download is bounded
to 30 seconds, 4 MiB of UTF-8 source or 8 MiB of binary data. IPC is capped at
16 MiB. Failed downloads are reported instead of retried indefinitely.
The native write queue is bounded to 64 MiB. If a burst exceeds that budget,
only the affected file identities are retained; after writes drain, those files
are downloaded again one at a time without activating the hosted editor. A
second overflow or a 35-second retry timeout is reported rather than looping.

## Local-copy storage and reopening

Choose **Local folders → New Workspace…** in the workspace chooser to create an
empty folder under an existing parent and open it as a local workspace. Enter a
single folder name; an existing file or folder is never replaced. Cancelling
before creation leaves the filesystem unchanged.

The optional **Use for an Overleaf project** checkbox reveals no extra fields.
Creating the workspace opens the normal Overleaf window with **Keep local copy**
enabled and the new folder selected as its offline-storage location. Choose the
project in that window. The usual storage controls show the pending folder even
on the login page, before a project is selected, and it stays selected while the
project address changes. **Hide** retains this pending choice; **Reset** clears
it and hides the storage row on a projectless login page. For a new connection,
choose **Connect** to download the project into
its own namespaced subfolder. For the already connected project, close its tabs
before using **Apply** to change the storage folder; **Reconnect** retains the
active folder. On that project, **Reset** restores the active folder and discards
the pending choice. Creating the folder alone makes no project connection or cloud changes
and preserves the current source and unsaved work.

The connection window has a project-specific **Keep local copy** choice. It is
on by default. With no custom folder, macOS uses
`~/Library/Application Support/dev.tiptoptyp.editor/overleaf/`; other platforms
use their application-data directory. A chosen folder is a **base**, not the
project root: the app appends `<origin-hash>-<project-id>` so projects from
different Overleaf servers cannot share a mirror accidentally. Preparing folders
and the file picker do not block the UI. Reconnect always resumes the current
project's existing working copy, including unsaved tabs; unfinished storage
fields do not change that destination. Changed storage fields expose Apply and
Reset in the same row. Apply changes the folder or storage mode only after that
project's tabs are closed and local work is safely checkpointed or resolved. Reset
restores the active working copy's storage choices without changing files or
saving the pending options.

Settings JSON exposes the choices in `overleaf_storage`, keyed by canonical
project URL. Each value contains `keep_local_copy` and nullable `directory`.
At most 32 explicit project preferences are retained. Folder paths must be
absolute and contain no control characters; validation normalizes `.` and `..`
without filesystem work on the UI thread. Authentication still lives only in the
WebView's separate browser-data store.

Turning local copies off creates a unique private temporary directory. The UI
uses the project name and project-relative file labels instead of showing its
physical path; hovering the workspace root reveals that path. Temporary mirrors
are excluded from local-workspace and last-file history. Retained workspaces stay
in history and restore their Overleaf association; temporary projects reopen by URL.
The temporary directory stays alive while its workspace, tabs, unsynced drafts,
or mirror jobs still need it. Releasing the last lease schedules deletion on a
background worker. Selected base folders and persistent copies are never removed
by that cleanup. Normal shutdown waits up to ten seconds for outstanding cleanup
and reports an incomplete cleanup; forced termination can leave temporary files.

Each managed root has a bounded version-1 descriptor at
`.tiptoptyp/overleaf-project.json` containing its project URL and persistence
mode, without credentials. A chosen workspace base also has
`.tiptoptyp/overleaf-workspace.json`, which points only to its validated,
project-specific child. Reopening either folder restores the association even
when the global project storage preference now points to another workspace.
Descriptor discovery is bounded and runs on an explicit-open worker. It cannot
steal navigation from an editor that changed while discovery was running.

`.tiptoptyp/overleaf-session.json` remembers the opaque browser profile ID,
online/offline choice, open pages, active page and preview page. Closing every
page restores an empty workspace. Metadata changes use a small exclusive worker;
idle frames neither read nor rewrite metadata. Workspace departure checkpoints
page IDs before closing the old pages. Browser passwords and cookies are never
included in workspace files.

Choose **Work offline** in the Overleaf window to edit downloaded text documents
and compile with the selected local compiler. **Save** and auto-save write the
local file and its sync record, then mark the editor locally saved; the workspace
still shows **Offline · unsynced changes**. Offline mode and saved edits survive
workspace changes and app restarts. New/renamed/deleted remote files and binary
asset uploads remain online operations. Undownloaded files require reconnecting.

**Reconnect** restores that workspace's browser profile, downloads a fresh tree
and files, and reconciles against the retained remote ancestor. Separate edits
merge automatically. Overlapping text edits retain both versions and open through
**Review changes**, with **Use Overleaf**, **Keep local**, and **Edit merge** choices.
Cancelling retains the conflict. Choosing a version creates a draft; **Save**
publishes it through the normal acknowledged Overleaf save path. Reopening,
refreshing, and local disk writes never stand in for a server acknowledgement.
An existing local text file without a recorded sync history is also reviewed
when its contents differ. Neither an empty local file nor a later remote update
can silently choose a version in that case.

The worker-only `.tiptoptyp/overleaf-sync/` records remote ancestors, local
recovery drafts and conflict versions per document. A fresh browser message is
still required before publishing a restored document. Text, metadata, tree,
aggregate restore work and queued writes are bounded. Writes use retained
no-follow directory handles and atomic replacement. A bounded advisory lock
serializes this app's workers across windows/processes; it does not turn
uncooperative external editors into transactional writers. Changed untracked
files, local deletions and changed binary assets are preserved rather than
overwritten. Some deletion/asset conflicts stop sync and name the affected path;
restore or keep a separate local copy before refreshing. These cases do not
pretend that a text merge can safely recreate or overwrite a remote entity.

Connecting and reconnecting keep an editing barrier until the fresh tree,
all files and the current generation's writes have completed. An authentication
or sync failure can be followed by **Work offline** when a retained copy is
available. Temporary copies retain the original online-only editing barrier.
Native typing, paste, generated insertions and formatter replies respect these
states; selection and ordinary widget editing remain available.

## Hosted frontend resource use

Before the hosted editor boots, the bridge sets Overleaf's actual browser-local
`autocompile_enabled:<project-id>` preference to JSON `false`. This setting is
per project in the app's own WebView profile; it does not update Overleaf account
settings or Safari preferences. If storage is unavailable, connection setup
continues. We do not send synthetic layout hotkeys to the authentication window.

This prevents continuous background compilation from that browser preference.
It does not guarantee zero server compiles: the hosted editor may fetch a cached
compile or perform one initial compile independently of the auto-compile toggle.
The complete hosted frontend, including its PDF component, is still loaded, with
its corresponding browser-process memory cost. The bridge neither alters private
React state nor intercepts compilation requests to manufacture success responses.

## Frontend compatibility and failure boundaries

Project publication and downloads require a validated tree, usable editor-store
subscriptions with callable unsubscribe functions, and the collaboration socket
observer. Projects containing source also require an observable editor `setState`,
text access and `dispatch`. If the hosted page initially shows only an asset, the
bridge waits one event tick for natural editor mounting, then opens the verified
root document (or first source document) once through a mounted tree row. With
no mounted row, it allows normal editor loading to finish without navigating.
A tree-row click waits for the real opened event and installed state before publication;
an already-selected source keeps its normal lazy mounting path. Asset-only
projects can hydrate without creating a source editor. Edits and cursor commands
still require the verified source view. The existing setup watchdog bounds failed
mounting/bootstrap; a repeated deep link cannot cause a reload loop. Absent or
changed interfaces produce a compatibility error. There is no synthetic write
probe, guessed private-state fallback, or periodic idle probe.

Subscriptions are installed transactionally and disposed independently. Cleanup
marks the bridge stopped first; failed disposers and retained callbacks cannot
restart it. Replaced sockets, views and stores lose their ownership before their
hooks are removed. Factory interception preserves constructor/subclass behavior,
callback arguments, receiver identity and the original method's return value.
Bridge observation failures cannot throw through Overleaf's own callbacks.

Document activation still requires its exact ID, opened event and installed
source state. Tests also exercise Overleaf's asynchronous, coalesced store
notifications: selection-only changes to the previous document cannot certify an
open. Every known assumption stays in the small browser bridge; Rust consumes
validated project/source events rather than scraping the frontend itself.

Native main-frame navigation away from the exact project immediately locks
managed source, including third-party sign-in pages where our bridge does not
run. Returning needs fresh hydration. IPC is accepted only from the current
project page; stale messages from a previous page cannot reconnect it.

Native tabs retain stable server entity IDs across renames and deletion. A new
entity reusing a removed file's path cannot inherit its tab, draft, save request
or cursor navigation. Malformed or invalid authoritative messages terminate the
current stream; buffered connected/project events cannot repair a missing event.
Explicit Reconnect creates a fresh decoder and hydration generation.

The offline bridge and localhost fixture contracts run in CI. They cover missing
and non-interceptable APIs, throwing disposers, constructor behavior, reordered
store notifications, server-confirmed saves, stale callbacks and bounded work.
Native journeys additionally exercise cross-origin page departure and reconnect.
These checks protect the known contracts; they cannot guarantee compatibility
with a future unobserved Overleaf deployment. Review the pinned primary sources
below when changing the bridge rather than adding speculative aliases.

## Native bridge contract

Every message has a native connection `epoch`, fixed for the lifetime of that
WebView. Commands from an obsolete epoch are ignored. The native wrapper pins the
origin and rejects IPC from subframes or another origin. The browser never sends
cookies or authentication headers through IPC.

Commands are tagged with `command`:

| Command | Fields |
| --- | --- |
| `open` | `id`, exact `path` |
| `edit` | `id`, exact `base`, complete `source`, `request` |
| `cursor` | `id`, `source_fingerprint`, UTF-16 `anchor`, `head` |
| `refresh` | reload only after pending writes finish |
| `disconnect` | unsubscribe, abort downloads, disconnect |

Events are tagged with `event`:

| Event | Fields |
| --- | --- |
| `project` | `project_id`, `name`, nullable `root_doc`, `entries` |
| `document` | `id`, `path`, server-confirmed `source` |
| `ack` | `id`, `request`, server-confirmed complete `source` |
| `file` | `id`, `path`, `base64` |
| `conflict` | `id`, `request`, current `source`, `message` |
| `presence` | `origin`, nullable `own_id`, `self_user_id`, complete `peers`, complete `documents` |
| `status` | `state`, optional `message` |
| `error` | `message` |

Each project entry is `{id, path, kind}`, where kind is `doc`, `file`, or `folder`.
A rejected pre-dispatch edit preserves the native text and gives an explicit
conflict. Its baseline comes from the last authoritative HTTP/saved result, or
the native command's base if that result was evicted, never optimistic hosted
text. This source cache is capped at 16 MiB and pruned with project entries; a
conflict queues a fresh guarded server download. Disconnection or a server save error never pretends that a queued write
was accepted. After an ambiguous failure, the official browser client retains
its own pending operations and the native editor retains the local edit.

## Collaborators and carets

The bridge observes the official frontend's existing
`clientTracking.getConnectedUsers` callback and project-wide
`clientTracking.clientUpdated` / `clientTracking.clientDisconnected` events. It
never requests an extra user-list refresh. Connections are identified by their
public client ID; only this WebView's connection is excluded. Another window
signed into the same account remains visible. Late initial replies cannot
resurrect clients that disconnected or replace a newer position update.

Each `presence` message replaces the complete previous snapshot. A peer is
`{id, user_id, name, doc_id, row, column}`; unknown identity or coordinates are
`null`. Names are bounded to 128 characters, with Overleaf's name/email/Anonymous
fallback. The message also carries the current origin, own public connection ID
and account ID. Native badges use the exact site color algorithm: MD5-derived
hue, the site's reserved own-user hue of 200 and anonymous hue of 100, at
`hsl(hue, 70%, 50%)`. This is not a theme-selected approximation.

A document cursor snapshot is
`{id, source_fingerprint, source_utf16, cursors:[{id,anchor,head}]}`. The
fingerprint is lowercase 16-digit FNV-1a64 over UTF-8; offsets and total length
are UTF-16, matching CodeMirror. Only the cached authoritative server source can
anchor a remote cursor. A cursor disappears while that document has unconfirmed
writes, after its text revision changes until a new position arrives, or when
coordinates are invalid. Native rendering independently requires a matching
source fingerprint. The bridge never exports source text as part of presence.

**Ordinary Overleaf publishes a caret, not a selected range.** Its server uses a
strict schema containing only `doc_id`, `row` and `column`; adding selection keys
is rejected and can disconnect the client. Consequently received cursors have
`anchor === head`. We do not guess a range or represent the caret as a selection.
Native selections are installed using public CodeMirror selection dispatch when
that document is active, and their head is published through the exact supported
server schema. A parked tab can publish its head against an exact authoritative
source fingerprint without opening or modifying the hosted document. Invalid or
stale ephemeral cursor commands are quietly discarded.

Collaborator badges support explicit source navigation: single-click seeks once,
double-click follows, and triple-click stops. Single clicks wait through the
multi-click interval so a double/triple gesture cannot move the local caret.
Following uses fingerprint-validated projected source positions and only reveals
the viewport/folds; it never publishes the collaborator's location as our caret.
Cross-document navigation uses the existing fresh-server open/receipt barrier
and preserves the designated preview. Source input/navigation, peer departure,
disconnection or a project change cancels following. Unchanged positions reuse
the cached mapping without repeated scroll requests or idle polling.

Presence and outgoing cursor bursts are coalesced with at most one 100 ms timer
each, no idle polling, at most 128 peers and 128 total cursors. The native inbox
keeps only the newest presence snapshot; it does not compete with source/save
acknowledgements. UTF-8 hashing and source conversion are cached per authoritative
source or immutable CodeMirror Text identity; line lookup uses sparse checkpoints
and bounded recent-line caches. Disconnect, access revocation and teardown clear
all peers, cursors and timers. This feature does not add another OT writer or
change source synchronization authority.

## Primary implementation references

Checked against the official source on 25 October 2026. Collaborator protocol and
colors were verified at upstream commit
[`e039ad26c5bf5422eb57b89fc7e57c75055e631d`](https://github.com/overleaf/overleaf/commit/e039ad26c5bf5422eb57b89fc7e57c75055e631d).
These are implementation references, not guarantees about a particular Overleaf Cloud rollout.

- [Connected-user payloads, live events and own-client filtering](https://github.com/overleaf/overleaf/blob/e039ad26c5bf5422eb57b89fc7e57c75055e631d/services/web/frontend/js/features/ide-react/context/online-users-context.tsx)
- [Exact site collaborator hue algorithm](https://github.com/overleaf/overleaf/blob/e039ad26c5bf5422eb57b89fc7e57c75055e631d/services/web/frontend/js/shared/utils/colors.ts)
- [Site caret and badge color rendering](https://github.com/overleaf/overleaf/blob/e039ad26c5bf5422eb57b89fc7e57c75055e631d/services/web/frontend/js/features/source-editor/extensions/cursor-highlights.ts)
- [Cursor head row/UTF-16 column semantics](https://github.com/overleaf/overleaf/blob/e039ad26c5bf5422eb57b89fc7e57c75055e631d/services/web/frontend/js/features/source-editor/extensions/cursor-position.ts)
- [Strict server position schema: selection extensions are not accepted](https://github.com/overleaf/overleaf/blob/e039ad26c5bf5422eb57b89fc7e57c75055e631d/services/real-time/app/js/Router.js)
- [Project-wide collaborator updates and identity fields](https://github.com/overleaf/overleaf/blob/e039ad26c5bf5422eb57b89fc7e57c75055e631d/services/real-time/app/js/WebsocketController.js)
- [Exposed `editor.view`, document ID and name](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/shared/context/ide-context.tsx)
- [CodeMirror view exposure](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/ide-react/context/editor-view-context.tsx)
- [Exposed store `get` and subscribing `watch`](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/ide-react/scope-value-store/react-scope-value-store.ts)
- [CodeMirror dispatch and the two OT adapters](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/source-editor/extensions/realtime.ts)
- [Joined document lifecycle, changes and saved events](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/ide-react/editor/document-container.ts)
- [Socket creation, connection states and project join](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/ide-react/connection/connection-manager.ts)
- [Editor page loading the global Socket.IO script](https://github.com/overleaf/overleaf/blob/main/services/web/app/views/project/ide-react.pug#L28-L34)
- [Pinned Socket.IO client global initialization](https://github.com/overleaf/socket.io-client/blob/0.9.17-overleaf-6/dist/socket.io.js#L3)
- [Socket.IO shim consuming the global client](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/ide/connection/SocketIoShim.js)
- [Socket.IO fork/version](https://github.com/overleaf/overleaf/blob/main/services/real-time/package.json)
- [Session-cookie authentication](https://github.com/overleaf/overleaf/blob/main/services/real-time/app/js/SessionSockets.js)
- [Document join encoding, versioning and history-OT requirement](https://github.com/overleaf/overleaf/blob/main/services/real-time/app/js/WebsocketController.js)
- [Removal of per-document rooms and capability introduction](https://github.com/overleaf/overleaf/commit/e7148106328880201636014b2fc9e384a3be3da3)
- [Authoritative OT acknowledgement broadcasts](https://github.com/overleaf/overleaf/blob/main/services/real-time/app/js/DocumentUpdaterController.js)
- [Live tree events](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/file-tree/hooks/file-tree-socket-listener.ts)
- [File ID attributes](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/file-tree/components/file-tree-item/file-tree-item-inner.tsx)
- [Document deep links](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/features/ide-react/context/deep-link-context.tsx)
- [Authenticated download routes](https://github.com/overleaf/overleaf/blob/main/services/web/app/src/router.mjs)
- [Browser-local per-project auto-compile preference and initial compile](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/shared/context/local-compile-context.tsx)
- [Browser-local preference serialization](https://github.com/overleaf/overleaf/blob/main/services/web/frontend/js/infrastructure/local-storage.ts)
- [Text download](https://github.com/overleaf/overleaf/blob/main/services/web/app/src/Features/DocumentUpdater/DocumentUpdaterController.mjs)
- [History-OT download normalization](https://github.com/overleaf/overleaf/blob/main/services/document-updater/app/js/HttpController.js)
- [Project file-tree storage migration](https://github.com/overleaf/overleaf/blob/main/libraries/overleaf-editor-core/lib/ot_migration_stages.js)

The alternative Git bridge is documented and uses separately generated Git
authentication tokens. It is a Premium feature on Overleaf Cloud and automatic
polling can hit rate limits. It does not provide the same continuous collaborative
editing channel, so it is not used here. See
[Overleaf Git tokens](https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/git-integration/git-integration-authentication-tokens)
and [Git operation limits](https://docs.overleaf.com/integrations-and-add-ons/git-integration-and-github-synchronization/git-integration/advanced-git-operations).

## Tests

```sh
node --test scripts/test_overleaf_bridge.cjs
python3 -m unittest discover -s scripts -p 'test_overleaf_*fixture.py'
```

The tests exercise the real bridge script with a small fake official-client
surface. They cover full project hydration, save acknowledgements, collaborator
changes, stale download rejection, exact bases, Unicode, permissions, changing
files, nested deep links, live tree updates, invalid paths and cleanup. Presence
regressions cover own-client filtering, late callbacks, full replacement/leave
cleanup, Unicode positions and fingerprints, revision gating, bounded/coalesced
bursts, cached source conversion, parked native tabs, and source-free selection
publication through the strict official schema. A logged-in
Overleaf project must additionally be checked before claiming live compatibility.
