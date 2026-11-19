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

#[test]
fn a_number_nothing_was_handed_over_at_is_refused_before_anything_else() {
    for command in [
        vec!["input", "relay", "--to", "nobody", "--input-fd", "987"],
        vec!["agent", "--no-discovery", "--input-fd", "987"],
        // Never the standard descriptors.
        vec!["input", "relay", "--to", "nobody", "--input-fd", "0"],
    ] {
        let result = istesso().args(&command).output().unwrap();
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{command:?}");
        assert!(stderr.contains("cannot take input there"), "{stderr}");
    }
}

#[test]
fn a_socket_with_a_name_is_not_taken_as_an_end() {
    let dir = TestDir::new("named");
    let name = dir.0.join("n.sock");
    let _listens = std::os::unix::net::UnixDatagram::bind(&name).unwrap();
    // Connected to a name: whoever takes the name's place would be written
    // to, so it is not an end of a pair.
    let to_a_name = std::os::unix::net::UnixDatagram::unbound().unwrap();
    to_a_name.connect(&name).unwrap();
    let mut command = istesso();
    let number = hand_to_child(&mut command, to_a_name);
    let result = command
        .args(["input", "relay", "--to", "nobody", "--input-fd"])
        .arg(number.to_string())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(!result.status.success());
    assert!(stderr.contains("has a name"), "{stderr}");
}

#[test]
fn a_name_and_an_end_are_not_given_together() {
    let result = istesso()
        .args(["input", "relay", "--to", "nobody", "--input-fd", "5"])
        .args(["--socket", "/nonexistent/in.sock"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("cannot be used with"), "{stderr}");
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
             --input-fd \"$NUMBER\" >/dev/null 2>&1 & echo $!; sleep 2",
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
    // Should the relay stay, it is ended by its recorded number when the
    // test is over.
    struct Left(u32);
    impl Drop for Left {
        fn drop(&mut self) {
            let _ = Command::new("/bin/kill")
                .arg(self.0.to_string())
                .stderr(Stdio::null())
                .status();
        }
    }
    let _left = Left(relay);
    // `kill -0` asks whether the process is there without doing anything to it.
    let there = || {
        Command::new("/bin/kill")
            .args(["-0", &relay.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success()
    };
    let began = Instant::now();
    while there() {
        assert!(
            began.elapsed() < Duration::from_secs(30),
            "the relay outlived the program that started it"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}
