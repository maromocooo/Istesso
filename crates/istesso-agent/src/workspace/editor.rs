//! Opening a file of a shell's workspace in the host's main editor, whose
//! window is then shown on the device over the picture's path. Proposed in
//! ADR 0007 ("メインのエディターで開く頼み"); the design is ADR 0025, 3.1
//! (form A).
//!
//! * Inside the workspace, `istesso-edit <file>` (this program under
//!   another name) asks on the agent's socket for pages
//!   ([`super::opener`]), with one line: `file <absolute place>`.
//! * The agent takes it only from a process of the workspace of an open
//!   shell whose device said that it shows windows, and only for a place
//!   inside that shell's project folder, looked at one name at a time
//!   without following a link ([`checked_place`]).
//! * Which editor, and how it is started, is the host's own setting
//!   (`istesso agent --main-editor <launcher>`), never the request's: the
//!   launcher is given one argument, the place the agent built itself from
//!   the project folder and the names it checked ([`MainEditor::open`]).
//! * The launcher says on which ports the window's picture and input wait,
//!   and the device is told so on the shell's conversation
//!   ([`istesso_link::shell::ShellFrame::OpenWindow`]).
//!
//! Never written down: the place, the file's name, or any part of them.

use std::ffi::{CString, OsStr, OsString};
use std::io::{BufRead, BufReader, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use istesso_link::forward::ascii_name;
use istesso_link::shell::MAX_TITLE;

/// The name this program answers to as the asker.
pub const EDIT_NAME: &str = "istesso-edit";
/// How a request to open a file begins, on the agent's socket. A web
/// page's address never begins so.
pub const FILE_REQUEST: &str = "file ";
/// The longest place that is taken, in bytes.
pub const MAX_PLACE: usize = 4096;
/// How long the launcher has to say where the window waits.
pub const LAUNCH_WITHIN: Duration = Duration::from_secs(30);
/// What a window is called when the launcher does not say.
const DEFAULT_TITLE: &str = "Main editor";

/// Why a place is not opened, in words fit for whoever asked. Never the
/// place itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// Not an absolute place, not UTF-8, with a control character, or too
    /// long.
    Malformed,
    /// It has `..` in it.
    Climbs,
    /// It is not below the workspace's folder.
    Outside,
    /// It is the workspace's folder itself.
    TheFolderItself,
    /// A symbolic link on the way, or at the end.
    Link,
    /// Something on the way is not a folder, or is not there.
    NotAFolder,
    /// The last name is not an ordinary file.
    NotAFile,
    /// The file has more names than one (a hard link may lead outside).
    MoreThanOneName,
    /// The place could not be looked at.
    Unreadable,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Refusal::Malformed => {
                "give a file's name in UTF-8, without control characters, of 4096 bytes at most"
            }
            Refusal::Climbs => "give the file's place without '..'",
            Refusal::Outside => "only files inside the shell's project folder can be opened",
            Refusal::TheFolderItself => "that is the project folder itself, not a file in it",
            Refusal::Link => "a symbolic link is on the way to that file: it is not followed",
            Refusal::NotAFolder => "a folder on the way to that file is not there",
            Refusal::NotAFile => "that is not an ordinary file",
            Refusal::MoreThanOneName => {
                "that file has more than one name (a hard link): it is not opened"
            }
            Refusal::Unreadable => "that place cannot be looked at",
        })
    }
}

/// The folder of a shell's workspace below which files may be opened: the
/// project folder by its real place, or none when it is the root or the
/// home folder itself (then "inside" would be everything).
pub fn workspace_folder(project: &Path, home: &Path) -> Option<PathBuf> {
    let folder = project.canonicalize().ok()?;
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    (folder.parent().is_some() && folder != home).then_some(folder)
}

/// The place to hand the editor, built from `folder` (the workspace's,
/// by its real place) and the names of `asked` below it, once each name on
/// the way has been looked at without following a link: folders all the
/// way, and at the end an ordinary file with one name, or no file yet (the
/// editor makes it when it saves).
///
/// What a program that changes the folders between this look and the
/// editor's start does is not stopped (ADR 0007, "守りの限り").
pub fn checked_place(folder: &Path, asked: &str) -> Result<PathBuf, Refusal> {
    if asked.is_empty()
        || asked.len() > MAX_PLACE
        || asked.chars().any(char::is_control)
        || !asked.starts_with('/')
    {
        return Err(Refusal::Malformed);
    }
    let mut names: Vec<&OsStr> = Vec::new();
    for component in Path::new(asked).components() {
        match component {
            Component::RootDir | Component::CurDir => {}
            Component::ParentDir => return Err(Refusal::Climbs),
            Component::Normal(name) => names.push(name),
            Component::Prefix(_) => return Err(Refusal::Malformed),
        }
    }
    let base: Vec<&OsStr> = folder
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name),
            _ => None,
        })
        .collect();
    if names.len() < base.len() || names[..base.len()] != base[..] {
        return Err(Refusal::Outside);
    }
    let below = &names[base.len()..];
    let Some((last, on_the_way)) = below.split_last() else {
        return Err(Refusal::TheFolderItself);
    };
    let mut at = open_folder(None, folder.as_os_str())?;
    for name in on_the_way {
        at = open_folder(Some(&at), name)?;
    }
    match look(&at, last) {
        Ok(found) => {
            let kind = found.st_mode & libc::S_IFMT;
            if kind == libc::S_IFLNK {
                return Err(Refusal::Link);
            }
            if kind != libc::S_IFREG {
                return Err(Refusal::NotAFile);
            }
            if found.st_nlink != 1 {
                return Err(Refusal::MoreThanOneName);
            }
        }
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => {}
        Err(_) => return Err(Refusal::Unreadable),
    }
    Ok(below
        .iter()
        .fold(folder.to_path_buf(), |place, name| place.join(name)))
}

fn c_name(name: &OsStr) -> Result<CString, Refusal> {
    CString::new(name.as_bytes()).map_err(|_| Refusal::Malformed)
}

/// Opens the folder `name` in `at` (or from the root, when `at` is none),
/// never through a symbolic link.
fn open_folder(at: Option<&OwnedFd>, name: &OsStr) -> Result<OwnedFd, Refusal> {
    let c = c_name(name)?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let dir = at.map_or(libc::AT_FDCWD, |fd| fd.as_raw_fd());
    // SAFETY: a name that ends with NUL, and flags; the descriptor returned
    // is owned here.
    let fd = unsafe { libc::openat(dir, c.as_ptr(), flags) };
    if fd >= 0 {
        // SAFETY: a descriptor that was just opened and belongs to no one else.
        return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    // Why, without following a link: a link, something else, or nothing.
    match at.map(|at| look(at, name)) {
        Some(Ok(found)) if found.st_mode & libc::S_IFMT == libc::S_IFLNK => Err(Refusal::Link),
        Some(Ok(_)) | Some(Err(_)) => Err(Refusal::NotAFolder),
        None => Err(Refusal::Unreadable),
    }
}

/// What `name` in the folder `at` is, without following a link.
fn look(at: &OwnedFd, name: &OsStr) -> std::io::Result<libc::stat> {
    let c = c_name(name).map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: `found` is written by fstatat only; the name ends with NUL.
    unsafe {
        let mut found: libc::stat = std::mem::zeroed();
        if libc::fstatat(
            at.as_raw_fd(),
            c.as_ptr(),
            &mut found,
            libc::AT_SYMLINK_NOFOLLOW,
        ) == 0
        {
            Ok(found)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// Where a window of the main editor waits on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub picture: u16,
    pub input: u16,
    pub title: String,
}

/// The host's main editor: the program that starts it, set by whoever
/// runs the agent (`istesso agent --main-editor`).
pub struct MainEditor {
    launcher: PathBuf,
    within: Duration,
    /// One start at a time: the launcher chooses the ports.
    starting: Mutex<()>,
}

impl MainEditor {
    /// The launcher must be an absolute place of a file that can be run.
    pub fn new(launcher: &Path) -> Result<MainEditor, String> {
        use std::os::unix::fs::PermissionsExt;
        if !launcher.is_absolute() {
            return Err("--main-editor needs the absolute place of a program".into());
        }
        let meta = std::fs::metadata(launcher)
            .map_err(|e| format!("--main-editor: cannot use that program: {e}"))?;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            return Err("--main-editor: that is not a program that can be run".into());
        }
        Ok(MainEditor {
            launcher: launcher.to_path_buf(),
            within: LAUNCH_WITHIN,
            starting: Mutex::new(()),
        })
    }

    /// The same, with another time limit (for tests).
    pub fn with_time_limit(mut self, within: Duration) -> MainEditor {
        self.within = within;
        self
    }

    /// Starts the editor on `place` (one that [`checked_place`] gave), and
    /// says where its window waits. The launcher gets `place` as its one
    /// argument, never through a shell, and answers with one line:
    /// `window <picture port> <input port> [title]`, or `fail <why>`.
    pub fn open(&self, place: &Path) -> Result<Opened, String> {
        let _one = self.starting.lock().unwrap_or_else(|e| e.into_inner());
        let mut child = Command::new(&self.launcher)
            .arg(place)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("the main editor's launcher did not start: {e}"))?;
        let Some(out) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the main editor's launcher cannot be heard".into());
        };
        let (said, heard) = mpsc::channel::<String>();
        let reading = super::spawn("istesso-editor-launcher", move || {
            for line in BufReader::new(out).lines() {
                let Ok(line) = line else { return };
                if said.send(line).is_err() {
                    return;
                }
            }
        });
        if reading.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("the main editor's launcher cannot be heard".into());
        }
        let deadline = std::time::Instant::now() + self.within;
        let answer = loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match heard.recv_timeout(left) {
                Ok(line) => match read_answer(&line) {
                    Some(answer) => break answer,
                    None => continue,
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = child.kill();
                    break Err(format!(
                        "the main editor did not start within {} seconds",
                        self.within.as_secs()
                    ));
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    break Err("the main editor's launcher ended without a window".into())
                }
            }
        };
        // Collected when it ends (it ends once it has answered).
        let _ = super::spawn("istesso-editor-launcher-end", move || {
            let _ = child.wait();
        });
        answer
    }
}

/// One line of the launcher's: an answer, or something else (passed over).
fn read_answer(line: &str) -> Option<Result<Opened, String>> {
    if let Some(why) = line.strip_prefix("fail ") {
        return Some(Err(format!(
            "the main editor did not start: {}",
            ascii_name(why.trim(), 160)
        )));
    }
    let rest = line.strip_prefix("window ")?;
    let mut words = rest.splitn(3, ' ');
    let port = |word: Option<&str>| word?.parse::<u16>().ok().filter(|port| *port != 0);
    let (Some(picture), Some(input)) = (port(words.next()), port(words.next())) else {
        return Some(Err("the main editor's launcher said no ports".into()));
    };
    let title = words
        .next()
        .map(|title| ascii_name(title.trim(), MAX_TITLE))
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    Some(Ok(Opened {
        picture,
        input,
        title,
    }))
}

/// `istesso-edit <file>`: what this program does under that name, inside
/// a shell of another device. Returns the exit status.
pub fn run_as_edit(args: &[OsString]) -> u8 {
    let say = |line: &str| {
        let _ = writeln!(std::io::stderr(), "{EDIT_NAME}: {line}");
    };
    let [one] = args else {
        say("give one file to open in the main editor");
        return 2;
    };
    let Some(text) = one.to_str() else {
        say(&Refusal::Malformed.to_string());
        return 1;
    };
    let place = if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        match std::env::current_dir() {
            Ok(here) => here.join(text),
            Err(err) => {
                say(&format!("cannot tell which folder this is: {err}"));
                return 1;
            }
        }
    };
    let Some(place) = place.to_str().filter(|place| {
        !place.is_empty() && place.len() <= MAX_PLACE && !place.chars().any(char::is_control)
    }) else {
        say(&Refusal::Malformed.to_string());
        return 1;
    };
    let Some(socket) = std::env::var_os(super::opener::SOCKET_VARIABLE).map(PathBuf::from) else {
        say("this is not a shell opened from another device with Istesso");
        return 1;
    };
    let reply = (|| -> std::io::Result<String> {
        let mut stream = UnixStream::connect(&socket)?;
        stream.set_read_timeout(Some(LAUNCH_WITHIN + Duration::from_secs(5)))?;
        stream.write_all(format!("{FILE_REQUEST}{place}\n").as_bytes())?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        Ok(reply)
    })();
    match reply {
        Ok(reply) if reply.trim_end() == "ok" => 0,
        Ok(reply) if reply.is_empty() => {
            say("the Istesso agent did not answer");
            1
        }
        Ok(reply) => {
            say(reply.trim_end().trim_start_matches("refused: "));
            1
        }
        Err(err) => {
            say(&format!("cannot reach the Istesso agent: {err}"));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn folder(tag: &str) -> (TempDir, PathBuf) {
        let dir = TempDir::new(tag);
        let real = dir.path().canonicalize().unwrap();
        (dir, real)
    }

    fn at(base: &Path, rest: &str) -> String {
        format!("{}/{rest}", base.display())
    }

    #[test]
    fn a_file_inside_the_folder_is_given_back_as_the_host_built_it() {
        let (_dir, base) = folder("ed-inside");
        std::fs::create_dir_all(base.join("src/deep")).unwrap();
        std::fs::write(base.join("src/deep/notes.txt"), "x").unwrap();
        assert_eq!(
            checked_place(&base, &at(&base, "src/deep/notes.txt")),
            Ok(base.join("src/deep/notes.txt"))
        );
        // `.` and doubled slashes are read past.
        assert_eq!(
            checked_place(&base, &at(&base, "./src//deep/./notes.txt")),
            Ok(base.join("src/deep/notes.txt"))
        );
        // A file that is not there yet, in a folder that is.
        assert_eq!(
            checked_place(&base, &at(&base, "src/new.txt")),
            Ok(base.join("src/new.txt"))
        );
    }

    #[test]
    fn nothing_outside_the_folder_is_opened_whatever_the_way() {
        let (_dir, base) = folder("ed-outside");
        let (_other, elsewhere) = folder("ed-elsewhere");
        std::fs::write(elsewhere.join("secret.txt"), "x").unwrap();
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/a.txt"), "x").unwrap();
        // Outside, and a folder whose name only begins like the project's.
        assert_eq!(
            checked_place(&base, &at(&elsewhere, "secret.txt")),
            Err(Refusal::Outside)
        );
        let sibling = format!("{}x/secret.txt", base.display());
        assert_eq!(checked_place(&base, &sibling), Err(Refusal::Outside));
        assert_eq!(checked_place(&base, "/etc/passwd"), Err(Refusal::Outside));
        // Climbing out, even when it would come back in.
        assert_eq!(
            checked_place(&base, &at(&base, "../secret.txt")),
            Err(Refusal::Climbs)
        );
        assert_eq!(
            checked_place(&base, &at(&base, "src/../src/a.txt")),
            Err(Refusal::Climbs)
        );
        // Links, on the way and at the end.
        std::os::unix::fs::symlink(&elsewhere, base.join("away")).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("secret.txt"), base.join("src/link.txt"))
            .unwrap();
        assert_eq!(
            checked_place(&base, &at(&base, "away/secret.txt")),
            Err(Refusal::Link)
        );
        assert_eq!(
            checked_place(&base, &at(&base, "src/link.txt")),
            Err(Refusal::Link)
        );
        // A link that points inside is refused too: no link is followed.
        std::os::unix::fs::symlink(base.join("src/a.txt"), base.join("in.txt")).unwrap();
        assert_eq!(
            checked_place(&base, &at(&base, "in.txt")),
            Err(Refusal::Link)
        );
        // A hard link to a file outside has two names.
        if std::fs::hard_link(elsewhere.join("secret.txt"), base.join("hard.txt")).is_ok() {
            assert_eq!(
                checked_place(&base, &at(&base, "hard.txt")),
                Err(Refusal::MoreThanOneName)
            );
        }
    }

    #[test]
    fn only_ordinary_files_are_opened_and_the_request_is_read_strictly() {
        let (_dir, base) = folder("ed-kinds");
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("file"), "x").unwrap();
        assert_eq!(
            checked_place(&base, &at(&base, "src")),
            Err(Refusal::NotAFile)
        );
        assert_eq!(
            checked_place(&base, &base.display().to_string()),
            Err(Refusal::TheFolderItself)
        );
        assert_eq!(
            checked_place(&base, &format!("{}/", base.display())),
            Err(Refusal::TheFolderItself)
        );
        assert_eq!(
            checked_place(&base, &at(&base, "file/inside.txt")),
            Err(Refusal::NotAFolder)
        );
        assert_eq!(
            checked_place(&base, &at(&base, "nowhere/new.txt")),
            Err(Refusal::NotAFolder)
        );
        let fifo = base.join("pipe");
        let c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: a name that ends with NUL.
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } == 0 {
            assert_eq!(
                checked_place(&base, &at(&base, "pipe")),
                Err(Refusal::NotAFile)
            );
        }
        for bad in [
            String::new(),
            "relative/file".to_string(),
            at(&base, "a\nb"),
            at(&base, "a\u{1b}[2J"),
            at(&base, &"a".repeat(MAX_PLACE)),
        ] {
            assert_eq!(
                checked_place(&base, &bad),
                Err(Refusal::Malformed),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn the_root_and_the_home_folder_are_no_workspace_folder() {
        let (_dir, base) = folder("ed-ws");
        let (_home, home) = folder("ed-home");
        assert_eq!(workspace_folder(&base, &home), Some(base.clone()));
        assert_eq!(workspace_folder(&home, &home), None);
        assert_eq!(workspace_folder(Path::new("/"), &home), None);
        assert_eq!(workspace_folder(&base.join("not-there"), &home), None);
    }

    /// A launcher of the test's own: writes its arguments down, then runs
    /// `then` (shell code).
    fn launcher(dir: &Path, then: &str) -> (PathBuf, PathBuf) {
        let program = dir.join("launcher.sh");
        let args = dir.join("args.txt");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$#\" \"$@\" > '{}'\n{then}\n",
                args.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        (program, args)
    }

    #[test]
    fn the_launcher_gets_one_argument_and_says_where_the_window_waits() {
        let (_dir, base) = folder("ed-launch");
        let (program, args) = launcher(
            &base,
            "echo 'starting'\necho 'window 26891 26892 Text Editor'",
        );
        let editor = MainEditor::new(&program).unwrap();
        let place = base.join("a file with spaces; $(no) `shell`.txt");
        assert_eq!(
            editor.open(&place),
            Ok(Opened {
                picture: 26891,
                input: 26892,
                title: "Text Editor".into()
            })
        );
        let written = std::fs::read_to_string(&args).unwrap();
        assert_eq!(written, format!("1\n{}\n", place.display()));
    }

    #[test]
    fn a_launcher_that_fails_or_says_nothing_is_a_refusal() {
        let (_dir, base) = folder("ed-fail");
        let place = base.join("x.txt");
        let (program, _) = launcher(&base, "echo 'fail the encoder is taken'; exit 1");
        assert_eq!(
            MainEditor::new(&program).unwrap().open(&place),
            Err("the main editor did not start: the encoder is taken".into())
        );
        let (program, _) = launcher(&base, "echo 'window 0 1'");
        assert!(MainEditor::new(&program).unwrap().open(&place).is_err());
        let (program, _) = launcher(&base, "exit 0");
        assert_eq!(
            MainEditor::new(&program).unwrap().open(&place),
            Err("the main editor's launcher ended without a window".into())
        );
        let (program, _) = launcher(&base, "exec sleep 5");
        let began = std::time::Instant::now();
        assert!(MainEditor::new(&program)
            .unwrap()
            .with_time_limit(Duration::from_millis(300))
            .open(&place)
            .unwrap_err()
            .contains("did not start within"));
        assert!(began.elapsed() < Duration::from_secs(3));
        // The setting must name a program.
        assert!(MainEditor::new(Path::new("relative/launcher")).is_err());
        assert!(MainEditor::new(&base).is_err());
        std::fs::write(base.join("plain"), "").unwrap();
        assert!(MainEditor::new(&base.join("plain")).is_err());
    }

    #[test]
    fn the_asker_refuses_what_it_cannot_send_before_asking_anyone() {
        // No socket is named: these must fail before one would be needed.
        assert_eq!(run_as_edit(&[]), 2);
        assert_eq!(run_as_edit(&[OsString::from("a"), OsString::from("b")]), 2);
        assert_eq!(run_as_edit(&[OsString::from("a\nb")]), 1);
    }
}
