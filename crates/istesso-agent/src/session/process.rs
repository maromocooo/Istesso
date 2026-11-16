//! The real things a session is made of: programs.
//!
//! A runner and a viewer are programs that are told what to do, and say what
//! they did, one JSON object per line ([`LineProcess`]). An application is a
//! program that is started in a process group of its own, so that everything
//! it starts can be ended with it.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use istesso_catalog::launch::LaunchPlan;
use istesso_proto::session::{decode_line, encode_line};
use istesso_proto::{AppEntry, RunnerCommand, RunnerEvent};
use serde::de::DeserializeOwned;
use serde::Serialize;

use super::{Launcher, Runner, RunningApp, RunningStream, StreamEvent};
use crate::serve::Log;

/// How often a program that is expected to end is looked at.
const LOOK_EVERY: Duration = Duration::from_millis(10);

/// A program that speaks one of the two line protocols.
pub(crate) struct LineProcess<E> {
    child: Child,
    /// Open for as long as the program is wanted. Closing it is how the
    /// program is asked to end.
    stdin: Option<ChildStdin>,
    lines: Receiver<E>,
}

/// What such a program did.
pub(crate) enum Said<E> {
    Line(E),
    Exited(ExitStatus),
}

impl<E: DeserializeOwned + Send + 'static> LineProcess<E> {
    /// Starts `program` without arguments. What it writes to its standard
    /// error goes to `log`, line by line, with `label` in front.
    pub(crate) fn spawn(program: &Path, label: String, log: Log) -> std::io::Result<Self> {
        let mut tries = 0;
        let mut child = loop {
            let spawned = Command::new(program)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn();
            match spawned {
                Ok(child) => break child,
                // A program that was written a moment ago can still be held
                // open: by whoever installs it, or, for an instant, by
                // another process this one is starting. That passes.
                Err(busy) if busy.kind() == ErrorKind::ExecutableFileBusy && tries < 10 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(other) => return Err(other),
            }
        };
        let stdin = child.stdin.take();
        let (sender, lines) = mpsc::channel();
        if let Some(stdout) = child.stdout.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    // Lines that are not understood are skipped: the
                    // program may say more than this version knows.
                    if let Some(event) = decode_line::<E>(&line) {
                        if sender.send(event).is_err() {
                            break;
                        }
                    }
                }
            });
        }
        if let Some(stderr) = child.stderr.take() {
            std::thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    log(format!("{label}: {line}"));
                }
            });
        }
        Ok(LineProcess {
            child,
            stdin,
            lines,
        })
    }

    /// Writes one line to the program.
    pub(crate) fn tell<C: Serialize>(&mut self, command: &C) -> std::io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        let mut line = encode_line(command);
        line.push('\n');
        stdin.write_all(line.as_bytes())?;
        stdin.flush()
    }

    /// The next thing the program did, without waiting.
    pub(crate) fn poll(&mut self) -> Option<Said<E>> {
        if let Ok(event) = self.lines.try_recv() {
            return Some(Said::Line(event));
        }
        let status = self.child.try_wait().ok().flatten()?;
        // What it said last may still be on its way.
        match self.lines.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => Some(Said::Line(event)),
            Err(_) => Some(Said::Exited(status)),
        }
    }

    /// Ends the program: by closing its input, which is the agreed way to
    /// ask; by telling it to end when it has not after `grace`; by force
    /// when it still has not after `grace` again.
    pub(crate) fn stop(&mut self, grace: Duration) {
        drop(self.stdin.take());
        if !self.ends_within(grace) {
            terminate(&self.child);
            if !self.ends_within(grace) {
                let _ = self.child.kill();
            }
        }
        let _ = self.child.wait();
    }

    fn ends_within(&mut self, time: Duration) -> bool {
        let deadline = Instant::now() + time;
        loop {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(LOOK_EVERY);
        }
    }
}

impl<E> Drop for LineProcess<E> {
    /// A program that was not stopped is not left behind.
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(unix)]
fn terminate(child: &Child) {
    use rustix::process::{kill_process, Pid, Signal};
    let _ = kill_process(Pid::from_child(child), Signal::TERM);
}

#[cfg(not(unix))]
fn terminate(_child: &Child) {}

/// How a program ended, for a person.
pub(crate) fn how_it_ended(status: ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    match status.code() {
        Some(code) => format!("exit status {code}"),
        None => "ended".to_string(),
    }
}

// ---------------------------------------------------------------------------
// The runner
// ---------------------------------------------------------------------------

/// Starts the host's side of a stream as a program: the one the person who
/// runs the agent named. It gets no arguments; see the ADR for what it is
/// told and what it says.
pub struct CommandRunner {
    program: PathBuf,
    log: Log,
}

impl CommandRunner {
    pub fn new(program: impl Into<PathBuf>, log: Log) -> Self {
        CommandRunner {
            program: program.into(),
            log,
        }
    }
}

impl Runner for CommandRunner {
    fn start(&self, start: &RunnerCommand) -> Result<Box<dyn RunningStream>, String> {
        let label = match start {
            RunnerCommand::Start { session, .. } => format!("session {session}: runner"),
            _ => "runner".to_string(),
        };
        let mut process = LineProcess::spawn(&self.program, label, self.log.clone())
            .map_err(|e| format!("the host's session runner could not be started ({e})"))?;
        process.tell(start).map_err(|e| {
            format!("the host's session runner did not take its instructions ({e})")
        })?;
        Ok(Box::new(ProcessStream(process)))
    }
}

struct ProcessStream(LineProcess<RunnerEvent>);

impl RunningStream for ProcessStream {
    fn poll(&mut self) -> Option<StreamEvent> {
        self.0.poll().map(|said| match said {
            Said::Line(event) => StreamEvent::Said(event),
            Said::Exited(status) => StreamEvent::Exited(how_it_ended(status)),
        })
    }

    fn tell(&mut self, command: &RunnerCommand) -> Result<(), String> {
        self.0
            .tell(command)
            .map_err(|e| format!("the host's session runner does not listen any more ({e})"))
    }

    fn stop(&mut self, grace: Duration) {
        self.0.stop(grace);
    }
}

// ---------------------------------------------------------------------------
// The application
// ---------------------------------------------------------------------------

/// Variables that make the common toolkits open their window on a Wayland
/// display. A first list: to be settled on a real host (ADR 0010).
const WAYLAND_HINTS: [(&str, &str); 5] = [
    ("GDK_BACKEND", "wayland"),
    ("QT_QPA_PLATFORM", "wayland"),
    ("SDL_VIDEODRIVER", "wayland"),
    ("MOZ_ENABLE_WAYLAND", "1"),
    ("ELECTRON_OZONE_PLATFORM_HINT", "wayland"),
];

/// What changes in an application's environment, compared with the
/// agent's own: variables to set and variables to take away.
///
/// What the runner gave is set. When the runner offers a Wayland display
/// and no X11 one, `DISPLAY` is taken away, so that the window cannot open
/// on the host's own desktop without anyone noticing, and the toolkits are
/// pointed at Wayland.
pub fn app_environment(
    from_runner: &BTreeMap<String, String>,
) -> (BTreeMap<String, String>, Vec<&'static str>) {
    let mut set = BTreeMap::new();
    let mut unset = Vec::new();
    if from_runner.contains_key("WAYLAND_DISPLAY") && !from_runner.contains_key("DISPLAY") {
        unset.push("DISPLAY");
        for (name, value) in WAYLAND_HINTS {
            set.insert(name.to_string(), value.to_string());
        }
    }
    // The runner knows its display best: what it says wins.
    set.extend(from_runner.clone());
    (set, unset)
}

/// Starts the applications of this machine's catalogue as programs.
pub struct DesktopLauncher;

impl Launcher for DesktopLauncher {
    fn plan(&self, app: &AppEntry) -> Result<LaunchPlan, String> {
        istesso_catalog::launch::plan(app).map_err(|why| why.to_string())
    }

    #[cfg(unix)]
    fn launch(
        &self,
        plan: &LaunchPlan,
        env: &BTreeMap<String, String>,
    ) -> Result<Box<dyn RunningApp>, String> {
        let app = unix::ProcessApp::start(plan, env).map_err(|e| e.to_string())?;
        Ok(Box::new(app))
    }

    #[cfg(not(unix))]
    fn launch(
        &self,
        _plan: &LaunchPlan,
        _env: &BTreeMap<String, String>,
    ) -> Result<Box<dyn RunningApp>, String> {
        Err("starting applications for another device is not built for this system yet".into())
    }
}

#[cfg(unix)]
mod unix {
    use std::os::unix::process::CommandExt;

    use rustix::io::Errno;
    use rustix::process::{kill_process_group, test_kill_process_group, Pid, Signal};

    use super::*;

    /// An application, and everything it started, as one process group.
    pub(super) struct ProcessApp {
        child: Child,
        group: Pid,
        status: Option<ExitStatus>,
        /// The whole group is gone. Its number may be given to others
        /// from now on, so nothing is sent to it any more.
        gone: bool,
    }

    impl ProcessApp {
        pub(super) fn start(
            plan: &LaunchPlan,
            env: &BTreeMap<String, String>,
        ) -> std::io::Result<Self> {
            let mut command = Command::new(&plan.program);
            command
                .args(&plan.args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(dir) = &plan.working_dir {
                command.current_dir(dir);
            }
            let (set, unset) = app_environment(env);
            for name in unset {
                command.env_remove(name);
            }
            command.envs(set);
            // A group of its own, led by the application: what it starts
            // stays in the group and can be found and ended with it.
            command.process_group(0);
            let child = command.spawn()?;
            let group = Pid::from_child(&child);
            Ok(ProcessApp {
                child,
                group,
                status: None,
                gone: false,
            })
        }
    }

    impl RunningApp for ProcessApp {
        fn pid(&self) -> Option<u32> {
            Some(self.child.id())
        }

        fn poll(&mut self) -> Option<String> {
            if self.status.is_none() {
                self.status = self.child.try_wait().ok().flatten();
            }
            let status = self.status?;
            // The program that was started is gone. What it started may
            // go on: a script that hands over to the real program, say.
            if !self.gone {
                self.gone = test_kill_process_group(self.group) == Err(Errno::SRCH);
            }
            self.gone.then(|| how_it_ended(status))
        }

        /// Ends the application by force: tells it to end (SIGTERM), and
        /// ends it (SIGKILL) when it has not after `grace`. Asking it to
        /// close, which it may answer with a question, is not done here:
        /// that goes through the runner, to the application's windows.
        fn stop(&mut self, grace: Duration) {
            if self.poll().is_some() {
                return;
            }
            let _ = kill_process_group(self.group, Signal::TERM);
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline {
                if self.poll().is_some() {
                    return;
                }
                std::thread::sleep(LOOK_EVERY);
            }
            let _ = kill_process_group(self.group, Signal::KILL);
            let _ = self.child.wait();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    use istesso_proto::{DisplayMode, SessionId, SessionPeer, StreamKind};

    use super::*;
    use crate::testing::TempDir;

    fn script(dir: &TempDir, name: &str, body: &str) -> PathBuf {
        let path = dir.paths().config.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn start_line() -> RunnerCommand {
        RunnerCommand::Start {
            session: SessionId("5b0c9e3a7f21d684".into()),
            kind: StreamKind::AppWindow,
            display: DisplayMode {
                width: 1280,
                height: 720,
                fps: 60,
            },
            peer: SessionPeer {
                id: "1a2b-3c4d-5e6f-7a8b".into(),
                address: "192.0.2.10".into(),
            },
            input_socket: "/tmp/istesso-test/input.sock".into(),
            offer: serde_json::json!({"port": 50004}),
        }
    }

    /// Asks until the stream did something, for at most five seconds.
    fn next(stream: &mut Box<dyn RunningStream>) -> StreamEvent {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = stream.poll() {
                return event;
            }
            assert!(Instant::now() < deadline, "the runner said nothing");
            std::thread::sleep(LOOK_EVERY);
        }
    }

    fn silent() -> Log {
        Arc::new(|_| {})
    }

    #[test]
    fn a_runner_is_told_in_one_line_and_answers_in_lines() {
        let dir = TempDir::new("runner-lines");
        let told = dir.paths().config.join("told.txt");
        let lines = Arc::new(Mutex::new(Vec::new()));
        let log: Log = {
            let lines = lines.clone();
            Arc::new(move |line| lines.lock().unwrap().push(line))
        };
        // Keeps what it is told, chatters, says it is ready, and waits for
        // its input to close.
        let runner = script(
            &dir,
            "runner.sh",
            &format!(
                r#"IFS= read -r start
printf '%s\n' "$start" > '{}'
echo 'warming up' >&2
echo 'this is not a line of the protocol'
echo '{{"v":1,"event":"progress","percent":50}}'
echo '{{"v":1,"event":"ready","env":{{"WAYLAND_DISPLAY":"wayland-7"}},"display":{{"width":1280,"height":720,"fps":60}},"answer":{{"port":50005}}}}'
cat > /dev/null
exit 0"#,
                told.display()
            ),
        );

        let mut stream = CommandRunner::new(&runner, log)
            .start(&start_line())
            .unwrap();
        // Lines it does not know are skipped; the one it knows arrives.
        assert_eq!(
            next(&mut stream),
            StreamEvent::Said(RunnerEvent::Ready {
                env: BTreeMap::from([("WAYLAND_DISPLAY".into(), "wayland-7".into())]),
                display: DisplayMode {
                    width: 1280,
                    height: 720,
                    fps: 60
                },
                answer: serde_json::json!({"port": 50005}),
            })
        );
        // It was told everything in one line, the place for input included.
        let line = std::fs::read_to_string(&told).unwrap();
        assert_eq!(line.lines().count(), 1);
        assert_eq!(decode_line::<RunnerCommand>(&line), Some(start_line()));
        assert!(line.contains(r#""input_socket":"/tmp/istesso-test/input.sock""#));

        // It runs until its input is closed, and then ends by itself.
        assert_eq!(stream.poll(), None);
        let asked = Instant::now();
        stream.stop(Duration::from_secs(5));
        assert!(asked.elapsed() < Duration::from_secs(2));
        // What it wrote for people went to the log, marked as its own.
        let deadline = Instant::now() + Duration::from_secs(5);
        while lines.lock().unwrap().is_empty() && Instant::now() < deadline {
            std::thread::sleep(LOOK_EVERY);
        }
        assert_eq!(
            *lines.lock().unwrap(),
            ["session 5b0c9e3a7f21d684: runner: warming up"]
        );
    }

    #[test]
    fn a_runner_that_ends_or_cannot_start_is_noticed() {
        let dir = TempDir::new("runner-ends");
        let runner = script(
            &dir,
            "runner.sh",
            r#"read -r start
echo '{"v":1,"event":"error","message":"no encoder"}'
exit 3"#,
        );
        let mut stream = CommandRunner::new(&runner, silent())
            .start(&start_line())
            .unwrap();
        assert_eq!(
            next(&mut stream),
            StreamEvent::Said(RunnerEvent::Error {
                message: "no encoder".into()
            })
        );
        assert_eq!(
            next(&mut stream),
            StreamEvent::Exited("exit status 3".into())
        );
        stream.stop(Duration::from_millis(100));

        let missing = CommandRunner::new(dir.paths().config.join("nothing-here"), silent());
        let refusal = missing.start(&start_line()).err().unwrap();
        assert!(refusal.contains("could not be started"), "{refusal}");
    }

    #[test]
    fn a_runner_that_does_not_listen_is_ended_by_force() {
        let dir = TempDir::new("runner-deaf");
        // Ignores its input being closed and being told to end.
        let runner = script(
            &dir,
            "runner.sh",
            r#"trap '' TERM
read -r start
echo '{"v":1,"event":"empty"}'
while :; do sleep 1; done"#,
        );
        let mut stream = CommandRunner::new(&runner, silent())
            .start(&start_line())
            .unwrap();
        assert_eq!(next(&mut stream), StreamEvent::Said(RunnerEvent::Empty));
        let asked = Instant::now();
        stream.stop(Duration::from_millis(200));
        let took = asked.elapsed();
        assert!(took >= Duration::from_millis(400), "{took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        assert!(matches!(stream.poll(), Some(StreamEvent::Exited(how)) if how == "signal 9"));
    }

    fn sh(script: &str) -> LaunchPlan {
        LaunchPlan {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            working_dir: None,
        }
    }

    fn wait_until_gone(app: &mut Box<dyn RunningApp>) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(how) = app.poll() {
                return how;
            }
            assert!(Instant::now() < deadline, "the application did not end");
            std::thread::sleep(LOOK_EVERY);
        }
    }

    #[test]
    fn an_application_gets_the_runners_display_and_loses_the_hosts() {
        let dir = TempDir::new("app-env");
        let out = dir.paths().config.join("env.txt");
        let env = BTreeMap::from([
            ("WAYLAND_DISPLAY".to_string(), "wayland-7".to_string()),
            ("ISTESSO_TEST_OUT".to_string(), out.display().to_string()),
        ]);
        let plan = LaunchPlan {
            working_dir: Some(dir.paths().config),
            ..sh(
                r#"printf '%s|%s|%s|%s' "$WAYLAND_DISPLAY" "${DISPLAY-unset}" "$GDK_BACKEND" "$PWD" > "$ISTESSO_TEST_OUT""#,
            )
        };
        let mut app = DesktopLauncher.launch(&plan, &env).unwrap();
        assert_eq!(wait_until_gone(&mut app), "exit status 0");
        let seen = std::fs::read_to_string(&out).unwrap();
        let seen: Vec<&str> = seen.split('|').collect();
        assert_eq!(seen[..3], ["wayland-7", "unset", "wayland"]);
        // Started where its entry says.
        assert_eq!(
            std::fs::canonicalize(seen[3]).unwrap(),
            std::fs::canonicalize(dir.paths().config).unwrap()
        );

        // A runner that also offers X11 keeps it, and decides the rest.
        let both = BTreeMap::from([
            ("WAYLAND_DISPLAY".to_string(), "wayland-7".to_string()),
            ("DISPLAY".to_string(), ":7".to_string()),
        ]);
        let (set, unset) = app_environment(&both);
        assert!(unset.is_empty());
        assert_eq!(set, both);
        let (set, unset) = app_environment(&BTreeMap::from([
            ("WAYLAND_DISPLAY".to_string(), "wayland-7".to_string()),
            ("GDK_BACKEND".to_string(), "wayland,x11".to_string()),
        ]));
        assert_eq!(unset, ["DISPLAY"]);
        assert_eq!(set["GDK_BACKEND"], "wayland,x11");
        assert_eq!(set["QT_QPA_PLATFORM"], "wayland");
        // Nothing about displays: nothing is touched.
        assert_eq!(app_environment(&BTreeMap::new()), (BTreeMap::new(), vec![]));
    }

    #[test]
    fn an_application_is_over_when_everything_it_started_is() {
        // The program that was started hands over to another and ends, as
        // launcher scripts do. The application is still running.
        let mut app = DesktopLauncher
            .launch(&sh("sleep 30 & exit 0"), &BTreeMap::new())
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(app.poll(), None);
        // Stopping reaches what was left behind.
        let asked = Instant::now();
        app.stop(Duration::from_secs(5));
        assert!(asked.elapsed() < Duration::from_secs(2));
        assert_eq!(app.poll().as_deref(), Some("exit status 0"));
    }

    #[test]
    fn an_application_that_ignores_the_request_is_ended() {
        let dir = TempDir::new("app-deaf");
        let ready = dir.paths().config.join("ready");
        let env = BTreeMap::from([(
            "ISTESSO_TEST_READY".to_string(),
            ready.display().to_string(),
        )]);
        let mut app = DesktopLauncher
            .launch(
                &sh(r#"trap '' TERM; : > "$ISTESSO_TEST_READY"; while :; do sleep 1; done"#),
                &env,
            )
            .unwrap();
        // Not before it has said that it will not listen.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(Instant::now() < deadline, "the application did not start");
            std::thread::sleep(LOOK_EVERY);
        }
        let asked = Instant::now();
        app.stop(Duration::from_millis(300));
        let took = asked.elapsed();
        assert!(took >= Duration::from_millis(300), "{took:?}");
        assert!(took < Duration::from_secs(3), "{took:?}");
        assert_eq!(wait_until_gone(&mut app), "signal 9");

        // A program that is not there.
        let missing = LaunchPlan {
            program: "/nonexistent/istesso-test-program".into(),
            args: Vec::new(),
            working_dir: None,
        };
        assert!(DesktopLauncher.launch(&missing, &BTreeMap::new()).is_err());
    }
}
