# Dev mode and the frame monitor

`TIPTOPTYP_DEV=1` enables verbose stderr logging for debugging the app
itself. Logs never include document contents.

## Frame monitor (always on)

Each document window owns a fixed-size monitor (`src/frame_monitor.rs`). Its
actual UI passes are observed, including deferred secondary windows; painting
a parent or sibling does not advance that window's counters. Hidden windows
have no monitored UI passes. The monitor does not request repaints, retain
input events, write files or run a worker.

- **Slow frames** (dev mode only): passes slower than 50 ms log
  `tiptoptyp[dev] slow frame: N ms`. Dev mode also logs burst frame counts
  every 5 seconds while passes continue. A one-second gap resets the burst
  and its reporting cadence.
- **Sustained redraws without input**: after 60 seconds of continuous passes
  without input, the monitor logs `tiptoptyp[warn] sustained redraw` and shows
  an informational notice in that same document window once per interval.
  Typing, pointer movement, wheel input and held keys or drags reset this
  interval. A one-second gap also resets it. Terminal output, background work
  and animation can legitimately produce continuous redraws, so the notice
  does not diagnose a stuck app or recommend restarting. Activity can help
  identify ongoing work; a profiling build with `TIPTOPTYP_PROFILE_DIR`
  attributes repaint sources.

## What dev mode does not do

Dev mode changes logging only: no extra windows, overlays, workers, or
profiling sessions. For measured wall-time scopes and native CPU samples,
use the profiling workflow in `performance.md`.
