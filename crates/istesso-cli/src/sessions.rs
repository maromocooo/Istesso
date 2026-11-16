//! `istesso sessions`: the applications of other devices that run for this
//! one, shown in a window or not, and how to end one
//! (`docs/adr/0010-session-start.md`).

use std::time::Duration;

use istesso_agent::session::client::{link_to, sessions_on, stop_session, ClientRules, OpenError};
use istesso_agent::session::Rules;
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
            Ok(sessions) => all.extend(sessions.into_iter().map(|info| {
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

/// The session the person means: by its id, or by the beginning of it.
pub fn pick<'a>(
    running: impl Iterator<Item = &'a Running>,
    asked: &str,
) -> Result<&'a Running, String> {
    let asked = asked.trim().to_lowercase();
    let all: Vec<&Running> = running.collect();
    if let Some(exact) = all.iter().copied().find(|r| r.info.id.0 == asked) {
        return Ok(exact);
    }
    let begins: Vec<&Running> = all
        .into_iter()
        .filter(|r| asked.len() >= 4 && r.info.id.0.starts_with(&asked))
        .collect();
    match begins.as_slice() {
        &[one] => Ok(one),
        [] => Err(format!(
            "no session '{asked}' runs for this device; `istesso sessions` lists them"
        )),
        _ => Err(format!(
            "'{asked}' is the beginning of several sessions; give more of the id"
        )),
    }
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
            state_words(r.info.state)
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
