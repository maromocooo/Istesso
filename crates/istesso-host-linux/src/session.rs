//! One session, as the thread that owns it sees it: what it is told, by
//! whoever started the program and by the threads of the pipeline, and what
//! it does and answers.
//!
//! Everything here happens on that one thread. Nothing here is on a frame's
//! path: a frame goes from the compositor through the gate into the encoder
//! and out to the viewers without this thread having to wake.

use std::sync::Arc;
use std::time::Duration;

use crate::contract::{self, Command, Display, Event};
use crate::cursor;
use crate::framelog::{line, FrameLog};
use crate::picture::{At, Picture, State, Told, Who, Works};
use crate::stage::{CursorSeen, Handed, Heard, Note, Out, Stage, ToCompositor};
use crate::words::{self, Answer, Word};

/// How long after its windows were asked to close an application is said to
/// be still open, in seconds.
const STILL_OPEN_AFTER_S: u32 = 5;

/// One key frame answers every request made within this long.
const KEY_FRAME_EVERY_NS: u64 = 50_000_000;

/// The way out to the devices that are shown the picture: the library that
/// carries it, as far as a session needs it. Who may be shown the picture
/// is decided behind this, by the library, as it always was.
pub trait Door: Send + Sync {
    /// Offers the picture, or stops offering it and cuts every viewer off.
    fn offer(&self, offered: bool);
    /// Says who is shown the picture. `false` when that could not be said.
    fn admit(&self, who: &Who) -> bool;
    /// Whether a viewer needs a key frame; true once for each need.
    fn key_frame_wanted(&self) -> bool;
    /// How many devices are being shown the picture now.
    fn viewers(&self) -> usize;
    /// Tells the viewers what the cursor looks like. `false` when the
    /// cursor may not travel.
    fn cursor(&self, cursor: &CursorSeen) -> bool;
    /// Hands one encoded frame to the viewers, on the encoder's thread.
    fn frame(&self, data: &[u8], pts_ns: u64, encoded_ns: u64, key: bool) -> Option<Handed>;
}

/// The door as the place encoded frames go.
pub struct ThroughDoor(pub Arc<dyn Door>);

impl Out for ThroughDoor {
    fn frame(&self, data: &[u8], pts_ns: u64, encoded_ns: u64, key: bool) -> Option<Handed> {
        self.0.frame(data, pts_ns, encoded_ns, key)
    }
}

/// Who started the program, and so how it is spoken to.
#[derive(Debug, Clone, PartialEq)]
pub enum Tongue {
    /// The measuring tools: the script's words and answers.
    Measure,
    /// The agent: the runner's lines. `display` is the picture as it is
    /// made, `port` where the device finds it.
    Runner { display: Display, port: u16 },
}

/// Whether the session goes on.
#[derive(Debug, Clone, PartialEq)]
pub enum Flow {
    On,
    /// It is over: because the compositor ended (`Ok`), or because
    /// something failed that the program cannot go on after.
    End(Result<(), String>),
}

/// The pipeline and the door, as what the picture's decisions are carried out with.
struct Hands<'a> {
    stage: &'a mut Stage,
    door: &'a dyn Door,
}

impl Works for Hands<'_> {
    fn close_encoder(&mut self) {
        self.stage.close_encoder();
    }
    fn open_encoder(&mut self) -> Result<(), String> {
        self.stage.open_encoder()
    }
    fn offer(&mut self, offered: bool) {
        self.door.offer(offered);
    }
    fn admit(&mut self, who: &Who) -> bool {
        self.door.admit(who)
    }
}

/// One session.
pub struct Session {
    stage: Stage,
    door: Arc<dyn Door>,
    picture: Picture,
    tongue: Tongue,
    /// Writes a line for whoever started the program.
    say: Box<dyn FnMut(String) + Send>,
    /// Writes a line for a person who reads the program's own output.
    remark: Box<dyn FnMut(String) + Send>,
    log: FrameLog,
    clock: fn() -> u64,
    to_compositor: Option<ToCompositor>,
    /// Whether the viewers draw the cursor and are told what it looks like.
    cursor_told: bool,
    /// The host's cursor themes, for telling a shape by its name.
    cursors: Option<Arc<cursor::Book>>,
    last_key_frame_ns: u64,
    windows: u32,
    /// When the application's windows were asked to close, until they have
    /// all gone or it has been said that they are still open.
    close_asked_ns: Option<u64>,
    environment: Option<Vec<(String, String)>>,
    /// A runner's lines that came up before it has said `ready`, in their
    /// order: an agent hears `ready` first. `None` once `ready` has been
    /// said, and always for a measuring run, which holds nothing back.
    held: Option<Vec<String>>,
}

/// What a session is begun with, besides its pipeline.
pub struct Begin {
    pub door: Arc<dyn Door>,
    pub tongue: Tongue,
    pub say: Box<dyn FnMut(String) + Send>,
    pub remark: Box<dyn FnMut(String) + Send>,
    pub log: FrameLog,
    pub clock: fn() -> u64,
    pub cursor_told: bool,
    /// The host's cursor themes; `None` passes every cursor on as it comes.
    pub cursors: Option<Arc<cursor::Book>>,
}

impl Session {
    /// Starts the pipeline. `Err` when the compositor does not start: the
    /// program cannot go on. What is returned with the session is still to
    /// be told ([`Session::tell`]): a runner says it is ready first.
    pub fn start(mut stage: Stage, begin: Begin) -> Result<(Self, Vec<Told>), String> {
        let started = stage.start()?;
        let (picture, told) = Picture::begin(started, begin.clock);
        let to_compositor = stage.to_compositor();
        let held = matches!(begin.tongue, Tongue::Runner { .. }).then(Vec::new);
        let session = Session {
            stage,
            door: begin.door,
            picture,
            tongue: begin.tongue,
            say: begin.say,
            remark: begin.remark,
            log: begin.log,
            clock: begin.clock,
            to_compositor,
            cursor_told: begin.cursor_told,
            cursors: begin.cursors,
            last_key_frame_ns: 0,
            windows: 0,
            close_asked_ns: None,
            environment: None,
            held,
        };
        Ok((session, told))
    }

    /// The pipeline, for the threads that are made from it.
    pub fn stage(&self) -> &Stage {
        &self.stage
    }

    /// How an application gets into the compositor, once the compositor has said so.
    pub fn environment(&self) -> Option<&[(String, String)]> {
        self.environment.as_deref()
    }

    /// Says one of the runner's lines. Before `ready` has been said, every
    /// other line is kept; `ready` goes out first and they follow it, in the
    /// order they came up.
    pub fn event(&mut self, event: &Event) {
        let line = contract::write(event);
        if matches!(event, Event::Ready { .. }) {
            (self.say)(line);
            for kept in self.held.take().unwrap_or_default() {
                (self.say)(kept);
            }
        } else if let Some(held) = &mut self.held {
            held.push(line);
        } else {
            (self.say)(line);
        }
    }

    /// Tells whoever started the program what has become of the picture,
    /// and writes it to the frame log.
    pub fn tell(&mut self, told: Vec<Told>) {
        let now = (self.clock)();
        for told in told {
            match (self.tongue.clone(), told) {
                (tongue, Told::Paused { offered, took_ms }) => {
                    self.log.line(line::paused(now, offered, took_ms));
                    match tongue {
                        Tongue::Measure => {
                            (self.say)(words::write(&Answer::Paused { offered, took_ms }));
                        }
                        Tongue::Runner { .. } => self.event(&Event::Paused { took_ms }),
                    }
                }
                (
                    tongue,
                    Told::Resumed {
                        opened_ms,
                        encoded_ms,
                        written_ms,
                        key,
                    },
                ) => {
                    self.log
                        .line(line::resumed(now, opened_ms, encoded_ms, written_ms, key));
                    match tongue {
                        Tongue::Measure => (self.say)(words::write(&Answer::Resumed {
                            opened_ms,
                            encoded_ms,
                            written_ms,
                            key,
                        })),
                        Tongue::Runner { .. } => self.event(&Event::Resumed {
                            opened_ms,
                            encoded_ms,
                            written_ms,
                        }),
                    }
                }
                (tongue, Told::NoEncoder { at, reason }) => {
                    self.log.line(line::refused(now, &reason));
                    let message = format!(
                        "no encoder could be had ({reason}); no picture is made, the application goes on"
                    );
                    match tongue {
                        Tongue::Measure => (self.say)(words::write(&Answer::Refused {
                            word: match at {
                                At::Start => "start",
                                At::Resume => "resume",
                                At::Run => "run",
                            },
                            reason: message,
                        })),
                        Tongue::Runner { .. } => self.event(&Event::NoEncoder { message }),
                    }
                }
                (Tongue::Measure, Told::Admitted { device }) => {
                    (self.say)(words::write(&Answer::Admitted { device }));
                }
                // A runner answers `attach` with `attached`, where it is handled.
                (Tongue::Runner { .. }, Told::Admitted { .. }) => {}
            }
        }
    }

    fn hands(&mut self) -> (&mut Picture, Hands<'_>) {
        let hands = Hands {
            stage: &mut self.stage,
            door: &*self.door,
        };
        (&mut self.picture, hands)
    }

    fn pause(&mut self) {
        let (picture, mut hands) = self.hands();
        let told = picture.pause(&mut hands);
        self.tell(told);
    }

    fn detach(&mut self) {
        let (picture, mut hands) = self.hands();
        let told = picture.detach(&mut hands);
        self.tell(told);
    }

    fn resume(&mut self) {
        let (picture, mut hands) = self.hands();
        let told = picture.resume(&mut hands);
        self.tell(told);
    }

    fn admit(&mut self, who: Who) {
        let (picture, mut hands) = self.hands();
        let told = picture.admit(&mut hands, who);
        self.tell(told);
    }

    fn close_windows(&mut self) -> bool {
        let asked = self
            .to_compositor
            .as_ref()
            .is_some_and(ToCompositor::close_windows);
        self.log.line(line::close((self.clock)(), asked));
        asked
    }

    /// A line from whoever started the program.
    pub fn line(&mut self, text: &str) {
        match self.tongue.clone() {
            Tongue::Measure => match words::read(text) {
                Some(Word::Pause) => self.pause(),
                Some(Word::Detach) => self.detach(),
                Some(Word::Resume) => self.resume(),
                Some(Word::Close) => {
                    let asked = self.close_windows();
                    (self.say)(words::write(&Answer::Close { asked }));
                }
                Some(Word::Admit(Some((id, public_key)))) => {
                    self.admit(Who::Only { id, public_key })
                }
                Some(Word::Admit(None)) => self.admit(Who::Nobody),
                Some(Word::Unknown(word)) => {
                    (self.remark)(format!(
                        "note: {word:?} is not something this program can be told"
                    ));
                }
                None => {}
            },
            Tongue::Runner { display, port } => match contract::read(text) {
                Some(Command::Pause) => self.pause(),
                Some(Command::Resume) => self.resume(),
                Some(Command::Detach) => {
                    // Nobody first, so that nothing goes to the device that left.
                    self.admit(Who::Nobody);
                    self.detach();
                }
                Some(Command::Attach { peer, .. }) => {
                    self.admit(who(peer));
                    // The picture keeps the size it was started with.
                    self.event(&Event::Attached {
                        display,
                        answer: contract::answer(port),
                    });
                    self.resume();
                }
                // Also how an application is ended: every window it has is
                // asked to close, and none is closed by force.
                Some(Command::Close) => {
                    let windows = self.windows;
                    let asked = self.close_windows();
                    self.event(&Event::CloseAsked { asked, windows });
                    self.close_asked_ns = (asked && windows > 0).then(|| (self.clock)());
                    if !asked {
                        (self.remark)(
                            "note: this compositor plugin cannot ask windows to close".to_string(),
                        );
                    }
                }
                // Applications come to the compositor by themselves.
                Some(Command::Application { .. }) => {}
                Some(Command::Start { .. }) => {
                    (self.remark)("note: the session has been started already".to_string());
                }
                // Lines that are not known are passed over.
                None => {}
            },
        }
    }

    /// Something a thread of the pipeline has to say.
    pub fn note(&mut self, note: Note) {
        match note {
            Note::EncoderFailed(reason) => {
                // What the encoder's own element says is the better reason, and
                // arrives on the bus a moment after a frame was refused.
                let said = self.stage.encoder_error(Duration::from_millis(50));
                self.encoder_failed(said.unwrap_or(reason));
            }
            Note::FirstFrame {
                encoded_ns,
                written_ns,
                key,
            } => {
                let told = self.picture.frame_out(encoded_ns, written_ns, key);
                self.tell(told);
            }
        }
    }

    fn encoder_failed(&mut self, reason: String) {
        let (picture, mut hands) = self.hands();
        let told = picture.encoder_failed(&mut hands, reason);
        self.tell(told);
    }

    /// Something the pipeline's bus carried.
    pub fn heard(&mut self, heard: Heard) -> Flow {
        let now = (self.clock)();
        match heard {
            Heard::Cursor(cursor) => {
                if self.cursor_told {
                    let cursor = self.by_name_where_it_can(cursor);
                    let accepted = self.door.cursor(&cursor);
                    let (shape, image) = match &cursor {
                        CursorSeen::Hidden => ("hidden", None),
                        CursorSeen::Named(_) => ("named", None),
                        CursorSeen::Image {
                            width,
                            height,
                            hotspot_x,
                            hotspot_y,
                            scale,
                            ..
                        } => (
                            "image",
                            Some([*width, *height, *hotspot_x, *hotspot_y, *scale]),
                        ),
                    };
                    self.log.line(line::cursor(now, shape, image, accepted));
                }
            }
            Heard::Windows(count) => {
                self.log.line(line::windows(now, count));
                let gone = count == 0 && self.windows > 0;
                self.windows = count;
                if count == 0 {
                    self.close_asked_ns = None;
                }
                match self.tongue {
                    Tongue::Measure => {
                        (self.say)(words::write(&Answer::Windows { count }));
                        if gone {
                            (self.say)(words::write(&Answer::Empty));
                        }
                    }
                    Tongue::Runner { .. } => {
                        self.event(&Event::Windows { count });
                        if gone {
                            // The last window has gone: the application is over,
                            // whatever its processes do.
                            self.event(&Event::Empty);
                        }
                    }
                }
            }
            Heard::Environment(environment) => {
                if self.tongue == Tongue::Measure {
                    // Whoever starts an application by hand reads this.
                    let text: Vec<String> = environment
                        .iter()
                        .map(|(name, value)| format!("{name}={value}"))
                        .collect();
                    (self.remark)(format!(
                        "note: an application gets into the compositor with {}",
                        text.join(" ")
                    ));
                }
                self.environment = Some(environment);
            }
            Heard::EncoderError(reason) => self.encoder_failed(reason),
            Heard::Error(reason) => return Flow::End(Err(reason)),
            Heard::End => return Flow::End(Ok(())),
        }
        Flow::On
    }

    /// Turns a cursor into what travels: the name of its shape where the
    /// device has a cursor of its own for it, an image otherwise (see
    /// [`cursor`]). On the thread that hears the bus; never on a frame's path.
    fn by_name_where_it_can(&self, seen: CursorSeen) -> CursorSeen {
        let Some(book) = &self.cursors else {
            return seen;
        };
        match seen {
            CursorSeen::Hidden => CursorSeen::Hidden,
            CursorSeen::Named(name) if cursor::travels_as_name(&name) => CursorSeen::Named(name),
            // A shape the set does not have: as the theme draws it. Where no
            // theme has it, the name goes: a device that knows it shows its
            // own, one that does not shows its arrow.
            CursorSeen::Named(name) => match book.image_of(&name, 24) {
                Some(image) => CursorSeen::Image {
                    width: image.width,
                    height: image.height,
                    hotspot_x: image.hotspot_x,
                    hotspot_y: image.hotspot_y,
                    scale: 1,
                    pixels: image.rgba,
                },
                None => CursorSeen::Named(name),
            },
            CursorSeen::Image {
                width,
                height,
                hotspot_x,
                hotspot_y,
                scale,
                pixels,
            } => {
                let image = cursor::Image {
                    width,
                    height,
                    hotspot_x,
                    hotspot_y,
                    rgba: pixels,
                };
                match book.name_of(&image) {
                    Some(name) => CursorSeen::Named(name.to_string()),
                    None => CursorSeen::Image {
                        width,
                        height,
                        hotspot_x,
                        hotspot_y,
                        scale,
                        pixels: image.rgba,
                    },
                }
            }
        }
    }

    /// Looked at every few milliseconds, also while no frame is made: a
    /// viewer has connected, has asked for a key frame, or was held back.
    pub fn tick(&mut self) {
        if self.door.key_frame_wanted() {
            self.key_frame("client");
        }
        if let Some(asked) = self.close_asked_ns {
            let waited = (self.clock)().saturating_sub(asked);
            if waited >= u64::from(STILL_OPEN_AFTER_S) * 1_000_000_000 {
                // The application asks something, or stays: the session goes on.
                self.close_asked_ns = None;
                if self.windows > 0 && matches!(self.tongue, Tongue::Runner { .. }) {
                    self.event(&Event::StillOpen {
                        windows: self.windows,
                        after_s: STILL_OPEN_AFTER_S,
                    });
                }
            }
        }
    }

    /// Makes a key frame, unless one was made a moment ago or there is no
    /// encoder to ask: a picture that comes back begins with one anyway.
    pub fn key_frame(&mut self, by: &str) {
        let now = (self.clock)();
        let made = now.saturating_sub(self.last_key_frame_ns) >= KEY_FRAME_EVERY_NS
            && self.picture.state() == State::Running;
        if made {
            self.last_key_frame_ns = now;
            self.stage.key_frame();
        }
        self.log.line(line::key_request(now, made, by));
    }

    /// Ends the session: nothing more goes out, and the pipeline stops.
    pub fn stop(&mut self) {
        self.door.offer(false);
        self.stage.stop();
    }
}

/// The device a runner's line names: by its id and its key together, or nobody.
pub fn who(peer: Option<contract::Peer>) -> Who {
    match peer {
        Some(contract::Peer {
            id,
            public_key: Some(public_key),
            ..
        }) => Who::Only { id, public_key },
        _ => Who::Nobody,
    }
}

#[cfg(test)]
mod tests {
    //! With the stand-ins of the stage's tests for the compositor and the
    //! encoder, and a door that writes down what it was asked.

    use super::*;
    use crate::stage::Plan;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc::{channel, Receiver};
    use std::sync::Mutex;
    use std::time::Instant;

    #[derive(Default)]
    struct Asked {
        calls: Mutex<Vec<String>>,
        frames: AtomicUsize,
        key_wanted: AtomicBool,
    }

    impl Door for Asked {
        fn offer(&self, offered: bool) {
            self.calls.lock().unwrap().push(format!("offer {offered}"));
        }
        fn admit(&self, who: &Who) -> bool {
            self.calls.lock().unwrap().push(match who {
                Who::Only { id, .. } => format!("admit {id}"),
                Who::Nobody => "admit nobody".to_string(),
                Who::Paired => "admit paired".to_string(),
            });
            true
        }
        fn key_frame_wanted(&self) -> bool {
            self.key_wanted.swap(false, Ordering::SeqCst)
        }
        fn viewers(&self) -> usize {
            1
        }
        fn cursor(&self, cursor: &CursorSeen) -> bool {
            self.calls
                .lock()
                .unwrap()
                .push(format!("cursor {cursor:?}"));
            true
        }
        fn frame(&self, _: &[u8], _: u64, _: u64, _: bool) -> Option<Handed> {
            self.frames.fetch_add(1, Ordering::SeqCst);
            Some(Handed::default())
        }
    }

    fn clock() -> u64 {
        static BEGAN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        // Well past nought, so that a test can speak of some seconds ago.
        BEGAN.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 100_000_000_000
    }

    const COMPOSITOR: &str = "videotestsrc name=compositor is-live=true ! video/x-raw,width=32,height=32,framerate=100/1";
    const ENCODER: &str =
        "identity name=convert ! identity name=enc ! appsink name=net sync=false async=false";
    const FAILING: &str = "identity name=convert ! identity name=enc error-after=30 ! appsink name=net sync=false async=false";
    const MISSING: &str = "identity name=convert ! istessonosuchencoder name=enc ! appsink name=net sync=false async=false";

    struct Set {
        session: Session,
        told: Vec<Told>,
        door: Arc<Asked>,
        said: Arc<Mutex<Vec<String>>>,
        notes: Receiver<Note>,
    }

    fn set(encoder: &str, tongue: Tongue) -> Set {
        let door = Arc::new(Asked::default());
        let said = Arc::new(Mutex::new(Vec::new()));
        let (notes_in, notes) = channel();
        let stage = Stage::build(Plan {
            compositor_side: COMPOSITOR.to_string(),
            encoder_side: encoder.to_string(),
            waits: false,
            log: FrameLog::none(),
            clock,
            out: Arc::new(ThroughDoor(door.clone())),
            notes: notes_in,
            on_path_thread: None,
        })
        .unwrap();
        let lines = said.clone();
        let (session, told) = Session::start(
            stage,
            Begin {
                door: door.clone(),
                tongue,
                say: Box::new(move |line| lines.lock().unwrap().push(line)),
                remark: Box::new(|_| {}),
                log: FrameLog::none(),
                clock,
                cursor_told: true,
                cursors: None,
            },
        )
        .unwrap();
        Set {
            session,
            told,
            door,
            said,
            notes,
        }
    }

    impl Set {
        /// Lets the session hear what the pipeline's threads have to say
        /// until `holds`, as the program's loop does.
        fn until(&mut self, what: &str, mut holds: impl FnMut(&Set) -> bool) {
            let began = Instant::now();
            while !holds(self) {
                assert!(began.elapsed() < Duration::from_secs(10), "never: {what}");
                if let Ok(note) = self.notes.recv_timeout(Duration::from_millis(4)) {
                    self.session.note(note);
                }
            }
        }

        fn frames(&self) -> usize {
            self.door.frames.load(Ordering::SeqCst)
        }

        fn said(&self) -> Vec<serde_json::Value> {
            let said = self.said.lock().unwrap();
            said.iter()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }

        fn calls(&self) -> Vec<String> {
            self.door.calls.lock().unwrap().clone()
        }
    }

    fn runner() -> Tongue {
        Tongue::Runner {
            display: Display {
                width: 32,
                height: 32,
                fps: Some(100),
            },
            port: 47850,
        }
    }

    fn ready() -> Event {
        Event::Ready {
            env: Default::default(),
            display: Display {
                width: 32,
                height: 32,
                fps: Some(100),
            },
            answer: contract::answer(47850),
        }
    }

    /// A runner that has said `ready`, as it has by the time an agent tells
    /// it anything; what it says from here on is all that `said` has.
    fn ready_runner(encoder: &str) -> Set {
        let mut set = set(encoder, runner());
        set.session.event(&ready());
        set.said.lock().unwrap().clear();
        set
    }

    #[test]
    fn a_runner_says_nothing_before_ready_and_what_came_up_follows_it_in_order() {
        let mut set = set(MISSING, runner());
        // A compositor that begins with no window says so before it says how
        // an application gets into it; the encoder was refused before that.
        set.session.heard(Heard::Windows(0));
        let told = std::mem::take(&mut set.told);
        set.session.tell(told);
        set.session.heard(Heard::Windows(1));
        assert!(set.said().is_empty());
        set.session.event(&ready());
        let events = |set: &Set| -> Vec<String> {
            set.said()
                .iter()
                .map(|line| line["event"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(events(&set), ["ready", "windows", "no_encoder", "windows"]);
        assert_eq!(set.said()[1]["count"], 0);
        assert_eq!(set.said()[3]["count"], 1);
        // From here on a line is said when it comes up, and none twice.
        set.session.heard(Heard::Windows(2));
        assert_eq!(events(&set).len(), 5);
        assert_eq!(set.said()[4]["count"], 2);
        set.session.stop();

        // A measuring run holds nothing back: its tools read the lines as they come.
        let mut set = self::set(ENCODER, Tongue::Measure);
        set.session.heard(Heard::Windows(0));
        assert_eq!(set.said().len(), 1);
        set.session.stop();
    }

    #[test]
    fn the_measuring_tools_words_stop_the_picture_and_bring_it_back() {
        let mut set = set(ENCODER, Tongue::Measure);
        assert!(set.told.is_empty());
        set.until("frames", |set| set.frames() >= 5);
        set.session.line("pause");
        let paused_at = set.frames();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(set.frames(), paused_at);
        set.session.line("resume");
        set.until("the answer to resume", |set| set.said().len() >= 2);
        let said = set.said();
        assert_eq!(said[0]["istesso"], "paused");
        assert_eq!(said[0]["offered"], true);
        assert!(said[0]["took_ms"].as_f64().unwrap() < 300.0);
        assert_eq!(said[1]["istesso"], "resumed");
        assert!(set.frames() > paused_at);
        assert_eq!(set.calls(), ["offer true"]);
        set.session.stop();
    }

    #[test]
    fn a_runner_s_detach_and_attach_name_nobody_and_then_the_device() {
        let mut set = ready_runner(ENCODER);
        set.until("frames", |set| set.frames() >= 5);
        set.session.line(r#"{"v":1,"cmd":"detach"}"#);
        assert_eq!(set.calls(), ["admit nobody", "offer false"]);
        let detached_at = set.frames();
        set.session.line(
            r#"{"v":1,"cmd":"attach","peer":{"id":"1a2b","public_key":"KEY"},"display":{"width":64,"height":64,"fps":60}}"#,
        );
        set.until("resumed", |set| set.said().len() >= 3);
        let said = set.said();
        assert_eq!(said[0]["event"], "paused");
        assert_eq!(said[1]["event"], "attached");
        assert_eq!(said[1]["display"]["width"], 32);
        assert_eq!(said[1]["answer"]["channel_port"], 47850);
        assert_eq!(said[2]["event"], "resumed");
        assert!(said.iter().all(|line| line["v"] == 1));
        assert_eq!(
            set.calls(),
            ["admit nobody", "offer false", "admit 1a2b", "offer true"]
        );
        assert!(set.frames() > detached_at);
        // A device without its key is nobody.
        set.session
            .line(r#"{"v":1,"cmd":"attach","peer":{"id":"1a2b"}}"#);
        assert_eq!(set.calls().last().unwrap(), "offer true");
        assert!(set.calls().contains(&"admit nobody".to_string()));
        set.session.stop();
    }

    #[test]
    fn an_encoder_that_cannot_be_had_at_the_start_is_said_and_resume_tries_again() {
        let mut set = ready_runner(MISSING);
        assert!(matches!(
            set.told[..],
            [Told::NoEncoder { at: At::Start, .. }]
        ));
        let told = std::mem::take(&mut set.told);
        set.session.tell(told);
        set.session.line(r#"{"v":1,"cmd":"resume"}"#);
        let said = set.said();
        assert_eq!(said.len(), 2);
        assert!(said.iter().all(|line| line["event"] == "no_encoder"));
        assert!(said[0]["message"]
            .as_str()
            .unwrap()
            .contains("the application goes on"));
        assert_eq!(set.frames(), 0);
        set.session.stop();
    }

    #[test]
    fn an_encoder_that_fails_while_the_picture_runs_is_said_and_nothing_ends() {
        let mut set = set(FAILING, Tongue::Measure);
        let listener = set.session.stage().listener();
        set.until("the refusal", |set| !set.said().is_empty());
        let said = set.said();
        assert_eq!(said[0]["istesso"], "refused");
        assert_eq!(said[0]["word"], "run");
        // What the bus carries about it changes nothing more, and ends nothing.
        while let Some(heard) = listener.hear(Duration::from_millis(100)) {
            assert_eq!(set.session.heard(heard), Flow::On);
        }
        assert_eq!(set.said().len(), 1);
        // The same encoder fails again after its thirtieth frame: the picture
        // comes back, and is refused again, and still nothing ends.
        set.session.line("resume");
        set.until("the second refusal", |set| set.said().len() >= 3);
        let said = set.said();
        assert_eq!(said[1]["istesso"], "resumed");
        assert_eq!(said[2]["istesso"], "refused");
        set.session.stop();
    }

    #[test]
    fn windows_are_counted_out_loud_and_the_last_one_gone_is_the_end_of_the_application() {
        let mut set = ready_runner(ENCODER);
        for count in [1, 2, 1, 0] {
            assert_eq!(set.session.heard(Heard::Windows(count)), Flow::On);
        }
        let events: Vec<String> = set
            .said()
            .iter()
            .map(|line| line["event"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            events,
            ["windows", "windows", "windows", "windows", "empty"]
        );
        set.session.stop();
        // A compositor that begins with no window has not lost its last one.
        let mut set = self::set(ENCODER, Tongue::Measure);
        set.session.heard(Heard::Windows(0));
        assert_eq!(set.said().len(), 1);
        set.session.stop();
    }

    #[test]
    fn ending_the_application_asks_its_windows_and_says_once_when_they_are_still_open() {
        let mut set = ready_runner(ENCODER);
        set.session.heard(Heard::Windows(2));
        // The stand-in for the compositor cannot be asked: that is said, and nothing is waited for.
        set.session.line(r#"{"v":1,"cmd":"close"}"#);
        let said = set.said();
        assert_eq!(
            said[1],
            serde_json::json!({"v": 1, "event": "close_asked", "asked": false, "windows": 2})
        );
        assert_eq!(set.session.close_asked_ns, None);

        // A compositor that was asked a moment ago: nothing is said yet.
        set.session.close_asked_ns = Some(clock());
        set.session.tick();
        assert_eq!(set.said().len(), 2);
        // Asked more than five seconds ago with a window left: said, once.
        set.session.close_asked_ns = Some(clock() - 6_000_000_000);
        set.session.tick();
        set.session.tick();
        let said = set.said();
        assert_eq!(said.len(), 3);
        assert_eq!(
            said[2],
            serde_json::json!({"v": 1, "event": "still_open", "windows": 2, "after_s": 5})
        );
        // The last window going ends the waiting: `empty`, and no `still_open` after it.
        set.session.close_asked_ns = Some(clock() - 6_000_000_000);
        set.session.heard(Heard::Windows(0));
        set.session.tick();
        let events: Vec<String> = set.said()[3..]
            .iter()
            .map(|line| line["event"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(events, ["windows", "empty"]);
        set.session.stop();
    }

    #[test]
    fn a_key_frame_is_made_when_a_viewer_wants_one_and_not_twice_in_a_moment() {
        let mut set = set(ENCODER, Tongue::Measure);
        set.until("frames", |set| set.frames() >= 3);
        set.door.key_wanted.store(true, Ordering::SeqCst);
        set.session.tick();
        assert!(set.session.last_key_frame_ns > 0);
        let first = set.session.last_key_frame_ns;
        set.door.key_wanted.store(true, Ordering::SeqCst);
        set.session.tick();
        assert_eq!(set.session.last_key_frame_ns, first);
        set.session.stop();
    }

    #[test]
    fn a_cursor_that_is_the_theme_s_image_goes_to_the_viewers_by_its_name() {
        // A theme with one cursor, the I-beam, of one pixel.
        let icons = std::env::temp_dir().join(format!("ihl-session-icons-{}", std::process::id()));
        let cursors = icons.join("Theme/cursors");
        std::fs::create_dir_all(&cursors).unwrap();
        let mut file = b"Xcur".to_vec();
        for number in [16u32, 0x1_0000, 1, 0xfffd_0002, 24, 28] {
            file.extend_from_slice(&number.to_le_bytes());
        }
        for number in [36u32, 0xfffd_0002, 24, 1, 1, 1, 0, 0, 0] {
            file.extend_from_slice(&number.to_le_bytes());
        }
        file.extend_from_slice(&[30, 20, 10, 255]);
        std::fs::write(cursors.join("xterm"), &file).unwrap();
        let book = cursor::Book::open(vec![icons.clone()]);
        let began = Instant::now();
        while !book.is_read() {
            assert!(began.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(2));
        }

        let mut set = set(ENCODER, Tongue::Measure);
        set.session.cursors = Some(book);
        let image = |pixels: Vec<u8>| CursorSeen::Image {
            width: 1,
            height: 1,
            hotspot_x: 0,
            hotspot_y: 0,
            scale: 1,
            pixels,
        };
        // The theme's own image: told by name. Another image: told as it is.
        set.session
            .heard(Heard::Cursor(image(vec![10, 20, 30, 255])));
        set.session
            .heard(Heard::Cursor(image(vec![11, 20, 30, 255])));
        // A name of the set stays a name; one outside it, which no theme here has, too.
        set.session
            .heard(Heard::Cursor(CursorSeen::Named("pointer".into())));
        set.session
            .heard(Heard::Cursor(CursorSeen::Named("crosshair".into())));
        set.session.heard(Heard::Cursor(CursorSeen::Hidden));
        let calls = set.calls();
        assert_eq!(calls[0], r#"cursor Named("text")"#);
        assert!(calls[1].starts_with("cursor Image"), "{}", calls[1]);
        assert_eq!(calls[2], r#"cursor Named("pointer")"#);
        assert_eq!(calls[3], r#"cursor Named("crosshair")"#);
        assert_eq!(calls[4], "cursor Hidden");
        set.session.stop();
        let _ = std::fs::remove_dir_all(&icons);
    }

    #[test]
    fn the_cursor_goes_to_the_viewers_and_what_fails_elsewhere_ends_the_session() {
        let mut set = set(ENCODER, Tongue::Measure);
        set.session
            .heard(Heard::Cursor(CursorSeen::Named("text".into())));
        assert_eq!(set.calls(), [r#"cursor Named("text")"#]);
        set.session.heard(Heard::Environment(vec![(
            "WAYLAND_DISPLAY".into(),
            "wayland-3".into(),
        )]));
        assert_eq!(set.session.environment().unwrap()[0].1, "wayland-3");
        assert_eq!(
            set.session
                .heard(Heard::Error("the compositor is gone".into())),
            Flow::End(Err("the compositor is gone".into()))
        );
        set.session.stop();
    }
}
