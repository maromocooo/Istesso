//! Test application for latency measurements.
//!
//! A window filled with one of two shades of grey that switches to the other
//! on every input: any key, any mouse button, or a synthetic input over UDP
//! (`istesso_measure::flash`). It does nothing else, so the time from an
//! input to the changed picture is as short and as even as an application
//! can make it. What a relay adds on top of that is what M1 measures.
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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use clap::Parser;
use istesso_measure::{
    Clock, FlashAck, FlashInput, Header, HostSample, InputSource, LogWriter, Record, Rgb, Shade,
    Side,
};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

#[derive(Parser)]
#[command(
    name = "istesso-flash",
    version,
    about = "A window that switches shade on every input, for latency measurements",
    after_help = "Any key or mouse button switches the shade. Escape quits."
)]
struct Args {
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
    window: Option<Rc<Window>>,
    surface: Option<Surface>,
    /// Why the application has to stop, if it does.
    failure: Option<String>,
}

impl App {
    fn open_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let (width, height) = self.args.size;
        let mut attributes = Window::default_attributes()
            .with_title(self.args.title.clone())
            .with_inner_size(LogicalSize::new(f64::from(width), f64::from(height)))
            .with_decorations(!self.args.borderless);
        if let Some((x, y)) = self.args.position {
            attributes = attributes.with_position(LogicalPosition::new(f64::from(x), f64::from(y)));
        }
        if self.args.fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        let window = Rc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| format!("cannot open a window: {e}"))?,
        );
        if let Some((x, y)) = self.args.position {
            // Asked for again once the window exists: at creation macOS moves a window that
            // was placed on another display than the one in use back onto that one.
            window.set_outer_position(LogicalPosition::new(f64::from(x), f64::from(y)));
        }
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| format!("cannot draw on this display: {e}"))?;
        let surface = softbuffer::Surface::new(&context, window.clone())
            .map_err(|e| format!("cannot draw into the window: {e}"))?;
        self.window = Some(window);
        self.surface = Some(surface);
        Ok(())
    }

    /// Fills the window with the current shade and commits the frame.
    ///
    /// `false` when there is nothing to draw into: the window is not open
    /// yet, or it is minimised.
    fn draw(&mut self) -> Result<bool, String> {
        let pixel = pixel(self.shade.rgb());
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(false);
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(false);
        };
        let failed = |e: softbuffer::SoftBufferError| format!("cannot draw a frame: {e}");
        surface.resize(width, height).map_err(failed)?;
        let mut buffer = surface.buffer_mut().map_err(failed)?;
        buffer.fill(pixel);
        buffer.present().map_err(failed)?;
        Ok(true)
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
        self.shade = self.shade.toggled();
        if !self.draw()? {
            self.shade = self.shade.toggled();
            return Ok(false);
        }
        let commit_ns = self.clock.now_ns();

        // Everything below happens after the frame is out, so that recording
        // does not delay what is being measured.
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
        Ok(true)
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

/// softbuffer's pixel layout: 0x00RRGGBB.
fn pixel(colour: Rgb) -> u32 {
    u32::from(colour.r) << 16 | u32::from(colour.g) << 8 | u32::from(colour.b)
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

fn open_log(path: &PathBuf, clock: &Clock) -> Result<LogWriter<BufWriter<File>>, String> {
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
    }))
    .map_err(|e| format!("cannot write to {}: {e}", path.display()))?;
    Ok(log)
}

fn run(args: Args) -> Result<(), String> {
    let clock = Clock::new();
    let log = args
        .log
        .as_ref()
        .map(|path| open_log(path, &clock))
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
        window: None,
        surface: None,
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
    fn pixels_are_packed_as_softbuffer_expects() {
        assert_eq!(pixel(Rgb::new(0x12, 0x34, 0x56)), 0x0012_3456);
        assert_eq!(pixel(Shade::Dark.rgb()), 0x0020_2020);
        assert_eq!(pixel(Shade::Light.rgb()), 0x00E0_E0E0);
    }

    #[test]
    fn the_command_line_is_consistent() {
        use clap::CommandFactory;
        Args::command().debug_assert();
    }
}
