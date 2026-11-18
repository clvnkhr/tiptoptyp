// Copyright 2026 tiptoptyp contributors
// SPDX-License-Identifier: Apache-2.0
// SPDX-License-Identifier: MIT

/// Accept only the owning native WebView's main frame. URLs and JavaScript
/// globals cannot identify the sender when a popup shares its configuration.
pub(crate) fn main_frame_sender_allowed(
    owner: usize,
    sender: Option<usize>,
    main_frame: bool,
) -> bool {
    owner != 0 && sender == Some(owner) && main_frame
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_owning_webviews_main_frame_can_reach_the_handler() {
        let owner = 1;
        assert!(main_frame_sender_allowed(owner, Some(owner), true));
        assert!(!main_frame_sender_allowed(owner, Some(owner), false));
        assert!(!main_frame_sender_allowed(owner, Some(2), true));
        assert!(!main_frame_sender_allowed(owner, Some(2), false));
        assert!(!main_frame_sender_allowed(owner, None, true));
        assert!(!main_frame_sender_allowed(owner, None, false));
        assert!(!main_frame_sender_allowed(0, Some(0), true));
    }
}
