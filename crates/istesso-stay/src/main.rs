//! A test application for trying a host: it does, on purpose, what the
//! awkward applications do.
//!
//! - It keeps its window when it is asked to close, as an application does
//!   that asks its user whether to save first (`--keep-when-asked`).
//! - Its process stays when its last window has gone, as an application
//!   does that goes on in the background (`--stay-without-window`).
//! - It has no window for a while between a first window and its main
//!   one, as an application does whose splash closes before its window
//!   opens (`--splash`, `--gap`).
//! - It names the shape of its pointer (`--cursor`).
//!
//! It says what it does on standard output, one line each time, so that a
//! test can hold the host's lines against it. It is not the measuring
//! tool: that is `istesso-flash`, which stays as it was measured.

use std::num::NonZeroU32;
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use clap::Parser;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{StartCause, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, Window, WindowId};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "istesso-stay",
    version,
    about = "A window that can stay when asked to close, and a process that can stay without a window"
)]
struct Args {
    /// Keep the window the first TIMES times it is asked to close, as an
    /// application does that asks its user something first. Asked once
    /// more, it closes
    #[arg(long, value_name = "TIMES", default_value_t = 0)]
    keep_when_asked: u32,

    /// Open the first window only this long after the process started, as
    /// an application does that is slow to start
    #[arg(long, value_name = "MS")]
    open_after: Option<u64>,

    /// Close the window without being asked, this long after it opened
    #[arg(long, value_name = "MS")]
    close_after: Option<u64>,

    /// When the window has gone, go on without one for this long before
    /// ending, as an application does that stays in the background
    #[arg(long, value_name = "SECONDS", default_value_t = 0)]
    stay_without_window: u64,

    /// Show a first window for this long, then none for `--gap`, then the
    /// window, as an application does whose splash closes before its
    /// window opens
    #[arg(long, value_name = "MS")]
    splash: Option<u64>,

    /// How long there is no window after the first one. Only with `--splash`
    #[arg(long, value_name = "MS", default_value_t = 1000, requires = "splash")]
    gap: u64,

    /// The shape the pointer has over the window, by its name (`text`,
    /// `pointer`, `crosshair`, ...). The window system's own unless given
    #[arg(long, value_name = "NAME", value_parser = parse_cursor)]
    cursor: Option<CursorIcon>,

    /// Size of the window's content, in points
    #[arg(long, value_name = "WIDTHxHEIGHT", default_value = "640x400", value_parser = parse_size)]
    size: (u32, u32),

    /// Title of the window
    #[arg(long, default_value = "Istesso Stay")]
    title: String,
}

fn parse_cursor(text: &str) -> Result<CursorIcon, String> {
    text.parse()
        .map_err(|_| format!("`{text}` is not the name of a cursor's shape"))
}

fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let (width, height) = text
        .split_once('x')
        .ok_or("a size is written WIDTHxHEIGHT")?;
    match (width.parse::<u32>(), height.parse::<u32>()) {
        (Ok(width), Ok(height)) if width > 0 && height > 0 => Ok((width, height)),
        _ => Err("a size is two numbers above 0, written WIDTHxHEIGHT".into()),
    }
}

/// Where the application is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// No window yet: the application is slow to start.
    Before,
    /// The first window is shown.
    Splash,
    /// No window, and one is to come.
    Gap,
    /// The window is shown.
    Main,
    /// No window, and none is to come.
    Staying,
}

/// What happens next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Next {
    /// Go on as `Phase`, for so long if it ends by itself.
    Go(Phase, Option<Duration>),
    /// End the process.
    End,
}

/// The application's life, without its window: what a test can hold to.
#[derive(Debug)]
struct Life {
    args: Args,
    phase: Phase,
    /// How often the window has been asked to close.
    asked: u32,
}

impl Life {
    /// How it begins.
    fn begin(args: Args) -> (Self, Next) {
        let mut life = Life {
            args,
            phase: Phase::Before,
            asked: 0,
        };
        let next = match life.args.open_after {
            Some(ms) => Next::Go(Phase::Before, Some(Duration::from_millis(ms))),
            None => life.first_window(),
        };
        if let Next::Go(phase, _) = next {
            life.phase = phase;
        }
        (life, next)
    }

    /// The first window there is: a splash, or the window itself.
    fn first_window(&self) -> Next {
        match self.args.splash {
            Some(ms) => Next::Go(Phase::Splash, Some(Duration::from_millis(ms))),
            None => Next::Go(
                Phase::Main,
                self.args.close_after.map(Duration::from_millis),
            ),
        }
    }

    /// The time the present phase was given is over.
    fn time_is_up(&mut self) -> Next {
        let next = match self.phase {
            Phase::Before => self.first_window(),
            Phase::Splash => Next::Go(Phase::Gap, Some(Duration::from_millis(self.args.gap))),
            Phase::Gap => Next::Go(
                Phase::Main,
                self.args.close_after.map(Duration::from_millis),
            ),
            Phase::Main => self.window_gone(),
            Phase::Staying => Next::End,
        };
        if let Next::Go(phase, _) = next {
            self.phase = phase;
        }
        next
    }

    /// What follows the window: the background, or the end.
    fn window_gone(&self) -> Next {
        match self.args.stay_without_window {
            0 => Next::End,
            seconds => Next::Go(Phase::Staying, Some(Duration::from_secs(seconds))),
        }
    }

    /// The window was asked to close. `None`: it stays, as it is.
    fn asked_to_close(&mut self) -> Option<Next> {
        self.asked += 1;
        if self.phase == Phase::Main && self.asked <= self.args.keep_when_asked {
            return None;
        }
        let next = match self.phase {
            // A first window that is closed takes what was to follow with it.
            Phase::Splash => Next::End,
            _ => self.window_gone(),
        };
        if let Next::Go(phase, _) = next {
            self.phase = phase;
        }
        Some(next)
    }
}

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

struct App {
    life: Life,
    /// The first step, until the window system lets a window be made.
    first: Option<Next>,
    /// When the present phase ends by itself.
    until: Option<Instant>,
    /// What draws into the window. Before the window, so that it is dropped first.
    surface: Option<Surface>,
    window: Option<Rc<Window>>,
    failure: Option<String>,
}

/// One colour for a window: grey for the first, blue for the window,
/// orange once it has kept itself open.
fn colour(phase: Phase, kept: bool) -> u32 {
    match (phase, kept) {
        (Phase::Splash, _) => 0x0060_6060,
        (_, false) => 0x002a_4d69,
        (_, true) => 0x00b0_5a1e,
    }
}

impl App {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let args = &self.life.args;
        let (width, height) = args.size;
        let attributes = Window::default_attributes()
            .with_title(args.title.clone())
            // Inside a host's compositor the device draws the window's frame.
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(f64::from(width), f64::from(height)));
        let window = Rc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| format!("cannot open a window: {e}"))?,
        );
        if let Some(cursor) = args.cursor {
            window.set_cursor(cursor);
        }
        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| format!("cannot draw on this display: {e}"))?;
        self.surface = Some(
            softbuffer::Surface::new(&context, window.clone())
                .map_err(|e| format!("cannot draw into the window: {e}"))?,
        );
        window.request_redraw();
        self.window = Some(window);
        Ok(())
    }

    fn close(&mut self) {
        self.surface = None;
        self.window = None;
    }

    fn draw(&mut self) -> Result<(), String> {
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        let failed = |e: softbuffer::SoftBufferError| format!("cannot draw a frame: {e}");
        surface.resize(width, height).map_err(failed)?;
        let mut buffer = surface.buffer_mut().map_err(failed)?;
        buffer.fill(colour(self.life.phase, self.life.asked > 0));
        buffer.present().map_err(failed)
    }

    /// Does what comes next, and says so.
    fn go(&mut self, event_loop: &ActiveEventLoop, next: Next) -> Result<(), String> {
        match next {
            Next::End => {
                self.close();
                println!("ending");
                event_loop.exit();
            }
            Next::Go(phase, time) => {
                self.until = time.map(|time| Instant::now() + time);
                let ms = time.map_or(0, |time| time.as_millis());
                match phase {
                    Phase::Before => println!("no window yet, for {ms} ms"),
                    Phase::Splash => {
                        self.open(event_loop)?;
                        println!("first window open, for {ms} ms");
                    }
                    Phase::Main => {
                        self.open(event_loop)?;
                        match time {
                            Some(_) => println!("window open, for {ms} ms"),
                            None => println!("window open"),
                        }
                    }
                    Phase::Gap => {
                        self.close();
                        println!("no window, for {ms} ms; one is to come");
                    }
                    Phase::Staying => {
                        self.close();
                        println!("no window; the process stays for {ms} ms");
                    }
                }
            }
        }
        Ok(())
    }

    fn check(&mut self, event_loop: &ActiveEventLoop, result: Result<(), String>) {
        if let Err(message) = result {
            self.failure = Some(message);
            event_loop.exit();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(first) = self.first.take() {
            let went = self.go(event_loop, first);
            self.check(event_loop, went);
        }
    }

    fn new_events(&mut self, event_loop: &ActiveEventLoop, _cause: StartCause) {
        if self.until.is_some_and(|until| Instant::now() >= until) {
            self.until = None;
            let next = self.life.time_is_up();
            let went = self.go(event_loop, next);
            self.check(event_loop, went);
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let result = match event {
            WindowEvent::CloseRequested => match self.life.asked_to_close() {
                None => {
                    println!(
                        "asked to close: kept ({} of {})",
                        self.life.asked, self.life.args.keep_when_asked
                    );
                    // The window shows that it was asked.
                    self.draw()
                }
                Some(next) => {
                    println!("asked to close: closing");
                    self.go(event_loop, next)
                }
            },
            WindowEvent::RedrawRequested | WindowEvent::Resized(_) => self.draw(),
            _ => Ok(()),
        };
        self.check(event_loop, result);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(match self.until {
            Some(until) => ControlFlow::WaitUntil(until),
            None => ControlFlow::Wait,
        });
    }
}

fn run(args: Args) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("no window system: {e}"))?;
    let (life, first) = Life::begin(args);
    let mut app = App {
        life,
        first: Some(first),
        until: None,
        surface: None,
        window: None,
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

    fn life(arguments: &[&str]) -> (Life, Next) {
        let mut all = vec!["istesso-stay"];
        all.extend_from_slice(arguments);
        Life::begin(Args::try_parse_from(all).unwrap())
    }

    #[test]
    fn as_it_comes_it_is_one_window_that_closes_when_asked_and_ends() {
        let (mut life, first) = life(&[]);
        assert_eq!(first, Next::Go(Phase::Main, None));
        assert_eq!(life.asked_to_close(), Some(Next::End));
    }

    #[test]
    fn a_window_that_keeps_itself_stays_so_many_times_and_then_closes() {
        let (mut life, _) = life(&["--keep-when-asked", "2"]);
        assert_eq!(life.asked_to_close(), None);
        assert_eq!(life.asked_to_close(), None);
        assert_eq!(life.phase, Phase::Main);
        assert_eq!(life.asked_to_close(), Some(Next::End));
    }

    #[test]
    fn a_process_that_stays_goes_on_without_its_window_and_ends_by_itself() {
        let (mut life, _) = life(&["--stay-without-window", "30"]);
        assert_eq!(
            life.asked_to_close(),
            Some(Next::Go(Phase::Staying, Some(Duration::from_secs(30))))
        );
        assert_eq!(life.phase, Phase::Staying);
        assert_eq!(life.time_is_up(), Next::End);
    }

    #[test]
    fn a_window_that_closes_by_itself_does_so_after_its_time() {
        let (mut life, first) = life(&["--close-after", "1500", "--stay-without-window", "9"]);
        assert_eq!(
            first,
            Next::Go(Phase::Main, Some(Duration::from_millis(1500)))
        );
        assert_eq!(
            life.time_is_up(),
            Next::Go(Phase::Staying, Some(Duration::from_secs(9)))
        );
        assert_eq!(life.time_is_up(), Next::End);
    }

    #[test]
    fn a_splash_is_followed_by_no_window_and_then_by_the_window() {
        let (mut life, first) = life(&["--splash", "700", "--gap", "2500"]);
        assert_eq!(
            first,
            Next::Go(Phase::Splash, Some(Duration::from_millis(700)))
        );
        assert_eq!(
            life.time_is_up(),
            Next::Go(Phase::Gap, Some(Duration::from_millis(2500)))
        );
        assert_eq!(life.time_is_up(), Next::Go(Phase::Main, None));
        assert_eq!(life.asked_to_close(), Some(Next::End));
        // A slow start comes before either.
        let (mut life, first) = self::life(&["--open-after", "8000", "--splash", "700"]);
        assert_eq!(
            first,
            Next::Go(Phase::Before, Some(Duration::from_millis(8000)))
        );
        assert_eq!(
            life.time_is_up(),
            Next::Go(Phase::Splash, Some(Duration::from_millis(700)))
        );
        // A splash that is closed takes the rest with it.
        let (mut life, _) = self::life(&["--splash", "700"]);
        assert_eq!(life.asked_to_close(), Some(Next::End));
    }

    #[test]
    fn what_cannot_be_meant_is_refused() {
        for wrong in [
            vec!["istesso-stay", "--gap", "100"],
            vec!["istesso-stay", "--cursor", "no-such-shape"],
            vec!["istesso-stay", "--size", "640"],
            vec!["istesso-stay", "--size", "0x400"],
        ] {
            assert!(Args::try_parse_from(wrong.clone()).is_err(), "{wrong:?}");
        }
        let args = Args::try_parse_from(["istesso-stay", "--cursor", "text"]).unwrap();
        assert_eq!(args.cursor, Some(CursorIcon::Text));
        assert_eq!(colour(Phase::Main, false), 0x002a_4d69);
    }
}
