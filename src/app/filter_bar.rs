//! Shared scroll-away filter placement; never overrides ordinary wheel scrolling.
#[derive(Clone, Copy)]
pub(super) struct ScrollLayout {
    pub(super) sticky: bool,
    pub(super) height: f32,
}

/// Translate the content offset only when the toolbar moves in or out of the
/// scroll area. Applying an offset on ordinary frames would fight the wheel.
pub(super) fn initial_offset(
    previous: Option<ScrollLayout>,
    offset: Option<f32>,
    sticky: bool,
    height: f32,
) -> Option<f32> {
    match (previous, offset) {
        (_, None) | (None, _) => Some(if sticky { 0.0 } else { height }),
        (Some(previous), Some(offset)) if previous.sticky != sticky => Some(if sticky {
            (offset - previous.height).max(0.0)
        } else {
            offset + height
        }),
        _ => None,
    }
}
