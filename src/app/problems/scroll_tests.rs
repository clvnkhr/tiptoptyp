use super::{ScrollLayout, initial_offset};

#[test]
fn first_layout_hides_exact_toolbar_height_or_starts_at_top_when_sticky() {
    assert_eq!(initial_offset(None, None, false, 28.0), Some(28.0));
    assert_eq!(initial_offset(None, Some(460.0), false, 28.0), Some(28.0));
    assert_eq!(initial_offset(None, None, true, 28.0), Some(0.0));
    assert_eq!(initial_offset(None, Some(460.0), true, 28.0), Some(0.0));
}

#[test]
fn lost_scroll_state_reinitializes_once_even_when_layout_survives() {
    let previous = Some(ScrollLayout {
        sticky: false,
        height: 28.0,
    });
    assert_eq!(initial_offset(previous, None, false, 52.0), Some(52.0));
    assert_eq!(initial_offset(previous, None, true, 52.0), Some(0.0));
}

#[test]
fn entering_sticky_preserves_the_list_offset_below_the_toolbar() {
    let previous = Some(ScrollLayout {
        sticky: false,
        height: 28.0,
    });
    // A wrapped toolbar's new height does not alter the old list position.
    assert_eq!(
        initial_offset(previous, Some(128.0), true, 52.0),
        Some(100.0)
    );
    assert_eq!(initial_offset(previous, Some(28.0), true, 52.0), Some(0.0));
    // The user may have scrolled into the toolbar before focusing its input.
    assert_eq!(initial_offset(previous, Some(10.0), true, 52.0), Some(0.0));
}

#[test]
fn leaving_sticky_and_returning_preserves_a_deep_list_anchor() {
    let sticky = Some(ScrollLayout {
        sticky: true,
        height: 28.0,
    });
    let offset = initial_offset(sticky, Some(240.0), false, 52.0).unwrap();
    assert_eq!(offset, 292.0);
    let hidden = Some(ScrollLayout {
        sticky: false,
        height: 52.0,
    });
    assert_eq!(
        initial_offset(hidden, Some(offset), true, 28.0),
        Some(240.0)
    );
}

#[test]
fn steady_layout_never_forces_a_scroll_offset_including_after_wrapping() {
    for sticky in [false, true] {
        let previous = Some(ScrollLayout {
            sticky,
            height: 28.0,
        });
        for offset in [0.0, 10.0, 28.0, 320.0] {
            for height in [28.0, 52.0] {
                assert_eq!(initial_offset(previous, Some(offset), sticky, height), None);
            }
        }
    }
}
