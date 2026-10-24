# Headless Overleaf connection

The native editor loads the real Overleaf editor in a background WebView. Sign-in
is done on Overleaf's own page. Its browser data store retains the signed-in
session; the app does not request, extract, log, or save a password or session
cookie. Files shown in Explorer come from the connected project. Local project
files are a cache of server-confirmed content, rather than a second authority.

This is an integration with Overleaf's internal browser interfaces, not a public
Overleaf API. The interface may change. The bridge fails visibly when its expected
project tree or editor interface is absent. A successful fake-server or pure test
is not proof of compatibility with the user's signed-in Cloud project.

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
The native acknowledgement contains the current complete text after any
collaborator transformations. An `applyOtUpdate` RPC callback alone is **not** an
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

The connection window has a project-specific **Keep local copy** choice. It is
on by default. With no custom folder, macOS uses
`~/Library/Application Support/dev.tiptoptyp.editor/overleaf/`; other platforms
use their application-data directory. A chosen folder is a **base**, not the
project root: the app appends `<origin-hash>-<project-id>` so projects from
different Overleaf servers cannot share a mirror accidentally. Preparing folders
and the file picker do not block the UI. Changing the folder or storage mode
requires closing that project's open tabs first.

Settings JSON exposes the choices in `overleaf_storage`, keyed by canonical
project URL. Each value contains `keep_local_copy` and nullable `directory`.
At most 32 explicit project preferences are retained. Folder paths must be
absolute and contain no control characters; validation normalizes `.` and `..`
without filesystem work on the UI thread. Authentication still lives only in the
WebView's separate browser-data store.

Turning local copies off creates a unique private temporary directory. The UI
uses the project name and project-relative file labels instead of showing its
physical path; hovering the workspace root reveals that path. Managed mirrors
are excluded from ordinary local-workspace and last-file history. The project
URL, rather than a disposable folder path, is the durable recent-project entry.
The temporary directory stays alive while its workspace, tabs, unsynced drafts,
or mirror jobs still need it. Releasing the last lease schedules deletion on a
background worker. Selected base folders and persistent copies are never removed
by that cleanup. Normal shutdown waits up to ten seconds for outstanding cleanup
and reports an incomplete cleanup; forced termination can leave temporary files.

Each managed root has a bounded version-1 descriptor at
`.tiptoptyp/overleaf-project.json` containing its project URL and persistence
mode, without credentials. Secure descriptor discovery is bounded and runs on an
explicit-open worker. A reopened managed root or a contained file reconnects to
its project instead of treating old disk contents as an ordinary local document.
Configured and previous default roots are also recognized without per-frame
filesystem probes. Discovery cannot steal navigation from an editor that changed
while the worker was running. Missing authentication or a sync failure leaves
managed source unavailable for editing; it never silently admits the cached copy.

Connecting and reconnecting start an editing barrier. The latest project tree,
all current text documents, and binary assets must be obtained from the server,
accepted for the current connection generation, and successfully written to the
working copy before the project becomes **Ready**. Stale tree/file receipts,
including receipts from before a rename or reconnect, cannot open that barrier.
Native typing, paste, replacement, generated insertions and formatter replies
cannot mutate locked source, while selection and ordinary widget editing remain
available. Existing unsynced drafts continue to use the conflict-preserving save
rules above. The mirror remains a compiler/LSP input; Overleaf remains the source
of truth whether the local files are retained or temporary.

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

Presence and outgoing cursor bursts are coalesced with at most one 100 ms timer
each, no idle polling, at most 128 peers and 128 total cursors. The native inbox
keeps only the newest presence snapshot; it does not compete with source/save
acknowledgements. UTF-8 hashing and source conversion are cached per authoritative
source or immutable CodeMirror Text identity; line lookup uses sparse checkpoints
and bounded recent-line caches. Disconnect, access revocation and teardown clear
all peers, cursors and timers. This feature does not add another OT writer or
change source synchronization authority.

## Primary implementation references

Checked against the official source on 24 October 2026. Collaborator protocol and
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
