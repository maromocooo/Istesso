//! The lines a host's agent and the program that shows one session (the
//! "runner") say to each other, as ADR 0010 sets them out on its branch.
//!
//! The agent that speaks them is not in `main` yet, so they are written down
//! here from the ADR, for the Mac's runner to be built and tested against.
//! One JSON object a line; lines and fields that are not known are passed
//! over. `offer` and `answer` belong to the picture's transport: they are
//! carried as they come and never logged.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The version of the lines.
pub const VERSION: u32 = 1;

/// Size and pace of the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Display {
    pub width: u32,
    pub height: u32,
    /// Most frames in any one second; `None` leaves it to the runner.
    pub fps: Option<u32>,
}

/// The device a session is shown on.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Peer {
    pub id: String,
    /// Where the device's control channel came from. Never logged.
    pub address: String,
}

/// What the agent tells the runner.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Begin: make the place the application is shown in and send to this device.
    Start {
        session: String,
        kind: String,
        display: Display,
        peer: Peer,
        input_socket: String,
        #[serde(default)]
        offer: Value,
    },
    /// The application has been started; this is its process. A Mac's runner
    /// finds the application's windows by it.
    Application { pid: u32 },
    /// The device has gone, or was sent away: stop everything to and from it
    /// within a tenth of a second. The application stays.
    Detach,
    /// Send to this device from now on.
    Attach {
        peer: Peer,
        display: Display,
        #[serde(default)]
        offer: Value,
    },
    /// Ask the application's window to close.
    Close,
    /// Stop making the picture and let the encoder go.
    Pause,
    /// Make the picture again, beginning with a frame that leans on none before.
    Resume,
}

/// What the runner tells the agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Ready {
        env: std::collections::BTreeMap<String, String>,
        display: Display,
        answer: Value,
    },
    Attached {
        display: Display,
        answer: Value,
    },
    Detached {
        message: String,
    },
    /// The encoder was refused; the application and the device stay.
    NoEncoder {
        message: String,
    },
    /// The last window has closed.
    Empty,
    /// Cannot go on; the runner ends after this.
    Error {
        message: String,
    },
}

/// Reads one line from the agent. `None` for a line that is not known (to be
/// passed over, as the ADR says) or that is of another version.
pub fn read(line: &str) -> Option<Command> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("v")?.as_u64()? != u64::from(VERSION) {
        return None;
    }
    serde_json::from_value(value).ok()
}

/// Writes one line for the agent, without the line's end.
pub fn write(event: &Event) -> String {
    let mut value = serde_json::to_value(event).expect("an event is plain data");
    value["v"] = VERSION.into();
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_of_the_adr_are_read() {
        let start = read(
            r#"{"v":1,"cmd":"start","session":"5b0c9e3a7f21d684","kind":"app_window","display":{"width":1720,"height":720,"fps":85},"peer":{"id":"1a2b-3c4d-5e6f-7a8b","address":"192.0.2.10"},"input_socket":"/run/user/1000/istesso/input-5b0c9e3a7f21d684.sock","offer":{"rtp_port":50004}}"#,
        );
        match start {
            Some(Command::Start {
                session,
                display,
                peer,
                offer,
                ..
            }) => {
                assert_eq!(session, "5b0c9e3a7f21d684");
                assert_eq!(
                    display,
                    Display {
                        width: 1720,
                        height: 720,
                        fps: Some(85)
                    }
                );
                assert_eq!(peer.id, "1a2b-3c4d-5e6f-7a8b");
                assert_eq!(offer["rtp_port"], 50004);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            read(r#"{"v":1,"cmd":"application","pid":4711}"#),
            Some(Command::Application { pid: 4711 })
        );
        assert_eq!(read(r#"{"v":1,"cmd":"detach"}"#), Some(Command::Detach));
        assert_eq!(read(r#"{"v":1,"cmd":"close"}"#), Some(Command::Close));
        assert_eq!(read(r#"{"v":1,"cmd":"pause"}"#), Some(Command::Pause));
        assert_eq!(read(r#"{"v":1,"cmd":"resume"}"#), Some(Command::Resume));
        assert!(matches!(
            read(
                r#"{"v":1,"cmd":"attach","peer":{"id":"x","address":"192.0.2.10"},"display":{"width":1280,"height":720,"fps":60},"offer":{}}"#
            ),
            Some(Command::Attach { .. })
        ));
    }

    #[test]
    fn lines_and_fields_that_are_not_known_are_passed_over() {
        assert_eq!(read(r#"{"v":1,"cmd":"dance"}"#), None);
        assert_eq!(read(r#"{"v":2,"cmd":"detach"}"#), None);
        assert_eq!(read("not json"), None);
        assert_eq!(read(r#"{"cmd":"detach"}"#), None);
        assert_eq!(
            read(r#"{"v":1,"cmd":"detach","later":true}"#),
            Some(Command::Detach)
        );
        assert_eq!(
            read(r#"{"v":1,"cmd":"application","pid":1,"name":"x"}"#),
            Some(Command::Application { pid: 1 })
        );
    }

    #[test]
    fn events_are_written_as_the_adr_shows_them() {
        let ready = write(&Event::Ready {
            env: std::collections::BTreeMap::new(),
            display: Display {
                width: 1720,
                height: 720,
                fps: Some(85),
            },
            answer: serde_json::json!({}),
        });
        let value: Value = serde_json::from_str(&ready).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["event"], "ready");
        assert_eq!(value["display"]["fps"], 85);
        assert!(!ready.contains('\n'));
        let empty: Value = serde_json::from_str(&write(&Event::Empty)).unwrap();
        assert_eq!(empty, serde_json::json!({"v": 1, "event": "empty"}));
        let refused: Value = serde_json::from_str(&write(&Event::NoEncoder {
            message: "busy".into(),
        }))
        .unwrap();
        assert_eq!(refused["event"], "no_encoder");
    }
}
