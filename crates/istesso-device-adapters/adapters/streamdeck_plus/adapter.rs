// The Elgato Stream Deck + (8 buttons with displays, 4 dials, a touch strip).
//
// Written from Elgato's published HID specification
// (https://docs.elgato.com/streamdeck/hid/), pages "Main API" and
// "Stream Deck +". Every report used here is named in the declaration below;
// the framework lets nothing else through.
//
// The serial number (feature report 0x06) is not declared on purpose: this
// adapter cannot read it.

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec;

use istesso_device_api::{
    Adapter, AdapterEntry, AdapterError, Command, Confirmation, ConfirmedBy, Context, Declaration,
    Encoding, Event, PictureFormat, PictureTarget, ReportKind, ReportRule, Rotation, Shape,
    Software, TouchKind, UsbId,
};

const BUTTONS: usize = 8;
const BUTTON_COLUMNS: u8 = 4;
const DIALS: usize = 4;

/// Input reports: `01 <command> <payload length, 2 bytes> <payload>`.
const INPUT: u8 = 0x01;
const INPUT_BUTTONS: u8 = 0x00;
const INPUT_TOUCH: u8 = 0x02;
const INPUT_DIALS: u8 = 0x03;

/// Output reports carry pictures, in pieces of at most one report.
const OUTPUT: u8 = 0x02;
const OUTPUT_BUTTON_PICTURE: u8 = 0x07;
const OUTPUT_STRIP_PICTURE: u8 = 0x0b;
const OUTPUT_LENGTH: usize = 1024;
/// `02 <command> <button or 0> <last piece?> <size, 2 bytes> <index, 2 bytes>`.
const PICTURE_HEADER: usize = 8;

/// Feature reports that set something: `03 <command> <value…>`.
const SET: u8 = 0x03;
const SET_SHOW_LOGO: u8 = 0x02;
const SET_BRIGHTNESS: u8 = 0x08;
const FEATURE_LENGTH: usize = 32;

/// Feature report that returns the version of the main firmware.
const GET_FIRMWARE: u8 = 0x05;

const fn picture(width: u16, height: u16) -> PictureFormat {
    PictureFormat {
        width,
        height,
        encoding: Encoding::Jpeg,
        rotation: Rotation::None,
        flip_horizontal: false,
        flip_vertical: false,
    }
}

static DECLARATION: Declaration = Declaration {
    name: "Stream Deck +",
    devices: &[UsbId {
        vendor: 0x0fd9,
        product: 0x0084,
    }],
    reports: &[
        ReportRule {
            kind: ReportKind::Input,
            id: INPUT,
            command: Some(INPUT_BUTTONS),
            purpose: "which buttons are down",
        },
        ReportRule {
            kind: ReportKind::Input,
            id: INPUT,
            command: Some(INPUT_TOUCH),
            purpose: "a touch on the strip",
        },
        ReportRule {
            kind: ReportKind::Input,
            id: INPUT,
            command: Some(INPUT_DIALS),
            purpose: "a dial was turned or pressed",
        },
        ReportRule {
            kind: ReportKind::Output,
            id: OUTPUT,
            command: Some(OUTPUT_BUTTON_PICTURE),
            purpose: "picture for one button",
        },
        ReportRule {
            kind: ReportKind::Output,
            id: OUTPUT,
            command: Some(OUTPUT_STRIP_PICTURE),
            purpose: "picture for the whole strip",
        },
        ReportRule {
            kind: ReportKind::FeatureSet,
            id: SET,
            command: Some(SET_SHOW_LOGO),
            purpose: "show the logo, as after plugging in",
        },
        ReportRule {
            kind: ReportKind::FeatureSet,
            id: SET,
            command: Some(SET_BRIGHTNESS),
            purpose: "brightness",
        },
        ReportRule {
            kind: ReportKind::FeatureGet,
            id: GET_FIRMWARE,
            command: None,
            purpose: "firmware version",
        },
    ],
    other_software: &[Software {
        name: "Elgato Stream Deck",
        macos_bundle_id: Some("com.elgato.StreamDeck"),
        linux_process: None,
    }],
    used_by_apps: &[],
    confirmation: Confirmation {
        by: ConfirmedBy::NotYet,
        systems: &[],
        date: "",
    },
};

pub fn entry() -> AdapterEntry {
    AdapterEntry {
        declaration: &DECLARATION,
        create: || Box::new(StreamDeckPlus::default()),
    }
}

#[derive(Default)]
struct StreamDeckPlus {
    buttons_down: [bool; BUTTONS],
    dials_down: [bool; DIALS],
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let pair = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([pair[0], pair[1]]))
}

impl StreamDeckPlus {
    fn buttons(&mut self, payload: &[u8], cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let states = payload
            .get(..BUTTONS)
            .ok_or(AdapterError::Malformed("button report too short"))?;
        for (index, &state) in states.iter().enumerate() {
            let pressed = state != 0;
            if pressed != self.buttons_down[index] {
                self.buttons_down[index] = pressed;
                (cx.emit)(Event::Button {
                    index: index as u8,
                    pressed,
                });
            }
        }
        Ok(())
    }

    fn dials(&mut self, payload: &[u8], cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let (&kind, values) = payload
            .split_first()
            .ok_or(AdapterError::Malformed("dial report too short"))?;
        let values = values
            .get(..DIALS)
            .ok_or(AdapterError::Malformed("dial report too short"))?;
        match kind {
            // Pressed or released.
            0x00 => {
                for (index, &state) in values.iter().enumerate() {
                    let pressed = state != 0;
                    if pressed != self.dials_down[index] {
                        self.dials_down[index] = pressed;
                        (cx.emit)(Event::DialPress {
                            index: index as u8,
                            pressed,
                        });
                    }
                }
            }
            // Turned: a signed number of steps per dial.
            0x01 => {
                for (index, &steps) in values.iter().enumerate() {
                    let ticks = steps as i8;
                    if ticks != 0 {
                        (cx.emit)(Event::DialTurn {
                            index: index as u8,
                            ticks,
                        });
                    }
                }
            }
            _ => return Err(AdapterError::Malformed("unknown kind of dial report")),
        }
        Ok(())
    }

    fn touch(&mut self, payload: &[u8], cx: &mut Context<'_>) -> Result<(), AdapterError> {
        const SHORT: AdapterError = AdapterError::Malformed("touch report too short");
        let kind = *payload.first().ok_or(SHORT)?;
        let x = u16_at(payload, 2).ok_or(SHORT)?;
        let y = u16_at(payload, 4).ok_or(SHORT)?;
        let kind = match kind {
            0x01 => TouchKind::Tap,
            0x02 => TouchKind::LongPress,
            0x03 => TouchKind::Flick {
                to_x: u16_at(payload, 6).ok_or(SHORT)?,
                to_y: u16_at(payload, 8).ok_or(SHORT)?,
            },
            _ => return Err(AdapterError::Malformed("unknown kind of touch")),
        };
        (cx.emit)(Event::Touch { x, y, kind });
        Ok(())
    }

    /// Send a picture in pieces. `place` is the button, or 0 for the strip.
    fn send_picture(
        command: u8,
        place: u8,
        data: &[u8],
        cx: &mut Context<'_>,
    ) -> Result<(), AdapterError> {
        if data.is_empty() {
            return Err(AdapterError::Unsupported("an empty picture"));
        }
        let room = OUTPUT_LENGTH - PICTURE_HEADER;
        let pieces = data.len().div_ceil(room);
        if pieces > usize::from(u16::MAX) {
            return Err(AdapterError::Unsupported("a picture this large"));
        }
        for (index, piece) in data.chunks(room).enumerate() {
            let last = index + 1 == pieces;
            let mut report = vec![0u8; OUTPUT_LENGTH];
            report[0] = OUTPUT;
            report[1] = command;
            report[2] = place;
            report[3] = u8::from(last);
            report[4..6].copy_from_slice(&(piece.len() as u16).to_le_bytes());
            report[6..8].copy_from_slice(&(index as u16).to_le_bytes());
            report[PICTURE_HEADER..PICTURE_HEADER + piece.len()].copy_from_slice(piece);
            cx.hid.write_output(&report)?;
        }
        Ok(())
    }

    fn set(command: u8, value: u8, cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let mut report = [0u8; FEATURE_LENGTH];
        report[0] = SET;
        report[1] = command;
        report[2] = value;
        cx.hid.send_feature(&report)?;
        Ok(())
    }
}

impl Adapter for StreamDeckPlus {
    fn connect(&mut self, cx: &mut Context<'_>) -> Result<(), AdapterError> {
        // `05 <length> <checksum, 4 bytes> <version, 8 characters>`
        let mut answer = [0u8; FEATURE_LENGTH];
        let length = cx.hid.get_feature(GET_FIRMWARE, &mut answer)?;
        let text = answer[..length]
            .get(6..14)
            .ok_or(AdapterError::Malformed("firmware version too short"))?;
        let version: String = text
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| char::from(b))
            .collect();
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_graphic()) {
            return Err(AdapterError::Malformed("firmware version is not text"));
        }
        (cx.emit)(Event::Info {
            name: "firmware",
            value: version,
        });

        *self = Self::default();
        (cx.emit)(Event::Shape(Shape {
            buttons: BUTTONS as u8,
            button_columns: BUTTON_COLUMNS,
            button_picture: Some(picture(120, 120)),
            dials: DIALS as u8,
            strip_picture: Some(picture(800, 100)),
            strip_touch: true,
        }));
        Ok(())
    }

    fn input(&mut self, report: &[u8], cx: &mut Context<'_>) -> Result<(), AdapterError> {
        let command = *report
            .get(1)
            .ok_or(AdapterError::Malformed("input report too short"))?;
        let length = usize::from(
            u16_at(report, 2).ok_or(AdapterError::Malformed("input report too short"))?,
        );
        let payload = report.get(4..).unwrap_or(&[]);
        let payload = &payload[..length.min(payload.len())];
        match command {
            INPUT_BUTTONS => self.buttons(payload, cx),
            INPUT_TOUCH => self.touch(payload, cx),
            INPUT_DIALS => self.dials(payload, cx),
            _ => Ok(()),
        }
    }

    fn command(&mut self, command: Command<'_>, cx: &mut Context<'_>) -> Result<(), AdapterError> {
        match command {
            Command::Picture {
                target: PictureTarget::Button(index),
                data,
            } => {
                if usize::from(index) >= BUTTONS {
                    return Err(AdapterError::Unsupported("no such button"));
                }
                Self::send_picture(OUTPUT_BUTTON_PICTURE, index, data, cx)
            }
            Command::Picture {
                target: PictureTarget::Strip,
                data,
            } => Self::send_picture(OUTPUT_STRIP_PICTURE, 0, data, cx),
            Command::Brightness(percent) => Self::set(SET_BRIGHTNESS, percent.min(100), cx),
            Command::Reset => Self::set(SET_SHOW_LOGO, 0, cx),
        }
    }
}
