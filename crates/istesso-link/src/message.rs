//! What one device can ask of another over the control channel.
//!
//! Messages are JSON objects with a `type` field. Devices are updated one at
//! a time, so both directions are forgiving: a request this version does not
//! know is answered with [`ErrorCode::Unsupported`], and readers skip the
//! parts of an answer they do not understand.

use istesso_proto::{AppEntry, HostManifest, HostStatus};
use serde::{Deserialize, Serialize};

// Streaming sessions (`docs/adr/0010-session-start.md`).
use istesso_proto::{DisplayMode, SessionId, SessionInfo, StreamKind};

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
    /// Start one of your applications and stream it to me. Answered at
    /// once with [`Response::Session`], before the application runs; needs
    /// the permissions to view and to control. Only an application from the
    /// host's own list can be asked for, by its id: no command line.
    SessionStart {
        /// The application's id on the host, as `apps` lists it.
        app: String,
        #[serde(default = "app_window")]
        kind: StreamKind,
        /// What the caller's window shows.
        display: DisplayMode,
        /// From the caller's media path, for the host's. Carried as it is.
        #[serde(default)]
        offer: serde_json::Value,
    },
    /// How is my session? Answered at once when it changed after revision
    /// `after`; otherwise the host may hold the answer back for up to
    /// `timeout_ms`. A request that names the attachment that is shown the
    /// session also tells the host that this client is still there. Only
    /// the device that started the session may ask.
    SessionWait {
        session: SessionId,
        /// The revision the caller has seen; 0 for none.
        #[serde(default)]
        after: u64,
        /// 0 asks for an answer at once.
        #[serde(default)]
        timeout_ms: u32,
        /// The attachment the caller is; 0 when it only asks.
        #[serde(default)]
        attachment: u64,
    },
    /// Ask the application to close, the way closing its window does: it
    /// may have a question first, and stay. With `force`, end it.
    SessionStop {
        session: SessionId,
        #[serde(default)]
        force: bool,
    },
    /// Which of my sessions are not over? Answered with
    /// [`Response::Sessions`].
    SessionList,
    /// Show me this session (again). Whoever is shown it now is not any
    /// more. Answered once the host's side of the stream is set up, with
    /// the new attachment and answer; a refusal leaves the session running
    /// and shown to nobody. Needs the permissions to view and to control.
    SessionAttach {
        session: SessionId,
        /// What the caller's window shows.
        display: DisplayMode,
        /// From the caller's media path, for the host's. Carried as it is.
        #[serde(default)]
        offer: serde_json::Value,
    },
    /// Stop showing me this session. The application runs on. Does nothing
    /// when `attachment` is no longer the one that is shown it.
    SessionDetach { session: SessionId, attachment: u64 },
}

fn app_window() -> StreamKind {
    StreamKind::AppWindow
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
    Error {
        code: ErrorCode,
        message: String,
    },
    /// The state of one session: the answer to every request about one.
    Session {
        session: Box<SessionInfo>,
    },
    /// The caller's sessions that are not over, oldest first.
    Sessions {
        sessions: Vec<SessionInfo>,
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
    /// No such application, or no such session of the caller's.
    NotFound,
    /// The host cannot do it now: it has no media path, the application
    /// cannot be shown this way, or too many sessions run already.
    Unavailable,
    // serde wants the catch-all to stay the last one.
    #[serde(other)]
    Other,
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
    fn session_requests_are_tagged_objects_with_defaults() {
        let id = SessionId("5b0c9e3a7f21d684".to_string());
        let start = Request::SessionStart {
            app: "org.mozilla.firefox.desktop".into(),
            kind: StreamKind::AppWindow,
            display: DisplayMode {
                width: 1720,
                height: 720,
                fps: 85,
            },
            offer: serde_json::json!({"rtp_port": 50004}),
        };
        let json = serde_json::to_string(&start).unwrap();
        assert_eq!(
            json,
            r#"{"type":"session_start","app":"org.mozilla.firefox.desktop","kind":"app_window","display":{"width":1720,"height":720,"fps":85},"offer":{"rtp_port":50004}}"#
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), start);
        // The kind and the offer may be left out.
        assert_eq!(
            serde_json::from_str::<Request>(
                r#"{"type":"session_start","app":"a.desktop","display":{"width":800,"height":600,"fps":60}}"#
            )
            .unwrap(),
            Request::SessionStart {
                app: "a.desktop".into(),
                kind: StreamKind::AppWindow,
                display: DisplayMode {
                    width: 800,
                    height: 600,
                    fps: 60
                },
                offer: serde_json::Value::Null,
            }
        );
        // A kind this version does not know makes the request one it does
        // not know: the host answers `unsupported`.
        assert!(serde_json::from_str::<Request>(
            r#"{"type":"session_start","app":"a","kind":"hologram","display":{"width":1,"height":1,"fps":1}}"#
        )
        .is_err());

        let wait = Request::SessionWait {
            session: id.clone(),
            after: 1,
            timeout_ms: 2000,
            attachment: 1,
        };
        assert_eq!(
            serde_json::to_string(&wait).unwrap(),
            r#"{"type":"session_wait","session":"5b0c9e3a7f21d684","after":1,"timeout_ms":2000,"attachment":1}"#
        );
        // Only asking: no patience, and no attachment to speak for.
        assert_eq!(
            serde_json::from_str::<Request>(
                r#"{"type":"session_wait","session":"5b0c9e3a7f21d684"}"#
            )
            .unwrap(),
            Request::SessionWait {
                session: id.clone(),
                after: 0,
                timeout_ms: 0,
                attachment: 0,
            }
        );
        // Asking to close is what is meant unless force is spelled out.
        assert_eq!(
            serde_json::to_string(&Request::SessionStop {
                session: id.clone(),
                force: false,
            })
            .unwrap(),
            r#"{"type":"session_stop","session":"5b0c9e3a7f21d684","force":false}"#
        );
        assert_eq!(
            serde_json::from_str::<Request>(
                r#"{"type":"session_stop","session":"5b0c9e3a7f21d684"}"#
            )
            .unwrap(),
            Request::SessionStop {
                session: id.clone(),
                force: false,
            }
        );
    }

    #[test]
    fn coming_back_to_a_session_has_its_own_requests() {
        let id = SessionId("5b0c9e3a7f21d684".to_string());
        assert_eq!(
            serde_json::to_string(&Request::SessionList).unwrap(),
            r#"{"type":"session_list"}"#
        );
        let attach = Request::SessionAttach {
            session: id.clone(),
            display: DisplayMode {
                width: 1280,
                height: 720,
                fps: 60,
            },
            offer: serde_json::json!({"rtp_port": 50010}),
        };
        let json = serde_json::to_string(&attach).unwrap();
        assert_eq!(
            json,
            r#"{"type":"session_attach","session":"5b0c9e3a7f21d684","display":{"width":1280,"height":720,"fps":60},"offer":{"rtp_port":50010}}"#
        );
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), attach);
        assert_eq!(
            serde_json::to_string(&Request::SessionDetach {
                session: id.clone(),
                attachment: 1,
            })
            .unwrap(),
            r#"{"type":"session_detach","session":"5b0c9e3a7f21d684","attachment":1}"#
        );

        let listed = Response::Sessions {
            sessions: vec![SessionInfo {
                id,
                app: "org.mozilla.firefox.desktop".into(),
                kind: StreamKind::AppWindow,
                state: istesso_proto::SessionState::Detached,
                revision: 3,
                attachment: 1,
                display: Some(DisplayMode {
                    width: 1720,
                    height: 720,
                    fps: 85,
                }),
                answer: None,
                end: None,
            }],
        };
        let json = serde_json::to_string(&listed).unwrap();
        assert_eq!(
            json,
            r#"{"type":"sessions","sessions":[{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"detached","revision":3,"attachment":1,"display":{"width":1720,"height":720,"fps":85},"answer":null,"end":null}]}"#
        );
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), listed);
    }

    #[test]
    fn a_session_is_answered_as_one_object() {
        let response = Response::Session {
            session: Box::new(SessionInfo {
                id: SessionId("5b0c9e3a7f21d684".to_string()),
                app: "org.mozilla.firefox.desktop".into(),
                kind: StreamKind::AppWindow,
                state: istesso_proto::SessionState::Running,
                revision: 2,
                attachment: 1,
                display: Some(DisplayMode {
                    width: 1720,
                    height: 720,
                    fps: 85,
                }),
                answer: Some(serde_json::json!({"key_request_port": 50005})),
                end: None,
            }),
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            json,
            r#"{"type":"session","session":{"id":"5b0c9e3a7f21d684","app":"org.mozilla.firefox.desktop","kind":"app_window","state":"running","revision":2,"attachment":1,"display":{"width":1720,"height":720,"fps":85},"answer":{"key_request_port":50005},"end":null}}"#
        );
        assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), response);
    }

    #[test]
    fn the_new_error_codes_have_names() {
        assert_eq!(
            serde_json::to_string(&Response::error(ErrorCode::Unavailable, "no media path"))
                .unwrap(),
            r#"{"type":"error","code":"unavailable","message":"no media path"}"#
        );
        let json = r#"{"type":"error","code":"not_found","message":""}"#;
        assert_eq!(
            serde_json::from_str::<Response>(json).unwrap(),
            Response::error(ErrorCode::NotFound, "")
        );
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
