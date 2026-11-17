//! The lines the host's agent and this program say to each other when the
//! agent starts it as the runner of one session (ADR 0010, "ホスト：ランナー
//! との約束"; ADR 0019 for what is added).
//!
//! One JSON object a line, on standard input and standard output. Lines and
//! fields that are not known are passed over, on both sides: that is what
//! lets this program say `paused`, `resumed` and `windows`, which an agent
//! that does not know them ignores.
//!
//! `offer`, `answer`, a device's address and its public key are carried and
//! never written to a log.

use std::collections::BTreeMap;
use std::fmt;

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
    #[serde(default)]
    pub fps: Option<u32>,
}

/// The device a session is shown on.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct Peer {
    pub id: String,
    /// The device's public key, as the list of paired devices has it. A
    /// device is known by its id and its key together: without the key
    /// nobody is shown the picture.
    #[serde(default)]
    pub public_key: Option<String>,
    /// Where the device's control channel came from. Not used: the device
    /// connects to this program for its picture.
    #[serde(default)]
    pub address: Option<String>,
}

impl fmt::Debug for Peer {
    /// The id only: the key and the address stay out of every log.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Peer").field("id", &self.id).finish()
    }
}

/// What the agent tells the runner.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Begin: make the place the application is shown in, and show it to this device.
    Start {
        session: String,
        #[serde(default)]
        kind: String,
        display: Display,
        #[serde(default)]
        peer: Option<Peer>,
        /// Where the agent hands the device's input over. Input is taken from nowhere else.
        input_socket: String,
        #[serde(default)]
        offer: Value,
    },
    /// The application has been started. A compositor's applications come
    /// to it by themselves, so this is passed over.
    Application {
        #[serde(default)]
        pid: u32,
    },
    /// The device has gone, or was sent away: within a tenth of a second
    /// nothing goes to it and nothing is taken from it. The application stays.
    Detach,
    /// Show the picture to this device, and to no other, from now on.
    Attach {
        #[serde(default)]
        peer: Option<Peer>,
        #[serde(default)]
        display: Option<Display>,
        #[serde(default)]
        offer: Value,
    },
    /// Ask the application's windows to close. What it does is up to it.
    Close,
    /// Stop making the picture and give the encoder back; the device stays.
    Pause,
    /// Make the picture again, beginning with a frame that leans on none before.
    Resume,
}

/// What the runner tells the agent.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// An application can come now, and the input socket is there.
    Ready {
        env: BTreeMap<String, String>,
        display: Display,
        answer: Value,
    },
    /// The device named in `attach` is the one that is shown the picture.
    Attached { display: Display, answer: Value },
    /// Nobody is sent anything at the moment; the application goes on.
    Detached { message: String },
    /// No encoder could be had: at the start, on the way back, or while the
    /// picture ran. No picture is made now; the application and the device
    /// stay, and `resume` tries again.
    NoEncoder { message: String },
    /// The last window has closed.
    Empty,
    /// Cannot go on; the runner ends after this, and the application with it.
    Error { message: String },
    /// Added (ADR 0019): the encoder has been closed, so long after `pause`
    /// or `detach` was read.
    Paused { took_ms: f64 },
    /// Added: the first frame after `resume` is out. From reading the word
    /// to the encoder being there, to that frame leaving it, to that frame
    /// having been handed on.
    Resumed {
        opened_ms: f64,
        encoded_ms: f64,
        written_ms: f64,
    },
    /// Added: how many windows the application has, whenever that changes.
    Windows { count: u32 },
    /// Added: the answer to `close`, which is also how an application is
    /// ended (a device's "quit this application"): its windows were asked
    /// to close, this many of them. `asked` false: this host cannot ask.
    /// Nothing is ever closed by force.
    CloseAsked { asked: bool, windows: u32 },
    /// Added: so long after its windows were asked to close, the application
    /// still has this many: it is asking something, or has decided to stay.
    /// Said once; the session goes on.
    StillOpen { windows: u32, after_s: u32 },
}

/// The answer a device is given for where its picture comes from: the port
/// this program listens on for it (ADR 0015). The agent carries it unread.
pub fn answer(port: u16) -> Value {
    serde_json::json!({"transport": "channel", "channel_port": port})
}

/// Reads one line from the agent. `None` for a line that is not known or
/// is of another version: it is passed over.
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
            r#"{"v":1,"cmd":"start","session":"5b0c9e3a7f21d684","kind":"app_window","display":{"width":1720,"height":720,"fps":85},"peer":{"id":"1a2b-3c4d-5e6f-7a8b","public_key":"KEY","address":"192.0.2.10"},"input_socket":"/run/user/1000/istesso/input-5b0c9e3a7f21d684.sock","offer":{"rtp_port":50004}}"#, // check-private: ok (the documentation address of the ADR's example)
        );
        let Some(Command::Start {
            session,
            display,
            peer: Some(peer),
            input_socket,
            ..
        }) = start
        else {
            panic!("{start:?}");
        };
        assert_eq!(session, "5b0c9e3a7f21d684");
        let size = Display {
            width: 1720,
            height: 720,
            fps: Some(85),
        };
        assert_eq!(display, size);
        assert_eq!(peer.id, "1a2b-3c4d-5e6f-7a8b");
        assert_eq!(peer.public_key.as_deref(), Some("KEY"));
        assert!(input_socket.ends_with("input-5b0c9e3a7f21d684.sock"));
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
                r#"{"v":1,"cmd":"attach","peer":{"id":"x","public_key":"k"},"display":{"width":1280,"height":720,"fps":60},"offer":{}}"#
            ),
            Some(Command::Attach {
                peer: Some(_),
                display: Some(_),
                ..
            })
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
    }

    #[test]
    fn a_device_without_its_key_is_read_and_its_key_is_missing() {
        let Some(Command::Attach {
            peer: Some(peer), ..
        }) = read(r#"{"v":1,"cmd":"attach","peer":{"id":"x"}}"#)
        else {
            panic!();
        };
        assert_eq!(peer.public_key, None);
    }

    #[test]
    fn a_device_is_written_to_a_log_by_its_id_alone() {
        let peer = Peer {
            id: "1a2b".into(),
            public_key: Some("SECRET-KEY".into()),
            address: Some("somewhere".into()),
        };
        let shown = format!("{peer:?}");
        assert!(shown.contains("1a2b"));
        assert!(!shown.contains("SECRET-KEY") && !shown.contains("somewhere"));
    }

    #[test]
    fn events_are_written_as_the_adr_shows_them() {
        let display = Display {
            width: 1720,
            height: 720,
            fps: Some(85),
        };
        let ready = write(&Event::Ready {
            env: BTreeMap::from([("WAYLAND_DISPLAY".to_string(), "wayland-3".to_string())]),
            display,
            answer: answer(47850),
        });
        assert!(!ready.contains('\n'));
        let value: Value = serde_json::from_str(&ready).unwrap();
        assert_eq!(value["v"], 1);
        assert_eq!(value["event"], "ready");
        assert_eq!(value["env"]["WAYLAND_DISPLAY"], "wayland-3");
        assert_eq!(value["display"]["fps"], 85);
        assert_eq!(value["answer"]["transport"], "channel");
        assert_eq!(value["answer"]["channel_port"], 47850);
        let empty: Value = serde_json::from_str(&write(&Event::Empty)).unwrap();
        assert_eq!(empty, serde_json::json!({"v": 1, "event": "empty"}));
        let refused: Value = serde_json::from_str(&write(&Event::NoEncoder {
            message: "busy".into(),
        }))
        .unwrap();
        assert_eq!(
            refused,
            serde_json::json!({"v": 1, "event": "no_encoder", "message": "busy"})
        );
    }

    #[test]
    fn the_added_lines_are_events_like_the_others() {
        let paused: Value = serde_json::from_str(&write(&Event::Paused { took_ms: 41.5 })).unwrap();
        assert_eq!(
            paused,
            serde_json::json!({"v": 1, "event": "paused", "took_ms": 41.5})
        );
        let windows: Value = serde_json::from_str(&write(&Event::Windows { count: 2 })).unwrap();
        assert_eq!(
            windows,
            serde_json::json!({"v": 1, "event": "windows", "count": 2})
        );
        let asked: Value = serde_json::from_str(&write(&Event::CloseAsked {
            asked: true,
            windows: 2,
        }))
        .unwrap();
        assert_eq!(
            asked,
            serde_json::json!({"v": 1, "event": "close_asked", "asked": true, "windows": 2})
        );
        let open: Value = serde_json::from_str(&write(&Event::StillOpen {
            windows: 1,
            after_s: 5,
        }))
        .unwrap();
        assert_eq!(
            open,
            serde_json::json!({"v": 1, "event": "still_open", "windows": 1, "after_s": 5})
        );
        let resumed: Value = serde_json::from_str(&write(&Event::Resumed {
            opened_ms: 1.0,
            encoded_ms: 2.0,
            written_ms: 3.0,
        }))
        .unwrap();
        assert_eq!(resumed["event"], "resumed");
        assert_eq!(resumed["written_ms"], 3.0);
    }
}
