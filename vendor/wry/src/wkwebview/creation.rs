// Copyright 2020-2023 Tauri Programme within The Commons Conservancy
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

/// A child view belongs to an existing native window. Creating it must never
/// make that application active; standalone webview activation is unchanged.
pub(super) const fn should_activate_application(is_child: bool) -> bool {
    !is_child
}
