//! Opt-in, bounded gesture diagnostics. Never records document names or text.
use super::*;
use std::collections::VecDeque;

const CAPACITY: usize = 32;
const SAMPLE_INTERVAL: f64 = 0.1;

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        let enabled = std::env::var("TIPTOPTYP_TAB_TRACE").as_deref() == Ok("1");
        if enabled {
            eprintln!(
                "ui.tabs.trace version=2 pid={} executable={:?}",
                std::process::id(),
                std::env::current_exe().ok()
            );
        }
        enabled
    })
}

#[derive(Default)]
pub(super) struct Trace {
    lines: VecDeque<String>,
    last_sample: Option<f64>,
    active: bool,
    omitted: usize,
}

impl Trace {
    pub(super) fn sample(&mut self, context: &egui::Context) -> bool {
        if !enabled() {
            return false;
        }
        let viewport = context.viewport_id();
        context.input(|input| {
            let pressed = input.pointer.button_pressed(egui::PointerButton::Primary);
            let released = input.pointer.button_released(egui::PointerButton::Primary);
            if pressed {
                self.flush("next-press");
                self.active = true;
                eprintln!(
                    "ui.tabs.press viewport={:?} pos={:?}",
                    viewport,
                    input.pointer.latest_pos()
                );
            }
            let sample = sample_due(
                self.last_sample,
                input.time,
                pressed || released,
                self.active && input.pointer.delta() != Vec2::ZERO,
            );
            if sample {
                self.last_sample = Some(input.time);
            }
            if self.active && !input.pointer.primary_down() && !released {
                self.flush("lost-release");
            }
            sample
        })
    }

    pub(super) fn record(&mut self, context: &egui::Context, details: String) {
        let viewport = context.viewport_id();
        let owner = context.dragged_id();
        let (released, line) = context.input(|input| {
            let released = input.pointer.button_released(egui::PointerButton::Primary);
            (released, format!(
                "ui.tabs.sample viewport={:?} t={:.3} pos={:?} origin={:?} down={} released={} threshold={} focused={:?} owner={:?} window={:?} {details}",
                viewport, input.time, input.pointer.latest_pos(), input.pointer.press_origin(),
                input.pointer.primary_down(), released, input.pointer.is_decidedly_dragging(),
                input.viewport().focused, owner, input.viewport().outer_rect,
            ))
        });
        self.push(line);
        if released {
            self.flush("release");
        }
    }

    pub(super) fn native_drag(&mut self, context: &egui::Context) {
        if enabled() {
            eprintln!(
                "ui.tabs.native-window-drag viewport={:?}",
                context.viewport_id()
            );
            self.flush("native-window-drag");
        }
    }

    fn push(&mut self, line: String) {
        if self.lines.len() == CAPACITY {
            self.lines.pop_front();
            self.omitted += 1;
        }
        self.lines.push_back(line);
    }

    fn flush(&mut self, reason: &str) {
        if !self.lines.is_empty() {
            eprintln!(
                "ui.tabs.gesture end={reason} omitted={}\n{}",
                self.omitted,
                self.lines
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        self.lines.clear();
        self.omitted = 0;
        self.active = false;
    }
}

fn sample_due(last: Option<f64>, now: f64, edge: bool, moved: bool) -> bool {
    edge || (moved && last.is_none_or(|last| now - last >= SAMPLE_INTERVAL))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movement_is_throttled_but_button_edges_are_retained() {
        assert!(!sample_due(None, 0.0, false, false));
        assert!(sample_due(None, 0.0, true, false));
        assert!(!sample_due(Some(0.0), 0.01, false, true));
        assert!(sample_due(Some(0.0), 0.01, true, false));
        assert!(sample_due(Some(0.0), 0.11, false, true));
        assert!(!sample_due(Some(0.0), 60.0, false, false));
    }

    #[test]
    fn long_gestures_keep_only_a_bounded_tail() {
        let mut trace = Trace::default();
        for index in 0..1000 {
            trace.push(index.to_string());
        }
        assert_eq!(trace.lines.len(), CAPACITY);
        assert_eq!(trace.omitted, 1000 - CAPACITY);
        assert_eq!(trace.lines.back().unwrap(), "999");
    }
}
