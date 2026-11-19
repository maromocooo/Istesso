//! A host's agent on the loopback that opens shells whose ports are
//! forwarded, and a device that forwards them. The shell is `/bin/sh`, the
//! workspace is told apart by its session (no scope is added to the service
//! manager of whoever runs the tests), and pages are "opened" by a script
//! that writes their address into a file.

#[cfg(target_os = "linux")]
use std::io::{Read, Write};
use std::net::SocketAddr;
#[cfg(target_os = "linux")]
use std::net::TcpStream;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::forward::PortList;
use istesso_link::shell::ShellFrame;
use istesso_link::{
    ChannelWriter, Client, ErrorCode, ForwardAnswer, LocalKeys, ShellAnswer, ShellRequest,
};
use istesso_pair::{Hello, Identity, Peer, PeerStore};

use crate::testing::{manifest, TempDir};

const SOON: Duration = Duration::from_secs(10);

struct Pair {
    _host: TempDir,
    device: TempDir,
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
}

impl Pair {
    fn new(tag: &str) -> Self {
        Pair::with(tag, |_| {})
    }

    /// As [`Pair::new`], with the host's agent's options changed by `change`.
    fn with(tag: &str, change: impl FnOnce(&mut crate::AgentOptions)) -> Self {
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
        let mut options = crate::AgentOptions {
            bind: std::net::Ipv4Addr::LOCALHOST.into(),
            port: 0,
            discovery: false,
            refresh_peers: None,
            shell_for: vec!["a laptop".to_string()],
            shell_program: Some("/bin/sh".into()),
            workspace_scopes: false,
            open_requests: false,
            ..crate::AgentOptions::default()
        };
        change(&mut options);
        let agent = crate::Agent::bind(
            host.paths(),
            &options,
            Arc::new(move |line| lines.lock().unwrap().push(line)),
        )
        .unwrap();
        let addr = agent.local_addr().unwrap();
        std::thread::spawn(move || agent.run());
        Pair {
            _host: host,
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

    /// Opens a shell, with or without forwards, in `project`.
    fn open(&self, forward: bool, project: Option<&std::path::Path>) -> Shell {
        self.open_with(ShellRequest {
            cols: 80,
            rows: 24,
            term: None,
            locale: None,
            project: project.map(|p| p.display().to_string()),
            forward,
            windows: false,
        })
    }

    /// Opens a shell as `request` asks.
    fn open_with(&self, request: ShellRequest) -> Shell {
        let mut client = self.connect();
        let peer_id = client.peer().id.clone();
        let answer = client.open_shell(request).unwrap();
        assert_eq!(answer, ShellAnswer::Open);
        let (mut reader, writer) = client.into_channel().split().unwrap();
        let (frames_in, frames) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(message) = reader.recv() {
                let Ok(frame) = ShellFrame::decode(&message) else {
                    return;
                };
                if !matches!(frame, ShellFrame::Data(_)) && frames_in.send(frame).is_err() {
                    return;
                }
            }
        });
        Shell {
            writer,
            frames,
            peer_id,
        }
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Shell {
    writer: ChannelWriter,
    frames: mpsc::Receiver<ShellFrame>,
    peer_id: String,
}

impl Shell {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn type_in(&mut self, text: &str) {
        self.writer
            .send(&ShellFrame::Data(text.as_bytes().to_vec()).encode())
            .unwrap();
    }

    /// The next window the host opened for the shell.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn window(&self) -> istesso_link::shell::WindowOpen {
        let began = Instant::now();
        while let Some(left) = SOON.checked_sub(began.elapsed()) {
            match self.frames.recv_timeout(left) {
                Ok(ShellFrame::OpenWindow(window)) => return window,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("no window was opened");
    }

    /// The next list of ports for which `wanted` holds.
    fn ports(&self, wanted: impl Fn(&PortList) -> bool) -> PortList {
        let began = Instant::now();
        while let Some(left) = SOON.checked_sub(began.elapsed()) {
            match self.frames.recv_timeout(left) {
                Ok(ShellFrame::Ports(list)) if wanted(&list) => return list,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("no such list of ports came");
    }
}

#[test]
fn the_ports_are_listed_on_the_shells_conversation_only_when_asked_for() {
    let pair = Pair::new("fwd-list");
    let shell = pair.open(true, None);
    let list = shell.ports(|_| true);
    if cfg!(target_os = "linux") {
        assert!(list.watching);
        assert_eq!(list.how.as_deref(), Some("session"));
    } else {
        assert!(!list.watching);
        assert!(list.reason.is_some());
    }
    assert!(pair
        .log()
        .iter()
        .any(|line| line.starts_with("opened a shell") && line.contains("ports")));

    let plain = pair.open(false, None);
    assert!(matches!(
        plain.frames.recv_timeout(Duration::from_millis(1500)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
}

#[test]
fn an_id_that_was_never_listed_is_refused_like_any_other() {
    let pair = Pair::new("fwd-unknown");
    let _shell = pair.open(true, None);
    for id in ["0123456789abcdef0123456789abcdef", "../../etc", ""] {
        let mut client = pair.connect();
        match client.open_forward(id).unwrap() {
            ForwardAnswer::Refused { code, .. } => assert_eq!(code, ErrorCode::Forbidden),
            other => panic!("unexpected: {other:?}"),
        }
    }
}

/// A web server in the shell, reached through the forward from a "browser"
/// on this machine: requests for localhost go through, a forged `Host`
/// does not, a page on the forwarded port is opened at the port here, and
/// the stop switch closes it.
#[cfg(target_os = "linux")]
#[test]
fn a_server_started_in_the_shell_is_reached_through_the_forward() {
    use crate::workspace::device::{self, Forwarder, Settings};
    use std::os::unix::fs::PermissionsExt;

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no python3 here");
        return;
    }
    let pair = Pair::new("fwd-e2e");
    let project = TempDir::new("fwd-project");
    std::fs::write(project.path().join("hello.txt"), "hello from the host\n").unwrap();
    let mut shell = pair.open(true, Some(project.path()));
    shell.type_in("exec python3 -m http.server --bind 127.0.0.1 0 >/dev/null 2>&1\n");
    let list = shell.ports(|list| !list.ports.is_empty());
    let there = list.ports[0].port;
    let id = list.ports[0].id.clone();
    assert_eq!(list.ports[0].process, "python3");
    assert!(pair
        .log()
        .iter()
        .any(|line| line.contains(&format!("listens on port {there}"))));

    // The device's side, with a stand-in for the browser's opener.
    let opened = project.path().join("opened.txt");
    let opener = project.path().join("opener.sh");
    std::fs::write(
        &opener,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
            opened.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&opener, std::fs::Permissions::from_mode(0o700)).unwrap();
    let notices = Arc::new(Mutex::new(Vec::<String>::new()));
    let heard = notices.clone();
    let forwarder = Forwarder::new(
        &pair.device.paths(),
        "a host",
        &shell.peer_id,
        pair.addr,
        Settings {
            enabled: true,
            pins: Vec::new(),
            open_outside: false,
            opener: Some(opener),
        },
        Arc::new(move |line: &str| heard.lock().unwrap().push(line.to_string())),
    )
    .unwrap();
    forwarder.update(list.clone());
    let shown = device::list(&pair.device.paths()).unwrap();
    let entry = &shown
        .iter()
        .flat_map(|shell| &shell.forwards)
        .find(|forward| forward.there == there)
        .unwrap();
    let here = entry.here;
    assert!(notices
        .lock()
        .unwrap()
        .iter()
        .any(|line| line.contains(&format!("is at localhost:{here}"))));

    let ask = |host: &str| {
        let mut browser = TcpStream::connect(("127.0.0.1", here)).unwrap();
        browser.set_read_timeout(Some(SOON)).unwrap();
        write!(
            browser,
            "GET /hello.txt HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut answer = Vec::new();
        let _ = browser.read_to_end(&mut answer);
        String::from_utf8_lossy(&answer).into_owned()
    };
    let answer = ask(&format!("localhost:{here}"));
    assert!(answer.starts_with("HTTP/1.0 200"), "{answer}");
    assert!(answer.contains("hello from the host"));
    // DNS rebinding: a page whose own name was resolved to 127.0.0.1.
    let answer = ask(&format!("rebind.example:{here}"));
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    assert!(!answer.contains("hello from the host"));
    // The host writes the line when the connection is over, which may be
    // a moment after the answer has come here.
    let forwarded = format!("forwarded connection to port {there}");
    let began = Instant::now();
    while !pair.log().iter().any(|line| line.contains(&forwarded)) {
        assert!(began.elapsed() < SOON, "{:?}", pair.log());
        std::thread::sleep(Duration::from_millis(20));
    }
    // The host's log has numbers, never what was carried.
    assert!(!pair.log().iter().any(|line| line.contains("hello")));

    // Pages: on the forwarded port, opened at the port here; anything else
    // not opened.
    forwarder.open_url(&format!("http://localhost:{there}/hello.txt?q=1"));
    forwarder.open_url("file:///etc/passwd");
    forwarder.open_url("http://localhost:1/");
    forwarder.open_url("https://outside.example/path?secret=1");
    let began = Instant::now();
    let recorded = loop {
        let recorded = std::fs::read_to_string(&opened).unwrap_or_default();
        if !recorded.is_empty() || began.elapsed() > SOON {
            break recorded;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    std::thread::sleep(Duration::from_millis(300));
    let recorded = std::fs::read_to_string(&opened).unwrap_or(recorded);
    assert_eq!(recorded, format!("http://localhost:{here}/hello.txt?q=1\n"));
    let said = notices.lock().unwrap().join("\n");
    assert!(said.contains("https://outside.example"));
    assert!(!said.contains("secret"));

    // The stop switch: the port closes here and stays closed.
    assert_eq!(
        device::stop_forwards(&pair.device.paths(), Some(here)).unwrap(),
        1
    );
    let began = Instant::now();
    while TcpStream::connect(("127.0.0.1", here)).is_ok() {
        assert!(began.elapsed() < SOON, "still listening");
        std::thread::sleep(Duration::from_millis(50));
    }
    forwarder.update(list);
    let shown = device::list(&pair.device.paths()).unwrap();
    assert!(shown.iter().all(|shell| shell.forwards.is_empty()));
    assert!(shown.iter().any(|shell| shell.stopped.contains(&there)));

    // The shell ends (Control-C to the server it became): its forwards go.
    shell.type_in("\u{3}");
    let began = Instant::now();
    loop {
        let mut client = pair.connect();
        if matches!(client.open_forward(&id), Ok(ForwardAnswer::Refused { .. })) {
            break;
        }
        assert!(began.elapsed() < SOON, "the forward outlived its shell");
        std::thread::sleep(Duration::from_millis(100));
    }
    drop(forwarder);
    std::fs::remove_dir(crate::workspace::private_dir(&pair.device.paths()).unwrap()).unwrap();
}

/// Asks for the forward `id` and sends `request` on it; what comes back.
#[cfg(target_os = "linux")]
fn through_the_forward(pair: &Pair, id: &str, request: &[u8]) -> Result<Vec<u8>, String> {
    use istesso_link::forward::ForwardFrame;
    let mut client = pair.connect();
    match client.open_forward(id).unwrap() {
        ForwardAnswer::Open => {}
        ForwardAnswer::Refused { message, .. } => return Err(message),
    }
    let mut channel = client.into_channel();
    channel.send(&ForwardFrame::data(request)).unwrap();
    channel.send(&ForwardFrame::End.encode()).unwrap();
    let mut answer = Vec::new();
    while let Ok(message) = channel.recv() {
        match ForwardFrame::decode(&message).unwrap() {
            ForwardFrame::Data(bytes) => answer.extend_from_slice(&bytes),
            ForwardFrame::End => break,
            ForwardFrame::Unknown(_) => {}
        }
    }
    Ok(answer)
}

/// A server of the workspace on 127.0.0.2, and another program on
/// 127.0.0.1 with the same port: the forward reaches the workspace's
/// server, never the other program.
#[cfg(target_os = "linux")]
#[test]
fn the_forward_goes_to_the_address_the_server_listens_on() {
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no python3 here");
        return;
    }
    let pair = Pair::new("fwd-exact");
    let project = TempDir::new("fwd-exact-project");
    std::fs::write(project.path().join("hello.txt"), "hello from the host\n").unwrap();
    let mut shell = pair.open(true, Some(project.path()));
    shell.type_in("exec python3 -m http.server --bind 127.0.0.2 0 >/dev/null 2>&1\n");
    let list = shell.ports(|list| !list.ports.is_empty());
    let (port, id) = (list.ports[0].port, list.ports[0].id.clone());
    // Someone else on 127.0.0.1 with the same number.
    let impostor = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
    std::thread::spawn(move || {
        for stream in impostor.incoming().flatten() {
            let mut stream = stream;
            let _ = stream.write_all(b"HTTP/1.0 200 OK\r\n\r\nimpostor");
        }
    });
    let answer = through_the_forward(&pair, &id, b"GET /hello.txt HTTP/1.0\r\n\r\n").unwrap();
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.contains("hello from the host"), "{answer}");
    assert!(!answer.contains("impostor"));
}

/// A connection through a forward ends when the socket it was made to
/// stops listening, although its own connection to the server stays open.
#[cfg(target_os = "linux")]
#[test]
fn a_forwarded_connection_ends_when_its_port_is_no_longer_listed() {
    use istesso_link::forward::ForwardFrame;
    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no python3 here");
        return;
    }
    let pair = Pair::new("fwd-retired");
    let mut shell = pair.open(true, None);
    // Takes one connection, then (a moment later) stops listening and keeps
    // that connection.
    shell.type_in(
        "exec python3 -c 'import socket,time; s=socket.socket(); s.bind((\"127.0.0.1\",0)); s.listen(); c,_=s.accept(); time.sleep(1); s.close(); time.sleep(30)'\n",
    );
    let list = shell.ports(|list| !list.ports.is_empty());
    let id = list.ports[0].id.clone();
    let mut client = pair.connect();
    assert_eq!(client.open_forward(&id).unwrap(), ForwardAnswer::Open);
    let mut channel = client.into_channel();
    shell.ports(|list| list.ports.is_empty());
    let began = Instant::now();
    while let Ok(Ok(ForwardFrame::Data(_) | ForwardFrame::Unknown(_))) =
        channel.recv().map(|m| ForwardFrame::decode(&m))
    {}
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "{:?}",
        began.elapsed()
    );
    // The host says why, once its side is over too.
    let said = |line: &String| line.contains("the shell or the port is gone");
    while !pair.log().iter().any(said) {
        assert!(began.elapsed() < Duration::from_secs(5), "{:?}", pair.log());
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A file of the shell's project folder, asked for by a program inside the
/// shell as `istesso-edit` asks, is handed to the host's main editor (a
/// stand-in that writes down what it was given), and the device is told on
/// which ports the window waits. A file outside, a link, a climb out, a
/// shell whose device shows no windows: refused, and the editor is not
/// started. Nothing of the places is written down.
#[cfg(target_os = "linux")]
#[test]
fn a_project_file_opens_in_the_main_editor_and_the_device_is_told_where_its_window_is() {
    use istesso_link::shell::WindowOpen;
    use std::os::unix::fs::PermissionsExt;

    if std::process::Command::new("python3")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: no python3 here");
        return;
    }
    let tools = TempDir::new("ed-tools");
    let launched = tools.path().join("launched.txt");
    let launcher = tools.path().join("launcher.sh");
    std::fs::write(
        &launcher,
        format!(
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$#\" \"$1\" \"$EDITOR_WINDOW_PLACE\" >> '{}'\n[ \"$1\" = stop ] && exit 0\necho 'window 26891 26892 Text Editor'\n",
            launched.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
    // What `istesso-edit` sends, from a program in the shell's session.
    let ask = tools.path().join("ask.py");
    std::fs::write(
        &ask,
        "import os, socket, sys\n\
         s = socket.socket(socket.AF_UNIX)\n\
         s.connect(os.environ['ISTESSO_OPEN_SOCKET'])\n\
         s.sendall(('file ' + sys.argv[1] + '\\n').encode())\n\
         print(s.makefile().readline().strip(), flush=True)\n",
    )
    .unwrap();
    let pair = Pair::with("ed-e2e", |options| {
        options.open_requests = true;
        options.main_editor = Some(launcher.clone());
    });
    let project = TempDir::new("ed-project");
    let folder = project.path().canonicalize().unwrap();
    std::fs::write(folder.join("notes.txt"), "x").unwrap();
    let outside = TempDir::new("ed-outside");
    let secret = outside.path().canonicalize().unwrap().join("secret.txt");
    std::fs::write(&secret, "x").unwrap();
    std::os::unix::fs::symlink(&secret, folder.join("link.txt")).unwrap();
    let request = ShellRequest {
        cols: 80,
        rows: 24,
        term: None,
        locale: None,
        project: Some(folder.display().to_string()),
        forward: true,
        windows: true,
    };
    let mut shell = pair.open_with(request.clone());
    let (answers, others) = (
        tools.path().join("answers.txt"),
        tools.path().join("others.txt"),
    );
    let asking_into = |file: &str, into: &std::path::Path| {
        format!(
            "python3 '{}' {file} >> '{}'\n",
            ask.display(),
            into.display()
        )
    };
    let asking = |file: &str| asking_into(file, &answers);
    shell.type_in(&asking("\"$PWD/notes.txt\""));
    assert_eq!(
        shell.window(),
        WindowOpen {
            picture: 26891,
            input: 26892,
            title: "Text Editor".into(),
            asked_by_program: false,
        }
    );
    // The same file asked for by a program in the shell (a script here), not
    // typed at its prompt: the device is told so.
    let script = tools.path().join("via-program.sh");
    std::fs::write(
        &script,
        format!(
            "python3 '{}' \"$1\"\nstatus=$?\nexit $status\n",
            ask.display()
        ),
    )
    .unwrap();
    shell.type_in(&format!(
        "sh '{}' \"$PWD/notes.txt\" >> '{}'\n",
        script.display(),
        answers.display()
    ));
    assert_eq!(
        shell.window(),
        WindowOpen {
            picture: 26891,
            input: 26892,
            title: "Text Editor".into(),
            asked_by_program: true,
        }
    );
    shell.type_in(&asking(&format!("'{}'", secret.display())));
    shell.type_in(&asking("\"$PWD/link.txt\""));
    shell.type_in(&asking("\"$PWD/../notes.txt\""));
    // A shell whose device does not show windows.
    let mut without = pair.open_with(ShellRequest {
        windows: false,
        ..request
    });
    without.type_in(&asking_into("\"$PWD/notes.txt\"", &others));
    let lines_of = |file: &std::path::Path, count: usize| {
        let began = Instant::now();
        loop {
            let lines: Vec<String> = std::fs::read_to_string(file)
                .unwrap_or_default()
                .lines()
                .map(str::to_string)
                .collect();
            if lines.len() >= count {
                return lines;
            }
            assert!(began.elapsed() < SOON, "{lines:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let mut lines = lines_of(&answers, 5);
    lines.extend(lines_of(&others, 1));
    assert_eq!(lines[..2], ["ok", "ok"]);
    assert!(lines[2].contains("only files inside the shell's project folder"));
    assert!(lines[3].contains("symbolic link"));
    assert!(lines[4].contains("without '..'"));
    assert!(lines[5].contains("does not show windows"));
    // The launcher was asked twice, with the place the host built, in its
    // environment.
    assert_eq!(
        std::fs::read_to_string(&launched).unwrap(),
        format!("1|open|{}\n", folder.join("notes.txt").display()).repeat(2)
    );
    // When the shell that asked is over, its editor is ended.
    shell.type_in("exit\n");
    let began = Instant::now();
    while !pair
        .log()
        .iter()
        .any(|line| line.contains("was ended with the shell"))
    {
        assert!(began.elapsed() < SOON, "{:?}", pair.log());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        std::fs::read_to_string(&launched).unwrap().lines().last(),
        Some("1|stop|")
    );
    // Nothing of the places is written down.
    let log = pair.log();
    assert!(log
        .iter()
        .any(|line| line.contains("in the main editor") && line.contains("port 26891")));
    assert!(log
        .iter()
        .any(|line| line.contains("(typed at the shell's prompt)")));
    assert!(log
        .iter()
        .any(|line| line.contains("(asked by a program in the shell)")));
    for line in &log {
        for word in ["notes", "secret", "link.txt", "ed-project", "ed-outside"] {
            assert!(!line.contains(word), "{line}");
        }
    }
}
