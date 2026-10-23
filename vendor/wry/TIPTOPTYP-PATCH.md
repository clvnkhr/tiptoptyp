# Local Wry patch

Source: crates.io Wry 0.57.0, MIT OR Apache-2.0, exactly the previously locked
version. Registry archive SHA-256:
`a819957a01b3119af85e638a38d242af76dbc87d130dca67bfd0441072e21ff0`.
Upstream package VCS receipt identifies commit
`792d0359ba6501a4fc360ece17de2ae42329a47c` and marks that package tree dirty.

On macOS, `build_as_child` previously called `NSApplication.activate` (or
`activateIgnoringOtherApps` on older systems) unconditionally. `with_focused`
is explicitly unsupported on macOS and cannot suppress this activation.

The activation patch skips activation for child views. The existing parent
window continues to own activation and keyboard focus. Standalone webviews keep
the upstream activation behavior. No scheduling policy, timers, WebKit drawing,
navigation, event handling, or other platform behavior changes.

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
read or write the general clipboard.
