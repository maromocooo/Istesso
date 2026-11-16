//! `istesso open`: start an application, wherever it is installed.
//!
//! An application of this machine is started here, with nothing in between.
//! One that only another device has is started there and shown in a window
//! here (`docs/adr/0010-session-start.md`): the command then stays for as
//! long as the window does.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use istesso_agent::session::client::{
    link_to, open_window, ClientRules, CommandViewer, OpenError, Opening, Outcome,
};
use istesso_agent::{survey, HostView, Paths, SurveyOptions};
use istesso_link::ErrorCode;
use istesso_proto::{
    AppEntry, AppInstance, Badge, EndReason, HostApps, HostState, Launch, SessionInfo,
    SessionState, StreamKind, UnifiedApp, ViewerWish,
};

/// What the command is called. Written down once: the name is still open
/// with the person who uses it (ADR 0010).
pub const COMMAND: &str = "open";

/// What was typed after `istesso open`.
pub struct Options {
    pub app: String,
    pub host: Option<String>,
    pub viewer: Option<PathBuf>,
    pub size: Option<(u32, u32)>,
    pub fps: Option<u32>,
    pub position: Option<(i32, i32)>,
}

pub fn run(options: Options) -> Result<(), String> {
    let paths = Paths::from_env();
    let survey_options = SurveyOptions {
        local_apps: true,
        ..SurveyOptions::default()
    };
    let views = survey(&paths, &survey_options)?;
    let hosts: Vec<HostApps> = views.iter().map(HostView::to_host_apps).collect();
    let catalogue = istesso_catalog::merge(&hosts);

    let app = find_app(&catalogue, &options.app)?;
    let instance = choose_instance(app, options.host.as_deref())?;
    if instance.local {
        open_here(&instance.app)?;
        println!("Opened {} on this device.", app.name);
        return Ok(());
    }
    open_there(&paths, app, instance, &options)
}

/// `WIDTHxHEIGHT`, in pixels.
pub fn parse_size(text: &str) -> Result<(u32, u32), String> {
    let parsed = text
        .split_once(['x', 'X'])
        .and_then(|(width, height)| Some((width.parse().ok()?, height.parse().ok()?)));
    match parsed {
        Some((width, height)) if width > 0 && height > 0 => Ok((width, height)),
        _ => Err("give a size as WIDTHxHEIGHT, for example 1720x720".to_string()),
    }
}

/// `X,Y`, in points from the top left of the main display.
pub fn parse_position(text: &str) -> Result<(i32, i32), String> {
    text.split_once(',')
        .and_then(|(x, y)| Some((x.trim().parse().ok()?, y.trim().parse().ok()?)))
        .ok_or_else(|| "give a position as X,Y, for example 100,50".to_string())
}

/// The application the person means: by its name as the catalogue shows
/// it, by the id it has on a device, or by a part of its name that fits
/// only one.
fn find_app<'a>(catalogue: &'a [UnifiedApp], asked: &str) -> Result<&'a UnifiedApp, String> {
    let key = istesso_catalog::match_key(asked);
    if key.is_empty() {
        return Err("give the name of an application".to_string());
    }
    let list = |apps: &[&UnifiedApp]| -> String {
        let mut names: Vec<&str> = apps.iter().take(8).map(|app| app.name.as_str()).collect();
        if apps.len() > names.len() {
            names.push("...");
        }
        names.join(", ")
    };

    let exact: Vec<&UnifiedApp> = catalogue
        .iter()
        .filter(|app| {
            app.key == key
                || app.name.eq_ignore_ascii_case(asked.trim())
                || app.instances.iter().any(|i| i.app.id == asked.trim())
        })
        .collect();
    let found = if exact.is_empty() {
        catalogue
            .iter()
            .filter(|app| {
                app.key.contains(&key) || istesso_catalog::match_key(&app.name).contains(&key)
            })
            .collect()
    } else {
        exact
    };
    match found.as_slice() {
        [] => Err(format!(
            "no application matches '{asked}'; `istesso apps list --all` shows what there is"
        )),
        &[app] => Ok(app),
        several => Err(format!(
            "'{asked}' matches several applications: {}; say which one",
            list(several)
        )),
    }
}

/// Where the application runs: on the device the person named, or else on
/// the one the catalogue prefers (this one, then one that answers, then the
/// least busy one).
fn choose_instance<'a>(app: &'a UnifiedApp, host: Option<&str>) -> Result<&'a AppInstance, String> {
    let label = |i: &AppInstance| match (i.local, i.host_state) {
        (true, _) => "this device".to_string(),
        (false, HostState::Awake) => i.host_name.clone(),
        (false, _) => format!("{} (no answer)", i.host_name),
    };
    let on: Vec<String> = app.instances.iter().map(label).collect();

    let Some(host) = host else {
        return match &app.preferred_host {
            Some(id) => app
                .instances
                .iter()
                .find(|i| &i.host_id == id)
                .ok_or_else(|| format!("{} is in the catalogue without a device", app.name)),
            None => {
                let wake = matches!(
                    app.badge,
                    Some(Badge::HostStopped {
                        wake_possible: true
                    })
                );
                Err(format!(
                    "{} is only on devices that do not answer: {}{}; `istesso hosts` shows why",
                    app.name,
                    on.join(", "),
                    if wake {
                        " (one of them could be woken; that is not built yet)"
                    } else {
                        ""
                    }
                ))
            }
        };
    };
    let instance = app
        .instances
        .iter()
        .find(|i| i.host_id == host || i.host_name.eq_ignore_ascii_case(host))
        .ok_or_else(|| {
            format!(
                "{} is not installed on '{host}'; it is on: {}",
                app.name,
                on.join(", ")
            )
        })?;
    if !instance.local && instance.host_state != HostState::Awake {
        return Err(format!(
            "{} does not answer; `istesso hosts` shows why",
            instance.host_name
        ));
    }
    Ok(instance)
}

/// Starts an application of this machine and leaves it to itself.
fn open_here(app: &AppEntry) -> Result<(), String> {
    match &app.launch {
        Launch::AppBundle { bundle_id, .. } => {
            let opened = Command::new("open")
                .arg("-b")
                .arg(bundle_id)
                .status()
                .map_err(|e| format!("cannot run `open`: {e}"))?;
            if !opened.success() {
                return Err(format!("{} could not be opened", app.name));
            }
        }
        Launch::DesktopEntry { .. } => {
            let plan = istesso_catalog::launch::plan(app).map_err(|why| why.to_string())?;
            let mut command = Command::new(&plan.program);
            command
                .args(&plan.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(dir) = &plan.working_dir {
                command.current_dir(dir);
            }
            // Not part of this command's group: Ctrl-C in the terminal is
            // not meant for the application.
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            command
                .spawn()
                .map_err(|e| format!("{} could not be started: {e}", app.name))?;
        }
    }
    Ok(())
}

/// Starts the application on the device that has it and shows it here,
/// until the window is closed, the application ends, or the person stops
/// the command.
fn open_there(
    paths: &Paths,
    app: &UnifiedApp,
    instance: &AppInstance,
    options: &Options,
) -> Result<(), String> {
    let (name, host) = (&app.name, &instance.host_name);
    let viewer = options
        .viewer
        .clone()
        .or_else(|| {
            std::env::var_os("ISTESSO_VIEWER")
                .filter(|program| !program.is_empty())
                .map(PathBuf::from)
        })
        .ok_or_else(|| {
            format!(
                "{name} is on {host}, and nothing on this device can show it yet: name the \
                 program that shows the picture with --viewer or ISTESSO_VIEWER (the media \
                 path is not part of this build)"
            )
        })?;
    let viewer = CommandViewer::new(viewer, Arc::new(|line| eprintln!("{line}")));
    let link = link_to(paths, &instance.host_id, Duration::from_millis(800))?;
    let opening = Opening {
        host: instance.host_id.clone(),
        app: instance.app.id.clone(),
        title: name.clone(),
        kind: StreamKind::AppWindow,
        wish: ViewerWish {
            width: options.size.map(|(width, _)| width),
            height: options.size.map(|(_, height)| height),
            fps: options.fps,
            x: options.position.map(|(x, _)| x),
            y: options.position.map(|(_, y)| y),
        },
    };

    let interrupted = Arc::new(AtomicBool::new(false));
    stop_on_signals(&interrupted);
    let mut progress = |info: &SessionInfo| match info.state {
        SessionState::Starting => {
            eprintln!("Starting {name} on {host} (session {})...", info.id)
        }
        SessionState::Running => {
            eprintln!("{name} is running on {host}. Close the window or press Ctrl-C to end it.")
        }
        _ => {}
    };
    let outcome = open_window(
        link,
        &viewer,
        &opening,
        &interrupted,
        &ClientRules::default(),
        &mut progress,
    )
    .map_err(|refusal| explain(&refusal, name, host))?;
    eprintln!("{}", conclude(outcome, name, host)?);
    Ok(())
}

/// Ctrl-C, a closed terminal and a request to end all mean "stop": the
/// host is told, instead of being left to find out.
fn stop_on_signals(interrupted: &Arc<AtomicBool>) {
    #[cfg(unix)]
    for signal in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        let _ = signal_hook::flag::register(signal, interrupted.clone());
    }
    #[cfg(not(unix))]
    let _ = interrupted;
}

/// Why no window was opened, for the person who asked.
fn explain(refusal: &OpenError, name: &str, host: &str) -> String {
    match refusal {
        OpenError::Refused(refusal) => match refusal.code {
            ErrorCode::Unsupported => format!(
                "the istesso on {host} is too old to start applications for other devices; \
                 update it"
            ),
            ErrorCode::Forbidden => format!(
                "{host} does not let this device start its applications ({})",
                refusal.message
            ),
            _ => format!("{host} cannot show {name}: {}", refusal.message),
        },
        OpenError::Unreachable(why) => format!("{host} cannot be reached: {why}"),
        OpenError::Viewer(why) => why.clone(),
    }
}

/// What to say when the window is gone: a line for a normal end, an error
/// for everything else.
fn conclude(outcome: Outcome, name: &str, host: &str) -> Result<String, String> {
    match outcome {
        Outcome::Closed => Ok(format!("Closed {name}; it was ended on {host}.")),
        Outcome::Interrupted => Ok(format!("Stopped {name} on {host}.")),
        Outcome::Ended(end) => match end.reason {
            EndReason::Stopped => Ok(format!("{name} was stopped on {host}.")),
            EndReason::AppExited => Ok(format!("{name} ended on {host}: {}.", end.message)),
            _ if end.message.is_empty() => Err(format!("{name} on {host} ended unexpectedly")),
            _ => Err(format!("{name} on {host} ended: {}", end.message)),
        },
        Outcome::HostLost(why) => Err(format!("lost {host}: {why}")),
        Outcome::ViewerFailed(why) => Err(format!("{why}; {name} was ended on {host}")),
    }
}

#[cfg(test)]
mod tests {
    use istesso_link::RemoteError;
    use istesso_proto::{HostRef, OsFamily, SessionEnd};

    use super::*;

    fn installed(name: &str) -> AppEntry {
        let id = format!("{}.desktop", name.to_lowercase().replace(' ', "-"));
        AppEntry {
            id: id.clone(),
            name: name.to_string(),
            platform: OsFamily::Linux,
            version: None,
            launch: Launch::DesktopEntry {
                desktop_id: id,
                exec: name.to_lowercase(),
            },
            icon: None,
            categories: Vec::new(),
        }
    }

    fn host(name: &str, local: bool, state: HostState, apps: &[&str]) -> HostApps {
        HostApps {
            host: HostRef {
                id: format!("id-{name}"),
                name: name.to_string(),
                state,
                local,
                wake_possible: name == "studio",
                load: 0.1,
            },
            apps: apps.iter().map(|app| installed(app)).collect(),
        }
    }

    fn catalogue() -> Vec<UnifiedApp> {
        istesso_catalog::merge(&[
            host("air", true, HostState::Awake, &["Firefox", "Notes"]),
            host(
                "ubuntu",
                false,
                HostState::Awake,
                &[
                    "Firefox Web Browser",
                    "Blender",
                    "Text Editor",
                    "Hex Editor",
                ],
            ),
            host("studio", false, HostState::Unknown, &["Blender", "Logic"]),
        ])
    }

    #[test]
    fn an_application_is_found_by_its_name_or_a_part_of_it() {
        let catalogue = catalogue();
        let name = |asked: &str| find_app(&catalogue, asked).map(|app| app.name.clone());
        assert_eq!(name("Blender").unwrap(), "Blender");
        assert_eq!(name("  blender ").unwrap(), "Blender");
        // A part that fits only one.
        assert_eq!(name("blen").unwrap(), "Blender");
        assert_eq!(name("hex").unwrap(), "Hex Editor");
        // The id it has on a device.
        assert_eq!(name("text-editor.desktop").unwrap(), "Text Editor");
        // The name another device has for the same application.
        assert_eq!(name("Firefox Web Browser").unwrap(), "Firefox");
        // An exact name wins over names that contain it.
        assert_eq!(name("notes").unwrap(), "Notes");

        let several = name("editor").unwrap_err();
        assert!(several.contains("Hex Editor, Text Editor"), "{several}");
        let none = name("emacs").unwrap_err();
        assert!(none.contains("no application matches 'emacs'"), "{none}");
        assert!(name(" - ").is_err());
    }

    #[test]
    fn the_device_is_chosen_like_the_catalogue_says_unless_named() {
        let catalogue = catalogue();
        let on = |app: &str, host: Option<&str>| {
            let app = find_app(&catalogue, app).unwrap();
            choose_instance(app, host).map(|i| (i.host_name.clone(), i.local))
        };
        // Here before anywhere else: no streaming at all.
        assert_eq!(on("Firefox", None).unwrap(), ("air".to_string(), true));
        // Only elsewhere: the device that answers.
        assert_eq!(on("Blender", None).unwrap(), ("ubuntu".to_string(), false));
        // Named by the person, by name or by id.
        assert_eq!(on("Firefox", Some("Ubuntu")).unwrap().0, "ubuntu");
        assert_eq!(on("Firefox", Some("id-ubuntu")).unwrap().0, "ubuntu");

        let wrong = on("Notes", Some("ubuntu")).unwrap_err();
        assert!(wrong.contains("not installed on 'ubuntu'"), "{wrong}");
        assert!(wrong.contains("this device"), "{wrong}");
        let silent = on("Blender", Some("studio")).unwrap_err();
        assert!(silent.contains("studio does not answer"), "{silent}");
        // Every device that has it is stopped.
        let stopped = on("Logic", None).unwrap_err();
        assert!(stopped.contains("studio (no answer)"), "{stopped}");
        assert!(stopped.contains("could be woken"), "{stopped}");
    }

    #[test]
    fn sizes_and_positions_are_read() {
        assert_eq!(parse_size("1720x720"), Ok((1720, 720)));
        assert_eq!(parse_size("3440X1440"), Ok((3440, 1440)));
        for bad in ["1720", "0x720", "x", "1720x", "axb", "-1x5"] {
            assert!(parse_size(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_position("100,50"), Ok((100, 50)));
        assert_eq!(parse_position("-1720, 0"), Ok((-1720, 0)));
        assert!(parse_position("100").is_err());
        assert!(parse_position("a,b").is_err());
    }

    #[test]
    fn refusals_say_what_to_do() {
        let refused = |code, message: &str| {
            let refusal = OpenError::Refused(RemoteError {
                code,
                message: message.to_string(),
            });
            explain(&refusal, "Blender", "ubuntu")
        };
        let old = refused(
            ErrorCode::Unsupported,
            "this version does not know that request",
        );
        assert!(old.contains("too old") && old.contains("ubuntu"), "{old}");
        let forbidden = refused(
            ErrorCode::Forbidden,
            "this device may not start applications",
        );
        assert!(
            forbidden.contains("does not let this device"),
            "{forbidden}"
        );
        assert_eq!(
            refused(
                ErrorCode::Unavailable,
                "this host has no session runner set up"
            ),
            "ubuntu cannot show Blender: this host has no session runner set up"
        );
        assert_eq!(
            explain(
                &OpenError::Unreachable("timed out".into()),
                "Blender",
                "ubuntu"
            ),
            "ubuntu cannot be reached: timed out"
        );
    }

    #[test]
    fn an_ordinary_end_is_a_line_and_anything_else_an_error() {
        let ended = |reason, message: &str| {
            conclude(
                Outcome::Ended(SessionEnd {
                    reason,
                    message: message.to_string(),
                }),
                "Blender",
                "ubuntu",
            )
        };
        assert!(conclude(Outcome::Closed, "Blender", "ubuntu").is_ok());
        assert!(conclude(Outcome::Interrupted, "Blender", "ubuntu").is_ok());
        assert_eq!(
            ended(
                EndReason::AppExited,
                "the application ended (exit status 0)"
            ),
            Ok("Blender ended on ubuntu: the application ended (exit status 0).".to_string())
        );
        assert!(ended(EndReason::Stopped, "").is_ok());
        assert_eq!(
            ended(EndReason::StreamFailed, "no encoder"),
            Err("Blender on ubuntu ended: no encoder".to_string())
        );
        assert!(ended(EndReason::Revoked, "").is_err());
        assert!(conclude(Outcome::HostLost("no answer".into()), "Blender", "ubuntu").is_err());
        assert!(conclude(
            Outcome::ViewerFailed("it broke".into()),
            "Blender",
            "ubuntu"
        )
        .is_err());
    }
}
