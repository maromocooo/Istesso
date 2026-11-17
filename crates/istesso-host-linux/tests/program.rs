//! The program itself, started as it is started, with elements every
//! GStreamer has standing in for the compositor and the encoder. What is
//! tried is everything around them: the two ways of being started, the
//! lines in and out, the input socket, the frame log, the ending.

#![cfg(all(feature = "pipeline", unix))]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use serde_json::Value;

const COMPOSITOR: &str =
    "videotestsrc name=compositor is-live=true ! video/x-raw,width=32,height=32,framerate=100/1";
const ENCODER: &str =
    "identity name=convert ! identity name=enc ! appsink name=net sync=false async=false";

/// A directory of this test's own, with an identity of this test's own in it.
fn place(name: &str) -> PathBuf {
    let directory = std::env::temp_dir().join(format!("ihl-prog-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let paths = istesso_agent::Paths::at(directory.clone());
    istesso_pair::Identity::load_or_create(&paths.identity()).unwrap();
    directory
}

struct Program {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
}

fn start(directory: &Path, arguments: &[&str]) -> Program {
    start_with(directory, arguments, ENCODER, "47990-47999")
}

fn start_with(directory: &Path, arguments: &[&str], encoder: &str, ports: &str) -> Program {
    let mut child = Command::new(env!("CARGO_BIN_EXE_istesso-host-linux"))
        .args(arguments)
        .env("ISTESSO_HOST_COMPOSITOR_SIDE", COMPOSITOR)
        .env("ISTESSO_HOST_ENCODER_SIDE", encoder)
        .env("ISTESSO_HOST_CONFIG_DIR", directory)
        .env("ISTESSO_HOST_PORTS", ports)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (lines_in, lines) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if lines_in.send(line).is_err() {
                break;
            }
        }
    });
    Program {
        stdin: child.stdin.take(),
        child,
        lines,
    }
}

impl Program {
    fn tell(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}").unwrap();
        stdin.flush().unwrap();
    }

    fn line(&self) -> String {
        self.lines
            .recv_timeout(Duration::from_secs(20))
            .expect("the program said nothing more")
    }

    fn json(&self) -> Value {
        serde_json::from_str(&self.line()).unwrap()
    }

    /// Waits for the program to end; whether it ended well, and how long that took.
    fn ended(&mut self) -> (bool, Duration) {
        let began = Instant::now();
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return (status.success(), began.elapsed());
            }
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "the program does not end"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn log_lines(path: &Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn a_measuring_run_takes_the_script_s_arguments_words_and_input_and_writes_its_log() {
    let directory = place("measure");
    let log = directory.join("frames.jsonl");
    let socket = directory.join("i.sock");
    let mut program = start(
        &directory,
        &[
            "measure",
            "--to-channel",
            "127.0.0.1:0",
            "--config-dir",
            directory.to_str().unwrap(),
            "--log",
            log.to_str().unwrap(),
            "--input-socket",
            socket.to_str().unwrap(),
            "--size",
            "32x32",
            "--fps",
            "100",
        ],
    );
    assert_eq!(program.line(), "playing");
    // The socket is there once the program says it plays.
    let agent = UnixDatagram::unbound().unwrap();
    agent
        .send_to(br#"{"kind":"key","code":30,"pressed":true}"#, &socket)
        .unwrap();
    agent
        .send_to(br#"{"kind":"key","code":30,"pressed":false}"#, &socket)
        .unwrap();
    std::thread::sleep(Duration::from_millis(150));
    program.tell("pause");
    let paused = program.json();
    assert_eq!(paused["istesso"], "paused");
    assert_eq!(paused["offered"], true);
    assert!(paused["took_ms"].as_f64().unwrap() < 300.0, "{paused}");
    program.tell("resume");
    assert_eq!(program.json()["istesso"], "resumed");
    program.tell("admit nobody");
    assert_eq!(
        program.json(),
        serde_json::json!({"istesso": "admitted", "device": null})
    );
    // Standard input closing does not end a measuring run; the signal does.
    program.stdin = None;
    std::thread::sleep(Duration::from_millis(100));
    assert!(program.child.try_wait().unwrap().is_none());
    // SAFETY: the process is this test's own child.
    unsafe { libc::kill(program.child.id() as libc::pid_t, libc::SIGTERM) };
    let (well, _) = program.ended();
    assert!(well);

    let lines = log_lines(&log);
    let kinds = |kind: &str| lines.iter().filter(|line| line["kind"] == kind).count();
    assert_eq!(lines[0]["kind"], "clock");
    assert_eq!(lines[1]["kind"], "header");
    assert_eq!(lines[1]["size"], "32x32");
    assert_eq!(lines[1]["pacing"], "on-commit");
    assert_eq!(lines[1]["switches"], serde_json::json!([]));
    assert!(kinds("frame_out") > 10 && kinds("converted") > 10);
    assert!(kinds("encoded") > 10 && kinds("sent") > 10);
    // Only the press is noted, and never which key.
    assert_eq!(kinds("key_in"), 1);
    assert_eq!(kinds("picture"), 2);
    let input = lines.iter().find(|line| line["kind"] == "input").unwrap();
    assert_eq!(input["events"], 2);
    assert_eq!(lines.last().unwrap()["kind"], "clock");
    assert!(!socket.exists());
}

#[test]
fn a_runner_says_ready_answers_the_agent_s_lines_and_ends_when_the_agent_goes() {
    let directory = place("runner");
    let socket = directory.join("input-5b0c.sock");
    let mut program = start(&directory, &[]);
    program.tell(&format!(
        r#"{{"v":1,"cmd":"start","session":"5b0c","kind":"app_window","display":{{"width":32,"height":32,"fps":100}},"peer":{{"id":"1a2b"}},"input_socket":"{}","offer":{{}}}}"#,
        socket.display()
    ));
    let ready = program.json();
    assert_eq!(ready["event"], "ready");
    assert_eq!(ready["v"], 1);
    assert_eq!(ready["display"]["width"], 32);
    assert_eq!(ready["answer"]["transport"], "channel");
    let port = ready["answer"]["channel_port"].as_u64().unwrap();
    assert!((47990..=47999).contains(&port), "{port}");
    assert!(socket.exists());
    // The application is started and has no window yet: a device can say so.
    program.tell(r#"{"v":1,"cmd":"application","pid":1}"#);
    assert_eq!(
        program.json(),
        serde_json::json!({"v": 1, "event": "opening"})
    );
    program.tell(r#"{"v":1,"cmd":"pause"}"#);
    let paused = program.json();
    assert_eq!(paused["event"], "paused");
    assert!(paused["took_ms"].as_f64().unwrap() < 300.0, "{paused}");
    program.tell(r#"{"v":1,"cmd":"resume"}"#);
    assert_eq!(program.json()["event"], "resumed");
    program.tell(r#"{"v":1,"cmd":"later-invention"}"#);
    program.tell(r#"{"v":1,"cmd":"detach"}"#);
    assert_eq!(program.json()["event"], "paused");
    // The agent goes: the runner ends within two seconds, and well.
    program.stdin = None;
    let (well, took) = program.ended();
    assert!(well);
    assert!(took < Duration::from_secs(2), "{took:?}");
}

#[test]
fn a_runner_whose_encoder_cannot_be_had_says_ready_and_then_no_encoder() {
    let directory = place("refused");
    let socket = directory.join("input.sock");
    let mut program = start_with(
        &directory,
        &[],
        "identity name=convert ! istessonosuchencoder name=enc ! appsink name=net",
        "47980-47989",
    );
    program.tell(&format!(
        r#"{{"v":1,"cmd":"start","session":"s","display":{{"width":32,"height":32,"fps":100}},"input_socket":"{}"}}"#,
        socket.display()
    ));
    assert_eq!(program.json()["event"], "ready");
    let refused = program.json();
    assert_eq!(refused["event"], "no_encoder");
    assert!(refused["message"]
        .as_str()
        .unwrap()
        .contains("the application goes on"));
    // It has not ended, and ends well when the agent goes.
    assert!(program.child.try_wait().unwrap().is_none());
    program.stdin = None;
    assert!(program.ended().0);
}

#[test]
fn a_device_without_an_identity_and_a_wrong_start_are_said_and_end_badly() {
    let directory = std::env::temp_dir().join(format!("ihl-prog-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    let mut program = start(
        &directory,
        &[
            "measure",
            "--to-channel",
            "127.0.0.1:0",
            "--config-dir",
            directory.to_str().unwrap(),
            "--log",
            "-",
            "--no-input",
        ],
    );
    assert!(!program.ended().0);
    let mut program = start(&directory, &["dance"]);
    assert!(!program.ended().0);
}
