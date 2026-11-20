//! Input handed over on a pair of sockets without a name instead of at a
//! socket any program of the same user can write to and replace
//! (docs/adr/0028). Nothing here reads or writes this account's real files:
//! the refusals come before the key is used, and the run from end to end is
//! made only by a development build (`dev-paths`), in folders of its own.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use istesso_agent::endpoint::{hand_to_child, pair};
use istesso_agent::paths::DEV_PATHS;

/// A folder of the test's own, removed also when the test fails.
struct TestDir(PathBuf);

impl TestDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("istesso-input-end-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        TestDir(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A program this test started, ended with the test whatever happens.
struct Started(Child);

impl Drop for Started {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn istesso() -> Command {
    Command::new(env!("CARGO_BIN_EXE_istesso"))
}

/// What a command that has to be refused said, and how it ended. Run by a
/// development build only, in a folder of the test's own: should the
/// refusal ever fail, the command goes on with a key and a list of the
/// test's, on this machine only, and never with this account's. It is given
/// a moment and then ended, so that a command that was not refused does not
/// stay.
fn refused(dir: &TestDir, mut command: Command) -> Option<(bool, String)> {
    use std::io::Read;

    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return None;
    }
    let mut child = command
        .env("ISTESSO_CONFIG_DIR", &dir.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let began = Instant::now();
    let ended = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break Some(status);
        }
        if began.elapsed() > Duration::from_secs(10) {
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut said = String::new();
    if ended.is_none() {
        // Ours, by the handle it was started with.
        let _ = child.kill();
        let _ = child.wait();
    } else {
        let _ = child.stderr.take().unwrap().read_to_string(&mut said);
    }
    Some((ended.is_some_and(|status| !status.success()), said))
}

#[test]
fn a_number_nothing_was_handed_over_at_is_refused_before_anything_else() {
    let dir = TestDir::new("refused");
    let port = a_port(3).to_string();
    let me = std::process::id().to_string();
    for arguments in [
        vec![
            "input",
            "relay",
            "--to",
            "nobody",
            "--input-fd",
            "987",
            "--started-by",
            &me,
        ],
        vec![
            "agent",
            "--no-discovery",
            "--bind",
            "127.0.0.1",
            "--port",
            &port,
            "--input-fd",
            "987",
        ],
        // Never the standard descriptors.
        vec![
            "input",
            "relay",
            "--to",
            "nobody",
            "--input-fd",
            "0",
            "--started-by",
            &me,
        ],
    ] {
        let mut command = istesso();
        command.args(&arguments);
        let Some((refused, said)) = refused(&dir, command) else {
            return;
        };
        assert!(refused, "{arguments:?}");
        assert!(said.contains("cannot take input there"), "{said}");
    }
    // Refused before the key: none was made.
    assert_eq!(std::fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[test]
fn a_socket_with_a_name_is_not_taken_as_an_end() {
    let dir = TestDir::new("named");
    let config = TestDir::new("named-config");
    let name = dir.0.join("n.sock");
    let _listens = std::os::unix::net::UnixDatagram::bind(&name).unwrap();
    // Connected to a name: whoever takes the name's place would be written
    // to, so it is not an end of a pair.
    let to_a_name = std::os::unix::net::UnixDatagram::unbound().unwrap();
    to_a_name.connect(&name).unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, to_a_name);
    command
        .args(["input", "relay", "--to", "nobody", "--input-fd"])
        .arg(number.to_string())
        .arg("--started-by")
        .arg(std::process::id().to_string());
    let Some((refused, said)) = refused(&config, command) else {
        return;
    };
    assert!(refused);
    assert!(said.contains("has a name"), "{said}");
    assert_eq!(std::fs::read_dir(&config.0).unwrap().count(), 0);
}

#[test]
fn a_name_and_an_end_are_not_given_together() {
    let config = TestDir::new("both-config");
    let mut command = istesso();
    command
        .args(["input", "relay", "--to", "nobody", "--input-fd", "5"])
        .arg("--started-by")
        .arg(std::process::id().to_string())
        .args(["--socket", "/nonexistent/in.sock"]);
    let Some((refused, said)) = refused(&config, command) else {
        return;
    };
    assert!(refused);
    assert!(said.contains("cannot be used with"), "{said}");
}

/// An end and no word of who holds the other one is not taken: without it,
/// a relay whose starter went before it could look would not know.
#[test]
fn an_end_needs_the_number_of_its_starter() {
    let config = TestDir::new("no-starter-config");
    let mut command = istesso();
    command.args(["input", "relay", "--to", "nobody", "--input-fd", "5"]);
    let Some((refused, said)) = refused(&config, command) else {
        return;
    };
    assert!(refused);
    assert!(said.contains("--started-by"), "{said}");
    assert_eq!(std::fs::read_dir(&config.0).unwrap().count(), 0);
}

/// The program that says it started the relay is not its parent: gone
/// already, or never there. The relay does not start, and uses no key.
#[test]
fn the_relay_does_not_start_for_a_starter_that_is_not_its_parent() {
    let config = TestDir::new("not-the-parent-config");
    // A program that was there and is gone.
    let mut gone = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    let gone_number = gone.id();
    gone.wait().unwrap();
    let (_window, relays) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, relays);
    command
        .args(["input", "relay", "--to", "nobody", "--input-fd"])
        .arg(number.to_string())
        .arg("--started-by")
        .arg(gone_number.to_string());
    let Some((refused, said)) = refused(&config, command) else {
        return;
    };
    assert!(refused);
    assert!(said.contains("gone already"), "{said}");
    assert_eq!(std::fs::read_dir(&config.0).unwrap().count(), 0);
}

/// What a build does not guard is said to a person, in one line, and is
/// never in the way of a program: `doctor --json` says nothing more than it
/// did, and an agent starts although nobody reads what it says.
#[test]
fn the_notes_are_for_people_and_never_in_the_way() {
    use std::io::Read;

    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let dir = TestDir::new("notes");
    let doctor = |arguments: &[&str]| {
        let result = istesso()
            .args(arguments)
            .env("ISTESSO_CONFIG_DIR", &dir.0)
            .output()
            .unwrap();
        assert!(result.status.success());
        (
            String::from_utf8_lossy(&result.stdout).into_owned(),
            String::from_utf8_lossy(&result.stderr).into_owned(),
        )
    };
    let (json, said) = doctor(&["doctor", "--json"]);
    assert!(!said.contains("keeps the device key"), "{said}");
    assert!(serde_json::from_str::<serde_json::Value>(&json).is_ok());
    let (_, said) = doctor(&["doctor"]);
    assert_eq!(said.matches("keeps the device key").count(), 1, "{said}");

    // An agent whose standard error is a pipe that was closed at the other
    // end: it starts, and answers on its port.
    let port = a_port(4);
    let mut agent = istesso()
        .args(["agent", "--no-discovery", "--bind", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .env("ISTESSO_CONFIG_DIR", &dir.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(agent.stderr.take());
    let mut agent = Started(agent);
    let began = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            agent.0.try_wait().unwrap().is_none(),
            "the agent ended instead of starting"
        );
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "no agent answers"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(agent.0.try_wait().unwrap().is_none());

    // With standard error read: one line at the start, and no more.
    drop(agent);
    let mut agent = istesso()
        .args(["agent", "--no-discovery", "--bind", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .env("ISTESSO_CONFIG_DIR", &dir.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let began = Instant::now();
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "no agent answers"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = agent.kill();
    let _ = agent.wait();
    let mut said = String::new();
    let _ = agent.stderr.take().unwrap().read_to_string(&mut said);
    let notes: Vec<&str> = said.lines().filter(|l| l.starts_with("note: ")).collect();
    assert_eq!(notes.len(), 1, "{said}");
    assert!(notes[0].contains("keeps the device key"), "{said}");
    assert!(notes[0].contains("a socket with a name"), "{said}");
}

/// A port for an agent of the test's own, on this machine only: one nothing
/// else has, or, on a machine where the ports for tests are given out,
/// `ISTESSO_TEST_PORT` plus `nth`.
fn a_port(nth: u16) -> u16 {
    match std::env::var("ISTESSO_TEST_PORT") {
        Ok(first) => first.parse::<u16>().unwrap() + nth,
        Err(_) => std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port(),
    }
}

/// Two devices that are paired with each other, each in a folder of its own.
fn paired(host: &Path, device: &Path) {
    use istesso_pair::{Hello, Peer};
    let keeping = |dir: &Path| istesso_agent::Paths::at(dir).keeping();
    let identities = [host, device].map(|dir| keeping(dir).identity().unwrap());
    for (dir, name, other) in [
        (host, "a laptop", &identities[1]),
        (device, "a host", &identities[0]),
    ] {
        keeping(dir)
            .peers()
            .unwrap()
            .add(Peer::from_hello(&Hello {
                device_name: name.to_string(),
                public_key: other.public_key(),
            }))
            .unwrap();
    }
}

/// From the window's end to the application's end, through both commands,
/// with no socket that has a name anywhere on the way.
#[test]
fn input_goes_from_end_to_end_without_a_name() {
    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let (host, device) = (TestDir::new("host"), TestDir::new("device"));
    paired(&host.0, &device.0);
    let port = a_port(0);

    // The program that shows the application starts an agent of its own.
    let (shows, agents) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, agents);
    let agent = command
        .args(["agent", "--no-discovery", "--bind", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .arg("--input-fd")
        .arg(number.to_string())
        .env("ISTESSO_CONFIG_DIR", &host.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _agent = Started(agent);
    drop(command);

    // The program that shows the window starts the relay.
    let (window, relays) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, relays);
    let relay = command
        .args(["input", "relay", "--to", "a host", "--keep-trying", "20"])
        .arg("--address")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--input-fd")
        .arg(number.to_string())
        .arg("--started-by")
        .arg(std::process::id().to_string())
        .env("ISTESSO_CONFIG_DIR", &device.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _relay = Started(relay);
    drop(command);

    // The key is sent until it arrives: the relay may still be opening its
    // conversation with the agent.
    let pressed = br#"{"kind":"key","code":30,"pressed":true}"#;
    shows
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let began = Instant::now();
    let mut buffer = [0u8; 256];
    let arrived = loop {
        window.send(pressed).unwrap();
        if let Ok(length) = shows.recv(&mut buffer) {
            break buffer[..length].to_vec();
        }
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "nothing arrived at the application's end"
        );
    };
    assert_eq!(arrived, pressed);

    // Both commands closed themselves to other programs of the same user
    // when they took their end: not even this test, which started them, can
    // read their environment or list what they have open.
    #[cfg(target_os = "linux")]
    for pid in [_agent.0.id(), _relay.0.id()] {
        assert!(std::fs::read(format!("/proc/{pid}/environ")).is_err());
        assert!(std::fs::read_dir(format!("/proc/{pid}/fd")).is_err());
    }

    // Neither command made a socket with a name.
    for dir in [&host.0, &device.0] {
        let mut left = vec![dir.clone()];
        while let Some(dir) = left.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let entry = entry.unwrap();
                let kind = entry.file_type().unwrap();
                use std::os::unix::fs::FileTypeExt;
                assert!(!kind.is_socket(), "{}", entry.path().display());
                if kind.is_dir() {
                    left.push(entry.path());
                }
            }
        }
    }
}

/// A relay whose standard error nobody reads any more keeps going: it is
/// still trying to reach the device when it says so for the first time,
/// and passes input once the device is there.
#[test]
fn a_relay_whose_notes_nobody_reads_keeps_going() {
    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let (host, device) = (TestDir::new("host-unread"), TestDir::new("device-unread"));
    paired(&host.0, &device.0);
    let port = a_port(6);

    // The relay first, with nobody at the address yet.
    let (window, relays) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, relays);
    let mut relay = command
        .args(["input", "relay", "--to", "a host", "--keep-trying", "30"])
        .arg("--address")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--input-fd")
        .arg(number.to_string())
        .arg("--started-by")
        .arg(std::process::id().to_string())
        .env("ISTESSO_CONFIG_DIR", &device.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Nobody reads what it says from now on.
    drop(relay.stderr.take());
    let mut relay = Started(relay);
    drop(command);
    // Long enough for "cannot reach the device yet" to have been said.
    std::thread::sleep(Duration::from_secs(2));
    assert!(
        relay.0.try_wait().unwrap().is_none(),
        "the relay ended when nobody read what it said"
    );

    let (shows, agents) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, agents);
    let agent = command
        .args(["agent", "--no-discovery", "--bind", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .arg("--input-fd")
        .arg(number.to_string())
        .env("ISTESSO_CONFIG_DIR", &host.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _agent = Started(agent);
    drop(command);

    let pressed = br#"{"kind":"key","code":32,"pressed":true}"#;
    shows
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let began = Instant::now();
    let mut buffer = [0u8; 256];
    let arrived = loop {
        window.send(pressed).unwrap();
        if let Ok(length) = shows.recv(&mut buffer) {
            break buffer[..length].to_vec();
        }
        assert!(
            relay.0.try_wait().unwrap().is_none(),
            "the relay ended when nobody read what it said"
        );
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "nothing arrived at the application's end"
        );
    };
    assert_eq!(arrived, pressed);
}

/// The form with a name still works, for the measuring tools, and the socket
/// the relay makes is the owner's alone whatever the starter's umask says.
/// That keeps other accounts out; the relay says that it does not keep other
/// programs of this account out.
#[test]
fn a_socket_with_a_name_is_for_its_owner_only_and_says_what_it_is() {
    use std::io::{BufRead, BufReader};
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};

    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let (host, device) = (TestDir::new("host-named"), TestDir::new("device-named"));
    paired(&host.0, &device.0);
    let port = a_port(2);
    let (shows, agents) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, agents);
    let agent = command
        .args(["agent", "--no-discovery", "--bind", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .arg("--input-fd")
        .arg(number.to_string())
        .env("ISTESSO_CONFIG_DIR", &host.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _agent = Started(agent);
    drop(command);

    let name = device.0.join("in.sock");
    let mut relay = istesso()
        .args(["input", "relay", "--to", "a host", "--keep-trying", "20"])
        .arg("--address")
        .arg(format!("127.0.0.1:{port}"))
        .arg("--socket")
        .arg(&name)
        .env("ISTESSO_CONFIG_DIR", &device.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let said = relay.stderr.take().unwrap();
    let _relay = Started(relay);
    // What it says about itself, on standard error, once it listens.
    let mut plain = false;
    for line in BufReader::new(said).lines() {
        if line
            .unwrap()
            .contains("any program of this account can write to")
        {
            plain = true;
            break;
        }
    }
    assert!(plain, "the relay did not say what a name does not guard");
    let meta = std::fs::symlink_metadata(&name).unwrap();
    assert!(meta.file_type().is_socket());
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);

    let window = std::os::unix::net::UnixDatagram::unbound().unwrap();
    let pressed = br#"{"kind":"key","code":31,"pressed":true}"#;
    window.send_to(pressed, &name).unwrap();
    shows
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0u8; 256];
    let length = shows.recv(&mut buffer).unwrap();
    assert_eq!(&buffer[..length], pressed);
}

/// Also while it is still trying to reach the device: nobody answers at the
/// address, the relay is told to keep trying for a minute, and the program
/// that started it goes away after a moment.
#[test]
fn the_relay_stops_trying_when_the_program_that_started_it_is_gone() {
    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let (host, device) = (TestDir::new("host-trying"), TestDir::new("device-trying"));
    paired(&host.0, &device.0);
    // Nothing listens at this port.
    let port = a_port(5);
    let (_window, relays) = pair().unwrap();
    let mut shell = Command::new("/bin/sh");
    let number = hand_to_child(&mut shell, relays);
    let output = shell
        .arg("-c")
        .arg(
            "\"$ISTESSO\" input relay --to 'a host' --keep-trying 60 --address \"$ADDRESS\" \
             --input-fd \"$NUMBER\" --started-by $$ >/dev/null 2>&1 & echo $!; sleep 2",
        )
        .env("ISTESSO", env!("CARGO_BIN_EXE_istesso"))
        .env("ADDRESS", format!("127.0.0.1:{port}"))
        .env("NUMBER", number.to_string())
        .env("ISTESSO_CONFIG_DIR", &device.0)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    drop(shell);
    let relay: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap();
    let _left = Left(relay);
    let began = Instant::now();
    while is_there(relay) {
        assert!(
            began.elapsed() < Duration::from_secs(20),
            "the relay kept trying for a program that was gone"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A process this test had started through a shell: should it stay, it is
/// ended by its recorded number when the test is over.
struct Left(u32);

impl Drop for Left {
    fn drop(&mut self) {
        let _ = Command::new("/bin/kill")
            .arg(self.0.to_string())
            .stderr(Stdio::null())
            .status();
    }
}

/// `kill -0` asks whether the process is there without doing anything to it.
fn is_there(process: u32) -> bool {
    Command::new("/bin/kill")
        .args(["-0", &process.to_string()])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// The relay ends when the program that holds the other end is gone, instead
/// of keeping a conversation with the device that nothing can feed.
#[test]
fn the_relay_ends_with_the_program_that_started_it() {
    if !DEV_PATHS {
        eprintln!("left out: only a development build runs in folders of its own");
        return;
    }
    let (host, device) = (TestDir::new("host-ends"), TestDir::new("device-ends"));
    paired(&host.0, &device.0);
    let port = a_port(1);
    let (_shows, agents) = pair().unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, agents);
    let agent = command
        .args(["agent", "--no-discovery", "--bind", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .arg("--input-fd")
        .arg(number.to_string())
        .env("ISTESSO_CONFIG_DIR", &host.0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _agent = Started(agent);
    drop(command);

    // A shell stands for the window: it starts the relay, says the relay's
    // process number and, a moment later, goes away, leaving the relay behind.
    let (_window, relays) = pair().unwrap();
    let mut shell = Command::new("/bin/sh");
    let number = hand_to_child(&mut shell, relays);
    let output = shell
        .arg("-c")
        .arg(
            "\"$ISTESSO\" input relay --to 'a host' --keep-trying 20 --address \"$ADDRESS\" \
             --input-fd \"$NUMBER\" --started-by $$ >/dev/null 2>&1 & echo $!; sleep 2",
        )
        .env("ISTESSO", env!("CARGO_BIN_EXE_istesso"))
        .env("ADDRESS", format!("127.0.0.1:{port}"))
        .env("NUMBER", number.to_string())
        .env("ISTESSO_CONFIG_DIR", &device.0)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    drop(shell);
    let relay: u32 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .unwrap();
    let _left = Left(relay);
    let began = Instant::now();
    while is_there(relay) {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "the relay outlived the program that started it"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
