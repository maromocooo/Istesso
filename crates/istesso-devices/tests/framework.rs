//! The framework with a device that is not there and an adapter made up for
//! the test. No real device is needed.

use istesso_device_api::{
    Adapter, AdapterEntry, AdapterError, Command, Confirmation, ConfirmedBy, Context, Declaration,
    Encoding, Event, PictureFormat, PictureTarget, ReportKind, ReportRule, Rotation, Shape, UsbId,
};
use istesso_devices::picture::Rgba;
use istesso_devices::record::{self, event_from_text, event_text, replay};
use istesso_devices::session::{Error, Session};
use istesso_devices::text::{unescape, INFO_VALUE_LIMIT};
use istesso_devices::transport::FakeTransport;
use istesso_devices::transport::Transport;

static DECLARATION: Declaration = Declaration {
    name: "Test Pad",
    devices: &[UsbId {
        vendor: 0xffff,
        product: 0x0001,
    }],
    reports: &[
        ReportRule {
            kind: ReportKind::Input,
            id: 0x01,
            command: Some(0x00),
            purpose: "button states",
        },
        ReportRule {
            kind: ReportKind::Output,
            id: 0x02,
            command: Some(0x07),
            purpose: "picture for a button",
        },
        ReportRule {
            kind: ReportKind::FeatureSet,
            id: 0x03,
            command: Some(0x08),
            purpose: "brightness",
        },
        ReportRule {
            kind: ReportKind::FeatureGet,
            id: 0x05,
            command: None,
            purpose: "version",
        },
    ],
    other_software: &[],
    used_by_apps: &[],
    confirmation: Confirmation {
        by: ConfirmedBy::NotYet,
        systems: &[],
        date: "",
    },
};

const PICTURE: PictureFormat = PictureFormat {
    width: 16,
    height: 16,
    encoding: Encoding::Jpeg,
    rotation: Rotation::None,
    flip_horizontal: false,
    flip_vertical: false,
};

/// Two buttons with pictures. `greedy` makes it try things it never declared.
struct Pad {
    greedy: bool,
    down: [bool; 2],
}

impl Adapter for Pad {
    fn connect(&mut self, cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let mut answer = [0u8; 8];
        let n = cx.hid.get_feature(0x05, &mut answer)?;
        if n < 2 {
            return Err(AdapterError::Malformed("version too short"));
        }
        (cx.emit)(Event::Info {
            name: "version".into(),
            value: answer[1].to_string(),
        });
        (cx.emit)(Event::Shape(Shape {
            buttons: 2,
            button_columns: 2,
            button_picture: Some(PICTURE),
            dials: 0,
            strip_picture: None,
            strip_touch: false,
        }));
        Ok(())
    }

    fn input(&mut self, report: &[u8], cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let states = report
            .get(2..4)
            .ok_or(AdapterError::Malformed("button report too short"))?;
        for (index, &state) in states.iter().enumerate() {
            let pressed = state != 0;
            if pressed != self.down[index] {
                self.down[index] = pressed;
                (cx.emit)(Event::Button {
                    index: index as u8,
                    pressed,
                });
            }
        }
        Ok(())
    }

    fn command(&mut self, command: Command<'_>, cx: &mut Context<'_>) -> Result<(), AdapterError> {
        match command {
            Command::Picture {
                target: PictureTarget::Button(index),
                data,
            } => {
                let mut report = vec![0x02, 0x07, index];
                report.extend_from_slice(data);
                cx.hid.write_output(&report)?;
            }
            Command::Picture {
                target: PictureTarget::Strip,
                ..
            } => {
                return Err(AdapterError::Unsupported("a strip"));
            }
            Command::Brightness(percent) => cx.hid.send_feature(&[0x03, 0x08, percent])?,
            Command::Reset => {
                if self.greedy {
                    // Same report ID as brightness, another command; and a
                    // report that was never declared at all. The results are
                    // thrown away on purpose: the guard must hold anyway.
                    let _ = cx.hid.send_feature(&[0x03, 0x0d, 0, 0, 0, 0]);
                    let mut serial = [0u8; 16];
                    let _ = cx.hid.get_feature(0x06, &mut serial);
                    let _ = cx.hid.write_output(&[0x02, 0x99, 1, 2, 3]);
                }
                cx.hid.send_feature(&[0x03, 0x08, 0])?;
            }
        }
        Ok(())
    }
}

fn polite() -> AdapterEntry {
    AdapterEntry {
        declaration: &DECLARATION,
        create: || {
            Box::new(Pad {
                greedy: false,
                down: [false; 2],
            })
        },
    }
}

fn greedy() -> AdapterEntry {
    AdapterEntry {
        declaration: &DECLARATION,
        create: || {
            Box::new(Pad {
                greedy: true,
                down: [false; 2],
            })
        },
    }
}

fn device() -> FakeTransport {
    let mut device = FakeTransport::default();
    device.answers.push_back((0x05, vec![0x05, 7]));
    device
}

#[test]
fn connecting_tells_what_the_device_looks_like() {
    let mut session = Session::new(polite(), device());
    let shape = session.connect().unwrap();
    assert_eq!(shape.buttons, 2);
    assert_eq!(
        session.next_event(),
        Some(Event::Info {
            name: "version".into(),
            value: "7".into()
        })
    );
    assert!(matches!(session.next_event(), Some(Event::Shape(_))));
    assert_eq!(session.next_event(), None);
}

#[test]
fn a_device_that_does_not_answer_fails_the_connection() {
    let mut session = Session::new(polite(), FakeTransport::default());
    assert!(matches!(
        session.connect(),
        Err(Error::Adapter(AdapterError::Io(_)))
    ));
    assert!(session.shape().is_none());
}

#[test]
fn input_reports_become_notices() {
    let mut device = device();
    device.inputs.push_back(vec![0x01, 0x00, 1, 0]);
    device.inputs.push_back(vec![0x01, 0x00, 1, 1]);
    device.inputs.push_back(vec![0x01, 0x00, 0, 1]);
    let mut session = Session::new(polite(), device);
    session.connect().unwrap();
    while session.next_event().is_some() {}

    assert!(session.poll(10).unwrap());
    assert_eq!(
        session.next_event(),
        Some(Event::Button {
            index: 0,
            pressed: true
        })
    );
    assert!(session.poll(10).unwrap());
    assert_eq!(
        session.next_event(),
        Some(Event::Button {
            index: 1,
            pressed: true
        })
    );
    assert!(session.poll(10).unwrap());
    assert_eq!(
        session.next_event(),
        Some(Event::Button {
            index: 0,
            pressed: false
        })
    );
    assert!(!session.poll(10).unwrap(), "nothing more comes");
}

#[test]
fn input_the_adapter_did_not_declare_never_reaches_it() {
    let mut session = Session::new(polite(), device());
    session.connect().unwrap();
    while session.next_event().is_some() {}
    // Another report ID, and the declared ID with another command.
    session.feed_input(&[0x09, 0x00, 1, 1]).unwrap();
    session.feed_input(&[0x01, 0x03, 1, 1]).unwrap();
    assert_eq!(session.next_event(), None);
    assert_eq!(session.ignored_inputs(), 2);
}

#[test]
fn a_picture_is_converted_before_the_adapter_sees_it() {
    let mut session = Session::new(polite(), device());
    session.connect().unwrap();
    session
        .show(
            PictureTarget::Button(1),
            &Rgba::solid(100, 40, [200, 30, 30]),
        )
        .unwrap();
    let report = &session.transport().written[0];
    assert_eq!(&report[..3], &[0x02, 0x07, 1]);
    assert_eq!(
        &report[3..5],
        &[0xff, 0xd8],
        "the adapter was handed a JPEG"
    );
}

#[test]
fn pictures_go_only_where_the_device_has_a_place() {
    let mut session = Session::new(polite(), device());
    let picture = Rgba::solid(8, 8, [1, 2, 3]);
    assert_eq!(
        session.show(PictureTarget::Button(0), &picture),
        Err(Error::NotConnected)
    );
    session.connect().unwrap();
    assert_eq!(
        session.show(PictureTarget::Button(2), &picture),
        Err(Error::NoSuchPlace)
    );
    assert_eq!(
        session.show(PictureTarget::Strip, &picture),
        Err(Error::NoSuchPlace)
    );
    assert!(session.transport().written.is_empty());
}

#[test]
fn what_an_adapter_did_not_declare_does_not_reach_the_device() {
    let mut session = Session::new(greedy(), device());
    session.start_recording();
    session.connect().unwrap();
    session.reset().unwrap();

    assert_eq!(session.refused(), 3);
    assert!(
        session.transport().written.is_empty(),
        "the undeclared output report was stopped"
    );
    assert_eq!(
        session.transport().features,
        vec![vec![0x03, 0x08, 0]],
        "only the declared report arrived"
    );

    let recording = session.recording();
    assert!(recording.contains("refused set 03 0d 00*4"), "{recording}");
    assert!(recording.contains("refused get 06"), "{recording}");
    assert!(
        recording.contains("refused out 02 99 01 02 03"),
        "{recording}"
    );
}

/// Talk to the made-up device and write the exchange down.
fn recorded(entry: AdapterEntry) -> String {
    let mut session = Session::new(entry, device());
    session.start_recording();
    session.connect().unwrap();
    session.set_elapsed_ms(120);
    session.brightness(40).unwrap();
    session
        .show(PictureTarget::Button(0), &Rgba::solid(4, 4, [0, 0, 255]))
        .unwrap();
    session.set_elapsed_ms(900);
    session.feed_input(&[0x01, 0x00, 0, 1]).unwrap();
    session.reset().unwrap();
    format!(
        "{}\nadapter test_pad\n{}",
        record::HEADER,
        session.recording()
    )
}

#[test]
fn a_recording_replays_without_the_device() {
    let text = recorded(polite());
    assert!(
        text.contains("\nconnect\nget 05 -> 05 07\nevent info version 7\n"),
        "{text}"
    );
    assert!(
        text.contains("\nt 120\nbrightness 40\nset 03 08 28\n"),
        "{text}"
    );
    assert!(
        text.contains("\nt 900\nin 01 00 00 01\nevent button 1 down\n"),
        "{text}"
    );
    replay(polite(), &text).unwrap();
    assert_eq!(record::adapter_of(&text), Some("test_pad"));
}

#[test]
fn replay_notices_when_the_adapter_writes_something_else() {
    // The recording was made with the polite adapter; the greedy one does
    // more on reset.
    let error = replay(greedy(), &recorded(polite())).unwrap_err();
    assert!(
        error.contains("differs") || error.contains("lines"),
        "{error}"
    );
}

#[test]
fn replay_notices_a_recording_that_was_changed() {
    let text = recorded(polite());
    let changed = text.replace("set 03 08 28", "set 03 08 29");
    assert_ne!(text, changed);
    let error = replay(polite(), &changed).unwrap_err();
    assert!(error.contains("recorded: set 03 08 29"), "{error}");
    assert!(error.contains("now:      set 03 08 28"), "{error}");

    let changed = text.replace("event button 1 down", "event button 0 down");
    assert!(replay(polite(), &changed).is_err());
}

#[test]
fn replay_refuses_what_is_not_a_recording() {
    assert!(replay(polite(), "").is_err());
    assert!(replay(polite(), "# only a comment\n").is_err());
    assert!(replay(polite(), "connect\nfly away\n").is_err());
    assert!(replay(polite(), "connect\nget 05 07\n").is_err());
}

// ---------------------------------------------------------------------------
// Text that a device supplies
// ---------------------------------------------------------------------------

/// Passes on, as the value of an info, whatever text the device answers
/// with, and does nothing to make it safe: that is the framework's work.
struct Chatty;

impl Adapter for Chatty {
    fn connect(&mut self, cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let mut answer = [0u8; 512];
        let n = cx.hid.get_feature(0x05, &mut answer)?;
        let text = String::from_utf8_lossy(&answer[1..n]).into_owned();
        (cx.emit)(Event::Info {
            name: "firmware".into(),
            value: text.clone(),
        });
        // A name that is not a plain word, with the same text in it.
        (cx.emit)(Event::Info {
            name: text,
            value: "x".into(),
        });
        (cx.emit)(Event::Button {
            index: 1,
            pressed: true,
        });
        Ok(())
    }

    fn input(&mut self, _: &[u8], _: &mut Context<'_>) -> Result<(), AdapterError> {
        Ok(())
    }

    fn command(&mut self, _: Command<'_>, _: &mut Context<'_>) -> Result<(), AdapterError> {
        Ok(())
    }
}

fn chatty() -> AdapterEntry {
    AdapterEntry {
        declaration: &DECLARATION,
        create: || Box::new(Chatty),
    }
}

/// What a device might call its firmware: a line break followed by what looks
/// like another notice, escape sequences that clear the screen and set the
/// window title, a NUL, text that is not ASCII, a backslash, blanks at the ends.
const HOSTILE: &str = " 1.0\nevent button 0 down\r\n\u{1b}[2J\u{1b}]0;owned\u{7}\0 バージョン \\n ";

fn device_saying(text: &str) -> FakeTransport {
    let mut device = FakeTransport::default();
    let mut answer = vec![0x05];
    answer.extend_from_slice(text.as_bytes());
    device.answers.push_back((0x05, answer));
    device
}

fn printable_ascii(text: &str) -> bool {
    text.chars().all(|c| (' '..='~').contains(&c))
}

#[test]
fn text_from_a_device_is_escaped_where_notices_are_collected() {
    let mut session = Session::new(chatty(), device_saying(HOSTILE));
    session.start_recording();
    // The adapter gives no shape; that is not what this test is about.
    let _ = session.connect();

    let Some(Event::Info { name, value }) = session.next_event() else {
        panic!("the first notice is the info");
    };
    assert_eq!(name, "firmware");
    assert!(printable_ascii(&value), "{value:?}");
    assert!(!value.starts_with(' ') && !value.ends_with(' '));
    // Nothing the device said is lost.
    assert_eq!(unescape(&value).as_deref(), Some(HOSTILE));

    // A name that is not a plain word is replaced, not passed on.
    assert_eq!(
        session.next_event(),
        Some(Event::Info {
            name: "invalid-name".into(),
            value: "x".into()
        })
    );
    assert_eq!(
        session.next_event(),
        Some(Event::Button {
            index: 1,
            pressed: true
        })
    );
    assert_eq!(session.next_event(), None);
}

#[test]
fn a_device_cannot_add_a_line_to_a_recording_or_a_control_character() {
    let mut session = Session::new(chatty(), device_saying(HOSTILE));
    session.start_recording();
    let _ = session.connect();
    let recording = session.recording();

    assert!(recording.lines().all(printable_ascii), "{recording:?}");
    assert!(
        !recording.contains('\u{1b}') && !recording.contains('\0') && !recording.contains('\r')
    );
    let notices: Vec<&str> = recording
        .lines()
        .filter_map(|line| line.strip_prefix("event "))
        .collect();
    // Two infos and the one real button notice; the "event button 0 down"
    // inside the firmware text stayed inside its line.
    assert_eq!(notices.len(), 3, "{recording}");
    assert_eq!(
        notices.iter().filter(|n| n.starts_with("button ")).count(),
        1
    );
    assert_eq!(notices[2], "button 1 down");
    assert_eq!(
        recording.lines().count(),
        5,
        "connect, get, three notices:\n{recording}"
    );

    // Every line of a notice reads back as the notice that was collected.
    let mut session = Session::new(chatty(), device_saying(HOSTILE));
    let _ = session.connect();
    let mut read_back = 0;
    while let Some(event) = session.next_event() {
        let line = event_text(&event);
        assert_eq!(event_from_text(&line).as_ref(), Ok(&event), "{line}");
        assert_eq!(notices[read_back], line);
        read_back += 1;
    }
    assert_eq!(read_back, 3);

    // And the recording replays: the same device text gives the same lines.
    replay(chatty(), &format!("{}\n{recording}", record::HEADER)).unwrap();
}

#[test]
fn overlong_text_from_a_device_is_cut() {
    let long = "A".repeat(400);
    let mut session = Session::new(chatty(), device_saying(&long));
    let _ = session.connect();
    let Some(Event::Info { value, .. }) = session.next_event() else {
        panic!("the first notice is the info");
    };
    assert_eq!(value.len(), INFO_VALUE_LIMIT + 2);
    assert!(value.ends_with("\\~"));
    assert_eq!(unescape(&value), Some("A".repeat(INFO_VALUE_LIMIT)));
}

/// A device whose errors carry text of their own.
struct Faulty;

impl Transport for Faulty {
    fn write(&mut self, _: &[u8]) -> Result<(), String> {
        Err("write failed\n\u{1b}[31mevent button 0 down".into())
    }
    fn send_feature(&mut self, _: &[u8]) -> Result<(), String> {
        Err("feature failed\u{7}".into())
    }
    fn get_feature(&mut self, _: u8, _: &mut [u8]) -> Result<usize, String> {
        Err("no answer\r\nevent button 0 down\u{1b}[2J".into())
    }
    fn read(&mut self, _: &mut [u8], _: u32) -> Result<usize, String> {
        Err("read failed\n\u{1b}]0;owned\u{7}".into())
    }
}

#[test]
fn what_the_system_says_about_a_failure_is_made_printable() {
    let mut session = Session::new(polite(), Faulty);
    session.start_recording();
    let error = session.connect().unwrap_err().to_string();
    assert!(printable_ascii(&error), "{error:?}");
    assert_eq!(error, "no answer\\r\\nevent button 0 down\\x1b[2J");

    let recording = session.recording();
    assert_eq!(recording.lines().count(), 2, "{recording}");
    assert!(recording.lines().all(printable_ascii), "{recording:?}");
    assert!(
        recording.ends_with("error no answer\\r\\nevent button 0 down\\x1b[2J\n"),
        "{recording}"
    );

    let error = session.poll(10).unwrap_err().to_string();
    assert!(printable_ascii(&error), "{error:?}");
    let error = session.brightness(10).unwrap_err().to_string();
    assert!(printable_ascii(&error), "{error:?}");
}

#[test]
fn a_message_about_a_recording_does_not_carry_its_control_characters() {
    for text in [
        "connect\nfly \u{1b}[2J away\n",
        "connect\nin 01 \u{1b}]0;owned\u{7}\n",
        "connect\nget 05 -> 05 07\nevent info version 7\nevent \u{1b}[31mred\n",
    ] {
        let error = replay(polite(), text).unwrap_err();
        assert!(printable_ascii(&error.replace('\n', " ")), "{error:?}");
        assert!(
            !error.contains('\u{1b}') && !error.contains('\u{7}'),
            "{error:?}"
        );
    }
}
