use std::{hint::black_box, time::Instant};
mod before {
    //! Content-coordinate tab motion. Scrolling changes the strip's origin, not
    //! these positions. Drop slots stay fixed; controls follow the visible tabs.
    const DURATION: f64 = 0.14;

    pub(super) struct Motion {
        x: f32,
        from: f32,
        target: f32,
        started: f64,
        grabbed: bool,
        lifted: bool,
    }
    impl Motion {
        pub(super) fn new(x: f32) -> Self {
            Self {
                x,
                from: x,
                target: x,
                started: 0.0,
                grabbed: false,
                lifted: false,
            }
        }
        pub(super) fn update(&mut self, target: f32, grabbed: Option<f32>, now: f64) -> f32 {
            if let Some(x) = grabbed {
                self.x = x;
                self.grabbed = true;
                self.lifted = true;
                self.target = target;
                return x;
            }
            if self.grabbed || target != self.target {
                self.from = self.x;
                self.started = now;
                self.target = target;
                self.grabbed = false;
            }
            let t = ((now - self.started) / DURATION).clamp(0.0, 1.0) as f32;
            self.x = if t >= 1.0 {
                target
            } else {
                self.from + (target - self.from) * (1.0 - (1.0 - t).powi(3))
            };
            if self.x == self.target {
                self.lifted = false;
            }
            self.x
        }
        pub(super) fn lifted(&self) -> bool {
            self.lifted
        }
        pub(super) fn moving(&self) -> bool {
            !self.grabbed && self.x != self.target
        }
    }
}
#[path = "../../src/app/tabs/motion.rs"]
mod after;
trait Moving {
    fn new(x: f32) -> Self;
    fn update(&mut self, target: f32, grabbed: Option<f32>, now: f64) -> f32;
    fn lifted(&self) -> bool;
    fn moving(&self) -> bool;
}
macro_rules! implement {
    ($module:ident) => {
        impl Moving for $module::Motion {
            fn new(x: f32) -> Self {
                Self::new(x)
            }
            fn update(&mut self, t: f32, g: Option<f32>, n: f64) -> f32 {
                Self::update(self, t, g, n)
            }
            fn lifted(&self) -> bool {
                Self::lifted(self)
            }
            fn moving(&self) -> bool {
                Self::moving(self)
            }
        }
    };
}
implement!(before);
implement!(after);
fn input(frame: usize, tab: usize) -> (f32, Option<f32>, f64) {
    let target = (((frame / 36 + tab * 3) % 16) * 48) as f32;
    let grab = (frame % 144 < 8 && tab == (frame / 144) % 32).then_some((frame % 720) as f32);
    (target, grab, frame as f64 / 240.0)
}
#[inline(never)]
fn run<M: Moving>() -> u64 {
    let mut tabs: [M; 32] = std::array::from_fn(|i| M::new((i * 48) as f32));
    let mut checksum = 0u64;
    for frame in 0..50_000 {
        for (tab, motion) in tabs.iter_mut().enumerate() {
            let (t, g, n) = input(black_box(frame), tab);
            let x = motion.update(t, g, n);
            checksum = checksum
                .wrapping_add(x.to_bits() as u64 + motion.lifted() as u64 + motion.moving() as u64);
        }
    }
    black_box(checksum)
}
fn main() {
    let mut a: [before::Motion; 32] = std::array::from_fn(|i| before::Motion::new((i * 48) as f32));
    let mut b: [after::Motion; 32] = std::array::from_fn(|i| after::Motion::new((i * 48) as f32));
    for frame in 0..10_000 {
        for tab in 0..32 {
            let (t, g, n) = input(frame, tab);
            assert_eq!(
                a[tab].update(t, g, n).to_bits(),
                b[tab].update(t, g, n).to_bits()
            );
            assert_eq!(a[tab].lifted(), b[tab].lifted());
            assert_eq!(a[tab].moving(), b[tab].moving());
        }
    }
    println!(
        "equivalent_updates=320000 before_bytes={} after_bytes={}",
        std::mem::size_of::<before::Motion>(),
        std::mem::size_of::<after::Motion>()
    );
    for _ in 0..2 {
        assert_eq!(run::<before::Motion>(), run::<after::Motion>());
    }
    for sample in 0..8 {
        for after in if sample % 2 == 0 {
            [false, true]
        } else {
            [true, false]
        } {
            let now = Instant::now();
            let sum = if after {
                run::<after::Motion>()
            } else {
                run::<before::Motion>()
            };
            println!(
                "after={after} sample={sample} ns={} checksum={sum}",
                now.elapsed().as_nanos()
            );
        }
    }
}
