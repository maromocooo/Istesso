//! The pipeline: the compositor's half, the gate, and the encoder's half
//! that is put in and taken out while the compositor runs.
//!
//! ```text
//!   compositor ! caps  ->  [gate]  ->  convert ! caps ! enc ! parse ! caps ! net
//!   (always there)                     (the encoder's side: a bin of its own)
//! ```
//!
//! Which thread does what:
//!
//! * A frame passes the gate, the colour conversion and into the encoder on
//!   the compositor element's streaming thread, by plain calls. The gate is
//!   an element of this program with two pads and nothing in between; see
//!   [`crate::gate`] for its rules.
//! * An encoded frame comes out on the encoder's own thread and is handed
//!   to [`Out::frame`] on that thread, before it returns.
//! * Everything that changes the pipeline (closing the encoder, opening
//!   one, starting, stopping) happens on the one thread that owns the
//!   [`Stage`]. The threads above only ever tell it something through
//!   [`Note`]s.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gstreamer as gst;
use gstreamer_app as gst_app;

use crate::framelog::{line, FrameLog};
use crate::gate::{self, Again, Gate, Went, Which};
use crate::input::{self, ForCompositor, Value};

/// The name of the bin that holds the encoder's side. A failure whose
/// source sits inside a bin of this name is the encoder's, and never the
/// compositor's.
const ENCODER_SIDE: &str = "encoder-side";

/// What became of a frame that was handed on to the viewers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Handed {
    pub seq: u64,
    pub viewers: u32,
    pub queued: u32,
    pub messages: u32,
}

/// Where encoded frames go.
pub trait Out: Send + Sync {
    /// One access unit, on the encoder's thread. `pts_ns` is its time stamp
    /// and `encoded_ns` the moment it left the encoder. `None` when it could
    /// not be handed on.
    fn frame(&self, data: &[u8], pts_ns: u64, encoded_ns: u64, key: bool) -> Option<Handed>;
}

/// What the threads of the pipeline tell the thread that owns the stage.
#[derive(Debug, Clone, PartialEq)]
pub enum Note {
    /// The encoder's side refused a frame, or one of its elements failed.
    EncoderFailed(String),
    /// The first frame of a newly opened encoder is out and handed on.
    FirstFrame {
        encoded_ns: u64,
        written_ns: u64,
        key: bool,
    },
}

/// What is called once on each thread of a frame's path, with which of them
/// it is: "frames" for the compositor's streaming thread, "encoded" for the
/// encoder's.
pub type PathThread = Box<dyn Fn(&str) + Send + Sync>;

/// What the stage shares with the threads of the pipeline.
struct Shared {
    gate: Gate<gst::Buffer>,
    log: FrameLog,
    clock: fn() -> u64,
    notes: Mutex<Sender<Note>>,
    out: Arc<dyn Out>,
    /// The time stamp of the last frame that went past the gate
    /// ([`NO_FRAME_YET`] before the first).
    last_pts: AtomicU64,
    /// How many new frames went on with another time stamp than their own.
    restamped: AtomicU64,
    /// When each frame left the encoder, until it is handed on: the time
    /// travels with it. Both ends are on the encoder's thread.
    encoded_at: Mutex<VecDeque<(u64, u64)>>,
    /// The next frame out of the encoder is the first of a new encoder.
    first_awaited: AtomicBool,
    /// What an element of the encoder's side last said went wrong.
    encoder_error: Mutex<Option<String>>,
    /// Called once on each thread of the frame's path, the first time a
    /// frame passes on it (for the `sched` switch).
    on_path_thread: Option<PathThread>,
}

/// What `last_pts` holds before any frame has gone past the gate.
const NO_FRAME_YET: u64 = u64::MAX;

/// A frame that waits for the gate's lock longer than this is written to
/// the frame log: another frame, one sent again as a rule, was in its way.
const WAIT_WORTH_WRITING_NS: u64 = 100_000;

thread_local! {
    static KNOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl Shared {
    fn note(&self, note: Note) {
        let notes = self.notes.lock().unwrap_or_else(|e| e.into_inner());
        let _ = notes.send(note);
    }

    fn path_thread(&self, which: &str) {
        if let Some(told) = &self.on_path_thread {
            if !KNOWN.get() {
                KNOWN.set(true);
                told(which);
            }
        }
    }

    /// Hands a frame on from the gate's source pad: the one place a frame
    /// goes into the encoder's side from, with the gate's lock held.
    ///
    /// Its time stamp is later than that of every frame before it
    /// ([`gate::stamp`]): a frame that goes on once more gets one of its own,
    /// and a new frame whose own is not later is given the next one, which
    /// is counted and written to the frame log. `at_ns` is when the frame
    /// came to the gate, for the log.
    fn push(
        &self,
        element: &GateElement,
        frame: gst::Buffer,
        which: Which,
        at_ns: u64,
    ) -> Result<(), String> {
        let own = frame.pts().map(|pts| pts.nseconds());
        let last = match self.last_pts.load(Ordering::Relaxed) {
            NO_FRAME_YET => None,
            last => Some(last),
        };
        let now = element
            .current_running_time()
            .map_or(0, |time| time.nseconds());
        let (pts, changed) = gate::stamp(which, own, last, now);
        let frame = if own == Some(pts) {
            frame
        } else {
            // The picture is shared; only what is written on it is this frame's own.
            let mut stamped = frame.copy();
            stamped
                .get_mut()
                .expect("a copy has one owner")
                .set_pts(gst::ClockTime::from_nseconds(pts));
            stamped
        };
        if changed {
            self.restamped.fetch_add(1, Ordering::Relaxed);
        }
        if self.log.on() {
            // With the stamp the frame goes on with, so that this line and the
            // lines after the gate are of the same frame.
            self.log
                .line(line::frame_out(pts, at_ns, which == Which::Repeat));
            if changed {
                self.log.line(line::restamped(pts, own, at_ns));
            }
        }
        self.last_pts.store(pts, Ordering::Relaxed);
        match element.imp().srcpad.push(frame) {
            Ok(_) => Ok(()),
            Err(flow) => Err(format!(
                "the encoder's side did not take a frame ({flow:?})"
            )),
        }
    }
}

mod imp {
    use super::*;
    use std::sync::LazyLock;

    pub struct GateElement {
        pub(super) sinkpad: gst::Pad,
        pub(super) srcpad: gst::Pad,
        pub(super) shared: OnceLock<Arc<Shared>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for GateElement {
        const NAME: &'static str = "IstessoGate";
        type Type = super::GateElement;
        type ParentType = gst::Element;

        fn with_class(class: &Self::Class) -> Self {
            // Everything but frames passes as if the gate were not there: what the
            // compositor asks the encoder's side, and the other way round.
            let through = gst::PadFlags::PROXY_CAPS
                | gst::PadFlags::PROXY_ALLOCATION
                | gst::PadFlags::PROXY_SCHEDULING;
            let template = class.pad_template("sink").expect("declared below");
            let sinkpad = gst::Pad::builder_from_template(&template)
                .chain_function(|_pad, parent, buffer| {
                    GateElement::catch_panic_pad_function(
                        parent,
                        || Ok(gst::FlowSuccess::Ok),
                        |gate| gate.chain(buffer),
                    )
                })
                .flags(through)
                .build();
            let template = class.pad_template("src").expect("declared below");
            let srcpad = gst::Pad::builder_from_template(&template)
                .flags(through)
                .build();
            GateElement {
                sinkpad,
                srcpad,
                shared: OnceLock::new(),
            }
        }
    }

    impl GateElement {
        /// A frame from the compositor, on its thread. Whatever comes of it,
        /// the compositor is told that all is well: a compositor that hears
        /// of a failure ends, and its application with it.
        fn chain(&self, buffer: gst::Buffer) -> Result<gst::FlowSuccess, gst::FlowError> {
            let Some(shared) = self.shared.get() else {
                return Ok(gst::FlowSuccess::Ok);
            };
            shared.path_thread("frames");
            let now = (shared.clock)();
            let element = self.obj();
            // A frame that stays at the shut gate is written to the log when,
            // and if, it goes on.
            let went = shared.gate.frame(buffer, now, |frame, which| {
                if shared.log.on() {
                    let waited = (shared.clock)().saturating_sub(now);
                    if waited > WAIT_WORTH_WRITING_NS {
                        let pts = frame.pts().map_or(0, |pts| pts.nseconds());
                        shared.log.line(line::gate_wait(pts, now, waited));
                    }
                }
                shared.push(&element, frame, which, now)
            });
            if let Went::Refused(reason) = went {
                shared.note(Note::EncoderFailed(reason));
            }
            Ok(gst::FlowSuccess::Ok)
        }
    }

    impl ObjectImpl for GateElement {
        fn constructed(&self) {
            self.parent_constructed();
            let element = self.obj();
            element
                .add_pad(&self.sinkpad)
                .expect("a new element takes its pads");
            element
                .add_pad(&self.srcpad)
                .expect("a new element takes its pads");
        }
    }

    impl GstObjectImpl for GateElement {}

    impl ElementImpl for GateElement {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
                gst::subclass::ElementMetadata::new(
                    "Istesso gate",
                    "Generic",
                    "Lets frames on to the encoder, or keeps them, without a thread of its own",
                    "Istesso",
                )
            });
            Some(&*METADATA)
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            static TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
                let any = gst::Caps::new_any();
                let make = |name, direction| {
                    gst::PadTemplate::new(name, direction, gst::PadPresence::Always, &any)
                        .expect("a template of any caps")
                };
                vec![
                    make("sink", gst::PadDirection::Sink),
                    make("src", gst::PadDirection::Src),
                ]
            });
            TEMPLATES.as_ref()
        }

        fn change_state(
            &self,
            transition: gst::StateChange,
        ) -> Result<gst::StateChangeSuccess, gst::StateChangeError> {
            if transition == gst::StateChange::PausedToReady {
                // A frame that waits at the shut gate holds the compositor's thread,
                // and the pipeline cannot stop before that thread is let go.
                if let Some(shared) = self.shared.get() {
                    shared.gate.quit();
                }
            }
            self.parent_change_state(transition)
        }
    }
}

glib::wrapper! {
    /// The gate as an element: a sink pad, a source pad, and nothing in between.
    pub struct GateElement(ObjectSubclass<imp::GateElement>) @extends gst::Element, gst::Object;
}

/// What the stage is made with.
pub struct Plan {
    /// The compositor and the caps of its frames (`Settings::compositor_side`).
    pub compositor_side: String,
    /// From frames to access units (`Settings::encoder_side`). Its elements
    /// are called "convert", "enc" and "net", and "net" is an appsink.
    pub encoder_side: String,
    /// Whether a frame waits at the shut gate (the compositor renders when
    /// an application commits) or is dropped there (fixed beat).
    pub waits: bool,
    pub log: FrameLog,
    pub clock: fn() -> u64,
    pub out: Arc<dyn Out>,
    pub notes: Sender<Note>,
    /// See [`Shared::on_path_thread`]: told "frames" or "encoded".
    pub on_path_thread: Option<PathThread>,
}

/// The pipeline of one session. Owned by one thread.
pub struct Stage {
    pipeline: gst::Pipeline,
    compositor: gst::Element,
    gate: GateElement,
    shared: Arc<Shared>,
    encoder_side: String,
    /// The encoder's side while it is in.
    encoder: Option<gst::Bin>,
    key_frames: u32,
}

/// Sends frames again from a thread of its own (the `keep-awake` switch).
#[derive(Clone)]
pub struct Repeater {
    gate: GateElement,
    shared: Arc<Shared>,
}

impl Repeater {
    /// Sends the last frame again if it is time; says how long to wait
    /// before looking again.
    pub fn again(&self, every: Duration, hold: Duration) -> Duration {
        let (every_ns, hold_ns) = (every.as_nanos() as u64, hold.as_nanos() as u64);
        let now = (self.shared.clock)();
        let push = |frame, which| self.shared.push(&self.gate, frame, which, now);
        match self.shared.gate.again(now, every_ns, hold_ns, push) {
            Again::Sent | Again::Idle => every,
            Again::Wait(wait_ns) => Duration::from_nanos(wait_ns),
            Again::Refused(reason) => {
                self.shared.note(Note::EncoderFailed(reason));
                every
            }
        }
    }
}

/// Hands input to the compositor from the input's thread.
#[derive(Clone)]
pub struct ToCompositor {
    pad: gst::Pad,
    shared: Arc<Shared>,
}

impl ToCompositor {
    /// Sends one event up the pipeline from the compositor's source pad,
    /// which is how the compositor takes its input. `at_ns` is when the
    /// event was read. After an event that is likely to be answered by a new
    /// frame ([`input::holds`]) no frame is sent again for a while.
    pub fn input(&self, event: &istesso_proto::InputEvent, at_ns: u64) -> bool {
        if input::holds(event) {
            self.shared.gate.input(at_ns);
        }
        let told = structure_of(&input::for_compositor(event));
        self.pad.send_event(gst::event::CustomUpstream::new(told))
    }

    /// Asks the application's windows to close, as a window manager's close
    /// button does. `false` from a compositor plugin that cannot.
    pub fn close_windows(&self) -> bool {
        let structure = gst::Structure::new_empty("CloseWindows");
        self.pad
            .send_event(gst::event::CustomUpstream::new(structure))
    }
}

/// The structure the compositor takes for an event of input.
fn structure_of(event: &ForCompositor) -> gst::Structure {
    let mut structure = gst::Structure::builder(event.name);
    for (name, value) in event.fields {
        structure = match value {
            Value::Uint(number) => structure.field(name, number),
            Value::Bool(yes) => structure.field(name, yes),
            Value::Double(number) => structure.field(name, number),
        };
    }
    structure.build()
}

/// What the pipeline's bus carried, for the thread that listens to it.
#[derive(Debug, Clone, PartialEq)]
pub enum Heard {
    /// The cursor changed (the compositor's `wayland.cursor`).
    Cursor(CursorSeen),
    /// The application has this many windows (`wayland.windows`).
    Windows(u32),
    /// How an application gets into the compositor (`wayland.src`): the
    /// environment to start it with.
    Environment(Vec<(String, String)>),
    /// An element of the encoder's side failed.
    EncoderError(String),
    /// Something else failed: the pipeline cannot go on.
    Error(String),
    /// The compositor has ended.
    End,
}

/// A cursor as the compositor reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorSeen {
    Hidden,
    Named(String),
    Image {
        width: u32,
        height: u32,
        hotspot_x: u32,
        hotspot_y: u32,
        scale: u32,
        /// Four bytes a pixel: red, green, blue, alpha.
        pixels: Vec<u8>,
    },
}

fn cursor_seen(structure: &gst::StructureRef) -> Option<CursorSeen> {
    match structure.get::<&str>("kind").ok()? {
        "hidden" => Some(CursorSeen::Hidden),
        "named" => Some(CursorSeen::Named(
            structure
                .get::<&str>("name")
                .unwrap_or("default")
                .to_string(),
        )),
        "image" => {
            let number = |name| structure.get::<u32>(name).ok();
            let image = structure.get::<gst::Buffer>("image").ok()?;
            let pixels = image.map_readable().ok()?.as_slice().to_vec();
            Some(CursorSeen::Image {
                width: number("width")?,
                height: number("height")?,
                hotspot_x: number("hotspot-x")?,
                hotspot_y: number("hotspot-y")?,
                scale: number("scale")?,
                pixels,
            })
        }
        _ => None,
    }
}

/// Whether `object` sits inside the encoder's side.
fn of_the_encoder(object: &gst::Object) -> bool {
    let mut at = Some(object.clone());
    while let Some(object) = at {
        if object.name().as_str() == ENCODER_SIDE {
            return true;
        }
        at = object.parent();
    }
    false
}

impl Stage {
    /// Builds the pipeline; nothing runs yet.
    pub fn build(plan: Plan) -> Result<Self, String> {
        gst::init().map_err(|error| format!("GStreamer does not start: {error}"))?;
        // The gate gets a name the description can use, so that the compositor's
        // side is written, caps and all, exactly as the script wrote it.
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let _ = gst::Element::register(
                None,
                "istessogate",
                gst::Rank::NONE,
                GateElement::static_type(),
            );
        });
        let description = format!("{} ! istessogate name=gate", plan.compositor_side);
        let pipeline = gst::parse::launch(&description)
            .map_err(|error| format!("the compositor's side cannot be made: {error}"))?
            .downcast::<gst::Pipeline>()
            .map_err(|_| "the compositor's side is not a pipeline".to_string())?;
        let compositor = pipeline
            .by_name("compositor")
            .ok_or("the pipeline has no element called compositor")?;
        let gate = pipeline
            .by_name("gate")
            .and_then(|gate| gate.downcast::<GateElement>().ok())
            .ok_or("the pipeline has no gate")?;
        let shared = Arc::new(Shared {
            gate: Gate::new(plan.waits),
            log: plan.log,
            clock: plan.clock,
            notes: Mutex::new(plan.notes),
            out: plan.out,
            last_pts: AtomicU64::new(NO_FRAME_YET),
            restamped: AtomicU64::new(0),
            encoded_at: Mutex::new(VecDeque::new()),
            first_awaited: AtomicBool::new(false),
            encoder_error: Mutex::new(None),
            on_path_thread: plan.on_path_thread,
        });
        let _ = gate.imp().shared.set(shared.clone());
        Ok(Stage {
            pipeline,
            compositor,
            gate,
            shared,
            encoder_side: plan.encoder_side,
            encoder: None,
            key_frames: 0,
        })
    }

    /// The compositor element, for setting what it can be set to.
    pub fn compositor(&self) -> &gst::Element {
        &self.compositor
    }

    /// For the thread that sends frames again.
    pub fn repeater(&self) -> Repeater {
        Repeater {
            gate: self.gate.clone(),
            shared: self.shared.clone(),
        }
    }

    /// For the thread that takes input.
    pub fn to_compositor(&self) -> Option<ToCompositor> {
        Some(ToCompositor {
            pad: self.compositor.static_pad("src")?,
            shared: self.shared.clone(),
        })
    }

    /// Starts the pipeline. The outer error is one the program cannot go on
    /// after (the compositor does not start). The inner one says that no
    /// encoder could be had: the compositor runs, and no picture is made.
    pub fn start(&mut self) -> Result<Result<(), String>, String> {
        // The encoder's side first, as a pipeline brings its elements up: from
        // the far end. The compositor then finds it there when it asks what
        // frames are wanted, as it did when this was one pipeline.
        let encoder = self.put_encoder_in();
        if encoder.is_err() {
            self.shared.gate.shut();
        }
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|_| "the pipeline does not start".to_string())?;
        Ok(encoder)
    }

    /// Stops everything. A frame that waits at the gate is let go first.
    pub fn stop(&mut self) {
        self.shared.gate.quit();
        // And none goes on while the encoder's side is taken apart.
        self.shared.gate.shut();
        self.take_encoder_out();
        let _ = self.pipeline.set_state(gst::State::Null);
    }

    fn make_encoder(&self) -> Result<gst::Bin, String> {
        let bin = gst::parse::bin_from_description(&self.encoder_side, true)
            .map_err(|error| format!("the encoder's side cannot be made: {error}"))?;
        bin.set_property("name", ENCODER_SIDE);
        // Its state is this program's business alone: the pipeline's own changes pass it by.
        bin.set_locked_state(true);
        let net = bin
            .by_name("net")
            .and_then(|net| net.downcast::<gst_app::AppSink>().ok())
            .ok_or("the encoder's side has no way out called net")?;
        net.set_property("emit-signals", false);
        let shared = self.shared.clone();
        net.set_callbacks(
            gst_app::AppSinkCallbacks::builder()
                .new_sample(move |net| {
                    access_unit(&shared, net);
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );
        let pad_of = |name: &str| bin.by_name(name).and_then(|it| it.static_pad("src"));
        if self.shared.log.on() {
            if let Some(pad) = pad_of("convert") {
                let shared = self.shared.clone();
                pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
                    if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data {
                        let pts = buffer.pts().map_or(0, |pts| pts.nseconds());
                        shared.log.line(line::converted(pts, (shared.clock)()));
                    }
                    gst::PadProbeReturn::Ok
                });
            }
        }
        let pad = pad_of("enc").ok_or("the encoder's side has no element called enc")?;
        let shared = self.shared.clone();
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(gst::PadProbeData::Buffer(buffer)) = &info.data {
                let now = (shared.clock)();
                let pts = buffer.pts().map_or(0, |pts| pts.nseconds());
                if shared.log.on() {
                    let key = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
                    shared.log.line(line::encoded(pts, now, buffer.size(), key));
                }
                let mut encoded_at = shared.encoded_at.lock().unwrap_or_else(|e| e.into_inner());
                // Never more than a few: each is taken out when its frame is handed on.
                if encoded_at.len() >= 64 {
                    encoded_at.pop_front();
                }
                encoded_at.push_back((pts, now));
            }
            gst::PadProbeReturn::Ok
        });
        Ok(bin)
    }

    /// Puts an encoder's side behind the gate and starts it. Nothing of it
    /// is left in when it does not start.
    fn put_encoder_in(&mut self) -> Result<(), String> {
        *self
            .shared
            .encoder_error
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = None;
        let bin = self.make_encoder()?;
        self.pipeline
            .add(&bin)
            .map_err(|error| format!("the encoder's side cannot be put in: {error}"))?;
        self.encoder = Some(bin.clone());
        let linked = bin
            .static_pad("sink")
            .ok_or("the encoder's side has no way in".to_string())
            .and_then(|sink| {
                self.gate
                    .imp()
                    .srcpad
                    .link(&sink)
                    .map_err(|error| format!("the encoder's side cannot be linked: {error:?}"))
            });
        let started = linked.and_then(|_| {
            bin.set_state(gst::State::Playing)
                .map(|_| ())
                .map_err(|_| "the encoder's side does not start".to_string())
        });
        if let Err(reason) = started {
            // What the element itself said is the better reason.
            let said = self.encoder_error(Duration::from_millis(100));
            self.take_encoder_out();
            return Err(said.unwrap_or(reason));
        }
        Ok(())
    }

    fn take_encoder_out(&mut self) {
        if let Some(bin) = self.encoder.take() {
            // Closing the elements closes the encoder's session with the card.
            let _ = bin.set_state(gst::State::Null);
            let _ = self.pipeline.remove(&bin);
        }
    }

    /// What an element of the encoder's side said went wrong, if it says so
    /// within `wait`. The bus is listened to on another thread; this is
    /// where what it heard is picked up.
    pub fn encoder_error(&self, wait: Duration) -> Option<String> {
        let until = Instant::now() + wait;
        loop {
            let said = self
                .shared
                .encoder_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            if said.is_some() || Instant::now() >= until {
                return said;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Shuts the gate and takes the encoder out. When this returns the
    /// encoder is closed.
    pub fn close_encoder(&mut self) {
        self.shared.gate.shut();
        self.shared.first_awaited.store(false, Ordering::SeqCst);
        self.take_encoder_out();
    }

    /// Puts an encoder in and opens the gate, which lets the last frame into
    /// it. An encoder that cannot be had is found out here or, when the card
    /// says so only later, through a [`Note`].
    pub fn open_encoder(&mut self) -> Result<(), String> {
        if self.encoder.is_some() {
            self.close_encoder();
        }
        self.put_encoder_in()?;
        self.shared.first_awaited.store(true, Ordering::SeqCst);
        let (shared, gate) = (&self.shared, &self.gate);
        let now = (shared.clock)();
        let opened = shared
            .gate
            .open(now, |frame, which| shared.push(gate, frame, which, now));
        if let Err(reason) = opened {
            let said = self.encoder_error(Duration::from_millis(100));
            self.close_encoder();
            return Err(said.unwrap_or(reason));
        }
        Ok(())
    }

    /// Asks the encoder for a key frame. `false` while there is no encoder:
    /// a new one begins with a key frame anyway.
    pub fn key_frame(&mut self) -> bool {
        let Some(pad) = self
            .encoder
            .as_ref()
            .and_then(|bin| bin.by_name("enc"))
            .and_then(|enc| enc.static_pad("src"))
        else {
            return false;
        };
        self.key_frames += 1;
        // What GStreamer's video library calls a force-key-unit event, with all
        // headers: the parameter sets come with the key frame.
        let structure = gst::Structure::builder("GstForceKeyUnit")
            .field("running-time", u64::MAX)
            .field("all-headers", true)
            .field("count", self.key_frames)
            .build();
        pad.send_event(gst::event::CustomUpstream::new(structure))
    }

    /// Changes what the encoder's side is made of from the next time it is
    /// put in. For trying what happens when no encoder can be had, without
    /// taking the card's encoders away from anybody: an encoder's side that
    /// names an element that does not exist is refused like a real one.
    pub fn set_encoder_side(&mut self, description: String) {
        self.encoder_side = description;
    }

    /// How many new frames went on with another time stamp than the
    /// compositor gave them. Each is in the frame log; this is the count.
    pub fn restamped(&self) -> u64 {
        self.shared.restamped.load(Ordering::Relaxed)
    }

    /// Whether frames go on to an encoder.
    pub fn has_encoder(&self) -> bool {
        self.encoder.is_some() && self.shared.gate.is_open()
    }

    /// Waits for the next thing on the pipeline's bus, for at most `wait`.
    /// For a thread of its own; `listener` gives what it needs.
    pub fn listener(&self) -> Listener {
        Listener {
            bus: self.pipeline.bus().expect("a pipeline has a bus"),
            pipeline: self.pipeline.clone(),
            shared: self.shared.clone(),
        }
    }
}

/// Listens to the pipeline's bus on a thread of its own.
pub struct Listener {
    bus: gst::Bus,
    pipeline: gst::Pipeline,
    shared: Arc<Shared>,
}

impl Listener {
    /// The next thing worth hearing, or `None` after `wait` without one.
    pub fn hear(&self, wait: Duration) -> Option<Heard> {
        let until = Instant::now() + wait;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let message = self
                .bus
                .timed_pop(gst::ClockTime::from_nseconds(left.as_nanos() as u64))?;
            // Most of what a bus carries is of no interest here.
            if let Some(heard) = self.worth_hearing(&message) {
                return Some(heard);
            }
        }
    }

    fn worth_hearing(&self, message: &gst::Message) -> Option<Heard> {
        match message.view() {
            gst::MessageView::Application(application) => {
                let structure = application.structure()?;
                match structure.name().as_str() {
                    "wayland.cursor" => cursor_seen(structure).map(Heard::Cursor),
                    "wayland.windows" => structure.get::<u32>("count").ok().map(Heard::Windows),
                    "wayland.src" => Some(Heard::Environment(
                        structure
                            .iter()
                            .filter_map(|(name, value)| {
                                Some((name.to_string(), value.get::<String>().ok()?))
                            })
                            .collect(),
                    )),
                    _ => None,
                }
            }
            gst::MessageView::Error(error) => {
                let text = error.error().to_string();
                if message.src().is_some_and(of_the_encoder) {
                    *self
                        .shared
                        .encoder_error
                        .lock()
                        .unwrap_or_else(|e| e.into_inner()) = Some(text.clone());
                    Some(Heard::EncoderError(text))
                } else {
                    Some(Heard::Error(text))
                }
            }
            gst::MessageView::Eos(_) => Some(Heard::End),
            gst::MessageView::Latency(_) => {
                let _ = self.pipeline.recalculate_latency();
                None
            }
            _ => None,
        }
    }
}

/// An access unit has come out of the encoder: hand it on, on this thread.
fn access_unit(shared: &Shared, net: &gst_app::AppSink) {
    shared.path_thread("encoded");
    let Ok(sample) = net.pull_sample() else {
        return;
    };
    let Some(buffer) = sample.buffer() else {
        return;
    };
    let Ok(unit) = buffer.map_readable() else {
        return;
    };
    let key = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
    let pts = buffer.pts().map_or(0, |pts| pts.nseconds());
    let handed = (shared.clock)();
    let encoded = {
        let mut encoded_at = shared.encoded_at.lock().unwrap_or_else(|e| e.into_inner());
        match encoded_at.iter().position(|(of, _)| *of == pts) {
            Some(place) => encoded_at.remove(place).map_or(handed, |(_, at)| at),
            None => handed,
        }
    };
    let went = shared.out.frame(unit.as_slice(), pts, encoded, key);
    let written = (shared.clock)();
    if let (Some(went), true) = (went, shared.log.on()) {
        shared.log.line(line::sent(
            pts,
            handed,
            went.seq,
            went.messages,
            written,
            went.viewers,
            went.queued,
            unit.len(),
        ));
    }
    if shared.first_awaited.swap(false, Ordering::SeqCst) {
        shared.note(Note::FirstFrame {
            encoded_ns: encoded,
            written_ns: written,
            key,
        });
    }
}

#[cfg(test)]
mod tests {
    //! With elements every GStreamer has in place of the compositor and the
    //! encoder: what is tested is this program's part, the gate between
    //! them and the putting in and taking out, on any machine with GStreamer.

    use super::*;
    use std::str::FromStr;
    use std::sync::mpsc::{channel, Receiver};

    /// Collects what comes out of the encoder's side.
    #[derive(Default)]
    struct Collected {
        frames: Mutex<Vec<(u64, bool)>>,
    }

    impl Out for Collected {
        fn frame(&self, _data: &[u8], pts_ns: u64, _encoded_ns: u64, key: bool) -> Option<Handed> {
            let mut frames = self.frames.lock().unwrap();
            frames.push((pts_ns, key));
            Some(Handed {
                seq: frames.len() as u64,
                viewers: 1,
                ..Handed::default()
            })
        }
    }

    impl Collected {
        fn count(&self) -> usize {
            self.frames.lock().unwrap().len()
        }
    }

    fn clock() -> u64 {
        static BEGAN: OnceLock<Instant> = OnceLock::new();
        BEGAN.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1_000_000_000
    }

    /// A compositor that makes a small frame 100 times a second.
    const COMPOSITOR: &str = "videotestsrc name=compositor is-live=true ! video/x-raw,width=32,height=32,framerate=100/1";
    /// One that makes three frames and then none.
    const STILL: &str = "videotestsrc name=compositor is-live=true num-buffers=3 ! video/x-raw,width=32,height=32,framerate=100/1";
    const ENCODER: &str = "identity name=convert ! identity name=enc ! appsink name=net emit-signals=true sync=false async=false";
    /// An encoder that fails at its fortieth frame.
    const FAILING: &str = "identity name=convert ! identity name=enc error-after=40 ! appsink name=net sync=false async=false";
    /// One that fails at its first.
    const REFUSING: &str = "identity name=convert ! identity name=enc error-after=1 ! appsink name=net sync=false async=false";
    /// One that cannot even be made.
    const MISSING: &str = "identity name=convert ! istessonosuchencoder name=enc ! appsink name=net sync=false async=false";

    struct Set {
        stage: Stage,
        out: Arc<Collected>,
        notes: Receiver<Note>,
    }

    fn set(compositor: &str, encoder: &str, waits: bool) -> Set {
        let out = Arc::new(Collected::default());
        let (notes_in, notes) = channel();
        let stage = Stage::build(Plan {
            compositor_side: compositor.to_string(),
            encoder_side: encoder.to_string(),
            waits,
            log: FrameLog::none(),
            clock,
            out: out.clone(),
            notes: notes_in,
            on_path_thread: None,
        })
        .unwrap();
        // The stand-in for a compositor whose picture stands still makes a few
        // frames and says it has ended; a compositor never says that.
        let pad = stage.compositor().static_pad("src").unwrap();
        pad.add_probe(gst::PadProbeType::EVENT_DOWNSTREAM, |_, info| {
            match &info.data {
                Some(gst::PadProbeData::Event(event)) if event.type_() == gst::EventType::Eos => {
                    gst::PadProbeReturn::Drop
                }
                _ => gst::PadProbeReturn::Ok,
            }
        });
        Set { stage, out, notes }
    }

    fn until(what: &str, mut holds: impl FnMut() -> bool) {
        let began = Instant::now();
        while !holds() {
            assert!(began.elapsed() < Duration::from_secs(10), "never: {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Whether frames come out during the next tenth of a second.
    fn frames_come(out: &Collected) -> bool {
        let before = out.count();
        std::thread::sleep(Duration::from_millis(100));
        out.count() > before
    }

    #[test]
    fn frames_pass_the_gate_and_come_out_of_the_encoder_s_side() {
        let mut set = set(COMPOSITOR, ENCODER, false);
        assert_eq!(set.stage.start(), Ok(Ok(())));
        assert!(set.stage.has_encoder());
        until("twenty frames", || set.out.count() >= 20);
        let frames = set.out.frames.lock().unwrap().clone();
        assert!(frames.windows(2).all(|pair| pair[0].0 < pair[1].0));
        set.stage.stop();
    }

    #[test]
    fn closing_the_encoder_stops_the_frames_at_once_and_opening_one_brings_them_back() {
        let mut set = set(COMPOSITOR, ENCODER, false);
        set.stage.start().unwrap().unwrap();
        until("frames", || set.out.count() >= 5);
        set.stage.close_encoder();
        // Closed means closed: not one frame comes out after this has returned.
        let closed_at = set.out.count();
        assert!(!frames_come(&set.out));
        assert_eq!(set.out.count(), closed_at);
        assert!(!set.stage.has_encoder());

        set.stage.open_encoder().unwrap();
        until("frames again", || set.out.count() >= closed_at + 5);
        // The first frame of the new encoder is told, once.
        let first = set.notes.try_iter().collect::<Vec<_>>();
        assert_eq!(first.len(), 1, "{first:?}");
        assert!(matches!(first[0], Note::FirstFrame { .. }));
        let frames = set.out.frames.lock().unwrap().clone();
        assert!(frames.windows(2).all(|pair| pair[0].0 < pair[1].0));
        drop(frames);
        set.stage.stop();
    }

    #[test]
    fn closing_takes_no_longer_while_the_picture_stands_still() {
        let mut set = set(STILL, ENCODER, true);
        set.stage.start().unwrap().unwrap();
        until("the three frames", || set.out.count() >= 3);
        assert!(!frames_come(&set.out));
        let began = Instant::now();
        set.stage.close_encoder();
        assert!(
            began.elapsed() < Duration::from_millis(300),
            "{:?}",
            began.elapsed()
        );
        set.stage.stop();
    }

    #[test]
    fn an_encoder_that_fails_while_it_runs_is_told_and_the_compositor_goes_on() {
        let mut set = set(COMPOSITOR, FAILING, false);
        set.stage.start().unwrap().unwrap();
        let listener = set.stage.listener();
        let note = set.notes.recv_timeout(Duration::from_secs(10)).unwrap();
        assert!(matches!(note, Note::EncoderFailed(_)), "{note:?}");
        assert!(!set.stage.has_encoder());
        // The bus has the failure as the encoder's, and none of the compositor's.
        let mut heard = Vec::new();
        while let Some(one) = listener.hear(Duration::from_millis(200)) {
            heard.push(one);
        }
        assert!(
            heard
                .iter()
                .any(|one| matches!(one, Heard::EncoderError(_))),
            "{heard:?}"
        );
        assert!(
            !heard
                .iter()
                .any(|one| matches!(one, Heard::Error(_) | Heard::End)),
            "{heard:?}"
        );
        // What the encoder's element said is kept as the reason.
        assert!(set.stage.encoder_error(Duration::ZERO).is_some());

        // The compositor still makes frames: with an encoder that works they come out again.
        let before = set.out.count();
        set.stage.close_encoder();
        set.stage.set_encoder_side(ENCODER.to_string());
        set.stage.open_encoder().unwrap();
        until("frames again", || set.out.count() >= before + 5);
        set.stage.stop();
    }

    #[test]
    fn an_encoder_that_cannot_be_had_at_the_start_leaves_the_compositor_running() {
        let mut set = set(COMPOSITOR, MISSING, false);
        let started = set.stage.start().unwrap();
        assert!(started.is_err());
        assert!(!set.stage.has_encoder());
        assert!(!frames_come(&set.out));
        // Trying again with the same encoder is refused again, and nothing ends.
        assert!(set.stage.open_encoder().is_err());
        set.stage.set_encoder_side(ENCODER.to_string());
        set.stage.open_encoder().unwrap();
        until("frames", || set.out.count() >= 5);
        set.stage.stop();
    }

    #[test]
    fn an_encoder_that_refuses_its_first_frame_is_refused_on_the_way_back() {
        let mut set = set(COMPOSITOR, ENCODER, false);
        set.stage.start().unwrap().unwrap();
        until("frames", || set.out.count() >= 5);
        set.stage.close_encoder();
        set.stage.set_encoder_side(REFUSING.to_string());
        let listener = set.stage.listener();
        let hearing = std::thread::spawn(move || {
            let began = Instant::now();
            while began.elapsed() < Duration::from_millis(500) {
                listener.hear(Duration::from_millis(50));
            }
        });
        let refused = set.stage.open_encoder();
        assert!(refused.is_err(), "{refused:?}");
        assert!(!set.stage.has_encoder());
        hearing.join().unwrap();
        set.stage.set_encoder_side(ENCODER.to_string());
        let before = set.out.count();
        set.stage.open_encoder().unwrap();
        until("frames again", || set.out.count() >= before + 5);
        set.stage.stop();
    }

    #[test]
    fn with_a_compositor_that_waits_at_the_gate_the_picture_comes_back_and_the_program_can_end() {
        let mut set = set(COMPOSITOR, ENCODER, true);
        set.stage.start().unwrap().unwrap();
        until("frames", || set.out.count() >= 5);
        set.stage.close_encoder();
        assert!(!frames_come(&set.out));
        set.stage.open_encoder().unwrap();
        let before = set.out.count();
        until("frames again", || set.out.count() >= before + 5);
        // And ending while a frame waits at the gate does not hang.
        set.stage.close_encoder();
        std::thread::sleep(Duration::from_millis(50));
        let began = Instant::now();
        set.stage.stop();
        assert!(began.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_picture_that_stands_still_is_sent_again_with_time_stamps_of_its_own() {
        let mut set = set(STILL, ENCODER, true);
        set.stage.start().unwrap().unwrap();
        until("the three frames", || set.out.count() >= 3);
        assert!(!frames_come(&set.out));
        let repeater = set.stage.repeater();
        let (every, hold) = (Duration::from_millis(10), Duration::from_millis(10));
        let began = Instant::now();
        while set.out.count() < 8 {
            assert!(began.elapsed() < Duration::from_secs(10));
            std::thread::sleep(repeater.again(every, hold).min(every));
        }
        let frames = set.out.frames.lock().unwrap().clone();
        assert!(
            frames.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "{frames:?}"
        );
        drop(frames);
        // Nothing is sent again while there is no encoder.
        set.stage.close_encoder();
        let closed_at = set.out.count();
        std::thread::sleep(Duration::from_millis(30));
        repeater.again(every, hold);
        assert_eq!(set.out.count(), closed_at);
        set.stage.stop();
    }

    #[test]
    fn an_input_holds_the_sending_again_back() {
        let mut set = set(STILL, ENCODER, true);
        set.stage.start().unwrap().unwrap();
        until("the three frames", || set.out.count() >= 3);
        std::thread::sleep(Duration::from_millis(60));
        let to_compositor = set.stage.to_compositor().unwrap();
        let repeater = set.stage.repeater();
        let (every, hold) = (Duration::from_millis(50), Duration::from_millis(50));
        // The stand-in for the compositor takes no input; what counts is the note of it.
        // A pointer that moves holds nothing back: the frame is sent again at once.
        to_compositor.input(
            &istesso_proto::InputEvent::PointerAt { x: 1, y: 1 },
            clock(),
        );
        assert_eq!(repeater.again(every, hold), every);
        assert_eq!(set.out.count(), 4);
        // A key that goes down does.
        let key = istesso_proto::InputEvent::Key {
            code: 30,
            pressed: true,
        };
        to_compositor.input(&key, clock());
        let wait = repeater.again(every, hold);
        assert_eq!(set.out.count(), 4);
        assert!(
            wait > Duration::from_millis(30) && wait <= Duration::from_millis(50),
            "{wait:?}"
        );
        set.stage.stop();
    }

    #[test]
    fn frames_sent_again_among_the_compositor_s_own_never_take_the_stamps_backwards() {
        let mut set = set(COMPOSITOR, ENCODER, false);
        set.stage.start().unwrap().unwrap();
        until("frames", || set.out.count() >= 5);
        // As often as it can, with no quiet while asked for: frames sent again
        // land between the compositor's own, stamped with the time now, which
        // is later than the stamps the stand-in counts out for its frames.
        let repeater = set.stage.repeater();
        let began = Instant::now();
        while began.elapsed() < Duration::from_millis(300) {
            repeater.again(Duration::ZERO, Duration::ZERO);
            std::thread::sleep(Duration::from_millis(3));
        }
        let frames = set.out.frames.lock().unwrap().clone();
        assert!(frames.len() > 40, "{}", frames.len());
        assert!(
            frames.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "{frames:?}"
        );
        drop(frames);
        set.stage.stop();
    }

    #[test]
    fn the_compositor_is_told_an_event_in_the_structure_the_script_sent() {
        gst::init().unwrap();
        use istesso_proto::InputEvent;
        for event in [
            InputEvent::Key {
                code: 30,
                pressed: true,
            },
            InputEvent::PointerAt { x: 12, y: 700 },
            InputEvent::Button {
                code: 0x110,
                pressed: false,
            },
            InputEvent::Scroll { dx: 0, dy: -15 },
        ] {
            let told = crate::input::for_compositor(&event);
            let theirs = gst::Structure::from_str(&told.text()).unwrap();
            assert_eq!(structure_of(&told), theirs, "{}", told.text());
        }
    }

    #[test]
    fn a_key_frame_is_asked_of_the_encoder_only_while_there_is_one() {
        let mut set = set(COMPOSITOR, ENCODER, false);
        set.stage.start().unwrap().unwrap();
        until("frames", || set.out.count() >= 3);
        // The stand-in passes the event up to the stand-in for the compositor, which takes it or not.
        let _ = set.stage.key_frame();
        set.stage.close_encoder();
        assert!(!set.stage.key_frame());
        set.stage.stop();
    }
}
