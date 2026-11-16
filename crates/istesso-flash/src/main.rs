//! Test application for latency measurements.
//!
//! A window that shows one of two shades of grey and switches to the other
//! on every input: any key, any mouse button, or a synthetic input over UDP
//! (`istesso_measure::flash`). It reacts to an input at once and does nothing
//! else with it, so the time from an input to the changed picture is as short
//! and as even as an application can make it. What a relay adds on top of
//! that is what M1 measures.
//!
//! Around the shade the window shows scrolling text, the screen of an
//! application at work, so that the relay has a real picture to carry
//! (`--picture text`, the default). `--picture grey` fills the whole window
//! with the shade instead: the easiest picture there is, for a best-case
//! reference.
//!
//! For every input the application records when the input arrived and when
//! the frame that shows it was committed, both on this machine's clock
//! (`--log`). The difference is the application's own response time, which
//! `docs/measurements/README.md` subtracts from the round trip.

use std::fs::{File, OpenOptions};
use std::io::BufWriter;
use std::net::{SocketAddr, UdpSocket};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser;
use istesso_measure::{
    Clock, FlashAck, FlashInput, Header, HostSample, InputSource, Jitter, LogWriter, Record, Shade,
    Side,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, KeyEvent, StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId, WindowLevel};

mod gpu;
mod picture;

use picture::{Picture, Sheet};

/// How the picture gets into the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Draw {
    /// Painted on the CPU and handed to the window system in shared memory
    Cpu,
    /// Drawn with OpenGL: the window system gets a buffer that is on the GPU already
    Gpu,
}

impl Draw {
    /// The name written into a log.
    fn name(self) -> &'static str {
        match self {
            Draw::Cpu => "cpu",
            Draw::Gpu => "gpu",
        }
    }
}

/// When an input is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum Answer {
    /// With a frame of its own, the moment the input arrives
    AtOnce,
    /// With the next frame of the moving text
    NextFrame,
}

#[derive(Parser)]
#[command(
    name = "istesso-flash",
    version,
    about = "A window that switches shade on every input, for latency measurements",
    after_help = "Any key or mouse button switches the shade. Escape quits."
)]
struct Args {
    /// What the window shows around the shade
    #[arg(long, value_enum, default_value_t = Picture::Text)]
    picture: Picture,

    /// How fast the text moves up, in points per second. 0 keeps it still
    #[arg(long, value_name = "POINTS", default_value_t = 240)]
    scroll: u32,

    /// Width of the text's strokes in pixels. Two points unless given, text
    /// of an everyday size; 1 is the hardest case for a relay that thins
    /// out colour
    #[arg(long, value_name = "PIXELS", value_parser = clap::value_parser!(u32).range(1..=16))]
    stroke: Option<u32>,

    /// Put a stamp in the top left corner of a moving picture that says how
    /// far it has moved. A frame caught on a screen then says which frame
    /// it is (`istesso measure quality --stamped`)
    #[arg(long)]
    stamp: bool,

    /// How often the moving text is redrawn, in frames per second. The
    /// refresh rate of the display when not given, or 60 where the display
    /// does not say
    #[arg(long, value_name = "HZ", value_parser = clap::value_parser!(u32).range(1..=1000))]
    fps: Option<u32>,

    /// How the picture is drawn. An application that draws with the GPU hands
    /// the window system a buffer that is on the GPU already, as most
    /// applications do; one that paints on the CPU hands over pixels that a
    /// compositor has to upload first
    #[arg(long, value_enum, default_value_t = Draw::Cpu)]
    draw: Draw,

    /// Draw the next frame only when the window system asks for it, as most
    /// applications do, instead of keeping a rhythm of their own: handing a
    /// frame over waits for that. Needs `--draw gpu`. `--fps` then says how
    /// fast the application would draw if it were asked often enough; give
    /// it a rate well above the display's
    #[arg(long)]
    when_told: bool,

    /// After every frame that answered an input, put the next frame off by a
    /// part of a frame interval chosen at random, the same sequence for the
    /// same seed. The application's beat then meets the display's refresh in
    /// every possible way over a run, and a measurement no longer depends on
    /// how the two happened to fall (ADR 0002). Needs the moving text
    /// answered with its next frame, and not `--when-told`
    #[arg(long, value_name = "SEED", conflicts_with = "when_told")]
    shift_beat: Option<u64>,

    /// When an input is answered. `next-frame` where the text moves, so that
    /// no more frames are drawn than `--fps` says, and `at-once` where
    /// nothing moves, unless given
    #[arg(long, value_enum)]
    answer: Option<Answer>,

    /// Write one line per input to this file: when the input arrived and when
    /// the frame that shows it was committed. The file must not exist yet
    #[arg(long, value_name = "FILE")]
    log: Option<PathBuf>,

    /// Also take synthetic inputs over UDP on this address, for example
    /// 0.0.0.0:47811. Unencrypted and unauthenticated: for measurements on a
    /// trusted network only
    #[arg(long, value_name = "ADDRESS:PORT")]
    listen: Option<SocketAddr>,

    /// No title bar and no borders. Use this inside the streaming compositor,
    /// where the client draws the window frame
    #[arg(long)]
    borderless: bool,

    /// Cover the whole screen
    #[arg(long)]
    fullscreen: bool,

    /// Keep the window above the others. A measuring tool that watches the
    /// middle of the window sees nothing once another window lies over it
    #[arg(long)]
    on_top: bool,

    /// Size of the window's content, in points
    #[arg(long, value_name = "WIDTHxHEIGHT", default_value = "960x600", value_parser = parse_size)]
    size: (u32, u32),

    /// Where the top left corner of the window goes, in points from the top
    /// left of the main display. Left to the window system when not given,
    /// and ignored where the window system does not let applications choose
    #[arg(long, value_name = "X,Y", value_parser = parse_position, allow_hyphen_values = true)]
    position: Option<(i32, i32)>,

    /// Title of the window
    #[arg(long, default_value = "Istesso Flash")]
    title: String,
}

fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let parsed = text.split_once(['x', 'X']).and_then(|(width, height)| {
        Some((width.parse::<u32>().ok()?, height.parse::<u32>().ok()?))
    });
    match parsed {
        Some((width, height)) if width > 0 && height > 0 => Ok((width, height)),
        _ => Err(format!(
            "expected WIDTHxHEIGHT, for example 960x600, not {text:?}"
        )),
    }
}

fn parse_position(text: &str) -> Result<(i32, i32), String> {
    text.split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse::<i32>().ok()?, y.trim().parse::<i32>().ok()?)))
        .ok_or_else(|| format!("expected X,Y, for example 100,80, not {text:?}"))
}

/// What reaches the event loop from outside the window system.
#[derive(Debug)]
enum Outside {
    /// A synthetic input, stamped when its datagram was read.
    Input {
        id: u64,
        input_ns: u64,
        from: SocketAddr,
    },
}

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

/// An input that waits for the next frame of the moving text.
struct Waiting {
    source: InputSource,
    id: u64,
    input_ns: u64,
    answer_to: Option<SocketAddr>,
}

struct App {
    args: Args,
    clock: Clock,
    shade: Shade,
    /// Number of the next input made by hand.
    next_id: u64,
    /// Number of the last synthetic input, to drop a datagram that arrives twice.
    last_synthetic: Option<u64>,
    log: Option<LogWriter<BufWriter<File>>>,
    /// Sends the answers to synthetic inputs.
    socket: Option<UdpSocket>,
    /// Inputs to be answered with the next frame.
    waiting: Vec<Waiting>,
    /// What draws with the GPU. Declared before the window it draws to, so
    /// that it is dropped first.
    gpu: Option<gpu::Gpu>,
    window: Option<Rc<Window>>,
    /// What paints on the CPU.
    surface: Option<Surface>,
    /// The text, painted for the window's present size.
    sheet: Option<Sheet>,
    /// Time between two frames of moving text. `None` when nothing moves.
    frame_period: Option<Duration>,
    /// When the next frame of moving text is due.
    next_frame: Option<Instant>,
    /// By how much the beat is put off after a frame that answered an input
    /// (`--shift-beat`).
    shifts: Option<Jitter>,
    /// Why the application has to stop, if it does.
    failure: Option<String>,
}

impl App {
    fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let (width, height) = self.args.size;
        let mut attributes = Window::default_attributes()
            .with_title(self.args.title.clone())
            .with_inner_size(LogicalSize::new(f64::from(width), f64::from(height)))
            .with_decorations(!self.args.borderless)
            .with_window_level(if self.args.on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });
        if let Some((x, y)) = self.args.position {
            attributes = attributes.with_position(LogicalPosition::new(f64::from(x), f64::from(y)));
        }
        if self.args.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = match self.args.draw {
            Draw::Gpu => {
                let (window, gpu) = gpu::Gpu::open(event_loop, attributes, self.args.when_told)?;
                self.gpu = Some(gpu);
                Rc::new(window)
            }
            Draw::Cpu => {
                if self.args.when_told {
                    return Err("--when-told needs --draw gpu".into());
                }
                let window = Rc::new(
                    event_loop
                        .create_window(attributes)
                        .map_err(|e| format!("cannot open a window: {e}"))?,
                );
                let context = softbuffer::Context::new(window.clone())
                    .map_err(|e| format!("cannot draw on this display: {e}"))?;
                let surface = softbuffer::Surface::new(&context, window.clone())
                    .map_err(|e| format!("cannot draw into the window: {e}"))?;
                self.surface = Some(surface);
                window
            }
        };
        if let Some((x, y)) = self.args.position {
            // Asked for again once the window exists: at creation macOS moves a window that
            // was placed on another display than the one in use back onto that one.
            window.set_outer_position(LogicalPosition::new(f64::from(x), f64::from(y)));
        }
        if self.args.picture != Picture::Grey && self.args.scroll > 0 {
            let refresh = window
                .current_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz())
                .map(|millihertz| (millihertz + 500) / 1000)
                .filter(|hertz| *hertz > 0);
            let fps = self.args.fps.or(refresh).unwrap_or(60);
            let period = Duration::from_secs(1) / fps;
            self.frame_period = Some(period);
            self.shifts = self.args.shift_beat.map(|seed| beat_shifts(seed, period));
        }
        self.window = Some(window);
        Ok(())
    }

    /// Paints the picture as it is now and commits the frame.
    ///
    /// `false` when there is nothing to draw into: the window is not open
    /// yet, or it is minimised.
    fn draw(&mut self) -> Result<bool, String> {
        let Some(window) = &self.window else {
            return Ok(false);
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(false);
        };
        let (across, down) = (size.width as usize, size.height as usize);

        // The sheet for a window of this size, and how far the picture has moved.
        let mut fresh = false;
        let mut moved = 0;
        let factor = window.scale_factor();
        match self.args.picture {
            Picture::Text => {
                // A font pixel is two points wide unless asked otherwise: text of an everyday size.
                let scale = match self.args.stroke {
                    Some(pixels) => pixels as usize,
                    None => (2.0 * factor).round().max(1.0) as usize,
                };
                if !self
                    .sheet
                    .as_ref()
                    .is_some_and(|sheet| sheet.fits(across, down, scale))
                {
                    self.sheet = Some(Sheet::new(across, down, scale));
                    fresh = true;
                }
            }
            Picture::Hard => {
                if !self
                    .sheet
                    .as_ref()
                    .is_some_and(|sheet| sheet.fits_photo(across, down))
                {
                    self.sheet = Some(Sheet::photo(across, down));
                    fresh = true;
                }
            }
            Picture::Grey => {}
        }
        if self.args.picture != Picture::Grey {
            let speed = (f64::from(self.args.scroll) * factor).round() as u32;
            moved = picture::moved(self.clock.now_ns(), speed);
        }
        let stamp = self.args.stamp && self.args.picture != Picture::Grey;

        if let Some(gpu) = &mut self.gpu {
            if let (true, Some(sheet)) = (fresh, &self.sheet) {
                gpu.upload(sheet);
            }
            let rows = self.sheet.as_ref().map_or(1, Sheet::height);
            gpu.draw(
                size.width,
                size.height,
                self.args.picture,
                rows,
                moved,
                self.shade,
                stamp,
            )?;
            return Ok(true);
        }
        let Some(surface) = &mut self.surface else {
            return Ok(false);
        };
        let failed = |e: softbuffer::SoftBufferError| format!("cannot draw a frame: {e}");
        surface.resize(width, height).map_err(failed)?;
        let mut buffer = surface.buffer_mut().map_err(failed)?;
        match (&self.sheet, self.args.picture) {
            (Some(sheet), Picture::Text) => {
                let offset = (moved % sheet.height() as u64) as usize;
                picture::paint_text(&mut buffer, across, down, sheet, offset, self.shade);
            }
            (Some(sheet), Picture::Hard) => {
                picture::paint_hard(&mut buffer, across, down, sheet, moved, self.shade);
            }
            _ => buffer.fill(picture::pixel(self.shade.rgb())),
        }
        if stamp {
            picture::paint_stamp(&mut buffer, across, down, moved);
        }
        buffer.present().map_err(failed)?;
        Ok(true)
    }

    /// Whether an input gets a frame of its own. Where the text moves it
    /// waits for the next frame instead, unless asked otherwise: then no more
    /// frames are drawn than `--fps` says, as with an application that draws
    /// once per refresh of its display.
    fn answers_at_once(&self) -> bool {
        self.frame_period.is_none() || self.args.answer == Some(Answer::AtOnce)
    }

    /// Draws the next frame of the moving text, and with it the answer to
    /// every input that has been waiting for it.
    fn next_frame(&mut self) -> Result<(), String> {
        let waiting = std::mem::take(&mut self.waiting);
        for _ in &waiting {
            self.shade = self.shade.toggled();
        }
        if !self.draw()? {
            // Nothing could be shown: the inputs are dropped, as in `flash`.
            for _ in &waiting {
                self.shade = self.shade.toggled();
            }
            return Ok(());
        }
        let commit_ns = self.clock.now_ns();
        for input in waiting {
            self.record(
                input.source,
                input.id,
                input.input_ns,
                commit_ns,
                input.answer_to,
            )?;
        }
        Ok(())
    }

    /// Reacts to one input: switches the shade, commits the frame, and
    /// records both moments.
    ///
    /// `false` when no frame could be committed. The input is then dropped
    /// without a trace: recording a commit that did not happen would put a
    /// response time into the log that nobody could have seen.
    fn flash(
        &mut self,
        source: InputSource,
        id: u64,
        input_ns: u64,
        answer_to: Option<SocketAddr>,
    ) -> Result<bool, String> {
        if !self.answers_at_once() {
            self.waiting.push(Waiting {
                source,
                id,
                input_ns,
                answer_to,
            });
            return Ok(true);
        }
        self.shade = self.shade.toggled();
        if !self.draw()? {
            self.shade = self.shade.toggled();
            return Ok(false);
        }
        let commit_ns = self.clock.now_ns();
        self.record(source, id, input_ns, commit_ns, answer_to)?;
        Ok(true)
    }

    /// Answers a synthetic input and writes the input into the log. Called
    /// after the frame is out, so that recording does not delay what is
    /// being measured.
    fn record(
        &mut self,
        source: InputSource,
        id: u64,
        input_ns: u64,
        commit_ns: u64,
        answer_to: Option<SocketAddr>,
    ) -> Result<(), String> {
        if let (Some(socket), Some(address)) = (&self.socket, answer_to) {
            let answer = FlashAck {
                id,
                input_ns,
                commit_ns,
            };
            // A lost answer only costs the client one sample.
            let _ = socket.send_to(&answer.encode(), address);
        }
        if let Some(log) = &mut self.log {
            log.write(&Record::Host(HostSample {
                id,
                source,
                input_ns,
                commit_ns,
                sent_ns: None,
            }))
            .map_err(|e| format!("cannot write the log: {e}"))?;
        }
        Ok(())
    }

    fn flash_by_hand(&mut self, source: InputSource, input_ns: u64) -> Result<(), String> {
        if self.flash(source, self.next_id, input_ns, None)? {
            self.next_id += 1;
        }
        Ok(())
    }

    /// Stops the application when something it relies on has failed: a
    /// measurement with missing frames or missing lines would mislead.
    fn check(&mut self, event_loop: &ActiveEventLoop, result: Result<(), String>) {
        if let Err(message) = result {
            self.failure.get_or_insert(message);
            event_loop.exit();
        }
    }
}

impl ApplicationHandler<Outside> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            let opened = self.open_window(event_loop);
            self.check(event_loop, opened);
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, _cause: StartCause) {
        // The text moves on its own, at a steady rate, and inputs are answered
        // with its next frame (or at once, with a frame of their own: `flash`).
        let Some(period) = self.frame_period else {
            return;
        };
        let now = Instant::now();
        match self.next_frame {
            Some(due) if now >= due => {
                let answers = !self.waiting.is_empty();
                let drawn = self.next_frame();
                self.check(event_loop, drawn);
                let shift = match &mut self.shifts {
                    Some(shifts) if answers => shifts.next().unwrap_or_default(),
                    _ => Duration::ZERO,
                };
                self.next_frame = Some(next_due(due, period, now, shift));
            }
            Some(_) => {}
            None => self.next_frame = Some(now + period),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(due) = self.next_frame {
            event_loop.set_control_flow(ControlFlow::WaitUntil(due));
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Outside) {
        let Outside::Input { id, input_ns, from } = event;
        if self.last_synthetic == Some(id) {
            return;
        }
        self.last_synthetic = Some(id);
        let flashed = self
            .flash(InputSource::Network, id, input_ns, Some(from))
            .map(|_| ());
        self.check(event_loop, flashed);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Taken first, so that it is as close as possible to the moment the
        // window system handed the input over.
        let arrived_ns = self.clock.now_ns();
        let result = match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                Ok(())
            }
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => self.draw().map(|_| ()),
            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        repeat: false,
                        logical_key,
                        ..
                    },
                is_synthetic: false,
                ..
            } => {
                if logical_key == Key::Named(NamedKey::Escape) {
                    event_loop.exit();
                    Ok(())
                } else {
                    self.flash_by_hand(InputSource::Key, arrived_ns)
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                ..
            } => self.flash_by_hand(InputSource::Pointer, arrived_ns),
            _ => Ok(()),
        };
        self.check(event_loop, result);
    }
}

/// Reads synthetic inputs and passes them to the event loop, stamped on arrival.
fn listen(socket: UdpSocket, clock: Clock, event_loop: EventLoopProxy<Outside>) {
    std::thread::spawn(move || {
        let mut buffer = [0u8; 64];
        loop {
            let (length, from) = match socket.recv_from(&mut buffer) {
                Ok(received) => received,
                Err(_) => {
                    // Transient on some systems (for example after an answer
                    // could not be delivered). Do not spin if it persists.
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
            };
            let input_ns = clock.now_ns();
            let Some(input) = FlashInput::decode(&buffer[..length]) else {
                continue;
            };
            let event = Outside::Input {
                id: input.id,
                input_ns,
                from,
            };
            if event_loop.send_event(event).is_err() {
                // The window is gone.
                return;
            }
        }
    });
}

/// When the frame after the one that was due at `due` is due: one period
/// later, keeping the rhythm; after a stall, one period from now. `shift`
/// puts it off further, which moves the whole beat from here on.
fn next_due(due: Instant, period: Duration, now: Instant, shift: Duration) -> Instant {
    let next = due + period;
    (if next > now { next } else { now + period }) + shift
}

/// The shifts of `--shift-beat`: anywhere within one frame interval, evenly.
fn beat_shifts(seed: u64, period: Duration) -> Jitter {
    Jitter::new(
        seed,
        Duration::ZERO,
        period.saturating_sub(Duration::from_nanos(1)),
    )
}

fn open_log(
    path: &PathBuf,
    clock: &Clock,
    picture: Picture,
    draw: Draw,
    beat_shift_seed: Option<u64>,
) -> Result<LogWriter<BufWriter<File>>, String> {
    // Never overwrite an earlier measurement.
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let mut log = LogWriter::new(BufWriter::new(file));
    let started_unix_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    log.write(&Record::Header(Header {
        side: Side::Host,
        tool: env!("CARGO_PKG_NAME").to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        started_unix_ms,
        clock_origin_unix_ns: Some(clock.origin_unix_ns()),
        picture: Some(picture.name().to_string()),
        draw: Some(draw.name().to_string()),
        beat_shift_seed,
    }))
    .map_err(|e| format!("cannot write to {}: {e}", path.display()))?;
    Ok(log)
}

fn run(args: Args) -> Result<(), String> {
    let clock = Clock::new();
    let log = args
        .log
        .as_ref()
        .map(|path| open_log(path, &clock, args.picture, args.draw, args.shift_beat))
        .transpose()?;

    let event_loop = EventLoop::<Outside>::with_user_event()
        .build()
        .map_err(|e| format!("cannot reach a window system: {e}"))?;
    // Sleep until something happens; an input wakes the loop at once.
    event_loop.set_control_flow(ControlFlow::Wait);

    let socket = match args.listen {
        Some(address) => {
            let socket =
                UdpSocket::bind(address).map_err(|e| format!("cannot listen on {address}: {e}"))?;
            let reader = socket
                .try_clone()
                .map_err(|e| format!("cannot listen on {address}: {e}"))?;
            listen(reader, clock, event_loop.create_proxy());
            Some(socket)
        }
        None => None,
    };

    let mut app = App {
        args,
        clock,
        shade: Shade::Dark,
        next_id: 0,
        last_synthetic: None,
        log,
        socket,
        waiting: Vec::new(),
        gpu: None,
        window: None,
        surface: None,
        sheet: None,
        frame_period: None,
        next_frame: None,
        shifts: None,
        failure: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("the window system stopped: {e}"))?;
    match app.failure {
        Some(message) => Err(message),
        None => Ok(()),
    }
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_width_by_height() {
        assert_eq!(parse_size("960x600"), Ok((960, 600)));
        assert_eq!(parse_size("3440X1440"), Ok((3440, 1440)));
        for wrong in [
            "",
            "960",
            "960x",
            "x600",
            "0x600",
            "960x0",
            "a×b",
            "960x600x2",
        ] {
            assert!(parse_size(wrong).is_err(), "{wrong:?} was accepted");
        }
    }

    #[test]
    fn positions_are_x_then_y_and_may_be_negative() {
        assert_eq!(parse_position("100,80"), Ok((100, 80)));
        // A display to the left of or above the main one has negative coordinates.
        assert_eq!(parse_position("-3440, 12"), Ok((-3440, 12)));
        for wrong in ["", "100", "100,", ",80", "a,b", "1,2,3", "100x80"] {
            assert!(parse_position(wrong).is_err(), "{wrong:?} was accepted");
        }
    }

    #[test]
    fn the_moving_text_is_what_the_window_shows_unless_asked_otherwise() {
        let args = Args::try_parse_from(["istesso-flash"]).unwrap();
        assert_eq!(args.picture, Picture::Text);
        assert_eq!((args.scroll, args.fps), (240, None));
        assert_eq!(args.stroke, None);
        assert_eq!((args.draw, args.answer), (Draw::Cpu, None));
        let args = Args::try_parse_from(["istesso-flash", "--draw", "gpu", "--answer", "at-once"])
            .unwrap();
        assert_eq!((args.draw, args.answer), (Draw::Gpu, Some(Answer::AtOnce)));
        let args = Args::try_parse_from(["istesso-flash", "--answer", "next-frame"]).unwrap();
        assert_eq!(args.answer, Some(Answer::NextFrame));
        let args = Args::try_parse_from(["istesso-flash", "--picture", "grey"]).unwrap();
        assert_eq!(args.picture, Picture::Grey);
        let args = Args::try_parse_from(["istesso-flash", "--scroll", "0", "--fps", "85"]).unwrap();
        assert_eq!((args.scroll, args.fps), (0, Some(85)));
        let args = Args::try_parse_from(["istesso-flash", "--stroke", "1"]).unwrap();
        assert_eq!((args.stroke, args.stamp), (Some(1), false));
        let args = Args::try_parse_from(["istesso-flash", "--picture", "hard", "--stamp"]).unwrap();
        assert_eq!((args.picture, args.stamp), (Picture::Hard, true));
        assert!(Args::try_parse_from(["istesso-flash", "--stroke", "0"]).is_err());
        for wrong in [
            vec!["istesso-flash", "--picture", "video"],
            vec!["istesso-flash", "--fps", "0"],
            vec!["istesso-flash", "--scroll", "-3"],
        ] {
            assert!(Args::try_parse_from(wrong.clone()).is_err(), "{wrong:?}");
        }
    }

    #[test]
    fn the_command_line_is_consistent() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }

    #[test]
    fn the_beat_keeps_its_rhythm_and_a_shift_moves_it_from_there_on() {
        let period = Duration::from_millis(10);
        let due = Instant::now();
        // In time: one period after the frame that was due.
        let now = due + Duration::from_millis(1);
        assert_eq!(next_due(due, period, now, Duration::ZERO), due + period);
        // After a stall: one period from now, not a burst to catch up.
        let late = due + Duration::from_millis(25);
        assert_eq!(next_due(due, period, late, Duration::ZERO), late + period);
        // A shift puts the next frame off, and with it every frame after it.
        let shift = Duration::from_millis(4);
        let shifted = next_due(due, period, now, shift);
        assert_eq!(shifted, due + period + shift);
        assert_eq!(
            next_due(shifted, period, shifted, Duration::ZERO),
            due + 2 * period + shift
        );
    }

    #[test]
    fn shifted_beats_fall_evenly_on_every_part_of_a_frame_interval() {
        let period = Duration::from_secs(1) / 85;
        let mut shifts = beat_shifts(1, period);
        // Where the beat stands within a frame interval after each shift, in eighths.
        let (mut phase, mut eighths) = (0u128, [0usize; 8]);
        for _ in 0..8000 {
            let shift = shifts.next().unwrap();
            assert!(shift < period);
            phase = (phase + shift.as_nanos()) % period.as_nanos();
            eighths[(phase * 8 / period.as_nanos()) as usize] += 1;
        }
        // 1000 each on average; a tenth off would be more than three times what chance gives.
        assert!(
            eighths.iter().all(|count| (900..=1100).contains(count)),
            "{eighths:?}"
        );
        // Another seed, another sequence; the same seed, the same.
        let first: Vec<Duration> = beat_shifts(1, period).take(4).collect();
        assert_eq!(first, beat_shifts(1, period).take(4).collect::<Vec<_>>());
        assert_ne!(first, beat_shifts(2, period).take(4).collect::<Vec<_>>());
    }

    #[test]
    fn the_beat_is_not_shifted_where_the_window_system_sets_it() {
        assert!(Args::try_parse_from(["istesso-flash", "--shift-beat", "1"]).is_ok());
        assert!(Args::try_parse_from([
            "istesso-flash",
            "--draw",
            "gpu",
            "--when-told",
            "--shift-beat",
            "1"
        ])
        .is_err());
    }
}
