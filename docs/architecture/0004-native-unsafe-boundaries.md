# Native unsafe boundaries

Every tiptoptyp Rust target denies unsafe code. Four platform adapter modules
in the executable receive narrowly scoped exceptions because they cross APIs
whose contracts Rust cannot express:

- `app_icon` calls AppKit's application-icon setter with a decoded, non-null
  `NSImage`. Winit deliberately ignores per-window icons on macOS.
- `native_window` borrows retained AppKit/Win32 handles for Wry and rfd, calls
  Win32's read-only active-window query, and enables a WKWebView property on
  eframe's main UI thread.
- `native_menu` adds one typed command selector to winit's existing AppKit
  delegate and connects retained native menu items to that delegate.
- `open_requests` adds Finder's typed `application:openURLs:` selector to the
  same delegate without replacing winit's lifecycle integration.

Every unsafe operation is adjacent to a `SAFETY` explanation. The crate-level
`unsafe_code` lint prevents new unsafe code from appearing outside these four
modules, while `unsafe_op_in_unsafe_fn` requires even their unsafe callbacks to
spell out any operation that relies on an unchecked contract.

The Objective-C selector injection cannot currently be replaced by a safe
crate API: winit owns and has already registered the application delegate, so
objc2's safe class-definition path cannot augment it. The raw-window-handle
borrows are required by those traits, and the AppKit/WebKit generated bindings
mark their relevant setters unsafe. Keep these adapters small rather than
expanding their allowlisted scope.
