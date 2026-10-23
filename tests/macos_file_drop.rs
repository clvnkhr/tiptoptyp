//! Exercise the exact pasteboard conversion used by the native webview.
//! Unique pasteboards keep these tests independent of the user's clipboard.
#![cfg(target_os = "macos")]

#[path = "../vendor/wry/src/wkwebview/drag_drop/paths.rs"]
mod paths;
