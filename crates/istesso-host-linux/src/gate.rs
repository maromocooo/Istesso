//! The gate between the compositor and the encoder.
//!
//! Every frame the compositor makes passes here on its way to the colour
//! conversion and the encoder, on the compositor's own thread and by a
//! plain call: the gate adds no thread and no queue to the frame's path.
//! It is the one place that decides whether a frame goes on:
//!
//! * While the gate is **shut** (the picture is paused, or no encoder could
//!   be had) no frame goes on. Whoever shuts it has, when [`Gate::shut`]
//!   returns, the promise that no frame is on its way into the encoder and
//!   none will follow: the encoder can be closed at once, also while the
//!   picture stands still and no frame comes.
//! * What the encoder's side answers to a frame **never reaches the
//!   compositor**. A compositor that is told of a failure ends, and the
//!   application loses the place it is shown in. A frame that is refused
//!   shuts the gate, and the caller is told why ([`Went::Refused`]).
//! * The gate **keeps the last frame**. It is the first to go into an
//!   encoder that is opened later, and it is what is sent again while the
//!   picture stands still ([`Gate::again`]), so that the encoder does not
//!   go to sleep between a person's key strokes.
//! * Frames from the compositor and frames sent again go on **one after
//!   the other**, never two at once: both take the gate's lock for as long
//!   as the frame is handed on.
//!
//! The gate knows nothing of GStreamer: a frame is any value that can be
//! copied cheaply, and handing it on is a function the caller gives. The
//! rules are tested here, on any machine.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

/// What became of a frame the compositor made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Went {
    /// It was handed on.
    Through,
    /// The gate is shut: it was kept as the last frame and not handed on.
    Kept,
    /// It was handed on and refused, with the reason. The gate is shut now.
    Refused(String),
}

/// What became of the wish to send the last frame again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Again {
    /// It was sent again.
    Sent,
    /// Not yet: a frame went on, or an input came, too short a while ago.
    /// Look again in this many nanoseconds.
    Wait(u64),
    /// The gate is shut, or there is no frame yet: nothing to send. It is
    /// looked at again when it has been opened.
    Idle,
    /// It was handed on and refused, with the reason. The gate is shut now.
    Refused(String),
}

/// Which frame is being handed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which {
    /// A frame as the compositor made it, for the first time.
    New,
    /// The last frame once more: it has been handed on before, so it needs
    /// a time stamp of its own.
    Repeat,
}

struct Inner<F> {
    open: bool,
    /// The last frame the compositor made, and whether it has been handed on.
    last: Option<(F, bool)>,
    /// Counts the times the gate was opened, for a frame that waits.
    opened: u64,
    quit: bool,
}

/// The gate. `F` is a frame: cheap to copy, as a counted reference is.
pub struct Gate<F> {
    inner: Mutex<Inner<F>>,
    moved: Condvar,
    /// A frame that finds the gate shut waits at it, instead of being
    /// dropped. See [`Gate::new`].
    waits: bool,
    /// When a frame last went on, and when an input last came. Read by the
    /// thread that sends frames again, and written by the input's thread,
    /// neither of which may wait for the gate.
    last_frame_ns: AtomicU64,
    last_input_ns: AtomicU64,
}

impl<F: Clone> Gate<F> {
    /// A gate that is open and has no frame yet.
    ///
    /// `waits` is how a frame is treated that finds the gate shut. With the
    /// compositor rendering whenever an application commits, the frame's
    /// thread waits at the gate with that one frame until the gate opens:
    /// the compositor renders nothing meanwhile and tells its applications
    /// to draw on a beat of its own. With the compositor on a fixed beat its
    /// frames are what tells the applications to draw, so there the frames
    /// go on being made and are dropped at the gate (`waits` false).
    pub fn new(waits: bool) -> Self {
        Gate {
            inner: Mutex::new(Inner {
                open: true,
                last: None,
                opened: 0,
                quit: false,
            }),
            moved: Condvar::new(),
            waits,
            last_frame_ns: AtomicU64::new(0),
            last_input_ns: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner<F>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A frame from the compositor, on the compositor's thread. `push`
    /// hands it on and says why it could not.
    ///
    /// Whatever comes of it, the compositor is to be told that all is well.
    pub fn frame(
        &self,
        frame: F,
        now_ns: u64,
        push: impl FnOnce(F, Which) -> Result<(), String>,
    ) -> Went {
        let mut inner = self.lock();
        if inner.open {
            inner.last = Some((frame.clone(), true));
            // Handed on with the lock held: nothing else goes on meanwhile, and
            // whoever shuts the gate waits for this one frame.
            return match push(frame, Which::New) {
                Ok(()) => {
                    self.last_frame_ns.store(now_ns, Ordering::Relaxed);
                    Went::Through
                }
                Err(reason) => {
                    inner.open = false;
                    // It did not get through: the next encoder begins with it.
                    if let Some(last) = &mut inner.last {
                        last.1 = false;
                    }
                    Went::Refused(reason)
                }
            };
        }
        inner.last = Some((frame, false));
        if self.waits {
            // Opening the gate hands this very frame on, as the last one.
            let seen = inner.opened;
            while inner.opened == seen && !inner.quit {
                inner = self.moved.wait(inner).unwrap_or_else(|e| e.into_inner());
            }
        }
        Went::Kept
    }

    /// Shuts the gate. When this returns, no frame is on its way past the
    /// gate and none follows until it is opened.
    pub fn shut(&self) {
        self.lock().open = false;
    }

    /// Opens the gate, once there is an encoder behind it again. The last
    /// frame goes on first, through `push`: that is where an encoder that
    /// cannot be had is found out, and then the gate stays shut and the
    /// reason is returned. Without a frame so far the gate just opens.
    pub fn open(
        &self,
        now_ns: u64,
        push: impl FnOnce(F, Which) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut inner = self.lock();
        if inner.open {
            return Ok(());
        }
        if let Some((frame, handed_on)) = inner.last.clone() {
            // A frame that waited at the gate goes on as what it is. One that an
            // earlier encoder has had already goes on once more.
            let which = if handed_on { Which::Repeat } else { Which::New };
            push(frame, which)?;
            if let Some(last) = &mut inner.last {
                last.1 = true;
            }
            self.last_frame_ns.store(now_ns, Ordering::Relaxed);
        }
        inner.open = true;
        inner.opened += 1;
        self.moved.notify_all();
        Ok(())
    }

    /// Notes that an input has come. Never waits.
    pub fn input(&self, now_ns: u64) {
        self.last_input_ns.store(now_ns, Ordering::Relaxed);
    }

    /// When the last frame may next be sent again: `every_ns` after the
    /// last frame went on and `hold_ns` after the last input, whichever is
    /// later. An application's answer to an input then never finds the
    /// encoder busy with a frame that shows nothing new.
    pub fn due_ns(&self, every_ns: u64, hold_ns: u64) -> u64 {
        let after_frame = self.last_frame_ns.load(Ordering::Relaxed) + every_ns;
        let after_input = self.last_input_ns.load(Ordering::Relaxed) + hold_ns;
        after_frame.max(after_input)
    }

    /// Sends the last frame again if it is time, through `push`.
    pub fn again(
        &self,
        now_ns: u64,
        every_ns: u64,
        hold_ns: u64,
        push: impl FnOnce(F, Which) -> Result<(), String>,
    ) -> Again {
        let due = self.due_ns(every_ns, hold_ns);
        if now_ns < due {
            return Again::Wait(due - now_ns);
        }
        let mut inner = self.lock();
        if !inner.open {
            return Again::Idle;
        }
        // A frame may have gone on while the lock was waited for.
        let due = self.due_ns(every_ns, hold_ns);
        if now_ns < due {
            return Again::Wait(due - now_ns);
        }
        let Some((frame, _)) = inner.last.clone() else {
            return Again::Idle;
        };
        match push(frame, Which::Repeat) {
            Ok(()) => {
                self.last_frame_ns.store(now_ns, Ordering::Relaxed);
                Again::Sent
            }
            Err(reason) => {
                inner.open = false;
                Again::Refused(reason)
            }
        }
    }

    /// Whether frames go on.
    pub fn is_open(&self) -> bool {
        self.lock().open
    }

    /// The program ends: a frame that waits at the gate is let go, and no
    /// frame waits there again.
    pub fn quit(&self) {
        self.lock().quit = true;
        self.moved.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    /// Where frames go in these tests: a list, or a refusal.
    #[derive(Default)]
    struct Encoder {
        got: Mutex<Vec<(u32, Which)>>,
        refuses: AtomicBool,
    }

    impl Encoder {
        fn push(&self) -> impl FnOnce(u32, Which) -> Result<(), String> + '_ {
            move |frame, which| {
                if self.refuses.load(Ordering::SeqCst) {
                    return Err("no encoder".to_string());
                }
                self.got.lock().unwrap().push((frame, which));
                Ok(())
            }
        }

        fn got(&self) -> Vec<(u32, Which)> {
            self.got.lock().unwrap().clone()
        }
    }

    const MS: u64 = 1_000_000;

    #[test]
    fn frames_go_on_while_the_gate_is_open() {
        let (gate, encoder) = (Gate::new(false), Encoder::default());
        assert_eq!(gate.frame(1, 10, encoder.push()), Went::Through);
        assert_eq!(gate.frame(2, 20, encoder.push()), Went::Through);
        assert_eq!(encoder.got(), [(1, Which::New), (2, Which::New)]);
    }

    #[test]
    fn no_frame_goes_on_while_the_gate_is_shut_and_the_last_one_is_first_when_it_opens() {
        let (gate, encoder) = (Gate::new(false), Encoder::default());
        gate.frame(1, 10, encoder.push());
        gate.shut();
        assert!(!gate.is_open());
        assert_eq!(gate.frame(2, 20, encoder.push()), Went::Kept);
        assert_eq!(gate.frame(3, 30, encoder.push()), Went::Kept);
        assert_eq!(encoder.got(), [(1, Which::New)]);
        gate.open(40, encoder.push()).unwrap();
        assert_eq!(gate.frame(4, 50, encoder.push()), Went::Through);
        assert_eq!(
            encoder.got(),
            [(1, Which::New), (3, Which::New), (4, Which::New)]
        );
    }

    #[test]
    fn a_picture_that_stood_still_while_shut_comes_back_with_the_frame_it_had() {
        let (gate, encoder) = (Gate::new(true), Encoder::default());
        gate.frame(1, 10, encoder.push());
        gate.shut();
        gate.open(40, encoder.push()).unwrap();
        assert_eq!(encoder.got(), [(1, Which::New), (1, Which::Repeat)]);
    }

    #[test]
    fn a_gate_that_never_had_a_frame_just_opens() {
        let (gate, encoder) = (Gate::<u32>::new(true), Encoder::default());
        gate.shut();
        gate.open(5, encoder.push()).unwrap();
        assert!(gate.is_open());
        assert!(encoder.got().is_empty());
    }

    #[test]
    fn a_refused_frame_shuts_the_gate_and_is_the_first_into_the_next_encoder() {
        let (gate, encoder) = (Gate::new(false), Encoder::default());
        gate.frame(1, 10, encoder.push());
        encoder.refuses.store(true, Ordering::SeqCst);
        assert_eq!(
            gate.frame(2, 20, encoder.push()),
            Went::Refused("no encoder".into())
        );
        assert!(!gate.is_open());
        // The compositor goes on making frames; none of them is handed on.
        assert_eq!(gate.frame(3, 30, encoder.push()), Went::Kept);
        // Opening it while the encoder still refuses leaves it shut.
        assert_eq!(gate.open(40, encoder.push()), Err("no encoder".into()));
        assert!(!gate.is_open());
        encoder.refuses.store(false, Ordering::SeqCst);
        gate.open(50, encoder.push()).unwrap();
        assert_eq!(encoder.got(), [(1, Which::New), (3, Which::New)]);
    }

    #[test]
    fn with_a_compositor_that_renders_on_commit_a_frame_waits_at_the_shut_gate() {
        let gate = Arc::new(Gate::new(true));
        let encoder = Arc::new(Encoder::default());
        gate.frame(1, 10, encoder.push());
        gate.shut();
        let compositor = {
            let (gate, encoder) = (gate.clone(), encoder.clone());
            std::thread::spawn(move || {
                let went = gate.frame(2, 20, encoder.push());
                // Only once the gate is open again does the compositor get on.
                assert!(gate.is_open());
                went
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        assert!(!compositor.is_finished());
        assert_eq!(encoder.got(), [(1, Which::New)]);
        gate.open(30, encoder.push()).unwrap();
        // The frame that waited has gone on as the first of the new encoder,
        // once, and is not handed on a second time by its own thread.
        assert_eq!(compositor.join().unwrap(), Went::Kept);
        assert_eq!(encoder.got(), [(1, Which::New), (2, Which::New)]);
    }

    #[test]
    fn a_frame_that_waits_is_let_go_when_the_program_ends() {
        let gate = Arc::new(Gate::new(true));
        gate.shut();
        let compositor = {
            let gate = gate.clone();
            std::thread::spawn(move || gate.frame(1, 10, |_, _| Ok(())))
        };
        std::thread::sleep(Duration::from_millis(20));
        gate.quit();
        assert_eq!(compositor.join().unwrap(), Went::Kept);
        // And nothing waits there afterwards.
        assert_eq!(gate.frame(2, 20, |_, _| Ok(())), Went::Kept);
    }

    #[test]
    fn shutting_the_gate_waits_for_the_frame_that_is_being_handed_on() {
        let gate = Arc::new(Gate::new(false));
        let handing = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let compositor = {
            let (gate, handing, done) = (gate.clone(), handing.clone(), done.clone());
            std::thread::spawn(move || {
                gate.frame(1, 10, |_, _| {
                    handing.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(60));
                    done.store(true, Ordering::SeqCst);
                    Ok(())
                })
            })
        };
        while !handing.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        gate.shut();
        // The encoder may be closed now: nothing is inside it any more.
        assert!(done.load(Ordering::SeqCst));
        assert_eq!(compositor.join().unwrap(), Went::Through);
    }

    #[test]
    fn the_last_frame_is_sent_again_only_after_a_quiet_while() {
        let (gate, encoder) = (Gate::new(true), Encoder::default());
        let (every, hold) = (50 * MS, 50 * MS);
        // No frame yet: nothing to send.
        assert_eq!(
            gate.again(60 * MS, every, hold, encoder.push()),
            Again::Idle
        );
        gate.frame(1, 100 * MS, encoder.push());
        assert_eq!(
            gate.again(120 * MS, every, hold, encoder.push()),
            Again::Wait(30 * MS)
        );
        assert_eq!(
            gate.again(150 * MS, every, hold, encoder.push()),
            Again::Sent
        );
        // And then every 50 ms, counted from the frame sent again.
        assert_eq!(
            gate.again(151 * MS, every, hold, encoder.push()),
            Again::Wait(49 * MS)
        );
        assert_eq!(
            gate.again(200 * MS, every, hold, encoder.push()),
            Again::Sent
        );
        assert_eq!(
            encoder.got(),
            [(1, Which::New), (1, Which::Repeat), (1, Which::Repeat)]
        );
    }

    #[test]
    fn an_input_holds_the_sending_again_back() {
        let (gate, encoder) = (Gate::new(true), Encoder::default());
        let (every, hold) = (50 * MS, 50 * MS);
        gate.frame(1, 100 * MS, encoder.push());
        gate.input(140 * MS);
        assert_eq!(
            gate.again(150 * MS, every, hold, encoder.push()),
            Again::Wait(40 * MS)
        );
        // The application's answer to the input goes on at once.
        assert_eq!(gate.frame(2, 148 * MS, encoder.push()), Went::Through);
        assert_eq!(
            gate.again(190 * MS, every, hold, encoder.push()),
            Again::Wait(8 * MS)
        );
        assert_eq!(
            gate.again(198 * MS, every, hold, encoder.push()),
            Again::Sent
        );
        assert_eq!(
            encoder.got(),
            [(1, Which::New), (2, Which::New), (2, Which::Repeat)]
        );
    }

    #[test]
    fn nothing_is_sent_again_while_the_gate_is_shut() {
        let (gate, encoder) = (Gate::new(false), Encoder::default());
        gate.frame(1, 100 * MS, encoder.push());
        gate.shut();
        assert_eq!(
            gate.again(900 * MS, 50 * MS, 50 * MS, encoder.push()),
            Again::Idle
        );
        assert_eq!(encoder.got(), [(1, Which::New)]);
    }

    #[test]
    fn a_frame_sent_again_that_is_refused_shuts_the_gate() {
        let (gate, encoder) = (Gate::new(false), Encoder::default());
        gate.frame(1, 100 * MS, encoder.push());
        encoder.refuses.store(true, Ordering::SeqCst);
        assert_eq!(
            gate.again(200 * MS, 50 * MS, 50 * MS, encoder.push()),
            Again::Refused("no encoder".into())
        );
        assert!(!gate.is_open());
    }

    #[test]
    fn a_frame_and_a_frame_sent_again_never_go_on_at_the_same_time() {
        let gate = Arc::new(Gate::new(false));
        let inside = Arc::new(AtomicU64::new(0));
        let both = Arc::new(AtomicBool::new(false));
        let count = Arc::new(AtomicU64::new(0));
        gate.frame(0, 0, |_, _| Ok(()));
        let hand = |inside: Arc<AtomicU64>, both: Arc<AtomicBool>, count: Arc<AtomicU64>| {
            move |_frame: u32, _which: Which| {
                if inside.fetch_add(1, Ordering::SeqCst) != 0 {
                    both.store(true, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_micros(200));
                inside.fetch_sub(1, Ordering::SeqCst);
                count.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        };
        let compositor = {
            let (gate, inside, both, count) =
                (gate.clone(), inside.clone(), both.clone(), count.clone());
            std::thread::spawn(move || {
                for frame in 1..=100u32 {
                    let push = hand(inside.clone(), both.clone(), count.clone());
                    gate.frame(frame, 0, push);
                }
            })
        };
        let repeater = {
            let (gate, inside, both, count) =
                (gate.clone(), inside.clone(), both.clone(), count.clone());
            std::thread::spawn(move || {
                for _ in 0..100 {
                    // Always due: every and hold are nothing.
                    let push = hand(inside.clone(), both.clone(), count.clone());
                    gate.again(1, 0, 0, push);
                }
            })
        };
        compositor.join().unwrap();
        repeater.join().unwrap();
        assert!(!both.load(Ordering::SeqCst));
        assert_eq!(count.load(Ordering::SeqCst), 200);
    }

    #[test]
    fn noting_an_input_does_not_wait_for_a_frame_that_is_being_handed_on() {
        let gate = Arc::new(Gate::new(false));
        let handing = Arc::new(AtomicBool::new(false));
        let compositor = {
            let (gate, handing) = (gate.clone(), handing.clone());
            std::thread::spawn(move || {
                gate.frame(1, 10, |_, _| {
                    handing.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(80));
                    Ok(())
                })
            })
        };
        while !handing.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        let before = std::time::Instant::now();
        gate.input(20);
        assert!(before.elapsed() < Duration::from_millis(40));
        compositor.join().unwrap();
    }
}
