# Local Wry patch

Source: crates.io Wry 0.57.1, MIT OR Apache-2.0. Registry archive SHA-256:
`759c50ddad0ef9913bb5496cb9d509a50433a3b78cb0238d61105526589cf0e9`.
Upstream package VCS receipt identifies commit
`2446feadae623d6f2ac8d2136e3ccef3d7197e0e` and marks that package tree dirty.

Upstream 0.57.1 activates only visible, focused webviews. The local policy also
requires a standalone view: creating an embedded child must not activate its
application even when those visibility and focus defaults are true.

The existing parent window continues to own activation and keyboard focus.
Standalone webviews keep the upstream visible-and-focused condition. This
activation change does not alter scheduling, timers, WebKit drawing, navigation,
or other platform behavior.

The pure creation policy is included by the application's deterministic tests;
a syntax-tree regression also checks both native activation calls remain inside
that policy guard. Real-app coverage opens full and partial Tinymist while its
document window is inactive and checks that the current window/app keeps focus.

Only library/build sources, manifest, provenance, and licenses are retained.
Example targets are removed from the local manifest because examples are omitted.
All dependency versions and features remain unchanged.

The macOS warning fixes register the build script's existing
`macos_12_unavailable` configuration, use the current `NSEventModifierFlags`
constant names, and remove unsafe blocks from APIs that are now safe in objc2.
File drops read NSURL pasteboard objects instead of the deprecated filenames
property list, retaining every file URL and its filesystem bytes while rejecting
web URLs and malformed items. Isolated-pasteboard unit tests cover multiple
files, spaces, Unicode, literal URL characters, and non-file input; they never
read or write the general clipboard. The upstream 0.57.1 URL-reading fix is
covered by this local byte-preserving reader; its deprecated string fallback is
not restored.

The rebase includes upstream 0.57.1's shared WKUserContentController ownership
fix: popup webviews do not duplicate initialization scripts or replace their
opener's IPC handler, and the handler remains installed until the last sharing
view is dropped. The local main-frame sender checks below remain in force for
that shared handler.

Release builds import `NSObject` only when debug assertions, developer tools,
or proxy support use it. The download-failure callback keeps its native error
parameter and debug logging while marking that parameter intentionally unused
in release builds. These compile-time changes remove release-only warnings;
WebKit behavior and dependency features are unchanged.

The opt-in `with_ipc_handler_for_main_frame` builder method restricts macOS IPC
admission to the owning WKWebView's main frame before reading the message body.
Native sender identity and frame metadata are checked independently of the URL,
so a popup inheriting a content controller cannot act as its opener even when
both show the same project. The owner is stored only as an identity, avoiding a
WebView/content-controller retain cycle. Existing `with_ipc_handler` callers keep
their upstream behavior. On Windows the owning WebView2 `WebMessageReceived`
handler already receives only top-document messages; iframe messages use a
separate frame event. The new API is available only on these two supported
platforms. Popup navigation and browser session storage are unchanged. Invalid
IPC tracing reports only a generic error, never a message body.
