//! What one device can ask of another over the control channel.
//!
//! Messages are JSON objects with a `type` field. Devices are updated one at
//! a time, so both directions are forgiving: a request this version does not
//! know is answered with [`ErrorCode::Unsupported`], and readers skip the
//! parts of an answer they do not understand.

use istesso_proto::{
    AppEntry, ClipboardResult, ClipboardWhy, HostManifest, HostStatus, InputEvent, SecretText,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Are you there?
    Ping,
    /// What can you do? (the host's entry in the role table)
    Manifest,
    /// Which applications are installed?
    Apps,
    /// How busy are you?
    Status,
    /// May I send you what a person does with keyboard and pointer, for the
    /// application you are showing me? Answered with [`Response::InputOpen`];
    /// needs the permission to control.
    ///
    /// With `alive`, the caller asks to be answered whenever it sends an
    /// empty [`Request::Input`], so that it can tell a connection that has
    /// gone silent from one that is merely idle. A device that knows this
    /// says so in its answer; one that does not ignores the field.
    InputOpen {
        #[serde(default, skip_serializing_if = "is_false")]
        alive: bool,
    },
    /// Keyboard and pointer input, after [`Request::InputOpen`] was granted
    /// on this connection. Never answered when it carries events: what a
    /// person types must not wait for a round trip. An empty list keeps the
    /// connection open, and is answered with [`Response::InputAlive`] when
    /// that was agreed on at [`Request::InputOpen`].
    Input {
        #[serde(deserialize_with = "istesso_proto::known_only")]
        events: Vec<InputEvent>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Manifest {
        manifest: Box<HostManifest>,
    },
    Apps {
        #[serde(deserialize_with = "istesso_proto::known_only")]
        apps: Vec<AppEntry>,
    },
    Status {
        status: HostStatus,
    },
    /// Input is taken from now on. With `alive`: and an empty
    /// [`Request::Input`] is answered from now on, as was asked. With
    /// `text`: committed text (`InputEvent::Text`) is taken too; a device
    /// that does not say so must not be sent any, as it cannot read a
    /// message with it (docs/adr/0007-control-channel.md, "確定した文字").
    InputOpen {
        #[serde(default, skip_serializing_if = "is_false")]
        alive: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        text: bool,
        /// Trial: the level of scrolls taken (`istesso_proto::SCROLL_LEVEL`); 0
        /// (left out): only `dx` and `dy` of a scroll are used.
        #[serde(default, skip_serializing_if = "is_zero")]
        scroll: u8,
        /// Copy and paste (`InputEvent::Copy`, `InputEvent::Paste`) are taken too: what takes
        /// input here said so just now (trial only, docs/adr/0022-clipboard-text.md). A device
        /// that does not say so is sent neither; they would be dropped without a word.
        #[serde(default, skip_serializing_if = "is_false")]
        clipboard: bool,
        /// Composing text (`InputEvent::Preedit`) is taken too.
        #[serde(default, skip_serializing_if = "is_false")]
        preedit: bool,
        /// The application's other windows are taken too (trial only, ADR 0027 proposal):
        /// window numbers on `InputEvent::PointerAt`, `Focus` and `Resize`, `InputEvent::Close`
        /// and `InputEvent::Windows` are passed on to what takes input here. A device that does
        /// not say so is sent none of them; a pointer meant for another window would land on
        /// the first one there.
        #[serde(default, skip_serializing_if = "is_false")]
        windows: bool,
    },
    /// Still here: the answer to an empty [`Request::Input`] on a
    /// connection where that was agreed on.
    InputAlive,
    /// What became of the copy or paste `id` (trial only). Sent on the conversation that asked,
    /// after one of its messages, at most once for each `id`. `text` only with
    /// [`ClipboardResult::Text`], `why` only with [`ClipboardResult::Refused`]. Never logged.
    Clipboard {
        id: u32,
        result: ClipboardResult,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<SecretText>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        why: Option<ClipboardWhy>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request is not known to this version.
    Unsupported,
    /// The paired device lacks the permission the request needs.
    Forbidden,
    /// The host failed to produce an answer.
    Internal,
    #[serde(other)]
    Other,
}

fn is_zero(value: &u8) -> bool {
    *value == 0
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

impl Response {
    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Response::Error {
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use istesso_proto::{Launch, OsFamily};

    #[test]
    fn requests_are_small_tagged_objects() {
        assert_eq!(
            serde_json::to_string(&Request::Manifest).unwrap(),
            r#"{"type":"manifest"}"#
        );
        let request: Request = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(request, Request::Ping);
        assert!(serde_json::from_str::<Request>(r#"{"type":"launch_rocket"}"#).is_err());
    }

    #[test]
    fn input_is_a_list_that_survives_events_from_a_newer_sender() {
        let request = Request::Input {
            events: vec![
                InputEvent::Key {
                    code: 30,
                    pressed: true,
                },
                InputEvent::pointer_at(1, 2),
            ],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"type":"input","events":[{"kind":"key","code":30,"pressed":true},{"kind":"pointer_at","x":1,"y":2}]}"#
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);
        // An event this version does not know is dropped; the rest arrives.
        let newer = r#"{"type":"input","events":[{"kind":"pen","pressure":0.5},{"kind":"button","code":272,"pressed":true}]}"#;
        assert_eq!(
            serde_json::from_str::<Request>(newer).unwrap(),
            Request::Input {
                events: vec![InputEvent::Button {
                    code: 272,
                    pressed: true
                }]
            }
        );
        assert_eq!(
            serde_json::to_string(&Request::InputOpen { alive: false }).unwrap(),
            r#"{"type":"input_open"}"#
        );
    }

    /// The two messages as the version before knew them: without `alive`.
    #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum Earlier {
        InputOpen,
    }

    #[test]
    fn asking_to_be_answered_is_one_field_that_an_earlier_version_passes_over() {
        // Not asked for, the messages are what they always were.
        let plain = r#"{"type":"input_open"}"#;
        assert_eq!(
            serde_json::to_string(&Response::InputOpen {
                alive: false,
                text: false,
                scroll: 0,
                clipboard: false,
                preedit: false,
                windows: false
            })
            .unwrap(),
            plain
        );
        assert_eq!(
            serde_json::from_str::<Request>(plain).unwrap(),
            Request::InputOpen { alive: false }
        );
        assert_eq!(
            serde_json::from_str::<Response>(plain).unwrap(),
            Response::InputOpen {
                alive: false,
                text: false,
                scroll: 0,
                clipboard: false,
                preedit: false,
                windows: false
            }
        );
        // Asked for, and agreed to.
        let asking = r#"{"type":"input_open","alive":true}"#;
        assert_eq!(
            serde_json::to_string(&Request::InputOpen { alive: true }).unwrap(),
            asking
        );
        assert_eq!(
            serde_json::from_str::<Response>(asking).unwrap(),
            Response::InputOpen {
                alive: true,
                text: false,
                scroll: 0,
                clipboard: false,
                preedit: false,
                windows: false
            }
        );
        assert_eq!(
            serde_json::from_str::<Response>(r#"{"type":"input_open","alive":true,"text":true}"#)
                .unwrap(),
            Response::InputOpen {
                alive: true,
                text: true,
                scroll: 0,
                clipboard: false,
                preedit: false,
                windows: false
            }
        );
        assert_eq!(
            serde_json::from_str::<Response>(r#"{"type":"input_open","text":true,"scroll":1}"#)
                .unwrap(),
            Response::InputOpen {
                alive: false,
                text: true,
                scroll: 1,
                clipboard: false,
                preedit: false,
                windows: false
            }
        );
        // An earlier version reads both as what it knows, and its own answer says
        // nothing about it: that is "no".
        assert_eq!(
            serde_json::from_str::<Earlier>(asking).unwrap(),
            Earlier::InputOpen
        );
        assert_eq!(
            serde_json::from_str::<Response>(&serde_json::to_string(&Earlier::InputOpen).unwrap())
                .unwrap(),
            Response::InputOpen {
                alive: false,
                text: false,
                scroll: 0,
                clipboard: false,
                preedit: false,
                windows: false
            }
        );
        assert_eq!(
            serde_json::to_string(&Response::InputAlive).unwrap(),
            r#"{"type":"input_alive"}"#
        );
    }

    #[test]
    fn taking_the_other_windows_is_one_field_that_an_earlier_version_passes_over() {
        let open = Response::InputOpen {
            alive: true,
            text: true,
            scroll: 1,
            clipboard: false,
            preedit: true,
            windows: true,
        };
        let json = serde_json::to_string(&open).unwrap();
        assert_eq!(
            json,
            r#"{"type":"input_open","alive":true,"text":true,"scroll":1,"preedit":true,"windows":true}"#
        );
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), open);
        // An earlier version reads it as what it knows; its own answer says nothing: "no".
        assert_eq!(
            serde_json::from_str::<Earlier>(&json).unwrap(),
            Earlier::InputOpen
        );
        assert_eq!(
            serde_json::from_str::<Response>(r#"{"type":"input_open","alive":true}"#).unwrap(),
            Response::InputOpen {
                alive: true,
                text: false,
                scroll: 0,
                clipboard: false,
                preedit: false,
                windows: false
            }
        );
        // Events with window numbers travel in an input message as they are.
        let request = Request::Input {
            events: vec![
                InputEvent::PointerAt {
                    x: 1,
                    y: 2,
                    window: Some(2),
                },
                InputEvent::Close { window: 2 },
                InputEvent::Windows { version: 1 },
            ],
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"type":"input","events":[{"kind":"pointer_at","x":1,"y":2,"window":2},{"kind":"close","window":2},{"kind":"windows","version":1}]}"#
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);
    }

    #[test]
    fn responses_round_trip() {
        let response = Response::Apps {
            apps: vec![AppEntry {
                id: "org.mozilla.firefox".into(),
                name: "Firefox".into(),
                platform: OsFamily::Macos,
                version: Some("133.0".into()),
                launch: Launch::AppBundle {
                    bundle_id: "org.mozilla.firefox".into(),
                    path: "/Applications/Firefox.app".into(),
                },
                icon: None,
                categories: Vec::new(),
            }],
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), response);

        let status = Response::Status {
            status: HostStatus {
                load: Some(0.25),
                memory_available_bytes: Some(1 << 30),
            },
        };
        let json = serde_json::to_string(&status).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), status);
    }

    #[test]
    fn apps_from_a_newer_host_lose_only_what_is_not_understood() {
        // One entry uses a way to launch apps that this version does not know.
        let json = r#"{"type":"apps","apps":[
            {"id":"a","name":"Known","platform":"linux","version":null,
             "launch":{"kind":"desktop_entry","desktop_id":"a.desktop","exec":"a"},
             "icon":null,"categories":[]},
            {"id":"b","name":"Future","platform":"windows","version":null,
             "launch":{"kind":"start_menu","link":"b.lnk"},
             "icon":null,"categories":[]}
        ]}"#;
        match serde_json::from_str::<Response>(json).unwrap() {
            Response::Apps { apps } => {
                assert_eq!(apps.len(), 1);
                assert_eq!(apps[0].name, "Known");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn unknown_error_codes_are_still_errors() {
        let json = r#"{"type":"error","code":"quota_exceeded","message":"later"}"#;
        assert_eq!(
            serde_json::from_str::<Response>(json).unwrap(),
            Response::error(ErrorCode::Other, "later")
        );
    }

    #[test]
    fn the_answer_to_a_copy_carries_its_text_and_shows_it_to_nobody() {
        use istesso_proto::{ClipboardResult, ClipboardWhy, SecretText};
        let answer = Response::Clipboard {
            id: 3,
            result: ClipboardResult::Text,
            text: Some(SecretText("hunter2".into())),
            why: None,
        };
        let json = serde_json::to_string(&answer).unwrap();
        assert_eq!(
            json,
            r#"{"type":"clipboard","id":3,"result":"text","text":"hunter2"}"#
        );
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), answer);
        assert!(!format!("{answer:?}").contains("hunter2"));
        assert_eq!(
            serde_json::from_str::<Response>(
                r#"{"type":"clipboard","id":4,"result":"refused","why":"secure_input"}"#
            )
            .unwrap(),
            Response::Clipboard {
                id: 4,
                result: ClipboardResult::Refused,
                text: None,
                why: Some(ClipboardWhy::SecureInput)
            }
        );
        assert_eq!(
            serde_json::from_str::<Response>(
                r#"{"type":"input_open","alive":true,"text":true,"clipboard":true}"#
            )
            .unwrap(),
            Response::InputOpen {
                alive: true,
                text: true,
                scroll: 0,
                clipboard: true,
                preedit: false,
                windows: false
            }
        );
    }
}
