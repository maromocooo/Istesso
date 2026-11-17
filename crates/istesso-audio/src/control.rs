//! Keeps the two clocks together.
//!
//! The source produces audio at the rate of its clock, the sound card of
//! the exit consumes it at the rate of another. The difference is small (a
//! few tens of millionths) but never zero, so without correction the queue
//! between them slowly runs empty or full.
//!
//! What is measured is the **wait** of a packet: the time between its
//! arrival and the moment its first frame is played. The wait of the
//! packets that travelled fastest is what the exit holds at a target, by
//! playing a tiny bit faster when the wait is long and a tiny bit slower
//! when it is short (through the ratio of the resampler). Packets that were
//! held up on the way have a shorter wait; they say something about the
//! network, not about the clocks, and are left out by looking at the longest
//! wait of the last second only.
//!
//! The correction is the sum of a part that follows the measured wait and a
//! part that remembers (a proportional and an integral term). The second
//! one ends up equal to the difference of the two clocks.
//!
//! Three limits make the effect on pitch something that can be stated:
//!
//! * the correction never exceeds [`ControlConfig::max_correction`];
//! * it never changes faster than [`ControlConfig::max_slew_per_s`];
//! * large errors are not its business. When the wait is far from the
//!   target, the playout skips or waits (see `playout`), and the
//!   controller starts from the target again.

/// How the correction is derived from the measured wait.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ControlConfig {
    /// The wait to hold, in seconds.
    pub target_s: f64,
    /// How fast the loop reacts, in radians per second. A full swing takes
    /// about `6.3 / natural_frequency` seconds.
    pub natural_frequency: f64,
    /// 1.0 settles without overshoot.
    pub damping: f64,
    /// For how long the fastest packet is remembered, in seconds.
    pub window_s: f64,
    /// Time constant that smooths the measured wait, in seconds.
    pub smoothing_s: f64,
    /// Largest correction, as a fraction of the rate (0.001 is 0.1%).
    pub max_correction: f64,
    /// Largest change of the correction in one second.
    pub max_slew_per_s: f64,
    /// When the source's own timeline moves later (a cycle of its sound
    /// system that began late), the wait is short by that much from then
    /// on. It is taken back at this rate, as a fraction of the rate, and
    /// not by the loop above, which would answer a step of 5 ms with 0.05%.
    pub payback: f64,
    /// The most that may be owed in this way, in seconds. A timeline that
    /// moved by more is a large error like any other.
    pub max_owed_s: f64,
}

impl ControlConfig {
    /// The settings used unless a test says otherwise.
    pub fn with_target(target_s: f64) -> Self {
        ControlConfig {
            target_s,
            natural_frequency: 0.05,
            damping: 1.0,
            window_s: 1.0,
            smoothing_s: 0.5,
            max_correction: 1000e-6,
            max_slew_per_s: 100e-6,
            payback: 100e-6,
            max_owed_s: 0.020,
        }
    }
}

/// Entries the window can hold. Only waits that are longer than every later
/// one are kept, so this is reached only by a wait that falls steadily for a
/// whole window at more than a thousand callbacks a second.
const WINDOW_ENTRIES: usize = 1024;

/// The longest wait seen within the last `window_s` seconds.
struct Window {
    /// Times and waits; the waits fall from front to back.
    entries: Box<[(f64, f64); WINDOW_ENTRIES]>,
    front: usize,
    len: usize,
}

impl Window {
    fn new() -> Self {
        Window {
            entries: Box::new([(0.0, 0.0); WINDOW_ENTRIES]),
            front: 0,
            len: 0,
        }
    }

    fn clear(&mut self) {
        self.front = 0;
        self.len = 0;
    }

    fn at(&self, offset: usize) -> (f64, f64) {
        self.entries[(self.front + offset) % WINDOW_ENTRIES]
    }

    fn push(&mut self, time_s: f64, wait_s: f64) {
        // A wait that is at least as long as an earlier one makes the
        // earlier one irrelevant: it leaves the window sooner.
        while self.len > 0 && self.at(self.len - 1).1 <= wait_s {
            self.len -= 1;
        }
        if self.len == WINDOW_ENTRIES {
            self.front = (self.front + 1) % WINDOW_ENTRIES;
            self.len -= 1;
        }
        self.entries[(self.front + self.len) % WINDOW_ENTRIES] = (time_s, wait_s);
        self.len += 1;
    }

    /// Adds `by_s` to every wait that is remembered.
    fn shift(&mut self, by_s: f64) {
        for offset in 0..self.len {
            self.entries[(self.front + offset) % WINDOW_ENTRIES].1 += by_s;
        }
    }

    fn longest(&mut self, now_s: f64, window_s: f64) -> Option<f64> {
        while self.len > 0 && self.at(0).0 < now_s - window_s {
            self.front = (self.front + 1) % WINDOW_ENTRIES;
            self.len -= 1;
        }
        (self.len > 0).then(|| self.at(0).1)
    }
}

/// Turns measured waits into the correction of the playback rate.
pub struct Controller {
    cfg: ControlConfig,
    proportional_gain: f64,
    integral_gain: f64,
    window: Window,
    /// The smoothed difference between the wait and its target.
    error: Option<f64>,
    /// The part of the correction that remembers.
    integral: f64,
    correction: f64,
    /// By how much the wait may be short of the target just now, because
    /// the source's timeline moved later. Paid back steadily.
    owed_s: f64,
}

impl Controller {
    pub fn new(cfg: ControlConfig) -> Self {
        Controller {
            cfg,
            proportional_gain: 2.0 * cfg.damping * cfg.natural_frequency,
            integral_gain: cfg.natural_frequency * cfg.natural_frequency,
            window: Window::new(),
            error: None,
            integral: 0.0,
            correction: 0.0,
            owed_s: 0.0,
        }
    }

    pub fn config(&self) -> &ControlConfig {
        &self.cfg
    }

    /// Reports the wait of a packet that just arrived.
    pub fn observe(&mut self, now_s: f64, wait_s: f64) {
        self.window.push(now_s, wait_s);
    }

    /// Says that the source's timeline moved by `by_s` seconds (later when
    /// positive): from now on its audio arrives that much later than its
    /// frame count alone would have it. That is not the clocks drifting
    /// apart, and it is not answered as if it were: the wait is allowed to
    /// be short by that much, and the allowance shrinks at
    /// [`ControlConfig::payback`].
    pub fn source_slipped(&mut self, by_s: f64) {
        let owed = (self.owed_s + by_s).clamp(0.0, self.cfg.max_owed_s);
        // The waits remembered were measured on the timeline as it was.
        self.window.shift(self.owed_s - owed);
        self.owed_s = owed;
    }

    /// By how much the wait may be short of the target just now.
    pub fn owed(&self) -> f64 {
        self.owed_s
    }

    /// Advances by `dt_s` seconds and returns the correction to apply: the
    /// playback rate is the nominal one times `1 + correction`. A positive
    /// correction takes audio out of the queue more slowly.
    pub fn step(&mut self, now_s: f64, dt_s: f64) -> f64 {
        let Some(wait) = self.window.longest(now_s, self.cfg.window_s) else {
            // Nothing arrived lately. That says nothing about the clocks.
            return self.correction;
        };
        let raw = wait - (self.cfg.target_s - self.owed_s);
        // What is owed is paid back by playing a little slower for a while.
        let paying = if self.owed_s > 0.0 {
            self.owed_s = (self.owed_s - self.cfg.payback * dt_s).max(0.0);
            self.cfg.payback
        } else {
            0.0
        };
        let error = match self.error {
            None => raw,
            Some(previous) => {
                let blend = 1.0 - (-dt_s / self.cfg.smoothing_s).exp();
                previous + (raw - previous) * blend
            }
        };
        self.error = Some(error);

        let limit = self.cfg.max_correction;
        // Waiting too long: take audio out faster, which is a negative
        // correction.
        let proportional = -self.proportional_gain * error;
        let integral = (self.integral - self.integral_gain * error * dt_s).clamp(-limit, limit);
        let wanted = (integral + proportional + paying).clamp(-limit, limit);
        // While the output is held at a limit, remembering more of the same
        // would only have to be forgotten again later.
        let pushing_further = (wanted >= limit && integral > self.integral)
            || (wanted <= -limit && integral < self.integral);
        if !pushing_further {
            self.integral = integral;
        }

        let step = self.cfg.max_slew_per_s * dt_s;
        self.correction = wanted.clamp(self.correction - step, self.correction + step);
        self.correction
    }

    /// Forgets the measured waits, after the playout has put the wait back
    /// to the target itself. What was learnt about the clocks is kept.
    pub fn restart(&mut self) {
        self.window.clear();
        self.error = None;
        self.owed_s = 0.0;
    }

    /// The correction returned by the last step.
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// The part of the correction that remembers: the estimate of how much
    /// faster the sound card's clock runs than the source's.
    pub fn drift(&self) -> f64 {
        self.integral
    }

    /// The longest wait of the last window, if a packet arrived in it.
    pub fn wait(&mut self, now_s: f64) -> Option<f64> {
        self.window.longest(now_s, self.cfg.window_s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f64 = 512.0 / 48_000.0;

    /// A queue whose wait changes by `correction - drift` every second, and
    /// a controller looking at it.
    fn run(drift: f64, seconds: f64, noise_s: impl Fn(u64) -> f64) -> (Controller, Vec<f64>, f64) {
        let cfg = ControlConfig::with_target(0.030);
        let mut controller = Controller::new(cfg);
        let mut wait = cfg.target_s;
        let mut corrections = Vec::new();
        let steps = (seconds / DT) as u64;
        for step in 0..steps {
            let now = step as f64 * DT;
            controller.observe(now, wait + noise_s(step));
            let correction = controller.step(now, DT);
            wait += (correction - drift) * DT;
            corrections.push(correction);
        }
        (controller, corrections, wait)
    }

    #[test]
    fn the_correction_settles_on_the_difference_of_the_clocks() {
        for drift in [100e-6, -100e-6, 20e-6, 0.0] {
            let (controller, corrections, wait) = run(drift, 600.0, |_| 0.0);
            let last = *corrections.last().unwrap();
            assert!(
                (last - drift).abs() < 1e-6,
                "drift {drift}: correction {last}"
            );
            assert!((controller.drift() - drift).abs() < 1e-6);
            assert!((wait - 0.030).abs() < 20e-6, "wait {wait}");
        }
    }

    #[test]
    fn the_wait_stays_close_to_the_target_while_the_drift_is_learnt() {
        let cfg = ControlConfig::with_target(0.030);
        let mut controller = Controller::new(cfg);
        let mut wait = cfg.target_s;
        let mut worst: f64 = 0.0;
        for step in 0..(600.0 / DT) as u64 {
            let now = step as f64 * DT;
            controller.observe(now, wait);
            wait += (controller.step(now, DT) - 100e-6) * DT;
            worst = worst.max((wait - cfg.target_s).abs());
        }
        // 100 ppm is a tenth of a millisecond a second. The loop is slow,
        // but never lets more than about a millisecond build up.
        assert!(worst < 1.2e-3, "worst deviation {worst}");
    }

    #[test]
    fn the_correction_is_bounded_in_size_and_in_speed() {
        // A wait that is wrong by far more than the playout would allow.
        let cfg = ControlConfig::with_target(0.030);
        let mut controller = Controller::new(cfg);
        let mut previous = 0.0;
        for step in 0..20_000u64 {
            let now = step as f64 * DT;
            let wait = if step < 10_000 { 0.5 } else { -0.5 };
            controller.observe(now, wait);
            let correction = controller.step(now, DT);
            assert!(correction.abs() <= cfg.max_correction + 1e-12);
            assert!((correction - previous).abs() <= cfg.max_slew_per_s * DT + 1e-12);
            previous = correction;
        }
        // Held at the limit, it has not remembered more than the limit, and
        // comes back as fast as it is allowed to once the error turns.
        assert!(controller.drift().abs() <= cfg.max_correction);
    }

    #[test]
    fn packets_that_were_held_up_do_not_move_the_correction() {
        // Every tenth packet arrives 20 ms late: its wait is 20 ms shorter.
        let (_, corrections, _) = run(
            50e-6,
            600.0,
            |step| {
                if step % 10 == 0 {
                    -0.020
                } else {
                    0.0
                }
            },
        );
        let settled = &corrections[corrections.len() / 2..];
        for correction in settled {
            assert!((correction - 50e-6).abs() < 2e-6, "correction {correction}");
        }
    }

    #[test]
    fn nothing_arriving_holds_the_correction() {
        let cfg = ControlConfig::with_target(0.030);
        let mut controller = Controller::new(cfg);
        for step in 0..2000u64 {
            let now = step as f64 * DT;
            controller.observe(now, 0.031);
            controller.step(now, DT);
        }
        // No more packets. For as long as the last ones are remembered, the
        // controller goes on as before; after that it holds still.
        for step in 2000..2200u64 {
            controller.step(step as f64 * DT, DT);
        }
        let before = controller.correction();
        let drift = controller.drift();
        for step in 2200..3000u64 {
            let now = step as f64 * DT;
            assert_eq!(controller.step(now, DT), before);
        }
        assert_eq!(controller.drift(), drift);
        assert_eq!(controller.wait(3000.0 * DT), None);
    }

    #[test]
    fn restarting_keeps_what_was_learnt_about_the_clocks() {
        let (mut controller, _, _) = run(80e-6, 600.0, |_| 0.0);
        controller.restart();
        assert!((controller.drift() - 80e-6).abs() < 1e-6);
        // The old waits are gone: a new measurement counts from scratch.
        assert_eq!(controller.wait(600.0), None);
        controller.observe(600.0, 0.030);
        let correction = controller.step(600.0, DT);
        assert!((correction - 80e-6).abs() < 2e-6);
    }

    #[test]
    fn the_window_keeps_the_longest_wait_of_the_last_second() {
        let mut window = Window::new();
        assert_eq!(window.longest(0.0, 1.0), None);
        window.push(0.0, 5.0);
        window.push(0.1, 3.0);
        window.push(0.2, 4.0);
        assert_eq!(window.longest(0.2, 1.0), Some(5.0));
        // The longest one leaves; the next longest of those still inside
        // takes over, not the one that was pushed out by a later, longer one.
        assert_eq!(window.longest(1.05, 1.0), Some(4.0));
        assert_eq!(window.longest(1.25, 1.0), None);
        // More entries than fit: the oldest are let go.
        for i in 0..(2 * WINDOW_ENTRIES) {
            window.push(2.0 + i as f64 * 1e-4, 10.0 - i as f64 * 1e-3);
        }
        let end = 2.0 + (2 * WINDOW_ENTRIES) as f64 * 1e-4;
        assert!(window.longest(end, 1.0).is_some());
    }
}
