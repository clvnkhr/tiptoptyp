# Compiled build identity

The app, `--version`, native inspection and profiling reports share one compiled
label: `<app name> <package version> (<revision><worktree suffix>.<build epoch>)`.
It performs no runtime Git queries, filesystem reads or timer work. The label is
returned by a non-inlined literal getter to limit constant propagation through
the editor's large UI codegen units when build metadata changes.

`build_support/build_identity.rs` declares the Rust, local dependency and embedded
data inputs. Their working-tree contents are compared with HEAD; staging alone
and personal documents or ordinary documentation outside those inputs do not
make the compiled app dirty. The epoch still identifies the actual build-script
execution. A binary hash remains necessary when identifying an exact executable.

Build-time Git queries disable optional locks and cannot refresh the index.
Cargo watches HEAD and its current loose ref, resolved through Git so linked
worktrees work correctly. It does not watch the index or unrelated loose refs.
For a packed or unborn ref, it watches packed-refs when present and the closest
existing ref parent to detect creation of a loose ref; it never emits a missing
Git watch path that would force Cargo to rerun forever. While this fallback is
active, unrelated changes within the same packed file or parent can still rerun
the script because Cargo's watcher uses mtimes rather than ref contents.

`tests/build_identity_inputs.rs` verifies source and staging distinctions,
untracked embedded inputs, index immutability, current branch movement, detached
HEADs, packed/unborn refs, linked worktrees and source archives without Git.
`tests/build_identity.rs` checks the actual executable's startup-free CLI output.
