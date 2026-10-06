# Dev mode and the frame monitor

`TIPTOPTYP_DEV=1` enables verbose stderr logging for debugging the app
itself. Ordinary runs stay quiet. Like the profiling recorder, dev logs
never include document contents.

## Frame monitor (always on)

Every root UI pass feeds a small frame monitor (`src/frame_monitor.rs`),
whether or not dev mode is set. It costs a few counters per pass and logs
nothing while the app settles normally. Two things escape the quiet:

- **Slow frames** (dev mode only): passes slower than 50 ms log
  `tiptoptyp[dev] slow frame: N ms`. Dev mode also logs burst frame counts
  every 5 seconds while passes continue.
- **Repaint storms**: egui only runs a pass when something requested a
  repaint, so a burst of back-to-back passes lasting 60 seconds means a
  stuck spinner, pending state, or animation loop is burning a core. The
  monitor logs `tiptoptyp[warn] repaint storm` to stderr and shows an
  in-app notice once per burst. A gap of one second without passes ends
  the burst. Restarting the app clears the stuck state; a profiling build
  with `TIPTOPTYP_PROFILE_DIR` attributes the repaint sources.

## What dev mode does not do

Dev mode changes logging only: no extra windows, overlays, workers, or
profiling sessions. For measured wall-time scopes and native CPU samples,
use the profiling workflow in `performance.md`.
