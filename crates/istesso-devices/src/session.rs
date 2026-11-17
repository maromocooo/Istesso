//! One device with its adapter.
//!
//! The session stands between the two: every report the adapter sends goes
//! through the guard, every notice it gives is collected here, and, when
//! asked, everything is written down as a recording.

use std::cell::RefCell;
use std::collections::VecDeque;

use istesso_device_api::{
    Adapter, AdapterEntry, AdapterError, Command, Context, Declaration, Event, Hid, IoError,
    PictureTarget, ReportKind, Shape,
};

use crate::guard::allows;
use crate::picture::{convert, Rgba};
use crate::record::{event_text, hex};
use crate::transport::Transport;

/// Why the session could not do what it was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Adapter(AdapterError),
    /// The device could not be read.
    Device(String),
    /// The picture could not be converted.
    Picture(String),
    /// The adapter has not said yet what the device looks like.
    NotConnected,
    /// The device has no such place for a picture.
    NoSuchPlace,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Adapter(error) => write!(f, "{error}"),
            Error::Device(text) | Error::Picture(text) => write!(f, "{text}"),
            Error::NotConnected => write!(f, "the device is not connected yet"),
            Error::NoSuchPlace => write!(f, "the device has no such place for a picture"),
        }
    }
}

impl std::error::Error for Error {}

/// What happened during one call into the adapter, in order.
enum Effect {
    Line(String),
    Event(Event),
}

/// The device as the adapter sees it: guarded, and watched.
struct Port<'a, T: Transport> {
    declaration: &'static Declaration,
    transport: &'a mut T,
    effects: &'a RefCell<Vec<Effect>>,
    refused: &'a mut u64,
    recording: bool,
}

impl<T: Transport> Port<'_, T> {
    fn note(&self, line: impl FnOnce() -> String) {
        if self.recording {
            self.effects.borrow_mut().push(Effect::Line(line()));
        }
    }

    fn refuse(&mut self, what: &str, report: &[u8]) -> IoError {
        *self.refused += 1;
        self.note(|| format!("refused {what} {}", hex(report)));
        IoError::NotDeclared
    }
}

impl<T: Transport> Hid for Port<'_, T> {
    fn write_output(&mut self, report: &[u8]) -> Result<(), IoError> {
        if !allows(self.declaration.reports, ReportKind::Output, report) {
            return Err(self.refuse("out", report));
        }
        self.note(|| format!("out {}", hex(report)));
        self.transport.write(report).map_err(IoError::Device)
    }

    fn send_feature(&mut self, report: &[u8]) -> Result<(), IoError> {
        if !allows(self.declaration.reports, ReportKind::FeatureSet, report) {
            return Err(self.refuse("set", report));
        }
        self.note(|| format!("set {}", hex(report)));
        self.transport.send_feature(report).map_err(IoError::Device)
    }

    fn get_feature(&mut self, id: u8, buffer: &mut [u8]) -> Result<usize, IoError> {
        if !allows(self.declaration.reports, ReportKind::FeatureGet, &[id]) {
            return Err(self.refuse("get", &[id]));
        }
        let n = self
            .transport
            .get_feature(id, buffer)
            .map_err(IoError::Device)?;
        let n = n.min(buffer.len());
        self.note(|| format!("get {id:02x} -> {}", hex(&buffer[..n])));
        Ok(n)
    }
}

/// One device, its adapter, and what passed between them.
pub struct Session<T: Transport> {
    declaration: &'static Declaration,
    adapter: Box<dyn Adapter>,
    transport: T,
    shape: Option<Shape>,
    events: VecDeque<Event>,
    log: Option<Vec<String>>,
    elapsed_ms: u64,
    logged_ms: u64,
    refused: u64,
    ignored_inputs: u64,
}

impl<T: Transport> Session<T> {
    pub fn new(entry: AdapterEntry, transport: T) -> Self {
        Self {
            declaration: entry.declaration,
            adapter: (entry.create)(),
            transport,
            shape: None,
            events: VecDeque::new(),
            log: None,
            elapsed_ms: 0,
            logged_ms: 0,
            refused: 0,
            ignored_inputs: 0,
        }
    }

    /// Write down everything from now on. See [`crate::record`].
    pub fn start_recording(&mut self) {
        self.log.get_or_insert_with(Vec::new);
    }

    /// The recording so far, one line per call and per effect.
    pub fn recording(&self) -> String {
        let mut text = String::new();
        for line in self.log.iter().flatten() {
            text.push_str(line);
            text.push('\n');
        }
        text
    }

    /// Tell the session how much time has passed since the device was opened.
    /// The adapter sees only this number, never a clock.
    pub fn set_elapsed_ms(&mut self, ms: u64) {
        self.elapsed_ms = ms;
    }

    /// What the device looks like, once the adapter has said so.
    pub fn shape(&self) -> Option<&Shape> {
        self.shape.as_ref()
    }

    /// How many reports of the adapter were stopped because it had not
    /// declared them.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// How many input reports of the device were dropped because the adapter
    /// had not declared them.
    pub fn ignored_inputs(&self) -> u64 {
        self.ignored_inputs
    }

    /// The next notice of the adapter, oldest first.
    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// The device itself. For tests.
    pub fn transport(&self) -> &T {
        &self.transport
    }

    /// Call into the adapter and collect what it does.
    fn call(
        &mut self,
        label: impl FnOnce() -> String,
        f: impl FnOnce(&mut dyn Adapter, &mut Context<'_>) -> Result<(), AdapterError>,
    ) -> Result<(), Error> {
        let recording = self.log.is_some();
        if let Some(log) = &mut self.log {
            if self.elapsed_ms != self.logged_ms {
                log.push(format!("t {}", self.elapsed_ms));
                self.logged_ms = self.elapsed_ms;
            }
            log.push(label());
        }

        let effects = RefCell::new(Vec::new());
        let result = {
            let mut port = Port {
                declaration: self.declaration,
                transport: &mut self.transport,
                effects: &effects,
                refused: &mut self.refused,
                recording,
            };
            let mut emit = |event: Event| effects.borrow_mut().push(Effect::Event(event));
            let mut cx = Context {
                hid: &mut port,
                emit: &mut emit,
                elapsed_ms: self.elapsed_ms,
            };
            f(self.adapter.as_mut(), &mut cx)
        };

        for effect in effects.into_inner() {
            match effect {
                Effect::Line(line) => {
                    if let Some(log) = &mut self.log {
                        log.push(line);
                    }
                }
                Effect::Event(event) => {
                    if let Some(log) = &mut self.log {
                        log.push(format!("event {}", event_text(&event)));
                    }
                    if let Event::Shape(shape) = &event {
                        self.shape = Some(*shape);
                    }
                    self.events.push_back(event);
                }
            }
        }
        if let (Err(error), Some(log)) = (&result, &mut self.log) {
            log.push(format!("error {error}"));
        }
        result.map_err(Error::Adapter)
    }

    /// Let the adapter greet the device. Afterwards [`Session::shape`] is
    /// known.
    pub fn connect(&mut self) -> Result<Shape, Error> {
        self.call(|| "connect".into(), |adapter, cx| adapter.connect(cx))?;
        self.shape.ok_or(Error::NotConnected)
    }

    /// Hand one input report of the device to the adapter. Reports the
    /// adapter did not declare are dropped and counted.
    pub fn feed_input(&mut self, report: &[u8]) -> Result<(), Error> {
        if !allows(self.declaration.reports, ReportKind::Input, report) {
            self.ignored_inputs += 1;
            return Ok(());
        }
        self.call(
            || format!("in {}", hex(report)),
            |adapter, cx| adapter.input(report, cx),
        )
    }

    /// Wait up to `timeout_ms` for the device to say something. `true` when
    /// it did.
    pub fn poll(&mut self, timeout_ms: u32) -> Result<bool, Error> {
        let mut buffer = [0u8; 4096];
        let n = self
            .transport
            .read(&mut buffer, timeout_ms)
            .map_err(Error::Device)?;
        if n == 0 {
            return Ok(false);
        }
        self.feed_input(&buffer[..n.min(buffer.len())])?;
        Ok(true)
    }

    /// Show `picture` at `target`. The picture is converted here into the
    /// form the adapter named for that place.
    pub fn show(&mut self, target: PictureTarget, picture: &Rgba) -> Result<(), Error> {
        let shape = self.shape.ok_or(Error::NotConnected)?;
        let format = match target {
            PictureTarget::Button(index) if index < shape.buttons => shape.button_picture,
            PictureTarget::Button(_) => None,
            PictureTarget::Strip => shape.strip_picture,
        }
        .ok_or(Error::NoSuchPlace)?;
        let data = convert(picture, &format).map_err(Error::Picture)?;
        self.show_encoded(target, &data)
    }

    /// Show a picture that is already in the form the device wants.
    pub fn show_encoded(&mut self, target: PictureTarget, data: &[u8]) -> Result<(), Error> {
        self.call(
            || match target {
                PictureTarget::Button(index) => format!("picture button {index} {}", hex(data)),
                PictureTarget::Strip => format!("picture strip {}", hex(data)),
            },
            |adapter, cx| adapter.command(Command::Picture { target, data }, cx),
        )
    }

    /// Set the brightness, 0 to 100.
    pub fn brightness(&mut self, percent: u8) -> Result<(), Error> {
        let percent = percent.min(100);
        self.call(
            || format!("brightness {percent}"),
            |adapter, cx| adapter.command(Command::Brightness(percent), cx),
        )
    }

    /// Clear the pictures and put the device back as it was when plugged in.
    pub fn reset(&mut self) -> Result<(), Error> {
        self.call(
            || "reset".into(),
            |adapter, cx| adapter.command(Command::Reset, cx),
        )
    }
}
