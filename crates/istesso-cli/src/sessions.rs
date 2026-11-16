//! `istesso sessions`: the applications of other devices that run for this
//! one, shown in a window or not, and how to end one
//! (`docs/adr/0010-session-start.md`).
//!
//! With `--local` it is the other way round: the applications of this
//! machine that run for other devices, whichever device started them. That
//! is for the person at the host, and is asked of the agent that runs here.

use std::time::Duration;

use istesso_agent::session::client::{link_to, sessions_on, stop_session, ClientRules, OpenError};
use istesso_agent::session::local::{after_unpairing, sessions_here, stop_here};
use istesso_agent::session::{HostSession, Rules};
use istesso_agent::{survey, HostView, Paths, SurveyOptions};
use istesso_link::ErrorCode;
use istesso_proto::{HostState, SessionInfo, SessionState};
use serde::Serialize;

use crate::open::{explain, COMMAND as OPEN};

/// An application of another device that runs for this one.
#[derive(Debug, Clone, Serialize)]
pub struct Running {
    pub host_id: String,
    pub host_name: String,
    /// The application's name, or its id where the host did not list it.
    pub app_name: String,
    #[serde(rename = "session")]
    pub info: SessionInfo,
}

/// Asks every device that answers what it runs for this one. The second
/// list names the devices that could not be asked, and why.
pub fn running(paths: &Paths, views: &[HostView]) -> (Vec<Running>, Vec<String>) {
    let rules = ClientRules::default();
    let mut all = Vec::new();
    let mut not_asked = Vec::new();
    for view in views
        .iter()
        .filter(|view| !view.local && view.state == HostState::Awake)
    {
        let listed = link_to(paths, &view.id, Duration::from_millis(800))
            .map_err(OpenError::Unreachable)
            .and_then(|link| sessions_on(&link, &rules));
        match listed {
            Ok(sessions) => all.extend(sessions.into_iter().map(|mut info| {
                // What the media path agreed on is for the window that is
                // shown the session. It has no place in a list.
                info.answer = None;
                Running {
                    host_id: view.id.clone(),
                    host_name: view.name.clone(),
                    app_name: view
                        .apps
                        .iter()
                        .find(|app| app.id == info.app)
                        .map_or_else(|| info.app.clone(), |app| app.name.clone()),
                    info,
                }
            })),
            // A device whose istesso knows no sessions runs none.
            Err(OpenError::Refused(refusal)) if refusal.code == ErrorCode::Unsupported => {}
            Err(why) => not_asked.push(format!("{} ({why})", view.name)),
        }
    }
    (all, not_asked)
}

/// A session's state, for a person.
pub fn state_words(state: SessionState) -> &'static str {
    match state {
        SessionState::Starting => "starting",
        SessionState::Running => "shown in a window",
        SessionState::Detached => "running, not shown",
        SessionState::Stopping => "ending",
        SessionState::Ended => "ended",
        SessionState::Unknown => "unknown",
    }
}

/// A session's state, and whose it was when the device that opened it has
/// been unpaired since.
pub fn words(info: &SessionInfo) -> String {
    let state = state_words(info.state);
    match &info.left_by {
        Some(device) => format!("{state}; opened by {device}, which was unpaired"),
        None => state.to_string(),
    }
}

/// Which of `ids` the person means: an id, or the beginning of one.
/// `where_` says where the sessions run and `lister` how to list them, for
/// the error.
fn pick_among(ids: &[&str], asked: &str, where_: &str, lister: &str) -> Result<usize, String> {
    let asked = asked.trim().to_lowercase();
    if let Some(exact) = ids.iter().position(|id| *id == asked) {
        return Ok(exact);
    }
    let begins: Vec<usize> = (0..ids.len())
        .filter(|at| asked.len() >= 4 && ids[*at].starts_with(&asked))
        .collect();
    match begins.as_slice() {
        &[one] => Ok(one),
        [] => Err(format!(
            "no session '{asked}' runs {where_}; `{lister}` lists them"
        )),
        _ => Err(format!(
            "'{asked}' is the beginning of several sessions; give more of the id"
        )),
    }
}

/// The session the person means: by its id, or by the beginning of it.
pub fn pick<'a>(
    running: impl Iterator<Item = &'a Running>,
    asked: &str,
) -> Result<&'a Running, String> {
    let all: Vec<&Running> = running.collect();
    let ids: Vec<&str> = all.iter().map(|r| r.info.id.0.as_str()).collect();
    pick_among(&ids, asked, "for this device", "istesso sessions").map(|at| all[at])
}

/// How wide a column is: as wide as its widest text, within reason.
fn column<'a>(title: &str, texts: impl Iterator<Item = &'a str>) -> usize {
    texts
        .map(unicode_width::UnicodeWidthStr::width)
        .max()
        .unwrap_or(0)
        .clamp(title.len(), 32)
}

/// The table `istesso sessions` prints.
pub fn render(running: &[Running]) -> String {
    let app = column("APPLICATION", running.iter().map(|r| r.app_name.as_str()));
    let host = column("DEVICE", running.iter().map(|r| r.host_name.as_str()));
    let mut out = format!(
        "{} {} {} STATE\n",
        crate::pad("SESSION", 16),
        crate::pad("APPLICATION", app),
        crate::pad("DEVICE", host)
    );
    for r in running {
        out.push_str(&format!(
            "{} {} {} {}\n",
            r.info.id,
            crate::pad(&r.app_name, app),
            crate::pad(&r.host_name, host),
            words(&r.info)
        ));
    }
    out
}

/// `istesso sessions`: what runs for this device, on every device that
/// answers.
pub fn list(json: bool) -> Result<(), String> {
    let paths = Paths::from_env();
    let views = survey(&paths, &SurveyOptions::default())?;
    let (running, not_asked) = running(&paths, &views);
    if json {
        crate::print_json(&running)?;
    } else if running.is_empty() {
        println!("No application of another device runs for this device.");
    } else {
        print!("{}", render(&running));
        println!(
            "\n`istesso {OPEN} <application>` shows one again; `istesso sessions stop <session>` \
             asks one to close."
        );
    }
    let silent: Vec<&str> = views
        .iter()
        .filter(|view| !view.local && view.state != HostState::Awake)
        .map(|view| view.name.as_str())
        .collect();
    if !silent.is_empty() {
        eprintln!(
            "\nDid not answer: {}. What runs there is not listed.",
            silent.join(", ")
        );
    }
    if !not_asked.is_empty() {
        eprintln!("\nCould not be asked: {}.", not_asked.join("; "));
    }
    Ok(())
}

/// `istesso sessions stop`: asks an application to close, the way closing
/// its window does, or with `force` ends it.
pub fn stop(session: &str, force: bool) -> Result<(), String> {
    let paths = Paths::from_env();
    let views = survey(&paths, &SurveyOptions::default())?;
    let (running, not_asked) = running(&paths, &views);
    let found = pick(running.iter(), session).map_err(|why| {
        if not_asked.is_empty() {
            why
        } else {
            format!("{why} (could not be asked: {})", not_asked.join("; "))
        }
    })?;
    let (name, host, id) = (&found.app_name, &found.host_name, &found.info.id);
    let link = link_to(&paths, &found.host_id, Duration::from_millis(800))?;
    let rules = ClientRules::default();

    if force {
        // The person is told how long the application has before it
        // happens.
        let grace = Rules::default().app_stop_grace;
        eprintln!(
            "Ending {name} on {host} by force. It has {} seconds to end by itself.",
            grace.as_secs()
        );
        let last = stop_session(&link, id, true, grace + Duration::from_secs(5), &rules)
            .map_err(|refusal| explain(&refusal, name, host))?;
        if last.state != SessionState::Ended {
            return Err(format!("{name} on {host} has not ended yet"));
        }
        println!("{name} was ended on {host}.");
    } else {
        eprintln!("Asking {name} on {host} to close...");
        let last = stop_session(&link, id, false, Duration::from_secs(5), &rules)
            .map_err(|refusal| explain(&refusal, name, host))?;
        println!("{}", after_asking(&last, name, host));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The person at the host
// ---------------------------------------------------------------------------

/// What `istesso sessions --local` says about a session's state.
fn local_state_words(session: &HostSession) -> String {
    if session.info.left_by.is_none() && session.revoked {
        // Still paired, without a right that starting takes: only from
        // here can the session be ended.
        return format!(
            "{}; {} may not use it now",
            state_words(session.info.state),
            session.started_by.name
        );
    }
    words(&session.info)
}

/// The table `istesso sessions --local` prints.
pub fn render_local(sessions: &[HostSession]) -> String {
    let app = column("APPLICATION", sessions.iter().map(|s| s.app_name.as_str()));
    let by = column(
        "STARTED BY",
        sessions.iter().map(|s| s.started_by.name.as_str()),
    );
    let mut out = format!(
        "{} {} {} STATE\n",
        crate::pad("SESSION", 16),
        crate::pad("APPLICATION", app),
        crate::pad("STARTED BY", by)
    );
    for session in sessions {
        out.push_str(&format!(
            "{} {} {} {}\n",
            session.info.id,
            crate::pad(&session.app_name, app),
            crate::pad(&session.started_by.name, by),
            local_state_words(session)
        ));
    }
    out
}

/// `istesso sessions --local`: what runs on this machine for other
/// devices.
pub fn list_local(json: bool) -> Result<(), String> {
    let sessions = sessions_here(&Paths::from_env())?;
    if json {
        crate::print_json(&sessions)?;
    } else if sessions.is_empty() {
        println!("No application of this machine runs for another device.");
    } else {
        print!("{}", render_local(&sessions));
        println!(
            "\n`istesso sessions stop <session> --local` asks one to close; with --force it \
             is ended."
        );
    }
    Ok(())
}

/// `istesso sessions stop --local`: the person at the host asks an
/// application to close, or ends it, whichever device started it.
pub fn stop_local(session: &str, force: bool) -> Result<(), String> {
    let paths = Paths::from_env();
    let sessions = sessions_here(&paths)?;
    let ids: Vec<&str> = sessions.iter().map(|s| s.info.id.0.as_str()).collect();
    let found =
        &sessions[pick_among(&ids, session, "on this machine", "istesso sessions --local")?];
    let (name, id) = (&found.app_name, &found.info.id);
    if force {
        let grace = Rules::default().app_stop_grace;
        eprintln!(
            "Ending {name} by force. It has {} seconds to end by itself.",
            grace.as_secs()
        );
        let last = stop_here(&paths, id, true, grace + Duration::from_secs(5))?;
        if last.info.state != SessionState::Ended {
            return Err(format!("{name} has not ended yet"));
        }
        println!("{name} was ended.");
    } else {
        eprintln!("Asking {name} to close...");
        let last = stop_here(&paths, id, false, Duration::from_secs(5))?;
        println!("{}", after_asking_here(&last, name));
    }
    Ok(())
}

/// `istesso peers remove`, after the pairing is gone: tells the agent on
/// this machine, so that the device is cut off now, and says what the
/// device had opened here. Those applications are kept; with `close` they
/// are asked to close, which they may answer with a question. The lines
/// to print.
pub fn unpaired(paths: &Paths, device: &str, name: &str, close: bool) -> Vec<String> {
    let left = match after_unpairing(paths, device) {
        // No agent here, so nothing that device started runs here.
        Ok(None) => return Vec::new(),
        Ok(Some(left)) => left,
        Err(why) => {
            return vec![format!(
                "The agent on this machine could not be told ({why}). It reads the list of \
                 paired devices every second, and cuts {name} off then."
            )]
        }
    };
    let mut lines = vec![format!(
        "{name} is cut off from this machine: its conversations are ended, and nothing is \
         shown to it."
    )];
    if left.is_empty() {
        return lines;
    }
    let still = if close {
        // All of them are asked first; then they get a moment together.
        for session in &left {
            let _ = stop_here(paths, &session.info.id, false, Duration::ZERO);
        }
        let until = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let running: Vec<String> = sessions_here(paths)
                .unwrap_or_default()
                .into_iter()
                .map(|session| session.info.id.0)
                .collect();
            let still: Vec<String> = left
                .iter()
                .map(|session| session.info.id.0.clone())
                .filter(|id| running.contains(id))
                .collect();
            if still.is_empty() || std::time::Instant::now() >= until {
                break Some(still);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    } else {
        None
    };
    lines.extend(unpaired_words(name, &left, still.as_deref()));
    lines
}

/// What is said about the applications a device that was unpaired had
/// opened on this machine. `still` is set when they were asked to close:
/// the ids of those that have not.
fn unpaired_words(name: &str, left: &[HostSession], still: Option<&[String]>) -> Vec<String> {
    let app = column("", left.iter().map(|s| s.app_name.as_str()));
    let count = match left.len() {
        1 => "an application".to_string(),
        n => format!("{n} applications"),
    };
    let mut lines = Vec::new();
    match still {
        None => {
            lines.push(format!(
                "It had opened {count} on this machine, which {} running:",
                if left.len() == 1 { "keeps" } else { "keep" }
            ));
            for session in left {
                lines.push(format!("  {} {}", session.info.id, session.app_name));
            }
            lines.push(format!(
                "Devices that may control this machine can show and end them: `istesso \
                 sessions` lists them there as opened by {name}. From here, `istesso sessions \
                 stop <session> --local` asks one to close."
            ));
        }
        Some(still) => {
            lines.push(format!(
                "It had opened {count} on this machine, which {} asked to close:",
                if left.len() == 1 { "was" } else { "were" }
            ));
            for session in left {
                let became = if still.contains(&session.info.id.0) {
                    "still running: it may be asking whether to save"
                } else {
                    "ended"
                };
                lines.push(format!(
                    "  {} {} {became}",
                    session.info.id,
                    crate::pad(&session.app_name, app)
                ));
            }
            if !still.is_empty() {
                lines.push(
                    "What still runs is kept. Devices that may control this machine can show \
                     it, to answer; `istesso sessions stop <session> --local --force` ends it."
                        .to_string(),
                );
            }
        }
    }
    lines
}

/// What became of an application of this machine that was asked to close.
fn after_asking_here(last: &HostSession, name: &str) -> String {
    if last.info.state == SessionState::Ended {
        return format!("{name} ended.");
    }
    format!(
        "{name} is still running: it may be asking whether to save, which a device that is \
         shown it can answer. `istesso sessions stop {} --local --force` ends it.",
        last.info.id
    )
}

/// What became of an application that was asked to close.
fn after_asking(last: &SessionInfo, name: &str, host: &str) -> String {
    if last.state == SessionState::Ended {
        return format!("{name} ended on {host}.");
    }
    format!(
        "{name} is still running on {host}: it may be asking whether to save. \
         `istesso {OPEN} {name}` shows it; `istesso sessions stop {} --force` ends it.",
        last.id
    )
}

#[cfg(test)]
mod tests {
    use istesso_proto::{SessionId, StreamKind};

    use super::*;

    fn running_on(host: &str, app: &str, id: &str, state: SessionState) -> Running {
        Running {
            host_id: format!("id-{host}"),
            host_name: host.to_string(),
            app_name: app.to_string(),
            info: SessionInfo {
                id: SessionId(id.to_string()),
                app: format!("{}.desktop", app.to_lowercase().replace(' ', "-")),
                kind: StreamKind::AppWindow,
                state,
                revision: 2,
                attachment: 1,
                display: None,
                answer: None,
                end: None,
                left_by: None,
            },
        }
    }

    fn some() -> Vec<Running> {
        vec![
            running_on(
                "ubuntu",
                "Blender",
                "5b0c9e3a7f21d684",
                SessionState::Detached,
            ),
            running_on(
                "ubuntu",
                "テキストエディター",
                "5b0c11112222aaaa",
                SessionState::Running,
            ),
            running_on(
                "studio",
                "Logic",
                "9f8e7d6c5b4a3921",
                SessionState::Starting,
            ),
        ]
    }

    #[test]
    fn a_session_is_named_by_its_id_or_the_beginning_of_it() {
        let running = some();
        let pick = |asked: &str| pick(running.iter(), asked).map(|r| r.app_name.clone());
        assert_eq!(pick("9f8e7d6c5b4a3921").unwrap(), "Logic");
        assert_eq!(pick(" 9F8E ").unwrap(), "Logic");
        assert_eq!(pick("5b0c9").unwrap(), "Blender");
        // Too short to mean anything, the beginning of two, or of none.
        assert!(pick("9f8").unwrap_err().contains("no session"));
        assert_eq!(
            pick("9f8").unwrap_err(),
            "no session '9f8' runs for this device; `istesso sessions` lists them"
        );
        assert!(pick("5b0c").unwrap_err().contains("several"));
        assert!(pick("0000").unwrap_err().contains("no session '0000'"));
    }

    #[test]
    fn the_table_lines_up_and_says_what_is_shown() {
        let text = render(&some());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4, "{text}");
        assert!(
            lines[0].starts_with(&format!("{:<16} APPLICATION", "SESSION")),
            "{text}"
        );
        assert!(lines[1].starts_with("5b0c9e3a7f21d684 Blender"), "{text}");
        assert!(lines[1].ends_with("ubuntu running, not shown"), "{text}");
        assert!(lines[2].ends_with("ubuntu shown in a window"), "{text}");
        assert!(lines[3].ends_with("studio starting"), "{text}");
        // The device column starts at the same place in every row, also
        // after a name of double-width characters.
        let column = |line: &str, word: &str| {
            unicode_width::UnicodeWidthStr::width(&line[..line.rfind(word).unwrap()])
        };
        assert_eq!(column(lines[0], "DEVICE"), column(lines[1], "ubuntu"));
        assert_eq!(column(lines[1], "ubuntu"), column(lines[2], "ubuntu"));
        assert_eq!(column(lines[2], "ubuntu"), column(lines[3], "studio"));
    }

    fn here(app: &str, id: &str, by: &str, state: SessionState, revoked: bool) -> HostSession {
        HostSession {
            info: running_on("this", app, id, state).info,
            app_name: app.to_string(),
            started_by: istesso_agent::session::StartedBy {
                id: format!("id-{by}"),
                name: by.to_string(),
            },
            revoked,
        }
    }

    #[test]
    fn the_person_at_the_host_sees_who_started_what() {
        let sessions = [
            here(
                "Blender",
                "5b0c9e3a7f21d684",
                "laptop",
                SessionState::Running,
                false,
            ),
            here(
                "Editor",
                "9f8e7d6c5b4a3921",
                "old tablet",
                SessionState::Detached,
                true,
            ),
        ];
        let text = render_local(&sessions);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3, "{text}");
        assert_eq!(lines[0], "SESSION          APPLICATION STARTED BY STATE");
        assert_eq!(
            lines[1],
            "5b0c9e3a7f21d684 Blender     laptop     shown in a window"
        );
        // A session its device may not use now says so: only from here
        // can it be ended.
        assert_eq!(
            lines[2],
            "9f8e7d6c5b4a3921 Editor      old tablet running, not shown; old tablet may not \
             use it now"
        );
        // One that was kept when its device was unpaired says who opened
        // it, here and in the list a device with the rights gets.
        let mut left = sessions[1].clone();
        left.info.left_by = Some("old tablet".to_string());
        assert_eq!(
            local_state_words(&left),
            "running, not shown; opened by old tablet, which was unpaired"
        );
        let mut remote = some();
        remote[0].info.left_by = Some("old tablet".to_string());
        let listed = render(&remote);
        assert!(
            listed
                .lines()
                .nth(1)
                .unwrap()
                .ends_with("ubuntu running, not shown; opened by old tablet, which was unpaired"),
            "{listed}"
        );

        let ids: Vec<&str> = sessions.iter().map(|s| s.info.id.0.as_str()).collect();
        let pick = |asked| pick_among(&ids, asked, "on this machine", "istesso sessions --local");
        assert_eq!(pick("9f8e"), Ok(1));
        assert_eq!(
            pick("0000").unwrap_err(),
            "no session '0000' runs on this machine; `istesso sessions --local` lists them"
        );

        let still = after_asking_here(&sessions[1], "Editor");
        assert!(
            still.contains("`istesso sessions stop 9f8e7d6c5b4a3921 --local --force`"),
            "{still}"
        );
        let mut over = sessions[0].clone();
        over.info.state = SessionState::Ended;
        assert_eq!(after_asking_here(&over, "Blender"), "Blender ended.");
    }

    #[test]
    fn removing_a_pairing_says_what_the_device_left_running() {
        let left = [
            here(
                "Blender",
                "5b0c9e3a7f21d684",
                "laptop",
                SessionState::Detached,
                true,
            ),
            here(
                "Editor",
                "9f8e7d6c5b4a3921",
                "laptop",
                SessionState::Detached,
                true,
            ),
        ];
        // Kept, which is the default: what runs, who can get to it, and
        // how to close it after all.
        let kept = unpaired_words("laptop", &left, None);
        assert_eq!(
            kept[..3],
            [
                "It had opened 2 applications on this machine, which keep running:",
                "  5b0c9e3a7f21d684 Blender",
                "  9f8e7d6c5b4a3921 Editor",
            ]
        );
        assert!(kept[3].contains("as opened by laptop"), "{kept:?}");
        assert!(
            kept[3].contains("`istesso sessions stop <session> --local` asks one to close"),
            "{kept:?}"
        );
        let one = unpaired_words("laptop", &left[..1], None);
        assert_eq!(
            one[0],
            "It had opened an application on this machine, which keeps running:"
        );

        // Asked to close: one did, one has a question.
        let still = ["9f8e7d6c5b4a3921".to_string()];
        let asked = unpaired_words("laptop", &left, Some(&still));
        assert_eq!(
            asked[..3],
            [
                "It had opened 2 applications on this machine, which were asked to close:",
                "  5b0c9e3a7f21d684 Blender ended",
                "  9f8e7d6c5b4a3921 Editor  still running: it may be asking whether to save",
            ]
        );
        assert!(asked[3].contains("--local --force"), "{asked:?}");
        // All of them did: nothing more to say.
        assert_eq!(unpaired_words("laptop", &left, Some(&[])).len(), 3);
    }

    #[test]
    fn an_application_that_was_asked_to_close_may_still_be_there() {
        let mut last = some().remove(0).info;
        let still = after_asking(&last, "Blender", "ubuntu");
        assert!(still.contains("may be asking whether to save"), "{still}");
        assert!(
            still.contains("`istesso sessions stop 5b0c9e3a7f21d684 --force`"),
            "{still}"
        );
        last.state = SessionState::Ended;
        assert_eq!(
            after_asking(&last, "Blender", "ubuntu"),
            "Blender ended on ubuntu."
        );
    }
}
