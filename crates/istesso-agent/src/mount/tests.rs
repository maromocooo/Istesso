//! A host's agent on the loopback that shows a folder, and a device that
//! reads it. The folder is made by each test in its own temporary directory;
//! the global ignore file of whoever runs the tests is never read.

use std::fs;
use std::net::SocketAddr;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use istesso_link::mount::{Answer, Failure, Kind, Question, MAX_ENTRIES, MAX_READ, ROOT_ID};
use istesso_link::{Client, ErrorCode, LocalKeys, MountAnswer};
use istesso_pair::{Hello, Identity, Peer, PeerStore, Permission};

use super::client::{Gone, Remote};
use super::export::Export;
use crate::testing::{manifest, TempDir};

const SOON: Duration = Duration::from_secs(5);

/// A host that shows `folder` as `proj` (or not), and a paired device.
struct MountPair {
    host: TempDir,
    device: TempDir,
    addr: SocketAddr,
    log: Arc<Mutex<Vec<String>>>,
}

struct Setup<'a> {
    folder: &'a Path,
    allow_device: bool,
    show_ignored: bool,
}

impl MountPair {
    fn new(tag: &str, setup: Setup<'_>) -> Self {
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
            mount_folders: vec![format!("proj={}", setup.folder.display())],
            mount_for: if setup.allow_device {
                vec!["a laptop".to_string()]
            } else {
                Vec::new()
            },
            mount_show_ignored: if setup.show_ignored {
                vec!["proj".to_string()]
            } else {
                Vec::new()
            },
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
        MountPair {
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

    fn ask(&self, folder: &str) -> MountAnswer {
        self.connect().open_mount(folder).unwrap()
    }

    fn open(&self) -> Remote {
        let mut client = self.connect();
        match client.open_mount("proj").unwrap() {
            MountAnswer::Open { root, max_read } => {
                assert_eq!(root.kind, Kind::Dir);
                assert_eq!(max_read, MAX_READ);
            }
            other => panic!("not opened: {other:?}"),
        }
        Remote::from_channel(client.into_channel(), |_| {})
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

/// A project folder: sources, a secret and build output that Git ignores,
/// `.git`, a link out of the folder, and a file larger than one read.
fn project(base: &Path) -> (PathBuf, Vec<u8>) {
    let root = base.join("project");
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("target/debug")).unwrap();
    fs::create_dir_all(root.join(".git/info")).unwrap();
    fs::create_dir_all(base.join("outside")).unwrap();
    fs::write(root.join("src/main.rs"), b"fn main() {}\n").unwrap();
    fs::write(root.join("README.md"), b"read me\n").unwrap();
    fs::write(root.join(".gitignore"), b".env\n/target/\n").unwrap();
    fs::write(root.join(".env"), b"TOKEN=not-a-real-one\n").unwrap();
    fs::write(root.join("target/debug/app"), b"built").unwrap();
    fs::write(root.join(".git/config"), b"[core]\n").unwrap();
    fs::write(root.join(".git/info/exclude"), b"local-notes.txt\n").unwrap();
    fs::write(root.join("local-notes.txt"), b"mine").unwrap();
    fs::write(base.join("outside/secret.txt"), b"outside").unwrap();
    symlink(base.join("outside/secret.txt"), root.join("link-out")).unwrap();
    let big: Vec<u8> = (0..(MAX_READ as usize * 2 + 12345))
        .map(|i| (i * 7 % 251) as u8)
        .collect();
    fs::write(root.join("big.bin"), &big).unwrap();
    (root, big)
}

fn lookup(remote: &Remote, dir: u64, name: &str) -> Result<(u64, Kind), Failure> {
    match remote
        .call(&Question::Lookup {
            dir,
            name: name.as_bytes().to_vec(),
        })
        .unwrap()
    {
        (Answer::Found { id, attr }, _) => Ok((id, attr.kind)),
        (Answer::Failed { error }, _) => Err(error),
        other => panic!("unexpected: {other:?}"),
    }
}

fn listing(remote: &Remote, id: u64) -> Vec<String> {
    let mut names = Vec::new();
    let mut start = 0;
    loop {
        match remote.call(&Question::ReadDir { id, start }).unwrap() {
            (Answer::Entries { entries, more }, _) => {
                assert!(entries.len() <= MAX_ENTRIES);
                start += entries.len() as u32;
                names.extend(
                    entries
                        .into_iter()
                        .map(|e| String::from_utf8(e.name).unwrap()),
                );
                if !more {
                    return names;
                }
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}

fn read_all(remote: &Remote, id: u64) -> Vec<u8> {
    let mut contents = Vec::new();
    loop {
        match remote
            .call(&Question::Read {
                id,
                offset: contents.len() as u64,
                count: u32::MAX,
            })
            .unwrap()
        {
            (Answer::Data { eof }, data) => {
                assert!(data.len() <= MAX_READ as usize);
                contents.extend_from_slice(&data);
                if eof {
                    return contents;
                }
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
}

#[test]
fn a_named_device_reads_a_named_folder_without_what_git_ignores() {
    let base = TempDir::new("mount-read");
    let (root, big) = project(base.path());
    let pair = MountPair::new(
        "mount-read",
        Setup {
            folder: &root,
            allow_device: true,
            show_ignored: false,
        },
    );
    let remote = pair.open();
    assert_eq!(
        listing(&remote, ROOT_ID),
        [".gitignore", "README.md", "big.bin", "link-out", "src"]
    );
    let (src, kind) = lookup(&remote, ROOT_ID, "src").unwrap();
    assert_eq!(kind, Kind::Dir);
    let (main, _) = lookup(&remote, src, "main.rs").unwrap();
    assert_eq!(read_all(&remote, main), b"fn main() {}\n");
    let (big_id, _) = lookup(&remote, ROOT_ID, "big.bin").unwrap();
    assert_eq!(read_all(&remote, big_id), big);
    // What Git ignores, and .git, are not there for the device.
    for hidden in [".env", "target", ".git", "local-notes.txt"] {
        assert_eq!(
            lookup(&remote, ROOT_ID, hidden),
            Err(Failure::NoEnt),
            "{hidden}"
        );
    }
    // A link is a link: its text, never what it points to.
    let (link, kind) = lookup(&remote, ROOT_ID, "link-out").unwrap();
    assert_eq!(kind, Kind::Symlink);
    assert!(matches!(
        remote
            .call(&Question::Read {
                id: link,
                offset: 0,
                count: 10
            })
            .unwrap()
            .0,
        Answer::Failed {
            error: Failure::Loop
        }
    ));
    // Names that would leave the folder are not names.
    for bad in ["..", "/etc", "a/b", ""] {
        assert_eq!(
            lookup(&remote, ROOT_ID, bad),
            Err(Failure::Invalid),
            "{bad:?}"
        );
    }
    // Ids of another conversation, or made up, mean nothing here.
    assert!(matches!(
        remote.call(&Question::Stat { id: 9999 }).unwrap().0,
        Answer::Failed {
            error: Failure::Stale
        }
    ));
    remote.close();
    let began = Instant::now();
    while !pair
        .log()
        .iter()
        .any(|line| line.starts_with("stopped showing"))
    {
        assert!(began.elapsed() < SOON, "the host did not notice");
        std::thread::sleep(Duration::from_millis(20));
    }
    let log = pair.log().join("\n");
    assert!(log.contains("showing the folder proj to a laptop, read-only"));
    // Never a path or a name from the folder.
    for private in [
        root.to_str().unwrap(),
        "main.rs",
        "big.bin",
        ".env",
        "TOKEN",
    ] {
        assert!(!log.contains(private), "{private} in the log: {log}");
    }
}

#[test]
fn a_file_ignored_after_it_was_handed_out_is_hidden_from_then_on() {
    let base = TempDir::new("mount-later");
    let (root, _) = project(base.path());
    let pair = MountPair::new(
        "mount-later",
        Setup {
            folder: &root,
            allow_device: true,
            show_ignored: false,
        },
    );
    let remote = pair.open();
    let (readme, _) = lookup(&remote, ROOT_ID, "README.md").unwrap();
    assert!(matches!(
        remote.call(&Question::Stat { id: readme }).unwrap().0,
        Answer::Attr { .. }
    ));
    fs::write(root.join(".gitignore"), b".env\n/target/\nREADME.md\n").unwrap();
    // The rules of a folder are kept for two seconds.
    std::thread::sleep(Duration::from_millis(2100));
    assert!(matches!(
        remote
            .call(&Question::Read {
                id: readme,
                offset: 0,
                count: 10
            })
            .unwrap()
            .0,
        Answer::Failed {
            error: Failure::NoEnt
        }
    ));
}

#[test]
fn the_owner_can_show_what_git_ignores_for_one_folder() {
    let base = TempDir::new("mount-all");
    let (root, _) = project(base.path());
    let pair = MountPair::new(
        "mount-all",
        Setup {
            folder: &root,
            allow_device: true,
            show_ignored: true,
        },
    );
    let remote = pair.open();
    let names = listing(&remote, ROOT_ID);
    for shown in [".env", "target", ".git", "local-notes.txt"] {
        assert!(
            names.iter().any(|name| name == shown),
            "{shown} in {names:?}"
        );
    }
}

#[test]
fn a_device_or_folder_that_was_not_named_is_refused_alike() {
    let base = TempDir::new("mount-refuse");
    let (root, _) = project(base.path());
    let named = MountPair::new(
        "mount-refuse",
        Setup {
            folder: &root,
            allow_device: true,
            show_ignored: false,
        },
    );
    let unnamed = MountPair::new(
        "mount-refuse",
        Setup {
            folder: &root,
            allow_device: false,
            show_ignored: false,
        },
    );
    let refusal = |answer: MountAnswer| match answer {
        MountAnswer::Refused { code, message } => (code, message),
        other => panic!("not refused: {other:?}"),
    };
    let (code, other_folder) = refusal(named.ask("other"));
    assert_eq!(code, ErrorCode::Forbidden);
    // A path is not an id, and is not looked up.
    let (_, path) = refusal(named.ask(root.to_str().unwrap()));
    let (code, other_device) = refusal(unnamed.ask("proj"));
    assert_eq!(code, ErrorCode::Forbidden);
    assert_eq!(other_folder, other_device);
    assert_eq!(path, other_device);
}

#[test]
fn losing_the_right_ends_the_conversation_within_a_second_or_two() {
    let base = TempDir::new("mount-revoke");
    let (root, _) = project(base.path());
    let pair = MountPair::new(
        "mount-revoke",
        Setup {
            folder: &root,
            allow_device: true,
            show_ignored: false,
        },
    );
    let remote = pair.open();
    assert!(lookup(&remote, ROOT_ID, "README.md").is_ok());
    PeerStore::open(pair.host.paths().peers())
        .unwrap()
        .set_permission("a laptop", Permission::Control, false)
        .unwrap();
    let began = Instant::now();
    loop {
        if remote.call(&Question::Stat { id: ROOT_ID }) == Err(Gone) {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_millis(2500),
            "still answered"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(remote.is_gone());
    let began = Instant::now();
    while !pair
        .log()
        .iter()
        .any(|line| line.contains("lost its right"))
    {
        assert!(began.elapsed() < SOON);
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_long_folder_is_listed_in_pages() {
    let base = TempDir::new("mount-pages");
    let root = base.path().join("many");
    fs::create_dir_all(&root).unwrap();
    let count = MAX_ENTRIES + 7;
    for i in 0..count {
        fs::write(root.join(format!("f{i:05}")), b"").unwrap();
    }
    let mut export = Export::open(&root, false, None).unwrap();
    let first = match export
        .handle(Question::ReadDir {
            id: ROOT_ID,
            start: 0,
        })
        .0
    {
        Answer::Entries { entries, more } => {
            assert!(more);
            entries
        }
        other => panic!("unexpected: {other:?}"),
    };
    assert_eq!(first.len(), MAX_ENTRIES);
    match export
        .handle(Question::ReadDir {
            id: ROOT_ID,
            start: MAX_ENTRIES as u32,
        })
        .0
    {
        Answer::Entries { entries, more } => {
            assert!(!more);
            assert_eq!(entries.len(), 7);
            assert_eq!(entries[6].name, format!("f{:05}", count - 1).as_bytes());
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn a_pipe_is_listed_and_not_opened() {
    let base = TempDir::new("mount-fifo");
    let pipe =
        std::ffi::CString::new(base.path().join("pipe").as_os_str().as_encoded_bytes()).unwrap();
    // SAFETY: valid C string.
    assert_eq!(unsafe { libc::mkfifo(pipe.as_ptr(), 0o600) }, 0);
    let mut export = Export::open(base.path(), false, None).unwrap();
    let id = match export
        .handle(Question::Lookup {
            dir: ROOT_ID,
            name: b"pipe".to_vec(),
        })
        .0
    {
        Answer::Found { id, attr } => {
            assert_eq!(attr.kind, Kind::Other);
            id
        }
        other => panic!("unexpected: {other:?}"),
    };
    assert_eq!(
        export
            .handle(Question::Read {
                id,
                offset: 0,
                count: 1
            })
            .0,
        Answer::Failed {
            error: Failure::Invalid
        }
    );
}

#[test]
fn a_link_to_a_folder_cannot_be_walked_through() {
    let base = TempDir::new("mount-dirlink");
    let root = base.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(base.path().join("outside")).unwrap();
    fs::write(base.path().join("outside/secret.txt"), b"outside").unwrap();
    symlink(base.path().join("outside"), root.join("dir-link")).unwrap();
    let mut export = Export::open(&root, false, None).unwrap();
    let id = match export
        .handle(Question::Lookup {
            dir: ROOT_ID,
            name: b"dir-link".to_vec(),
        })
        .0
    {
        Answer::Found { id, attr } => {
            assert_eq!(attr.kind, Kind::Symlink);
            id
        }
        other => panic!("unexpected: {other:?}"),
    };
    // macOS answers ENOTDIR, Linux ELOOP.
    for question in [
        Question::ReadDir { id, start: 0 },
        Question::Lookup {
            dir: id,
            name: b"secret.txt".to_vec(),
        },
    ] {
        match export.handle(question).0 {
            Answer::Failed {
                error: Failure::Loop | Failure::NotDir,
            } => {}
            other => panic!("unexpected: {other:?}"),
        }
    }
}

#[test]
fn a_question_this_version_does_not_know_is_refused_and_the_rest_goes_on() {
    let base = TempDir::new("mount-unknown");
    let mut export = Export::open(base.path(), false, None).unwrap();
    assert_eq!(
        export.handle(Question::Unknown).0,
        Answer::Failed {
            error: Failure::Invalid
        }
    );
    assert!(matches!(
        export.handle(Question::Stat { id: ROOT_ID }).0,
        Answer::Attr { .. }
    ));
}

#[test]
fn the_parent_of_a_mount_point_must_be_private() {
    use std::os::unix::fs::PermissionsExt;
    let base = TempDir::new("mount-private");
    let dir = base.path().join("Istesso");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(super::private_folder(&dir).is_err());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(super::private_folder(&dir).is_ok());
    symlink(&dir, base.path().join("link")).unwrap();
    assert!(super::private_folder(&base.path().join("link")).is_err());
}
