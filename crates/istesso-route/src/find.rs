//! Finding the project's task file, and refusing one that cannot be trusted.
//!
//! The commands in `.istesso/tasks.json` are started with this user's
//! rights, so a file that someone else could have put there or changed is
//! never used:
//!
//! - The search goes from the start folder up to the project's boundary: the
//!   first folder upwards that holds `.git` (a file or a folder). Without one,
//!   only the start folder is looked at. The search for `.git` never goes
//!   above the home folder and never reaches the root of the file system.
//! - The start folder is canonicalized once. The `.istesso` folder may be a
//!   link only to a place inside the boundary, and the file only to a place
//!   inside that (real) `.istesso` folder.
//! - On Unix, every folder from the boundary down to the file's real folder
//!   must belong to this user or to root, and the file to this user; none of
//!   them may be writable by group or others, and none may carry an access
//!   control list (ACL) at all, since an ACL can grant writing beyond the
//!   mode bits.
//! - Every folder above the boundary, up to the root of the file system,
//!   must belong to this user or to root and must not be writable by group
//!   or others unless its sticky bit is set (in a sticky folder no one can
//!   rename or remove another user's entries, so a project under a shared
//!   temporary folder is judged by its own folders). Otherwise someone could
//!   swap the whole project for a tree of their own. ACLs are not refused up
//!   there: on macOS a user's Documents, Desktop and Downloads carry a
//!   deny-delete ACL by default, and projects live under them.
//! - The file is opened once; its owner and mode are checked on the open
//!   handle, and the text is read from that same handle. The ACL is checked
//!   by path, which is sound because no one else can write to the folders
//!   that lead to it.
//!
//! Elsewhere than Unix, owner, mode and ACL are not checked; the boundary and
//! the link rules still are.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::tasks::TASKS_FILE;

/// A task file that may be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The file's real location.
    pub file: PathBuf,
    /// The project's root: the folder that holds `.istesso`.
    pub root: PathBuf,
    /// The file's text, read from the handle that was checked.
    pub text: String,
}

/// Why a task file that was found is not used. Never skipped silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindError {
    /// The file or a folder leading to it belongs to another user.
    NotOwned { path: PathBuf },
    /// The file or a folder leading to it can be written by group or others.
    Writable { path: PathBuf },
    /// A folder above the project's root belongs to another user.
    AboveNotOwned { path: PathBuf },
    /// A folder above the project's root can be written by group or others
    /// and is not sticky.
    AboveWritable { path: PathBuf },
    /// The file or a folder leading to it carries an access control list.
    HasAcl { path: PathBuf },
    /// A link that leads out of the place it must stay in.
    OutsideProject { path: PathBuf, allowed: PathBuf },
    /// Not a regular file (or, for a folder, not a folder).
    NotAFile { path: PathBuf },
    /// It cannot be looked at (e.g. a broken link), or the ACL check failed.
    Unreadable { path: PathBuf, why: String },
}

impl fmt::Display for FindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let run_as_you = "and the task file's commands would run as you";
        match self {
            FindError::NotOwned { path } => write!(
                f,
                "{} is not used: it belongs to another user, {run_as_you}",
                path.display()
            ),
            FindError::Writable { path } => write!(
                f,
                "{} is not used: others can write to it, {run_as_you} (`chmod go-w` it if that is not meant)",
                path.display()
            ),
            FindError::AboveNotOwned { path } => write!(
                f,
                "the project is not used: {}, a folder above it, belongs to another user, who could replace the project with their own, {run_as_you}",
                path.display()
            ),
            FindError::AboveWritable { path } => write!(
                f,
                "the project is not used: others can write to {}, a folder above it, and could replace the project with their own, {run_as_you} (`chmod go-w` it, or set its sticky bit, if that is not meant)",
                path.display()
            ),
            FindError::HasAcl { path } => write!(
                f,
                "{} is not used: it carries an access control list, which may let others write to it, {run_as_you}",
                path.display()
            ),
            FindError::OutsideProject { path, allowed } => write!(
                f,
                "{} is not used: it is a link that leads out of {}",
                path.display(),
                allowed.display()
            ),
            FindError::NotAFile { path } => {
                write!(f, "{} is not used: it is not what it should be (a file or a folder)", path.display())
            }
            FindError::Unreadable { path, why } => {
                write!(f, "{} cannot be checked: {why}", path.display())
            }
        }
    }
}

impl std::error::Error for FindError {}

/// Looks for the task file from `start`, with the home folder from `HOME`
/// as the upper limit. See [`find_tasks_within`].
pub fn find_tasks(start: &Path) -> Result<Option<Found>, FindError> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    find_tasks_within(start, home.as_deref())
}

/// Looks for `.istesso/tasks.json` in `start` and its parents up to the
/// project's boundary (see the module's description), checks it and reads it.
pub fn find_tasks_within(start: &Path, home: Option<&Path>) -> Result<Option<Found>, FindError> {
    let start = start.canonicalize().map_err(|e| unreadable(start, e))?;
    let home = home.and_then(|h| h.canonicalize().ok());
    let Some(boundary) = project_boundary(&start, home.as_deref()) else {
        return Ok(None);
    };
    for dir in start.ancestors() {
        if !dir.starts_with(&boundary) {
            break;
        }
        let file = dir.join(TASKS_FILE);
        if std::fs::symlink_metadata(&file).is_err() {
            continue;
        }
        let folder = inside(&dir.join(".istesso"), &boundary)?;
        let real = inside(&file, &folder)?;
        let parent = real.parent().unwrap_or(&folder);
        for step in parent.ancestors() {
            if !step.starts_with(&boundary) {
                break;
            }
            check_folder(step)?;
        }
        for above in boundary.ancestors().skip(1) {
            check_above(above)?;
        }
        let text = read_checked(&real)?;
        return Ok(Some(Found {
            file: real,
            root: dir.to_path_buf(),
            text,
        }));
    }
    Ok(None)
}

/// The highest folder the search may look in: the first folder from `start`
/// upwards that holds `.git`, else `start` itself. `None` when `start` is
/// the root of the file system.
pub(crate) fn project_boundary(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    start.parent()?;
    for dir in start.ancestors() {
        if dir.parent().is_none() {
            break;
        }
        if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Some(dir.to_path_buf());
        }
        if Some(dir) == home {
            break;
        }
    }
    Some(start.to_path_buf())
}

fn unreadable(path: &Path, e: impl fmt::Display) -> FindError {
    FindError::Unreadable {
        path: path.to_path_buf(),
        why: e.to_string(),
    }
}

/// The canonical place of `path`, which must lie inside `allowed`.
fn inside(path: &Path, allowed: &Path) -> Result<PathBuf, FindError> {
    let real = path.canonicalize().map_err(|e| unreadable(path, e))?;
    if !real.starts_with(allowed) {
        return Err(FindError::OutsideProject {
            path: path.to_path_buf(),
            allowed: allowed.to_path_buf(),
        });
    }
    Ok(real)
}

/// A folder on the way to the file: this user's or root's, not writable by
/// group or others, without an ACL.
#[cfg(unix)]
fn check_folder(path: &Path) -> Result<(), FindError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| unreadable(path, e))?;
    if !meta.is_dir() {
        return Err(FindError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    let uid = meta.uid();
    if uid != effective_uid() && uid != 0 {
        return Err(FindError::NotOwned {
            path: path.to_path_buf(),
        });
    }
    if meta.mode() & 0o022 != 0 {
        return Err(FindError::Writable {
            path: path.to_path_buf(),
        });
    }
    check_no_acl(path)
}

#[cfg(not(unix))]
fn check_folder(_path: &Path) -> Result<(), FindError> {
    Ok(())
}

/// A folder above the project's root: this user's or root's, and not
/// writable by group or others unless sticky. Its ACL is not looked at.
#[cfg(unix)]
fn check_above(path: &Path) -> Result<(), FindError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| unreadable(path, e))?;
    let uid = meta.uid();
    if uid != effective_uid() && uid != 0 {
        return Err(FindError::AboveNotOwned {
            path: path.to_path_buf(),
        });
    }
    let sticky = meta.mode() & 0o1000 != 0;
    if meta.mode() & 0o022 != 0 && !sticky {
        return Err(FindError::AboveWritable {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_above(_path: &Path) -> Result<(), FindError> {
    Ok(())
}

/// Opens the file once, checks the open handle, and reads from it.
fn read_checked(path: &Path) -> Result<String, FindError> {
    let mut file = std::fs::File::open(path).map_err(|e| unreadable(path, e))?;
    let meta = file.metadata().map_err(|e| unreadable(path, e))?;
    if !meta.is_file() {
        return Err(FindError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != effective_uid() {
            return Err(FindError::NotOwned {
                path: path.to_path_buf(),
            });
        }
        if meta.mode() & 0o022 != 0 {
            return Err(FindError::Writable {
                path: path.to_path_buf(),
            });
        }
        check_no_acl(path)?;
    }
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| unreadable(path, e))?;
    Ok(text)
}

/// Refuses a path that carries an access control list.
///
/// The standard library has no call for ACLs, and this crate takes no new
/// dependency and no `unsafe` for it, so `ls` is asked (no shell, the path
/// as one argument, `LC_ALL=C`). On macOS `ls -lde` prints one line per ACL
/// entry after the file's line; elsewhere `ls -ld` marks a file that has an
/// ACL with `+` right after the mode. Anything unexpected is a refusal.
#[cfg(unix)]
fn check_no_acl(path: &Path) -> Result<(), FindError> {
    if path.as_os_str().as_encoded_bytes().contains(&b'\n') {
        return Err(unreadable(path, "the path holds a line break"));
    }
    let flags = if cfg!(target_os = "macos") {
        "-lde"
    } else {
        "-ld"
    };
    let output = std::process::Command::new("/bin/ls")
        .arg(flags)
        .arg("--")
        .arg(path)
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            unreadable(
                path,
                format!("its access control list could not be checked: {e}"),
            )
        })?;
    if !output.status.success() {
        return Err(unreadable(
            path,
            "its access control list could not be checked (`ls` failed)",
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    if has_acl(&text, cfg!(target_os = "macos")) {
        return Err(FindError::HasAcl {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

/// Reads the output of `ls` for one path (see [`check_no_acl`]).
#[cfg(unix)]
fn has_acl(listing: &str, macos: bool) -> bool {
    if macos {
        listing.lines().filter(|l| !l.trim().is_empty()).count() > 1
    } else {
        // The mode is ten characters; an ACL shows as `+` in the eleventh.
        listing.lines().next().and_then(|l| l.chars().nth(10)) == Some('+')
    }
}

/// The user this process runs as (its effective user id).
///
/// The standard library has no safe call that gives the effective user id
/// (`std::os::unix` exposes the owner of files, not of the process), and
/// this crate takes no new dependency, so the C library's `geteuid` is
/// declared here; it is the crate's only `unsafe`.
#[cfg(unix)]
pub(crate) fn effective_uid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid(2) cannot fail: POSIX says it "shall always be
    // successful", it takes no arguments and touches no memory of the
    // caller. `uid_t` is a 32-bit unsigned integer on the Unix systems this
    // builds for (Linux, macOS), which matches the declaration.
    unsafe { geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = r#"{"version": 1, "tasks": {}}"#;

    /// A folder of its own under the temporary folder, removed afterwards.
    /// It also serves as `home`, so that nothing above it is ever looked at.
    struct Base(PathBuf);

    impl Base {
        fn new(name: &str) -> Base {
            let dir = std::env::temp_dir()
                .join(format!("istesso-route-find-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            // It is above every project here, so it must pass that check
            // whatever the umask.
            set_mode(&dir, 0o755);
            Base(dir.canonicalize().unwrap())
        }

        fn find(&self, start: &str) -> Result<Option<Found>, FindError> {
            find_tasks_within(&self.0.join(start), Some(&self.0))
        }

        fn found(&self, start: &str) -> PathBuf {
            self.find(start).unwrap().expect("a task file").file
        }

        fn dir(&self, rel: impl AsRef<Path>) -> PathBuf {
            let rel = rel.as_ref();
            assert!(rel.is_relative());
            let dir = self.0.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            // Every folder from the base down, whatever the umask.
            let mut step = self.0.clone();
            for part in rel.components() {
                step.push(part);
                set_mode(&step, 0o755);
            }
            dir
        }

        /// A project at `rel` with `.git`.
        fn project(&self, rel: &str) -> PathBuf {
            self.dir(Path::new(rel).join(".git"));
            self.0.join(rel)
        }

        /// A task file in `rel`, owned by this user and writable by it alone.
        fn tasks(&self, rel: &str) -> PathBuf {
            let folder = self.dir(Path::new(rel).join(".istesso"));
            let file = folder.join("tasks.json");
            std::fs::write(&file, TEXT).unwrap();
            set_mode(&file, 0o644);
            file
        }
    }

    impl Drop for Base {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = (path, mode);
    }

    #[test]
    fn the_search_from_a_subfolder_finds_the_project_s_file() {
        let base = Base::new("sub");
        base.project("proj");
        let file = base.tasks("proj");
        base.dir("proj/a/b/c");
        let found = base.find("proj/a/b/c").unwrap().unwrap();
        assert_eq!(found.file, file);
        assert_eq!(found.root, base.0.join("proj"));
        assert_eq!(found.text, TEXT);
        // `.git` may be a file too (a worktree).
        let other = Base::new("sub-file");
        other.dir("proj/x");
        std::fs::write(other.0.join("proj/.git"), "gitdir: elsewhere\n").unwrap();
        other.tasks("proj");
        assert!(other.find("proj/x").unwrap().is_some());
    }

    #[test]
    fn a_file_above_the_project_root_is_not_taken() {
        let base = Base::new("above");
        base.tasks("outer");
        base.project("outer/proj");
        base.dir("outer/proj/sub");
        assert_eq!(base.find("outer/proj/sub").unwrap(), None);
        assert_eq!(base.find("outer/proj").unwrap(), None);
    }

    #[test]
    fn without_a_project_root_only_the_start_folder_is_looked_at() {
        let base = Base::new("norepo");
        base.tasks("work");
        base.dir("work/sub");
        assert_eq!(base.find("work/sub").unwrap(), None);
        assert!(base.find("work").unwrap().is_some());
    }

    #[test]
    fn the_search_stops_at_home() {
        // `.git` and a task file above home are not used.
        let base = Base::new("home");
        base.dir(".git");
        let above = base.0.join(".istesso");
        std::fs::create_dir_all(&above).unwrap();
        std::fs::write(above.join("tasks.json"), TEXT).unwrap();
        base.dir("me/work");
        let home = base.0.join("me");
        assert_eq!(
            find_tasks_within(&home.join("work"), Some(&home)).unwrap(),
            None
        );
        // Home itself may be the project.
        base.project("me");
        base.tasks("me");
        assert!(find_tasks_within(&home.join("work"), Some(&home))
            .unwrap()
            .is_some());
    }

    #[test]
    fn the_root_of_the_file_system_is_never_a_project() {
        assert_eq!(project_boundary(Path::new("/"), None), None);
        assert_eq!(find_tasks_within(Path::new("/"), None).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_or_folder_others_can_write_is_refused() {
        let base = Base::new("writable");
        base.project("proj");
        let file = base.tasks("proj");
        assert_eq!(base.found("proj"), file);

        for mode in [0o666, 0o664, 0o646] {
            set_mode(&file, mode);
            let err = base.find("proj").unwrap_err();
            assert_eq!(err, FindError::Writable { path: file.clone() }, "{mode:o}");
            assert!(err.to_string().contains("others can write"));
        }
        set_mode(&file, 0o644);

        // Every folder from the boundary down: `.istesso`, a folder the file
        // is linked into, and the project's root.
        let folder = file.parent().unwrap().to_path_buf();
        set_mode(&folder, 0o775);
        assert_eq!(
            base.find("proj").unwrap_err(),
            FindError::Writable {
                path: folder.clone()
            }
        );
        set_mode(&folder, 0o755);
        let root = base.0.join("proj");
        set_mode(&root, 0o777);
        assert_eq!(
            base.find("proj").unwrap_err(),
            FindError::Writable { path: root.clone() }
        );
        set_mode(&root, 0o755);
        assert_eq!(base.found("proj"), file);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_owned_as_expected_is_taken() {
        use std::os::unix::fs::MetadataExt;
        let base = Base::new("owned");
        base.project("proj");
        let file = base.tasks("proj");
        assert_eq!(std::fs::metadata(&file).unwrap().uid(), effective_uid());
        assert_eq!(read_checked(&file).unwrap(), TEXT);
        assert_eq!(base.found("proj"), file);
        // Files and folders of another user. Creating one needs root, so the
        // checks are tried on what root owns. A folder of root is allowed on
        // the way; a file of root is not.
        if effective_uid() != 0 {
            assert!(matches!(
                read_checked(Path::new("/etc/hosts")),
                Err(FindError::NotOwned { .. })
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_out_of_the_checked_folders_are_refused() {
        use std::os::unix::fs::symlink;
        let base = Base::new("link");
        let outside = base.tasks("elsewhere");

        // The file as a link out of the project.
        base.project("proj");
        base.dir("proj/.istesso");
        symlink(&outside, base.0.join("proj/.istesso/tasks.json")).unwrap();
        assert!(matches!(
            base.find("proj"),
            Err(FindError::OutsideProject { .. })
        ));

        // The folder as a link out of the project.
        base.project("proj2");
        symlink(outside.parent().unwrap(), base.0.join("proj2/.istesso")).unwrap();
        assert!(matches!(
            base.find("proj2"),
            Err(FindError::OutsideProject { .. })
        ));

        // The file as a link inside the project but out of `.istesso`.
        base.project("proj3");
        let real = base.tasks("proj3/config");
        base.dir("proj3/.istesso");
        symlink(&real, base.0.join("proj3/.istesso/tasks.json")).unwrap();
        assert!(matches!(
            base.find("proj3"),
            Err(FindError::OutsideProject { .. })
        ));

        // `.istesso` as a link to a folder inside the project is fine, and
        // so is the file as a link inside `.istesso`.
        base.project("proj4");
        let kept = base.tasks("proj4/config");
        symlink(kept.parent().unwrap(), base.0.join("proj4/.istesso")).unwrap();
        assert_eq!(base.found("proj4"), kept);
        base.project("proj5");
        let folder = base.dir("proj5/.istesso");
        std::fs::write(folder.join("v1.json"), TEXT).unwrap();
        set_mode(&folder.join("v1.json"), 0o644);
        symlink(folder.join("v1.json"), folder.join("tasks.json")).unwrap();
        assert_eq!(base.found("proj5"), folder.join("v1.json"));

        // A broken link cannot be looked at.
        base.project("proj6");
        base.dir("proj6/.istesso");
        symlink(
            base.0.join("nothing"),
            base.0.join("proj6/.istesso/tasks.json"),
        )
        .unwrap();
        assert!(matches!(
            base.find("proj6"),
            Err(FindError::Unreadable { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_instead_of_the_file_is_refused() {
        let base = Base::new("notfile");
        base.project("proj");
        base.dir("proj/.istesso/tasks.json");
        assert!(matches!(base.find("proj"), Err(FindError::NotAFile { .. })));
    }

    #[cfg(unix)]
    #[test]
    fn the_listing_of_ls_is_read_for_acls() {
        let macos_plain = "-rw-r--r--@ 1 u  g  0 Nov 17 11:05 f\n";
        let macos_acl = "-rw-r--r--@ 1 u  g  0 Nov 17 11:05 f\n 0: group:everyone allow write\n";
        assert!(!has_acl(macos_plain, true));
        assert!(has_acl(macos_acl, true));
        assert!(!has_acl("-rw-r--r-- 1 u g 0 Nov 17 11:05 f\n", false));
        assert!(!has_acl("-rw-r--r--. 1 u g 0 Nov 17 11:05 f\n", false));
        assert!(has_acl("-rw-rw-r--+ 1 u g 0 Nov 17 11:05 f\n", false));
    }

    /// Gives `path` an ACL that lets everyone write, with the system's tool.
    /// False when the tool is not there (Linux without `setfacl`).
    #[cfg(unix)]
    fn give_acl(path: &Path, folder: bool) -> bool {
        let status = if cfg!(target_os = "macos") {
            let entry = if folder {
                "everyone allow add_file"
            } else {
                "everyone allow write"
            };
            std::process::Command::new("/bin/chmod")
                .args(["+a", entry])
                .arg(path)
                .status()
        } else {
            std::process::Command::new("setfacl")
                .args(["-m", "u:nobody:rwx"])
                .arg(path)
                .status()
        };
        matches!(status, Ok(s) if s.success())
    }

    #[cfg(unix)]
    #[test]
    fn a_file_or_folder_with_an_acl_is_refused() {
        let base = Base::new("acl");
        base.project("proj");
        let file = base.tasks("proj");
        if !give_acl(&file, false) {
            eprintln!("no tool to set an ACL here; the ACL refusal is not tried");
            return;
        }
        // `setfacl` may also widen the mode; put it back so that only the
        // ACL is left to refuse.
        set_mode(&file, 0o644);
        assert_eq!(
            base.find("proj").unwrap_err(),
            FindError::HasAcl { path: file.clone() }
        );

        let base = Base::new("acl-folder");
        base.project("proj");
        let file = base.tasks("proj");
        let folder = file.parent().unwrap().to_path_buf();
        assert!(give_acl(&folder, true));
        set_mode(&folder, 0o755);
        assert_eq!(
            base.find("proj").unwrap_err(),
            FindError::HasAcl { path: folder }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_project_under_a_group_writable_parent_is_refused() {
        let base = Base::new("above-writable");
        base.project("outer/proj");
        base.tasks("outer/proj");
        let outer = base.0.join("outer");
        set_mode(&outer, 0o775);
        let err = base.find("outer/proj").unwrap_err();
        assert_eq!(
            err,
            FindError::AboveWritable {
                path: outer.clone()
            }
        );
        assert!(err.to_string().contains("a folder above it"));
        set_mode(&outer, 0o757);
        assert!(matches!(
            base.find("outer/proj"),
            Err(FindError::AboveWritable { .. })
        ));
        set_mode(&outer, 0o755);
        assert!(base.find("outer/proj").unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_project_under_a_sticky_shared_folder_is_judged_by_its_own_folders() {
        let base = Base::new("above-sticky");
        base.project("shared/proj");
        base.tasks("shared/proj");
        let shared = base.0.join("shared");
        set_mode(&shared, 0o1777);
        assert!(base.find("shared/proj").unwrap().is_some());
        // Its own folders still count.
        set_mode(&base.0.join("shared/proj"), 0o1777);
        assert!(matches!(
            base.find("shared/proj"),
            Err(FindError::Writable { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn the_usual_home_layout_is_accepted() {
        // A home folder of the user's own (0750), a Documents folder (0700)
        // that on macOS carries the default deny-delete ACL, and a project
        // in it. Everything above is whatever the temporary folder sits in.
        let base = Base::new("home-layout");
        base.dir("me/Documents/work");
        set_mode(&base.0.join("me"), 0o750);
        set_mode(&base.0.join("me/Documents"), 0o700);
        if cfg!(target_os = "macos") {
            let status = std::process::Command::new("/bin/chmod")
                .args(["+a", "everyone deny delete"])
                .arg(base.0.join("me/Documents"))
                .status()
                .unwrap();
            assert!(status.success());
        }
        base.project("me/Documents/work/proj");
        let file = base.tasks("me/Documents/work/proj");
        let home = base.0.join("me");
        let found = find_tasks_within(&base.0.join("me/Documents/work/proj"), Some(&home))
            .unwrap()
            .unwrap();
        assert_eq!(found.file, file);
    }
}
