//! What one device can ask of another over the control channel.
//!
//! Messages are JSON objects with a `type` field. Devices are updated one at
//! a time, so both directions are forgiving: a request this version does not
//! know is answered with [`ErrorCode::Unsupported`], and readers skip the
//! parts of an answer they do not understand.

use istesso_proto::{AppEntry, HostManifest, HostStatus, InputEvent};
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
    /// May I have a shell on your machine? Answered with
    /// [`Response::ShellOpen`] or an error. Once it is open, the connection
    /// carries only the frames of [`crate::shell`] in both directions, and
    /// no more JSON. Proposed in ADR 0007 ("文字のシェル"); off unless the
    /// host was told to allow it for this device.
    ShellOpen(ShellRequest),
    /// May I see this folder of yours, read-only? Answered with
    /// [`Response::MountOpen`] or an error. Once it is open, the connection
    /// carries only the questions and answers of [`crate::mount`], and no
    /// more JSON of this kind. Proposed in ADR 0007 ("フォルダーを読むだけで
    /// 見せる"); off unless the host was told which folders and which devices.
    MountOpen(crate::mount::MountRequest),
}

/// What a device says when it asks for a shell ([`Request::ShellOpen`]).
/// The host checks every field; none of it is trusted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellRequest {
    /// The size of the terminal the shell is shown in.
    pub cols: u16,
    pub rows: u16,
    /// The terminal's type, for `TERM`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub term: Option<String>,
    /// The language the person uses, for `LANG` (`ja_JP.UTF-8`, say).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// A folder on the host to start in: absolute, or below the home folder
    /// (`~/...`). The home folder when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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
    /// [`Request::Input`] is answered from now on, as was asked.
    InputOpen {
        #[serde(default, skip_serializing_if = "is_false")]
        alive: bool,
    },
    /// Still here: the answer to an empty [`Request::Input`] on a
    /// connection where that was agreed on.
    InputAlive,
    /// The shell is running. From now on the connection carries the frames
    /// of [`crate::shell`] only.
    ShellOpen,
    /// The folder is open. From now on the connection carries the questions
    /// and answers of [`crate::mount`] only.
    MountOpen {
        /// The attributes of the folder itself.
        root: crate::mount::Attr,
        /// The most bytes one read is answered with.
        max_read: u32,
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
                InputEvent::PointerAt { x: 1, y: 2 },
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
            serde_json::to_string(&Response::InputOpen { alive: false }).unwrap(),
            plain
        );
        assert_eq!(
            serde_json::from_str::<Request>(plain).unwrap(),
            Request::InputOpen { alive: false }
        );
        assert_eq!(
            serde_json::from_str::<Response>(plain).unwrap(),
            Response::InputOpen { alive: false }
        );
        // Asked for, and agreed to.
        let asking = r#"{"type":"input_open","alive":true}"#;
        assert_eq!(
            serde_json::to_string(&Request::InputOpen { alive: true }).unwrap(),
            asking
        );
        assert_eq!(
            serde_json::from_str::<Response>(asking).unwrap(),
            Response::InputOpen { alive: true }
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
            Response::InputOpen { alive: false }
        );
        assert_eq!(
            serde_json::to_string(&Response::InputAlive).unwrap(),
            r#"{"type":"input_alive"}"#
        );
    }

    #[test]
    fn asking_for_a_shell_is_a_small_object_with_optional_parts() {
        let bare = Request::ShellOpen(ShellRequest {
            cols: 80,
            rows: 24,
            term: None,
            locale: None,
            project: None,
        });
        let json = serde_json::to_string(&bare).unwrap();
        assert_eq!(json, r#"{"type":"shell_open","cols":80,"rows":24}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), bare);

        let full = r#"{"type":"shell_open","cols":120,"rows":40,"term":"xterm-ghostty","locale":"ja_JP.UTF-8","project":"~/src/istesso","later":"ignored"}"#;
        match serde_json::from_str::<Request>(full).unwrap() {
            Request::ShellOpen(request) => {
                assert_eq!((request.cols, request.rows), (120, 40));
                assert_eq!(request.term.as_deref(), Some("xterm-ghostty"));
                assert_eq!(request.locale.as_deref(), Some("ja_JP.UTF-8"));
                assert_eq!(request.project.as_deref(), Some("~/src/istesso"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        // A size that does not fit is not a request at all.
        assert!(
            serde_json::from_str::<Request>(r#"{"type":"shell_open","cols":70000,"rows":1}"#)
                .is_err()
        );
        assert_eq!(
            serde_json::to_string(&Response::ShellOpen).unwrap(),
            r#"{"type":"shell_open"}"#
        );
    }

    #[test]
    fn asking_for_a_folder_names_it_by_id_only() {
        let request = Request::MountOpen(crate::mount::MountRequest {
            folder: "istesso".into(),
        });
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(json, r#"{"type":"mount_open","folder":"istesso"}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), request);
        let answer = Response::MountOpen {
            root: crate::mount::Attr {
                kind: crate::mount::Kind::Dir,
                mode: 0o755,
                nlink: 2,
                size: 0,
                mtime: 1,
                mtime_nsec: 0,
                ctime: 1,
                ctime_nsec: 0,
            },
            max_read: crate::mount::MAX_READ,
        };
        let json = serde_json::to_string(&answer).unwrap();
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), answer);
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
}
