//! Streaming sessions: one application of a host, shown as one window on
//! another device. `docs/adr/0010-session-start.md` gives the reasons.
//!
//! Three vocabularies are defined here:
//!
//! * what the control channel says about a session ([`SessionInfo`] and the
//!   types inside it);
//! * what a host's agent and the program that makes the picture, the
//!   *runner*, say to each other ([`RunnerCommand`], [`RunnerEvent`]);
//! * what the command that opens a window and the program that shows the
//!   picture, the *viewer*, say to each other ([`ViewerCommand`],
//!   [`ViewerEvent`]).
//!
//! The last two travel as one JSON object per line over the standard input
//! and output of the runner and of the viewer ([`encode_line`],
//! [`decode_line`]).

use std::collections::BTreeMap;
use std::fmt;
use std::net::SocketAddr;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::StreamKind;

// ---------------------------------------------------------------------------
// On the control channel
// ---------------------------------------------------------------------------

/// Names one session on the host that runs it. The host makes it up: 16
/// lower-case hexadecimal digits from the system's random source. It is not
/// a secret. Knowing it grants nothing, so it may be shown and logged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

impl SessionId {
    /// True when the text has the form of an id. Checked on everything that
    /// comes off the wire before it is looked up or put into a file name.
    pub fn is_well_formed(&self) -> bool {
        self.0.len() == 16
            && self
                .0
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Size and rate of a session's virtual display: the picture the client's
/// window shows, pixel for pixel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    /// Most frames a second the host sends: the refresh rate of the display
    /// the client's window is on.
    pub fps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    /// The host is setting up the virtual display and starting the
    /// application.
    Starting,
    /// The application runs and a client is shown it; `display` and
    /// `answer` are that client's.
    Running,
    /// The application runs; nobody is shown it.
    Detached,
    /// The host is stopping the application and the stream; `end` says why.
    Stopping,
    /// Over.
    Ended,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// Its owner ended it by force.
    Stopped,
    /// The application ended by itself, asked to close or not.
    AppExited,
    /// The application could not be started.
    AppFailed,
    /// The host's side of the stream could not be started, or it failed.
    StreamFailed,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEnd {
    pub reason: EndReason,
    /// One line for a person, in English. Programs go by `reason`.
    #[serde(default)]
    pub message: String,
}

/// Why a session that a device is shown gets no picture for the moment.
/// The application runs on, the client stays attached, and the host's side
/// of the stream holds no encoder meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The client said that its window cannot be seen.
    Hidden,
    /// The host compresses as many pictures at once as it can, and other
    /// windows were turned to more recently.
    NoSlot,
    /// A reason this version does not know.
    #[serde(other)]
    Unknown,
}

/// One session as its host reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    /// The application's id on the host.
    pub app: String,
    pub kind: StreamKind,
    pub state: SessionState,
    /// Goes up by one with every change of this record.
    pub revision: u64,
    /// Counts the clients that were shown this session. The one that is
    /// shown it now has this number; a client with a lower one has been
    /// replaced.
    #[serde(default)]
    pub attachment: u64,
    /// What the host produces. Set once the session has run.
    #[serde(default)]
    pub display: Option<DisplayMode>,
    /// From the host's media path, for the client that is shown the
    /// session. Set while `running`. The control plane does not look
    /// inside.
    #[serde(default)]
    pub answer: Option<serde_json::Value>,
    /// Why the session ends. Set from `stopping` on.
    #[serde(default)]
    pub end: Option<SessionEnd>,
    /// Set when the device that started the session was unpaired while it
    /// ran: the name that device had. The application was kept, and the
    /// session is open to every device that may start applications on the
    /// host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left_by: Option<String>,
    /// The name of the device that is shown the session. Set while
    /// `running`. One device is shown a session at a time; a client that
    /// finds another name here has been replaced by that device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown_on: Option<String>,
    /// Set while the session is `running` and its picture is stopped: the
    /// session is there and holds no encoder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<PauseReason>,
}

// ---------------------------------------------------------------------------
// Between an agent and a runner, and between a command and a viewer
// ---------------------------------------------------------------------------

/// Version of the two line protocols. A line of another version is not
/// understood and is skipped.
pub const CONTRACT_VERSION: u32 = 1;

/// The device at the other end of a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPeer {
    /// Its id among the paired devices (the fingerprint of its key).
    pub id: String,
    /// The other end of the control connection that belongs to this
    /// session, as text a program can use as it is ([`address_text`]).
    pub address: String,
    /// Its public key, as the list of paired devices has it. An agent
    /// says it to a runner, so that the picture can be kept to this one
    /// device: the id alone is a fingerprint, and whoever checks compares
    /// both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

/// What a host's agent tells a runner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum RunnerCommand {
    /// Set up a virtual display and the stream to `peer`, the first client.
    Start {
        session: SessionId,
        kind: StreamKind,
        /// What the client's viewer asked for.
        display: DisplayMode,
        /// Where the picture goes: nowhere else.
        peer: SessionPeer,
        /// A place for a datagram socket. The runner creates it before it
        /// says `ready`; the agent hands the client's keyboard and pointer
        /// input to it, one event per datagram. Input is taken from nowhere
        /// else.
        input_socket: String,
        /// From the client's viewer.
        #[serde(default)]
        offer: serde_json::Value,
    },
    /// Send the picture to this client from now on, and to nobody else.
    /// Answered with `attached`, or with `detached` when it cannot be done.
    Attach {
        peer: SessionPeer,
        /// What this client's viewer asked for.
        display: DisplayMode,
        /// From this client's viewer.
        #[serde(default)]
        offer: serde_json::Value,
    },
    /// The application was started on the display, as this process. For a
    /// runner that has to find the application's windows itself; one whose
    /// display the application connects to needs nothing from it.
    Application { pid: u32 },
    /// The client is gone: stop sending. The display and the application
    /// stay as they are.
    Detach,
    /// Ask the application's windows to close, the way a window manager
    /// does. What the application makes of it is its own business.
    Close,
    /// Stop making the picture and give the encoder back. The client stays
    /// attached; the display and the application stay as they are.
    Pause,
    /// Make the picture again, for the client that is attached, beginning
    /// with a frame that needs none before it.
    Resume,
}

/// What a runner tells its agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunnerEvent {
    /// The virtual display takes an application now.
    Ready {
        /// Environment variables that put an application on this display.
        env: BTreeMap<String, String>,
        /// What is really produced.
        display: DisplayMode,
        /// For the client's viewer.
        #[serde(default)]
        answer: serde_json::Value,
    },
    /// The picture goes to the client of the last `attach` now.
    Attached {
        /// What is really produced.
        display: DisplayMode,
        /// For that client's viewer.
        #[serde(default)]
        answer: serde_json::Value,
    },
    /// The picture goes to nobody: an `attach` could not be done, or the
    /// stream broke. The display and the application go on.
    Detached {
        /// One line for a person.
        #[serde(default)]
        message: String,
    },
    /// The last window on the display has closed.
    Empty,
    /// It cannot go on; it ends after this, and the application with it.
    Error { message: String },
}

/// What the person asked for on the command line. The viewer decides the
/// rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ViewerWish {
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    #[serde(default)]
    pub fps: Option<u32>,
    /// Where the window goes, in points from the top left of the main
    /// display.
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
}

/// What the command that opens a window tells a viewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum ViewerCommand {
    /// Get ready and say what you want.
    Prepare {
        /// The application's name, for the window's title.
        title: String,
        kind: StreamKind,
        #[serde(default)]
        wish: ViewerWish,
    },
    /// The session runs: show it.
    Show {
        session: SessionId,
        host: SessionPeer,
        /// What the host really produces.
        display: DisplayMode,
        /// From the host's runner.
        #[serde(default)]
        answer: serde_json::Value,
    },
    /// The host stopped the picture. The window keeps what it showed last
    /// and says so.
    Paused { reason: PauseReason },
    /// The picture comes again.
    Resumed,
}

/// What a viewer tells the command that started it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum ViewerEvent {
    /// The picture it wants, and what the host's runner has to know.
    Offer {
        display: DisplayMode,
        #[serde(default)]
        offer: serde_json::Value,
    },
    /// The first picture is on the screen.
    Shown,
    /// The person wants to close the window. The viewer keeps it open: the
    /// application is asked, and may have a question first.
    Close,
    /// It cannot go on; it ends after this.
    Error { message: String },
    /// The window cannot be seen: it is minimised, fully covered, or on
    /// another desktop. Its picture is of no use for now.
    Hidden,
    /// The window can be seen again, or the person turned to it.
    Visible,
}

#[derive(Serialize)]
struct LineOut<'a, T> {
    v: u32,
    #[serde(flatten)]
    body: &'a T,
}

#[derive(Deserialize)]
struct LineIn<T> {
    v: u32,
    #[serde(flatten)]
    body: T,
}

/// One line of the runner's or the viewer's protocol, without the line
/// break: the command or event, and the version.
pub fn encode_line<T: Serialize>(body: &T) -> String {
    serde_json::to_string(&LineOut {
        v: CONTRACT_VERSION,
        body,
    })
    // Plain data with string keys always serializes.
    .unwrap_or_default()
}

/// Reads one line. `None` for everything that is not understood: another
/// version, a kind of line this version does not know, or no JSON at all.
/// Such lines are skipped, so that either side can say more later.
pub fn decode_line<T: DeserializeOwned>(line: &str) -> Option<T> {
    let line: LineIn<T> = serde_json::from_str(line.trim()).ok()?;
    (line.v == CONTRACT_VERSION).then_some(line.body)
}

/// The address of the other end of a connection, as text a program can use
/// as it is: no port, an IPv4 address that was carried inside an IPv6 one
/// in its own form, and an IPv6 address that only means something on one
/// link together with that link's number (`fe80::1%4`).
pub fn address_text(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(v4) => v4.ip().to_string(),
        SocketAddr::V6(v6) => match v6.ip().to_ipv4_mapped() {
            Some(v4) => v4.to_string(),
            None if v6.scope_id() != 0 => format!("{}%{}", v6.ip(), v6.scope_id()),
            None => v6.ip().to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv6Addr, SocketAddrV6};

    use serde_json::json;

    use super::*;

    fn id() -> SessionId {
        SessionId("5b0c9e3a7f21d684".to_string())
    }

    fn mode() -> DisplayMode {
        DisplayMode {
            width: 1720,
            height: 720,
            fps: 85,
        }
    }

    #[test]
    fn ids_have_one_form() {
        assert!(id().is_well_formed());
        assert_eq!(id().to_string(), "5b0c9e3a7f21d684");
        for bad in [
            "",
            "5b0c9e3a7f21d68",
            "5b0c9e3a7f21d6840",
            "5B0C9E3A7F21D684",
            "../../etc/passwd",
            "5b0c9e3a7f21d68g",
        ] {
            assert!(!SessionId(bad.to_string()).is_well_formed(), "{bad}");
        }
    }

    #[test]
    fn a_session_is_reported_as_one_object() {
        let mut info = SessionInfo {
            id: id(),
            app: "org.mozilla.firefox.desktop".to_string(),
            kind: StreamKind::AppWindow,
            state: SessionState::Starting,
            revision: 1,
            attachment: 1,
            display: None,
            answer: None,
            end: None,
            left_by: None,
            shown_on: None,
            paused: None,
        };
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"starting","revision":1,"attachment":1,"display":null,"answer":null,"end":null}"#
        );

        // Nobody is shown it: the application runs on.
        info.state = SessionState::Detached;
        info.revision = 3;
        info.display = Some(mode());
        assert_eq!(
            serde_json::to_string(&info).unwrap(),
            r#"{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"detached","revision":3,"attachment":1,"display":{"width":1720,"height":720,"fps":85},"answer":null,"end":null}"#
        );

        info.state = SessionState::Stopping;
        info.revision = 5;
        info.attachment = 2;
        info.answer = Some(json!({"key_request_port": 50005}));
        info.end = Some(SessionEnd {
            reason: EndReason::Stopped,
            message: String::new(),
        });
        let text = serde_json::to_string(&info).unwrap();
        assert_eq!(
            text,
            r#"{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"stopping","revision":5,"attachment":2,"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005},"end":{"reason":"stopped","message":""}}"#
        );
        assert_eq!(serde_json::from_str::<SessionInfo>(&text).unwrap(), info);

        // A session that an unpaired device left behind says who opened
        // it. Other sessions do not carry the field.
        assert!(!text.contains("left_by"), "{text}");
        info.left_by = Some("old laptop".to_string());
        info.shown_on = Some("tablet".to_string());
        info.paused = Some(PauseReason::NoSlot);
        let text = serde_json::to_string(&info).unwrap();
        assert!(
            text.ends_with(
                r#""message":""},"left_by":"old laptop","shown_on":"tablet","paused":"no_slot"}"#
            ),
            "{text}"
        );
        assert_eq!(serde_json::from_str::<SessionInfo>(&text).unwrap(), info);
    }

    #[test]
    fn stopping_and_bringing_back_a_picture_are_lines_of_their_own() {
        assert_eq!(
            encode_line(&RunnerCommand::Pause),
            r#"{"v":1,"cmd":"pause"}"#
        );
        assert_eq!(
            encode_line(&RunnerCommand::Resume),
            r#"{"v":1,"cmd":"resume"}"#
        );
        assert_eq!(
            encode_line(&ViewerEvent::Hidden),
            r#"{"v":1,"event":"hidden"}"#
        );
        assert_eq!(
            decode_line::<ViewerEvent>(r#"{"v":1,"event":"visible"}"#),
            Some(ViewerEvent::Visible)
        );
        let paused = ViewerCommand::Paused {
            reason: PauseReason::Hidden,
        };
        assert_eq!(
            encode_line(&paused),
            r#"{"v":1,"cmd":"paused","reason":"hidden"}"#
        );
        assert_eq!(
            encode_line(&ViewerCommand::Resumed),
            r#"{"v":1,"cmd":"resumed"}"#
        );
        // A reason a newer host gives is read as one that is not known.
        assert_eq!(
            decode_line::<ViewerCommand>(r#"{"v":1,"cmd":"paused","reason":"cooling"}"#),
            Some(ViewerCommand::Paused {
                reason: PauseReason::Unknown
            })
        );
    }

    #[test]
    fn a_report_from_a_newer_host_is_still_read() {
        // A state, a reason and a field this version does not know; fields
        // it does know are missing.
        let text = r#"{"id":"5b0c9e3a7f21d684","app":"a","kind":"app_window","state":"suspended","revision":7,"end":{"reason":"host_asleep"},"owner":"1a2b"}"#;
        let info: SessionInfo = serde_json::from_str(text).unwrap();
        assert_eq!(info.state, SessionState::Unknown);
        assert_eq!(info.attachment, 0);
        assert_eq!(info.display, None);
        assert_eq!(info.answer, None);
        let end = info.end.unwrap();
        assert_eq!(end.reason, EndReason::Other);
        assert_eq!(end.message, "");
    }

    #[test]
    fn a_runner_is_told_everything_in_one_line() {
        let start = RunnerCommand::Start {
            session: id(),
            kind: StreamKind::AppWindow,
            display: mode(),
            peer: SessionPeer {
                id: "1a2b-3c4d-5e6f-7a8b".to_string(),
                address: "192.0.2.10".to_string(),
                public_key: None,
            },
            input_socket: "/run/user/1000/istesso/input-5b0c9e3a7f21d684.sock".to_string(),
            offer: json!({"rtp_port": 50004}),
        };
        let line = encode_line(&start);
        assert_eq!(
            line,
            r#"{"v":1,"cmd":"start","session":"5b0c9e3a7f21d684","kind":"app_window","display":{"width":1720,"height":720,"fps":85},"peer":{"id":"1a2b-3c4d-5e6f-7a8b","address":"192.0.2.10"},"input_socket":"/run/user/1000/istesso/input-5b0c9e3a7f21d684.sock","offer":{"rtp_port":50004}}"#
        );
        assert!(!line.contains('\n'));
        assert_eq!(decode_line::<RunnerCommand>(&line), Some(start));
    }

    #[test]
    fn a_runner_answers_in_lines() {
        let ready: RunnerEvent = decode_line(
            r#"{"v":1,"event":"ready","env":{"WAYLAND_DISPLAY":"wayland-3"},"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005}}"#,
        )
        .unwrap();
        assert_eq!(
            ready,
            RunnerEvent::Ready {
                env: BTreeMap::from([("WAYLAND_DISPLAY".to_string(), "wayland-3".to_string())]),
                display: mode(),
                answer: json!({"key_request_port": 50005}),
            }
        );
        assert_eq!(
            encode_line(&RunnerEvent::Empty),
            r#"{"v":1,"event":"empty"}"#
        );
        // A client comes back: the runner is told, and answers.
        let attach = RunnerCommand::Attach {
            peer: SessionPeer {
                id: "1a2b-3c4d-5e6f-7a8b".to_string(),
                address: "192.0.2.10".to_string(),
                public_key: None,
            },
            display: DisplayMode {
                width: 1280,
                height: 720,
                fps: 60,
            },
            offer: json!({"rtp_port": 50010}),
        };
        assert_eq!(
            encode_line(&attach),
            r#"{"v":1,"cmd":"attach","peer":{"id":"1a2b-3c4d-5e6f-7a8b","address":"192.0.2.10"},"display":{"width":1280,"height":720,"fps":60},"offer":{"rtp_port":50010}}"#
        );
        assert_eq!(
            decode_line::<RunnerEvent>(
                r#"{"v":1,"event":"attached","display":{"width":1280,"height":720,"fps":60},"answer":{"key_request_port":50011}}"#
            ),
            Some(RunnerEvent::Attached {
                display: DisplayMode {
                    width: 1280,
                    height: 720,
                    fps: 60
                },
                answer: json!({"key_request_port": 50011}),
            })
        );
        assert_eq!(
            decode_line::<RunnerEvent>(r#"{"v":1,"event":"detached"}"#),
            Some(RunnerEvent::Detached {
                message: String::new()
            })
        );
        // Leaving, and asking the application to close, need no more words.
        assert_eq!(
            encode_line(&RunnerCommand::Application { pid: 4711 }),
            r#"{"v":1,"cmd":"application","pid":4711}"#
        );
        assert_eq!(
            encode_line(&RunnerCommand::Detach),
            r#"{"v":1,"cmd":"detach"}"#
        );
        assert_eq!(
            encode_line(&RunnerCommand::Close),
            r#"{"v":1,"cmd":"close"}"#
        );
        assert_eq!(
            encode_line(&RunnerEvent::Error {
                message: "no encoder".to_string()
            }),
            r#"{"v":1,"event":"error","message":"no encoder"}"#
        );
        // A line break at the end and fields from a later version do no harm.
        assert_eq!(
            decode_line::<RunnerEvent>("{\"v\":1,\"event\":\"empty\",\"windows\":0}\r\n"),
            Some(RunnerEvent::Empty)
        );
    }

    #[test]
    fn lines_that_are_not_understood_are_skipped() {
        for line in [
            // A kind of line from a later version.
            r#"{"v":1,"event":"frame_rate","fps":85}"#,
            // Another version of the protocol.
            r#"{"v":2,"event":"empty"}"#,
            // No version at all.
            r#"{"event":"empty"}"#,
            // What a program prints by accident.
            "starting up...",
            "",
        ] {
            assert_eq!(decode_line::<RunnerEvent>(line), None, "{line}");
        }
    }

    #[test]
    fn a_viewer_is_asked_and_then_told() {
        let prepare = ViewerCommand::Prepare {
            title: "Firefox".to_string(),
            kind: StreamKind::AppWindow,
            wish: ViewerWish {
                width: Some(1720),
                height: Some(720),
                ..ViewerWish::default()
            },
        };
        assert_eq!(
            encode_line(&prepare),
            r#"{"v":1,"cmd":"prepare","title":"Firefox","kind":"app_window","wish":{"width":1720,"height":720,"fps":null,"x":null,"y":null}}"#
        );
        // Nothing wished for.
        assert_eq!(
            decode_line::<ViewerCommand>(
                r#"{"v":1,"cmd":"prepare","title":"Firefox","kind":"app_window"}"#
            ),
            Some(ViewerCommand::Prepare {
                title: "Firefox".to_string(),
                kind: StreamKind::AppWindow,
                wish: ViewerWish::default(),
            })
        );

        assert_eq!(
            decode_line::<ViewerEvent>(
                r#"{"v":1,"event":"offer","display":{"width":1720,"height":720,"fps":85},"offer":{"rtp_port":50004}}"#
            ),
            Some(ViewerEvent::Offer {
                display: mode(),
                offer: json!({"rtp_port": 50004}),
            })
        );

        let show = ViewerCommand::Show {
            session: id(),
            host: SessionPeer {
                id: "9f8e-7d6c-5b4a-3921".to_string(),
                address: "192.0.2.20".to_string(),
                public_key: None,
            },
            display: mode(),
            answer: json!({"key_request_port": 50005}),
        };
        let line = encode_line(&show);
        assert_eq!(
            line,
            r#"{"v":1,"cmd":"show","session":"5b0c9e3a7f21d684","host":{"id":"9f8e-7d6c-5b4a-3921","address":"192.0.2.20"},"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005}}"#
        );
        assert_eq!(decode_line::<ViewerCommand>(&line), Some(show));
        assert_eq!(
            encode_line(&ViewerEvent::Shown),
            r#"{"v":1,"event":"shown"}"#
        );
        assert_eq!(
            decode_line::<ViewerEvent>(r#"{"v":1,"event":"close"}"#),
            Some(ViewerEvent::Close)
        );
    }

    #[test]
    fn addresses_are_written_so_that_they_can_be_used() {
        let text = |addr: &str| address_text(&addr.parse().unwrap());
        assert_eq!(text("192.0.2.10:47801"), "192.0.2.10");
        assert_eq!(text("[2001:db8::7]:47801"), "2001:db8::7");
        assert_eq!(text("[::1]:47801"), "::1");
        // A host that listens on IPv6 sees an IPv4 caller like this.
        assert_eq!(text("[::ffff:192.0.2.10]:47801"), "192.0.2.10");
        // An address of one link is useless without the link.
        let link_local: Ipv6Addr = "fe80::1c2d:3e4f:5a6b:7c8d".parse().unwrap();
        let scoped = SocketAddr::V6(SocketAddrV6::new(link_local, 47801, 0, 4));
        assert_eq!(address_text(&scoped), "fe80::1c2d:3e4f:5a6b:7c8d%4");
    }
}
