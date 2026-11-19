//! A host's agent on the loopback that opens shells, and a device that asks
//! for them. The shell is `/bin/sh`, not a login shell: the tests never run
//! the login files of whoever runs them.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::shell::{ShellEnd, ShellFrame};
use istesso_link::{ChannelWriter, Client, ErrorCode, LocalKeys, ShellAnswer, ShellRequest};
use istesso_pair::{Hello, Identity, Peer, PeerStore, Permission};

use crate::testing::{manifest, TempDir};

const SOON: Duration = Duration::from_secs(5);

/// A host whose agent may open shells for the device, or not.
struct ShellPair {
    host: TempDir,
    device: TempDir,
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
}

impl ShellPair {
    fn new(tag: &str, allow: bool) -> Self {
        let (host, device) = (TempDir::new(tag), TempDir::new(tag));
        let identities =
            [&host, &device].map(|dir| Identity::load_or_create(&dir.paths().identity()).unwrap());
        for (dir, name, other) in [
            (&host, "a laptop", &identities[1]),
            (&device, "a host", &identities[0]),
        ] {
            PeerStore::open(dir.paths().peers())
                .unwrap()
                .add(Peer::from_hello(&Hello {
                    device_name: name.to_string(),
                    public_key: other.public_key(),
                }))
                .unwrap();
        }
        crate::cache::remember_local_manifest(&host.paths(), &manifest("a host"));
        let log = Arc::new(Mutex::new(Vec::new()));
        let lines = log.clone();
        let options = crate::AgentOptions {
            bind: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
            discovery: false,
            refresh_peers: None,
            shell_for: if allow {
                vec!["a laptop".to_string()]
            } else {
                Vec::new()
            },
            shell_program: Some("/bin/sh".into()),
            workspace_scopes: false,
            open_requests: false,
            ..crate::AgentOptions::default()
        };
        let agent = crate::Agent::bind(
            host.paths(),
            &options,
            Arc::new(move |line| lines.lock().unwrap().push(line)),
        )
        .unwrap();
        let addr = agent.local_addr().unwrap();
        std::thread::spawn(move || agent.run());
        ShellPair {
            host,
            device,
            addr,
            log,
        }
    }

    fn connect(&self) -> Client {
        let paths = self.device.paths();
        let identity = Identity::load_or_create(&paths.identity()).unwrap();
        let keys = LocalKeys::new(&identity).unwrap();
        let peers = PeerStore::open(paths.peers()).unwrap();
        Client::connect(self.addr, &keys, &peers, None, SOON).unwrap()
    }

    fn ask(&self, request: ShellRequest) -> (Client, ShellAnswer) {
        let mut client = self.connect();
        let answer = client.open_shell(request).unwrap();
        (client, answer)
    }

    fn open(&self, request: ShellRequest) -> Terminal {
        let (client, answer) = self.ask(request);
        assert_eq!(answer, ShellAnswer::Open);
        Terminal::new(client)
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    /// The process number of the shell the agent said it opened last.
    fn shell_pid(&self) -> i32 {
        let line = self
            .log()
            .into_iter()
            .rev()
            .find(|line| line.starts_with("opened a shell"))
            .expect("no shell was opened");
        let after = line.split("(process ").nth(1).unwrap();
        after.split(')').next().unwrap().parse().unwrap()
    }

    /// Waits until the agent has said that a shell is over.
    fn over(&self) -> String {
        let began = Instant::now();
        loop {
            if let Some(line) = self.log().into_iter().find(|line| line.contains("is over")) {
                return line;
            }
            assert!(began.elapsed() < SOON, "the shell was not ended");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn request(cols: u16, rows: u16) -> ShellRequest {
    ShellRequest {
        cols,
        rows,
        term: None,
        locale: None,
        project: None,
        forward: false,
    }
}

/// What the device has heard: what the shell printed so far, how it ended,
/// and whether the host has closed the connection.
type Screen = (Vec<u8>, Option<ShellEnd>, bool);

/// The device's side of an open shell.
struct Terminal {
    writer: Option<ChannelWriter>,
    screen: Arc<Mutex<Screen>>,
}

impl Terminal {
    fn new(client: Client) -> Self {
        let (mut reader, writer) = client.into_channel().split().unwrap();
        let screen = Arc::new(Mutex::new((Vec::new(), None, false)));
        let shared = screen.clone();
        std::thread::spawn(move || loop {
            let Ok(message) = reader.recv() else {
                shared.lock().unwrap().2 = true;
                return;
            };
            match ShellFrame::decode(&message).unwrap() {
                ShellFrame::Data(bytes) => shared.lock().unwrap().0.extend_from_slice(&bytes),
                ShellFrame::Exit(end) => shared.lock().unwrap().1 = Some(end),
                _ => {}
            }
        });
        Terminal {
            writer: Some(writer),
            screen,
        }
    }

    fn send(&mut self, frame: ShellFrame) {
        self.writer.as_mut().unwrap().send(&frame.encode()).unwrap();
    }

    fn type_in(&mut self, text: &str) {
        self.send(ShellFrame::Data(text.as_bytes().to_vec()));
    }

    fn printed(&self) -> String {
        String::from_utf8_lossy(&self.screen.lock().unwrap().0).into_owned()
    }

    /// Waits until the shell has printed `text`.
    fn shows(&self, text: &str) -> bool {
        let began = Instant::now();
        while began.elapsed() < SOON {
            if self.printed().contains(text) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Waits until the host has said how the shell ended.
    fn ended(&self) -> Option<ShellEnd> {
        let began = Instant::now();
        while began.elapsed() < SOON {
            if let Some(end) = self.screen.lock().unwrap().1 {
                return Some(end);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    fn closed(&self) -> bool {
        self.screen.lock().unwrap().2
    }

    fn hang_up(&mut self) {
        if let Some(writer) = self.writer.take() {
            writer.shut_down();
        }
    }
}

fn process_is_gone(pid: i32) -> bool {
    let began = Instant::now();
    while began.elapsed() < SOON {
        // SAFETY: signal 0 only asks whether the process exists.
        if unsafe { libc::kill(pid, 0) } != 0 {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn a_shell_runs_commands_knows_its_size_and_returns_their_status() {
    let pair = ShellPair::new("sh-run", true);
    let mut terminal = pair.open(request(100, 30));
    // The quotes keep the echo of what was typed apart from what the command
    // printed.
    terminal.type_in("echo o''k; stty size; exit 3\n");
    assert!(terminal.shows("ok"), "{}", terminal.printed());
    assert!(terminal.shows("30 100"), "{}", terminal.printed());
    assert_eq!(terminal.ended(), Some(ShellEnd::Code(3)));
    assert!(pair.over().contains("exited with status 3"));
    let pid = pair.shell_pid();
    assert!(process_is_gone(pid));
}

#[test]
fn a_new_size_reaches_the_programs_in_the_shell() {
    let pair = ShellPair::new("sh-size", true);
    let mut terminal = pair.open(request(80, 24));
    terminal.send(ShellFrame::Resize {
        cols: 132,
        rows: 43,
    });
    terminal.type_in("stty size; echo do''ne\n");
    assert!(terminal.shows("done"), "{}", terminal.printed());
    assert!(
        terminal.printed().contains("43 132"),
        "{}",
        terminal.printed()
    );
    // Sizes that a terminal cannot have are kept within bounds.
    terminal.send(ShellFrame::Resize { cols: 0, rows: 0 });
    terminal.type_in("stty size; echo ag''ain\n");
    assert!(terminal.shows("again"), "{}", terminal.printed());
    assert!(terminal.printed().contains("1 1"), "{}", terminal.printed());
    terminal.type_in("exit\n");
    assert_eq!(terminal.ended(), Some(ShellEnd::Code(0)));
    assert!(pair.over().contains("2 new sizes"));
}

#[test]
fn japanese_text_and_control_keys_pass_as_bytes() {
    // As for an agent started in the background of a script: Control-C is
    // ignored in the agent's process, and must not be in the shell's.
    // SAFETY: changes how this test process takes SIGINT, until the end of
    // the test; nothing in the tests sends it.
    let before = unsafe { libc::signal(libc::SIGINT, libc::SIG_IGN) };
    let pair = ShellPair::new("sh-bytes", true);
    let mut terminal = pair.open(request(80, 24));
    // UTF-8 goes both ways untouched.
    terminal.type_in("printf '[%s]\\n' 日本語のテキスト\n");
    assert!(
        terminal.shows("[日本語のテキスト]"),
        "{}",
        terminal.printed()
    );
    // Control-C interrupts the program in the foreground, not the shell.
    terminal.type_in("sleep 30\n");
    std::thread::sleep(Duration::from_millis(300));
    terminal.type_in("\u{3}");
    terminal.type_in("echo c-$((3+4))\n");
    assert!(terminal.shows("c-7"), "{}", terminal.printed());
    // Control-Z stops it, and the shell takes the next command.
    terminal.type_in("sleep 30\n");
    std::thread::sleep(Duration::from_millis(300));
    terminal.type_in("\u{1a}");
    terminal.type_in("jobs; kill -9 %1; wait; echo z-$((2+3))\n");
    assert!(terminal.shows("z-5"), "{}", terminal.printed());
    // Control-D at an empty line ends the shell. (dash may first say that there
    // are stopped jobs, before it has noticed that the job is gone; a second
    // Control-D ends it then.)
    terminal.type_in("\u{4}");
    std::thread::sleep(Duration::from_millis(500));
    if terminal.printed().contains("stopped jobs") {
        terminal.type_in("\u{4}");
    }
    assert_eq!(
        terminal.ended(),
        Some(ShellEnd::Code(0)),
        "{}",
        terminal.printed()
    );
    // SAFETY: puts back what was there before the test.
    unsafe { libc::signal(libc::SIGINT, before) };
}

#[test]
fn a_shell_ended_by_a_signal_says_so() {
    let pair = ShellPair::new("sh-signal", true);
    let mut terminal = pair.open(request(80, 24));
    terminal.type_in("kill -9 $$\n");
    assert_eq!(terminal.ended(), Some(ShellEnd::Signal(9)));
    assert_eq!(ShellEnd::Signal(9).exit_status(), 137);
}

#[test]
fn the_shell_is_hung_up_when_the_device_goes() {
    let pair = ShellPair::new("sh-gone", true);
    let mut terminal = pair.open(request(80, 24));
    terminal.type_in("echo st''arted\n");
    assert!(terminal.shows("started"));
    let pid = pair.shell_pid();
    terminal.hang_up();
    assert!(process_is_gone(pid), "the shell is still running");
    assert!(pair.over().contains("the device went away"));
}

#[test]
fn the_shell_starts_in_the_project_folder() {
    let pair = ShellPair::new("sh-project", true);
    let project = TempDir::new("project");
    let folder = project.path().canonicalize().unwrap();
    let mut terminal = pair.open(ShellRequest {
        project: Some(folder.display().to_string()),
        ..request(80, 24)
    });
    terminal.type_in("pwd -P; exit\n");
    assert_eq!(terminal.ended(), Some(ShellEnd::Code(0)));
    assert!(
        terminal.printed().contains(&folder.display().to_string()),
        "{}",
        terminal.printed()
    );
    assert!(pair
        .log()
        .iter()
        .any(|line| line.contains("in a project folder")));

    // A folder that is not there is refused, and no shell is started.
    let (_client, answer) = pair.ask(ShellRequest {
        project: Some("/no/such/folder/here".into()),
        ..request(80, 24)
    });
    assert!(matches!(
        answer,
        ShellAnswer::Refused {
            code: ErrorCode::Internal,
            ..
        }
    ));
    let (_client, answer) = pair.ask(ShellRequest {
        project: Some("relative/folder".into()),
        ..request(80, 24)
    });
    assert!(matches!(answer, ShellAnswer::Refused { .. }));
}

#[test]
fn a_device_the_host_did_not_name_gets_no_shell() {
    let pair = ShellPair::new("sh-off", false);
    let (_client, answer) = pair.ask(request(80, 24));
    assert!(
        matches!(
            answer,
            ShellAnswer::Refused {
                code: ErrorCode::Forbidden,
                ..
            }
        ),
        "{answer:?}"
    );
    assert!(!pair
        .log()
        .iter()
        .any(|line| line.contains("opened a shell")));
}

#[test]
fn a_device_that_may_not_control_the_host_gets_no_shell() {
    let pair = ShellPair::new("sh-view", true);
    PeerStore::open(pair.host.paths().peers())
        .unwrap()
        .set_permission("a laptop", Permission::Control, false)
        .unwrap();
    let (_client, answer) = pair.ask(request(80, 24));
    assert!(matches!(
        answer,
        ShellAnswer::Refused {
            code: ErrorCode::Forbidden,
            ..
        }
    ));
}

#[test]
fn losing_the_right_ends_the_shell_within_a_second_or_two() {
    let pair = ShellPair::new("sh-revoke", true);
    let mut terminal = pair.open(request(80, 24));
    terminal.type_in("echo st''arted\n");
    assert!(terminal.shows("started"));
    let pid = pair.shell_pid();
    let began = Instant::now();
    PeerStore::open(pair.host.paths().peers())
        .unwrap()
        .set_permission("a laptop", Permission::Control, false)
        .unwrap();
    assert_eq!(terminal.ended(), Some(ShellEnd::Revoked));
    assert!(
        began.elapsed() < Duration::from_millis(2500),
        "{:?}",
        began.elapsed()
    );
    assert!(process_is_gone(pid), "the shell is still running");
    let began = Instant::now();
    while !terminal.closed() {
        assert!(began.elapsed() < SOON, "the connection was left open");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pair.over().contains("lost its right"));
}

#[test]
fn nothing_typed_or_printed_is_written_down() {
    let pair = ShellPair::new("sh-quiet", true);
    let mut terminal = pair.open(ShellRequest {
        project: Some("/".into()),
        ..request(80, 24)
    });
    terminal.type_in("echo typed-$((40+2)); exit\n");
    assert!(terminal.shows("typed-42"));
    assert_eq!(terminal.ended(), Some(ShellEnd::Code(0)));
    pair.over();
    let log = pair.log();
    assert_eq!(log.len(), 2, "{log:?}");
    for line in log {
        assert!(!line.contains("typed"), "{line}");
        assert!(!line.contains("42"), "{line}");
        assert!(!line.contains("echo"), "{line}");
    }
}

#[test]
fn a_device_that_sends_an_oversized_frame_is_cut_off() {
    let pair = ShellPair::new("sh-huge", true);
    let mut terminal = pair.open(request(80, 24));
    let pid = pair.shell_pid();
    let mut huge = vec![0x01];
    huge.resize(2 + istesso_link::shell::MAX_DATA_ACCEPTED, b'a');
    terminal.writer.as_mut().unwrap().send(&huge).unwrap();
    assert!(process_is_gone(pid), "the shell is still running");
    assert!(pair.over().contains("the device went away"));
}
