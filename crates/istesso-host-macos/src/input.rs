//! Input for the application that is shown: from what the other device
//! sent to what the Mac is told.
//!
//! Two halves. [`Translator`] is plain arithmetic and is tested: it turns
//! events in the picture's pixels and Linux key numbers into the Mac's
//! events, keeps track of which modifiers and buttons are held, and counts
//! double clicks. [`post`] (macOS only) hands such an event to the system.
//! Posting needs the Accessibility permission (ADR 0017) and **has not been
//! run**: nothing here asks for the permission, and without it nothing is
//! posted.
//!
//! [`Event`] has the shape of `istesso_proto::InputEvent` on the branch of
//! ADR 0007; once that is in `main`, this type goes and that one is used.

use std::time::{Duration, Instant};

use crate::keys;

/// What a person did with the keyboard or the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A key went down or came up (Linux key number).
    Key { code: u32, pressed: bool },
    /// The pointer is here, in the picture's own pixels from its top left.
    PointerAt { x: i32, y: i32 },
    /// A button went down or came up (0x110 is the left one).
    Button { code: u32, pressed: bool },
    /// How far the content should move, in pixels, to the right and down.
    Scroll { dx: i32, dy: i32 },
}

/// Where the application's window lies on the Mac's desktop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    /// Top left corner of the window's content, in the desktop's points.
    pub x: f64,
    pub y: f64,
    /// Size of the content in points.
    pub width: f64,
    pub height: f64,
    /// Pixels of the picture to one point (2 on a Retina display).
    pub scale: f64,
}

impl Place {
    /// The desktop point for a pixel of the picture. Positions outside the
    /// picture are pulled to its edge: input never lands in another window.
    pub fn point(&self, x: i32, y: i32) -> (f64, f64) {
        let px = (f64::from(x) / self.scale).clamp(0.0, (self.width - 1.0).max(0.0));
        let py = (f64::from(y) / self.scale).clamp(0.0, (self.height - 1.0).max(0.0));
        (self.x + px, self.y + py)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Moved,
    Dragged(Button),
    Down(Button),
    Up(Button),
}

/// One event for the Mac.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Synthetic {
    Mouse {
        kind: MouseKind,
        x: f64,
        y: f64,
        clicks: u8,
        flags: u64,
    },
    Key {
        key: u16,
        pressed: bool,
        flags: u64,
    },
    Scroll {
        x: f64,
        y: f64,
        right: i32,
        down: i32,
        flags: u64,
    },
}

/// Two presses of a button count as a double click when they are this
/// close in time and place.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_DISTANCE: f64 = 4.0;

/// Remembers what is held and where the pointer is.
#[derive(Debug)]
pub struct Translator {
    place: Place,
    flags: u64,
    held: Vec<u32>,
    buttons: Vec<Button>,
    pointer: (f64, f64),
    last_press: Option<(Button, Instant, (f64, f64), u8)>,
}

impl Translator {
    pub fn new(place: Place) -> Self {
        Translator {
            place,
            flags: 0,
            held: Vec::new(),
            buttons: Vec::new(),
            pointer: (place.x, place.y),
            last_press: None,
        }
    }

    /// The window was moved or resized.
    pub fn set_place(&mut self, place: Place) {
        self.place = place;
    }

    /// Turns one event into what the Mac is told. Nothing for a key or a
    /// button the Mac does not have, for a key that repeats "down" while it
    /// is held as a modifier, or for a release of something not held.
    pub fn translate(&mut self, event: Event, now: Instant) -> Option<Synthetic> {
        match event {
            Event::Key { code, pressed } => {
                let key = keys::virtual_key(code)?;
                if pressed {
                    if !self.held.contains(&code) {
                        self.held.push(code);
                    }
                } else {
                    let at = self.held.iter().position(|held| *held == code)?;
                    self.held.remove(at);
                }
                if keys::modifier_flag(code).is_some() {
                    // The bit stays while either of the two keys of a kind is held.
                    self.flags = self
                        .held
                        .iter()
                        .filter_map(|held| keys::modifier_flag(*held))
                        .fold(0, |a, b| a | b);
                }
                Some(Synthetic::Key {
                    key,
                    pressed,
                    flags: self.flags,
                })
            }
            Event::PointerAt { x, y } => {
                self.pointer = self.place.point(x, y);
                let kind = match self.buttons.first() {
                    Some(button) => MouseKind::Dragged(*button),
                    None => MouseKind::Moved,
                };
                Some(self.mouse(kind, 0))
            }
            Event::Button { code, pressed } => {
                let button = match code {
                    keys::BUTTON_LEFT => Button::Left,
                    keys::BUTTON_RIGHT => Button::Right,
                    keys::BUTTON_MIDDLE => Button::Middle,
                    _ => return None,
                };
                if pressed {
                    if self.buttons.contains(&button) {
                        return None;
                    }
                    self.buttons.push(button);
                    let clicks = match self.last_press {
                        Some((last, at, place, clicks))
                            if last == button
                                && now.saturating_duration_since(at) <= DOUBLE_CLICK
                                && (place.0 - self.pointer.0).abs() <= DOUBLE_CLICK_DISTANCE
                                && (place.1 - self.pointer.1).abs() <= DOUBLE_CLICK_DISTANCE =>
                        {
                            clicks.saturating_add(1).min(3)
                        }
                        _ => 1,
                    };
                    self.last_press = Some((button, now, self.pointer, clicks));
                    Some(self.mouse(MouseKind::Down(button), clicks))
                } else {
                    let at = self.buttons.iter().position(|held| *held == button)?;
                    self.buttons.remove(at);
                    let clicks = self.last_press.map_or(1, |(_, _, _, clicks)| clicks);
                    Some(self.mouse(MouseKind::Up(button), clicks))
                }
            }
            Event::Scroll { dx, dy } => Some(Synthetic::Scroll {
                x: self.pointer.0,
                y: self.pointer.1,
                right: dx,
                down: dy,
                flags: self.flags,
            }),
        }
    }

    /// What has to be told the Mac when the other device goes away while it
    /// holds keys or buttons: every one of them released, so that nothing
    /// stays down for the next thing typed on the Mac.
    pub fn release_all(&mut self) -> Vec<Synthetic> {
        let mut events = Vec::new();
        for button in std::mem::take(&mut self.buttons) {
            events.push(self.mouse(MouseKind::Up(button), 1));
        }
        // Modifiers last, so that the other keys come up as they went down.
        let mut held = std::mem::take(&mut self.held);
        held.sort_by_key(|code| keys::modifier_flag(*code).is_some());
        for (index, code) in held.iter().enumerate() {
            if let Some(key) = keys::virtual_key(*code) {
                let flags = held[index + 1..]
                    .iter()
                    .filter_map(|c| keys::modifier_flag(*c))
                    .fold(0, |a, b| a | b);
                events.push(Synthetic::Key {
                    key,
                    pressed: false,
                    flags,
                });
            }
        }
        self.flags = 0;
        events
    }

    fn mouse(&self, kind: MouseKind, clicks: u8) -> Synthetic {
        Synthetic::Mouse {
            kind,
            x: self.pointer.0,
            y: self.pointer.1,
            clicks,
            flags: self.flags,
        }
    }
}

/// Why an event was not posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostError {
    /// The person has not allowed this program to control the Mac
    /// (System Settings > Privacy & Security > Accessibility).
    NotAllowed,
    /// The system would not make the event.
    NotMade,
}

impl std::fmt::Display for PostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PostError::NotAllowed => write!(
                f,
                "this program may not send input to applications; allow it under \
                 System Settings > Privacy & Security > Accessibility"
            ),
            PostError::NotMade => write!(f, "the system would not make the input event"),
        }
    }
}

impl std::error::Error for PostError {}

#[cfg(target_os = "macos")]
pub use mac::{may_post, post};

#[cfg(target_os = "macos")]
mod mac {
    use std::ffi::c_void;

    use super::{Button, MouseKind, PostError, Synthetic};

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    type CGEventRef = *mut c_void;

    // Event types and fields of Core Graphics (CGEventTypes.h).
    const MOUSE_MOVED: u32 = 5;
    const LEFT_DOWN: u32 = 1;
    const LEFT_UP: u32 = 2;
    const RIGHT_DOWN: u32 = 3;
    const RIGHT_UP: u32 = 4;
    const LEFT_DRAGGED: u32 = 6;
    const RIGHT_DRAGGED: u32 = 7;
    const OTHER_DOWN: u32 = 25;
    const OTHER_UP: u32 = 26;
    const OTHER_DRAGGED: u32 = 27;
    const FIELD_CLICK_STATE: u32 = 1;
    const UNIT_PIXEL: u32 = 0;
    /// Events enter where the session's own input enters.
    const TAP_SESSION: u32 = 1;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightPostEventAccess() -> bool;
        fn CGEventCreateMouseEvent(
            source: *const c_void,
            kind: u32,
            at: CGPoint,
            button: u32,
        ) -> CGEventRef;
        fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> CGEventRef;
        fn CGEventCreateScrollWheelEvent2(
            source: *const c_void,
            units: u32,
            wheels: u32,
            wheel1: i32,
            wheel2: i32,
            wheel3: i32,
        ) -> CGEventRef;
        fn CGEventSetFlags(event: CGEventRef, flags: u64);
        fn CGEventSetLocation(event: CGEventRef, at: CGPoint);
        fn CGEventSetIntegerValueField(event: CGEventRef, field: u32, value: i64);
        fn CGEventPost(tap: u32, event: CGEventRef);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    /// Whether the person has allowed this program to send input. Asks the
    /// system without showing anything.
    pub fn may_post() -> bool {
        // SAFETY: takes nothing and only reads the permission.
        unsafe { CGPreflightPostEventAccess() }
    }

    /// Hands one event to the system. Refuses, without asking for it, when
    /// the permission is missing.
    pub fn post(event: &Synthetic) -> Result<(), PostError> {
        if !may_post() {
            return Err(PostError::NotAllowed);
        }
        // SAFETY: every event is created here, used while it is live, and released once.
        unsafe {
            let made = match *event {
                Synthetic::Mouse {
                    kind,
                    x,
                    y,
                    clicks,
                    flags,
                } => {
                    let (kind, button) = match kind {
                        MouseKind::Moved => (MOUSE_MOVED, 0),
                        MouseKind::Dragged(Button::Left) => (LEFT_DRAGGED, 0),
                        MouseKind::Dragged(Button::Right) => (RIGHT_DRAGGED, 1),
                        MouseKind::Dragged(Button::Middle) => (OTHER_DRAGGED, 2),
                        MouseKind::Down(Button::Left) => (LEFT_DOWN, 0),
                        MouseKind::Down(Button::Right) => (RIGHT_DOWN, 1),
                        MouseKind::Down(Button::Middle) => (OTHER_DOWN, 2),
                        MouseKind::Up(Button::Left) => (LEFT_UP, 0),
                        MouseKind::Up(Button::Right) => (RIGHT_UP, 1),
                        MouseKind::Up(Button::Middle) => (OTHER_UP, 2),
                    };
                    let made =
                        CGEventCreateMouseEvent(std::ptr::null(), kind, CGPoint { x, y }, button);
                    if !made.is_null() {
                        CGEventSetIntegerValueField(made, FIELD_CLICK_STATE, i64::from(clicks));
                        CGEventSetFlags(made, flags);
                    }
                    made
                }
                Synthetic::Key {
                    key,
                    pressed,
                    flags,
                } => {
                    let made = CGEventCreateKeyboardEvent(std::ptr::null(), key, pressed);
                    if !made.is_null() {
                        CGEventSetFlags(made, flags);
                    }
                    made
                }
                Synthetic::Scroll {
                    x,
                    y,
                    right,
                    down,
                    flags,
                } => {
                    // The first wheel moves content down for positive values, the second to
                    // the right. Taken from the documentation; not tried.
                    let made = CGEventCreateScrollWheelEvent2(
                        std::ptr::null(),
                        UNIT_PIXEL,
                        2,
                        down,
                        right,
                        0,
                    );
                    if !made.is_null() {
                        CGEventSetLocation(made, CGPoint { x, y });
                        CGEventSetFlags(made, flags);
                    }
                    made
                }
            };
            if made.is_null() {
                return Err(PostError::NotMade);
            }
            CGEventPost(TAP_SESSION, made);
            CFRelease(made.cast_const());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keys::{FLAG_COMMAND, FLAG_SHIFT};

    fn place() -> Place {
        Place {
            x: 100.0,
            y: 50.0,
            width: 800.0,
            height: 600.0,
            scale: 2.0,
        }
    }

    #[test]
    fn pixels_of_the_picture_become_points_of_the_desktop() {
        assert_eq!(place().point(0, 0), (100.0, 50.0));
        assert_eq!(place().point(400, 300), (300.0, 200.0));
    }

    #[test]
    fn positions_outside_the_picture_stay_inside_the_window() {
        assert_eq!(place().point(-50, -50), (100.0, 50.0));
        assert_eq!(place().point(100_000, 100_000), (899.0, 649.0));
    }

    #[test]
    fn a_shortcut_carries_its_modifier() {
        let mut translator = Translator::new(place());
        let now = Instant::now();
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 125,
                    pressed: true
                },
                now
            ),
            Some(Synthetic::Key {
                key: 0x37,
                pressed: true,
                flags: FLAG_COMMAND
            })
        );
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 30,
                    pressed: true
                },
                now
            ),
            Some(Synthetic::Key {
                key: 0x00,
                pressed: true,
                flags: FLAG_COMMAND
            })
        );
        translator.translate(
            Event::Key {
                code: 30,
                pressed: false,
            },
            now,
        );
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 125,
                    pressed: false
                },
                now
            ),
            Some(Synthetic::Key {
                key: 0x37,
                pressed: false,
                flags: 0
            })
        );
    }

    #[test]
    fn a_modifier_stays_while_its_twin_is_held() {
        let mut translator = Translator::new(place());
        let now = Instant::now();
        translator.translate(
            Event::Key {
                code: 42,
                pressed: true,
            },
            now,
        );
        translator.translate(
            Event::Key {
                code: 54,
                pressed: true,
            },
            now,
        );
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 42,
                    pressed: false
                },
                now
            ),
            Some(Synthetic::Key {
                key: 0x38,
                pressed: false,
                flags: FLAG_SHIFT
            })
        );
    }

    #[test]
    fn a_release_of_something_not_held_is_dropped() {
        let mut translator = Translator::new(place());
        let now = Instant::now();
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 30,
                    pressed: false
                },
                now
            ),
            None
        );
        assert_eq!(
            translator.translate(
                Event::Button {
                    code: 0x110,
                    pressed: false
                },
                now
            ),
            None
        );
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 240,
                    pressed: true
                },
                now
            ),
            None
        );
        assert_eq!(
            translator.translate(
                Event::Button {
                    code: 0x119,
                    pressed: true
                },
                now
            ),
            None
        );
    }

    #[test]
    fn moving_with_a_button_held_is_a_drag() {
        let mut translator = Translator::new(place());
        let now = Instant::now();
        translator.translate(Event::PointerAt { x: 20, y: 20 }, now);
        translator.translate(
            Event::Button {
                code: 0x110,
                pressed: true,
            },
            now,
        );
        assert_eq!(
            translator.translate(Event::PointerAt { x: 40, y: 20 }, now),
            Some(Synthetic::Mouse {
                kind: MouseKind::Dragged(Button::Left),
                x: 120.0,
                y: 60.0,
                clicks: 0,
                flags: 0
            })
        );
        translator.translate(
            Event::Button {
                code: 0x110,
                pressed: false,
            },
            now,
        );
        assert!(matches!(
            translator.translate(Event::PointerAt { x: 41, y: 20 }, now),
            Some(Synthetic::Mouse {
                kind: MouseKind::Moved,
                ..
            })
        ));
    }

    #[test]
    fn two_quick_presses_in_one_place_are_a_double_click() {
        let mut translator = Translator::new(place());
        let start = Instant::now();
        let clicks = |translator: &mut Translator, at: Instant| {
            let down = translator.translate(
                Event::Button {
                    code: 0x110,
                    pressed: true,
                },
                at,
            );
            translator.translate(
                Event::Button {
                    code: 0x110,
                    pressed: false,
                },
                at,
            );
            match down {
                Some(Synthetic::Mouse { clicks, .. }) => clicks,
                other => panic!("{other:?}"),
            }
        };
        assert_eq!(clicks(&mut translator, start), 1);
        assert_eq!(
            clicks(&mut translator, start + Duration::from_millis(200)),
            2
        );
        assert_eq!(
            clicks(&mut translator, start + Duration::from_millis(2000)),
            1,
            "too late"
        );
        translator.translate(Event::PointerAt { x: 500, y: 500 }, start);
        assert_eq!(
            clicks(&mut translator, start + Duration::from_millis(2100)),
            1,
            "too far"
        );
    }

    #[test]
    fn everything_held_is_released_when_the_other_device_goes() {
        let mut translator = Translator::new(place());
        let now = Instant::now();
        translator.translate(
            Event::Key {
                code: 125,
                pressed: true,
            },
            now,
        );
        translator.translate(
            Event::Key {
                code: 30,
                pressed: true,
            },
            now,
        );
        translator.translate(
            Event::Button {
                code: 0x110,
                pressed: true,
            },
            now,
        );
        let released = translator.release_all();
        assert_eq!(released.len(), 3);
        assert!(matches!(
            released[0],
            Synthetic::Mouse {
                kind: MouseKind::Up(Button::Left),
                ..
            }
        ));
        assert_eq!(
            released[1],
            Synthetic::Key {
                key: 0x00,
                pressed: false,
                flags: FLAG_COMMAND
            }
        );
        assert_eq!(
            released[2],
            Synthetic::Key {
                key: 0x37,
                pressed: false,
                flags: 0
            }
        );
        assert!(translator.release_all().is_empty());
        assert_eq!(
            translator.translate(
                Event::Key {
                    code: 31,
                    pressed: true
                },
                now
            ),
            Some(Synthetic::Key {
                key: 0x01,
                pressed: true,
                flags: 0
            })
        );
    }

    #[test]
    fn scrolling_happens_where_the_pointer_is() {
        let mut translator = Translator::new(place());
        let now = Instant::now();
        translator.translate(Event::PointerAt { x: 200, y: 100 }, now);
        assert_eq!(
            translator.translate(Event::Scroll { dx: 0, dy: -30 }, now),
            Some(Synthetic::Scroll {
                x: 200.0,
                y: 100.0,
                right: 0,
                down: -30,
                flags: 0
            })
        );
    }
}
