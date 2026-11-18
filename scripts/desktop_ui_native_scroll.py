"""Bounded native wheel setup with measured movement and frame acknowledgments."""
import math
import time


def _current_scroll(state, section):
    scroll = state.get('scrolls', {}).get(section)
    frame = state.get('document', {}).get('frame')
    if (not isinstance(scroll, dict) or type(frame) is not int or scroll.get('frame') != frame
            or any(type(scroll.get(key)) not in (int, float) or not math.isfinite(scroll[key])
                   for key in ('x', 'y', 'offset', 'content_height', 'viewport_height'))
            or scroll['offset'] < 0 or scroll['viewport_height'] <= 0):
        return None
    return scroll


def native_scroll_advance(journey, section, *, lines, minimum_advance=20, minimum_offset=0):
    """Post at most four steps; a posted event alone never establishes scrolling."""
    state = journey.snapshot()
    scroll = _current_scroll(state, section)
    if scroll is None:
        raise AssertionError(section + ' has no current scroll geometry')
    before = scroll['offset']
    threshold = max(before + minimum_advance, minimum_offset)
    for attempt in range(1, 5):
        frame = state['document']['frame']
        journey.native('move', scroll['x'], scroll['y'])
        deadline = time.monotonic() + 0.75
        while True:
            state = journey.snapshot()
            current = _current_scroll(state, section)
            if current is not None and state['document']['frame'] > frame:
                scroll = current
                break
            if time.monotonic() >= deadline:
                raise AssertionError(section + ' did not acknowledge pointer movement in a fresh frame')
            time.sleep(0.025)
        posted_frame = state['document']['frame']
        journey.native('wheel', scroll['x'], scroll['y'], lines)
        deadline = time.monotonic() + 0.75
        acknowledged = False
        while True:
            state = journey.snapshot()
            current = _current_scroll(state, section)
            if current is not None and state['document']['frame'] > posted_frame:
                acknowledged = True
                scroll = current
                if scroll['offset'] > threshold:
                    journey.record('native.scroll.attempt', section=section, attempt=attempt,
                                   before=before, after=scroll['offset'], advanced=True)
                    return before, scroll['offset'], attempt
            if time.monotonic() >= deadline:
                break
            time.sleep(0.025)
        journey.record('native.scroll.attempt', section=section, attempt=attempt,
                       before=before, after=scroll['offset'], advanced=False)
        if not acknowledged:
            raise AssertionError(section + ' did not publish a fresh frame after native wheel input')
    raise AssertionError(f'{section} did not scroll past {threshold} after 4 native wheel steps: '
                         f'{before} -> {scroll["offset"]}')
