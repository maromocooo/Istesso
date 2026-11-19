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
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use istesso_link::forward::ascii_name;
use istesso_link::shell::harmless_title;

use super::watch::Workspace;

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
/// project folder by its real place, or none when it is the root, the home
/// folder itself or a folder above it, like `/home` (then "inside" would be
/// everything of this user's).
pub fn workspace_folder(project: &Path, home: &Path) -> Option<PathBuf> {
    let folder = project.canonicalize().ok()?;
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    (folder.parent().is_some() && !home.starts_with(&folder)).then_some(folder)
}

/// The place to hand the editor, built from `folder` (the workspace's,
/// by its real place) and the names of `asked` below it, once each name on
/// the way has been looked at without following a link: folders all the
/// way, and at the end an ordinary file with one name, or no file yet (the
/// editor makes it when it saves).
///
/// Every name is looked at from the root down, the workspace's folder's own
/// names too: a folder above the place that has been changed into a link
/// since the shell was opened is refused like any other link.
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
    let mut at = open_root()?;
    for name in base.iter().chain(on_the_way) {
        at = open_folder(&at, name)?;
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

/// The root folder.
fn open_root() -> Result<OwnedFd, Refusal> {
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC;
    // SAFETY: a name that ends with NUL, and flags; the descriptor returned
    // is owned here.
    let fd = unsafe { libc::open(c"/".as_ptr(), flags) };
    if fd < 0 {
        return Err(Refusal::Unreadable);
    }
    // SAFETY: a descriptor that was just opened and belongs to no one else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Opens the folder `name` in `at`, never through a symbolic link.
fn open_folder(at: &OwnedFd, name: &OsStr) -> Result<OwnedFd, Refusal> {
    let c = c_name(name)?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    // SAFETY: a name that ends with NUL, and flags; the descriptor returned
    // is owned here.
    let fd = unsafe { libc::openat(at.as_raw_fd(), c.as_ptr(), flags) };
    if fd >= 0 {
        // SAFETY: a descriptor that was just opened and belongs to no one else.
        return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
    }
    // Why, without following a link: a link, or something else, or nothing.
    match look(at, name) {
        Ok(found) if found.st_mode & libc::S_IFMT == libc::S_IFLNK => Err(Refusal::Link),
        _ => Err(Refusal::NotAFolder),
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
///
/// One editor runs at a time, for one shell: the shell whose workspace
/// asked for it. When that shell is over (it ended, its device went, or its
/// device lost its right), the editor and what shows it are ended
/// ([`MainEditor::workspace_over`]). A request from another shell ends the
/// editor of the first one and starts one afresh, so that no shell sees
/// what another one opened.
///
/// The launcher is told what to do by one argument (`open` or `stop`) and,
/// for `open`, by its environment: `EDITOR_WINDOW_PLACE` (the checked
/// place), `EDITOR_WINDOW_DEVICE_ID` and `EDITOR_WINDOW_DEVICE_KEY` (the
/// asking shell's device, the only one its window is shown to). The place
/// is never an argument, where any program of the machine could read it.
pub struct MainEditor {
    launcher: PathBuf,
    within: Duration,
    /// The workspace whose editor runs; held while one is started, so that
    /// one start runs at a time (the launcher chooses the ports).
    owner: Mutex<Option<Arc<Workspace>>>,
}

/// How long the launcher has to end what it started.
const STOP_WITHIN: Duration = Duration::from_secs(15);

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
            owner: Mutex::new(None),
        })
    }

    /// The same, with another time limit (for tests).
    pub fn with_time_limit(mut self, within: Duration) -> MainEditor {
        self.within = within;
        self
    }

    /// Opens `place` (one that [`checked_place`] gave) in the editor of
    /// `workspace`'s shell, started now when there is none, and says where
    /// its window waits. The launcher answers with one line:
    /// `window <picture port> <input port> [title]`, or `fail <why>`.
    pub fn open(&self, place: &Path, workspace: &Arc<Workspace>) -> Result<Opened, String> {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if workspace.is_over() {
            return Err("the shell is over".into());
        }
        // The editor of another shell is ended first.
        if owner
            .as_ref()
            .is_some_and(|earlier| !Arc::ptr_eq(earlier, workspace))
        {
            self.stop();
            *owner = None;
        }
        let opened = self.launch(place, workspace);
        if opened.is_ok() {
            *owner = Some(workspace.clone());
        } else {
            // Whatever it started is ended, by the launcher's own records.
            self.stop();
            *owner = None;
        }
        opened
    }

    /// `workspace`'s shell is over: its editor, when it has one, is ended.
    /// Returns whether it had one.
    pub fn workspace_over(&self, workspace: &Arc<Workspace>) -> bool {
        let mut owner = self.owner.lock().unwrap_or_else(|e| e.into_inner());
        if owner
            .as_ref()
            .is_some_and(|running| Arc::ptr_eq(running, workspace))
        {
            self.stop();
            *owner = None;
            return true;
        }
        false
    }

    fn launch(&self, place: &Path, workspace: &Workspace) -> Result<Opened, String> {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new(&self.launcher);
        command
            .arg("open")
            .env("EDITOR_WINDOW_PLACE", place)
            .env("EDITOR_WINDOW_DEVICE_ID", workspace.device_id())
            .env("EDITOR_WINDOW_DEVICE_KEY", workspace.device_key())
            // A process group of its own: on a time-out, the launcher and
            // whatever it started that stayed in its group are ended together.
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = spawn(&mut command)
            .map_err(|e| format!("the main editor's launcher did not start: {e}"))?;
        let group = child.id() as libc::pid_t;
        let end_group = || {
            // SAFETY: a signal to the process group this launcher leads.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        };
        let Some(out) = child.stdout.take() else {
            end_group();
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
            end_group();
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
                    end_group();
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

    /// Asks the launcher to end everything it started (by the numbers it
    /// wrote down), and waits for it a while.
    fn stop(&self) {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new(&self.launcher);
        command
            .arg("stop")
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let Ok(mut child) = spawn(&mut command) else {
            return;
        };
        let deadline = std::time::Instant::now() + STOP_WITHIN;
        while std::time::Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: a signal to the process group this launcher leads.
        unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
        let _ = child.wait();
    }
}

/// Starts `command`, again a few times when the system says that its
/// program is being written (Linux: a program just written while another
/// thread of this process starts one is open for writing in that one for a
/// moment).
fn spawn(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut tries = 0;
    loop {
        match command.spawn() {
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && tries < 10 => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            other => return other,
        }
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
        .map(|title| harmless_title(title.trim()))
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
    fn the_root_the_home_folder_and_folders_above_it_are_no_workspace_folder() {
        let (_dir, base) = folder("ed-ws");
        let (_home, home) = folder("ed-home");
        assert_eq!(workspace_folder(&base, &home), Some(base.clone()));
        assert_eq!(workspace_folder(&home, &home), None);
        assert_eq!(workspace_folder(Path::new("/"), &home), None);
        assert_eq!(workspace_folder(&base.join("not-there"), &home), None);
        // A folder above the home folder, like /home.
        std::fs::create_dir_all(base.join("someone")).unwrap();
        assert_eq!(workspace_folder(&base, &base.join("someone")), None);
        // A folder below it is a project.
        assert_eq!(
            workspace_folder(&base.join("someone"), &base),
            Some(base.join("someone"))
        );
    }

    #[test]
    fn a_folder_of_the_workspace_that_has_become_a_link_is_refused() {
        let (_dir, outer) = folder("ed-base");
        let base = outer.join("project");
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("a.txt"), "x").unwrap();
        assert!(checked_place(&base, &at(&base, "a.txt")).is_ok());
        // After the shell opened, the project folder is moved and a link put
        // in its place.
        std::fs::rename(&base, outer.join("moved")).unwrap();
        std::os::unix::fs::symlink(outer.join("moved"), &base).unwrap();
        assert_eq!(
            checked_place(&base, &at(&base, "a.txt")),
            Err(Refusal::Link)
        );
    }

    fn workspace(name: &str) -> Arc<Workspace> {
        let peer = istesso_pair::Peer::from_hello(&istesso_pair::Hello {
            device_name: name.into(),
            public_key: [7; 32],
        });
        Workspace::new(&peer, 1, super::super::watch::How::Session)
    }

    /// A launcher of the test's own: writes down what it is told (its
    /// arguments and the place and device it is given), then, for `open`,
    /// runs `then` (shell code).
    fn launcher(dir: &Path, then: &str) -> (PathBuf, PathBuf) {
        let program = dir.join("launcher.sh");
        let told = dir.join("told.txt");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s|%s|%s|%s\\n' \"$#\" \"$1\" \"$EDITOR_WINDOW_PLACE\" \"$EDITOR_WINDOW_DEVICE_ID\" >> '{}'\n[ \"$1\" = stop ] && exit 0\n{then}\n",
                told.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        (program, told)
    }

    fn lines(file: &Path) -> Vec<String> {
        std::fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn the_launcher_gets_the_place_in_its_environment_and_says_where_the_window_waits() {
        let (_dir, base) = folder("ed-launch");
        let (program, told) = launcher(
            &base,
            "echo 'starting'\necho 'window 26891 26892 --gst-plugin-path=x'",
        );
        let editor = MainEditor::new(&program).unwrap();
        let place = base.join("a file with spaces; $(no) `shell`.txt");
        let shell = workspace("a laptop");
        assert_eq!(
            editor.open(&place, &shell),
            Ok(Opened {
                picture: 26891,
                input: 26892,
                // Never an option for whatever it is handed to.
                title: "?-gst-plugin-path=x".into()
            })
        );
        assert_eq!(
            lines(&told),
            [format!("1|open|{}|{}", place.display(), shell.device_id())]
        );
    }

    #[test]
    fn the_editor_goes_with_the_shell_that_asked_and_another_shell_gets_one_afresh() {
        let (_dir, base) = folder("ed-owner");
        let (program, told) = launcher(&base, "echo 'window 26891 26892'");
        let editor = MainEditor::new(&program).unwrap();
        let (first, second) = (workspace("a laptop"), workspace("a laptop"));
        let place = base.join("x.txt");
        editor.open(&place, &first).unwrap();
        // The same shell again: handed to the editor that runs.
        editor.open(&place, &first).unwrap();
        // Another shell: the first one's editor is ended before.
        editor.open(&place, &second).unwrap();
        // The first shell is over: not the owner any more, nothing is ended.
        assert!(!editor.workspace_over(&first));
        // The second one is over: its editor is ended.
        assert!(editor.workspace_over(&second));
        let words: Vec<String> = lines(&told)
            .iter()
            .map(|line| line.split('|').nth(1).unwrap().to_string())
            .collect();
        assert_eq!(words, ["open", "open", "stop", "open", "stop"]);
        // A shell that is over may not start one.
        first.end();
        assert!(editor.open(&place, &first).is_err());
    }

    #[test]
    fn a_launcher_that_fails_or_says_nothing_is_a_refusal_and_is_told_to_stop() {
        let (_dir, base) = folder("ed-fail");
        let place = base.join("x.txt");
        let shell = workspace("a laptop");
        let (program, told) = launcher(&base, "echo 'fail the encoder is taken'; exit 1");
        assert_eq!(
            MainEditor::new(&program).unwrap().open(&place, &shell),
            Err("the main editor did not start: the encoder is taken".into())
        );
        assert_eq!(lines(&told).last().unwrap().split('|').nth(1), Some("stop"));
        let (program, _) = launcher(&base, "echo 'window 0 1'");
        assert!(MainEditor::new(&program)
            .unwrap()
            .open(&place, &shell)
            .is_err());
        let (program, _) = launcher(&base, "exit 0");
        assert_eq!(
            MainEditor::new(&program).unwrap().open(&place, &shell),
            Err("the main editor's launcher ended without a window".into())
        );
        // The setting must name a program.
        assert!(MainEditor::new(Path::new("relative/launcher")).is_err());
        assert!(MainEditor::new(&base).is_err());
        std::fs::write(base.join("plain"), "").unwrap();
        assert!(MainEditor::new(&base.join("plain")).is_err());
    }

    #[test]
    fn on_a_time_out_the_launcher_and_what_it_started_in_its_group_are_ended() {
        let (_dir, base) = folder("ed-timeout");
        let place = base.join("x.txt");
        let child = base.join("child.pid");
        let (program, told) = launcher(
            &base,
            &format!("sleep 30 &\necho $! > '{}'\nexec sleep 30", child.display()),
        );
        let began = std::time::Instant::now();
        assert!(MainEditor::new(&program)
            .unwrap()
            .with_time_limit(Duration::from_secs(2))
            .open(&place, &workspace("a laptop"))
            .unwrap_err()
            .contains("did not start within"));
        assert!(began.elapsed() < Duration::from_secs(8));
        let pid: i32 = std::fs::read_to_string(&child)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The launcher's child is gone (after a moment to be collected by
        // whoever adopted it).
        let gone = || {
            // SAFETY: signal 0 only asks whether the process is there.
            unsafe { libc::kill(pid, 0) != 0 }
        };
        let waited = std::time::Instant::now();
        while !gone() && waited.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(gone(), "the launcher's child is still there");
        // And the launcher was asked to end what it had started elsewhere.
        assert_eq!(lines(&told).last().unwrap().split('|').nth(1), Some("stop"));
    }

    #[test]
    fn the_asker_refuses_what_it_cannot_send_before_asking_anyone() {
        // No socket is named: these must fail before one would be needed.
        assert_eq!(run_as_edit(&[]), 2);
        assert_eq!(run_as_edit(&[OsString::from("a"), OsString::from("b")]), 2);
        assert_eq!(run_as_edit(&[OsString::from("a\nb")]), 1);
    }
}
