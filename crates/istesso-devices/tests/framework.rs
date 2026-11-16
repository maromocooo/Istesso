//! The framework with a device that is not there and an adapter made up for
//! the test. No real device is needed.

use istesso_device_api::{
    Adapter, AdapterEntry, AdapterError, Command, Confirmation, ConfirmedBy, Context, Declaration,
    Encoding, Event, PictureFormat, PictureTarget, ReportKind, ReportRule, Rotation, Shape, UsbId,
};
use istesso_devices::picture::Rgba;
use istesso_devices::record::{self, replay};
use istesso_devices::session::{Error, Session};
use istesso_devices::transport::FakeTransport;

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
            name: "version",
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
            name: "version",
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
