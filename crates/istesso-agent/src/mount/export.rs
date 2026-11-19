//! The host's end of a shown folder: answers questions about it, read-only,
//! and never leaves it.
//!
//! Every path is opened one name at a time from the folder's own
//! descriptor, with symbolic links refused (`O_NOFOLLOW`); names such as
//! `..` or names with `/` are refused. Links inside the folder are listed as
//! links; their text is returned and resolved (if at all) on the device,
//! never here. Pipes, sockets and devices are listed and never opened: a file
//! is opened only after it was seen to be a regular one. Nothing on another
//! file system (a volume mounted inside the folder) is shown.
//!
//! This began as `export.rs` of the research prototype (branch
//! `research-unified-mount`, `research/unified-mount-proto`), which answered
//! the same questions on standard input and output; it now answers the
//! questions of [`istesso_link::mount`] and hides what [`super::rules`]
//! says.

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use istesso_link::mount::{
    valid_name, Answer, Attr, Entry, Failure, Kind, Question, MAX_ENTRIES, MAX_READ, ROOT_ID,
};

use super::rules::{Rules, Unreadable, MAX_RULES_FILE};
use super::MAX_IDS;

/// The most entries of a folder looked at for one page: a page of a folder
/// whose entries are mostly hidden still comes back soon.
const MAX_SCANNED: usize = 4 * MAX_ENTRIES;
/// The most folders whose listing is kept.
const MAX_LISTINGS: usize = 256;

/// What says that a folder's entries have not changed since it was listed.
type Stamp = (u64, u64, i64, i64, i64, i64, i64);

/// The names in one folder, sorted, and when they were read.
struct Listing {
    stamp: Stamp,
    names: Arc<Vec<Vec<u8>>>,
}

pub struct Export {
    root: OwnedFd,
    /// The file system of the folder: nothing on another one is shown.
    dev: u64,
    /// id -> names from the root. The root is the empty list.
    paths: HashMap<u64, Vec<Vec<u8>>>,
    ids: HashMap<Vec<Vec<u8>>, u64>,
    /// The ids in the order they were handed out, for letting the oldest go.
    order: VecDeque<u64>,
    next_id: u64,
    max_ids: usize,
    rules: Rules,
    listings: HashMap<Vec<Vec<u8>>, Listing>,
    /// Bytes read for the device so far.
    pub bytes_read: u64,
}

fn errno_to_failure(e: &io::Error) -> Failure {
    match e.raw_os_error() {
        Some(libc::ENOENT) => Failure::NoEnt,
        Some(libc::ENOTDIR) => Failure::NotDir,
        Some(libc::EISDIR) => Failure::IsDir,
        Some(libc::EACCES) | Some(libc::EPERM) => Failure::Access,
        Some(libc::ELOOP) => Failure::Loop,
        Some(libc::EINVAL) | Some(libc::ENAMETOOLONG) => Failure::Invalid,
        _ => Failure::Io,
    }
}

fn io_failure(e: io::Error) -> Failure {
    errno_to_failure(&e)
}

fn openat(dir: &OwnedFd, name: &[u8], flags: libc::c_int) -> io::Result<OwnedFd> {
    let c = CString::new(name).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: `c` is a valid C string and `dir` an open descriptor; the result is checked.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            c.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `fd` was just returned by openat and is owned by nobody else.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn fstat(fd: &OwnedFd) -> io::Result<libc::stat> {
    // SAFETY: an all-zero `stat` is a valid value; fstat fills it.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid descriptor and pointer.
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

fn lstat_at(dir: &OwnedFd, name: &[u8]) -> io::Result<libc::stat> {
    let c = CString::new(name).map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
    // SAFETY: an all-zero `stat` is a valid value.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: valid descriptor, C string and pointer.
    if unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            c.as_ptr(),
            &mut st,
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(st)
}

#[allow(clippy::unnecessary_cast)] // field types differ between macOS and Linux
fn is_type(st: &libc::stat, kind: libc::mode_t) -> bool {
    st.st_mode as u32 & libc::S_IFMT as u32 == kind as u32
}

#[allow(clippy::unnecessary_cast)] // field types differ between macOS and Linux
fn to_attr(st: &libc::stat) -> Attr {
    let kind = if is_type(st, libc::S_IFREG) {
        Kind::File
    } else if is_type(st, libc::S_IFDIR) {
        Kind::Dir
    } else if is_type(st, libc::S_IFLNK) {
        Kind::Symlink
    } else {
        Kind::Other
    };
    Attr {
        kind,
        mode: st.st_mode as u32 & 0o7777,
        nlink: st.st_nlink as u32,
        size: st.st_size as u64,
        mtime: st.st_mtime as i64,
        mtime_nsec: st.st_mtime_nsec as u32,
        ctime: st.st_ctime as i64,
        ctime_nsec: st.st_ctime_nsec as u32,
    }
}

#[allow(clippy::unnecessary_cast)] // field types differ between macOS and Linux
fn dev_of(st: &libc::stat) -> u64 {
    st.st_dev as u64
}

#[allow(clippy::unnecessary_cast)] // field types differ between macOS and Linux
fn stamp(st: &libc::stat) -> Stamp {
    (
        st.st_dev as u64,
        st.st_ino as u64,
        st.st_mtime as i64,
        st.st_mtime_nsec as i64,
        st.st_ctime as i64,
        st.st_ctime_nsec as i64,
        st.st_size as i64,
    )
}

fn errno(e: &io::Error) -> i32 {
    e.raw_os_error().unwrap_or(0)
}

/// Opens a folder below `root`, one name at a time, refusing links and
/// other file systems.
fn open_dir(root: &OwnedFd, dev: u64, names: &[Vec<u8>]) -> Result<OwnedFd, Failure> {
    let mut cur = openat(root, b".", libc::O_RDONLY | libc::O_DIRECTORY).map_err(io_failure)?;
    for name in names {
        cur = openat(&cur, name, libc::O_RDONLY | libc::O_DIRECTORY).map_err(io_failure)?;
        if dev_of(&fstat(&cur).map_err(io_failure)?) != dev {
            return Err(Failure::NoEnt);
        }
    }
    Ok(cur)
}

/// The folder `names` below `root` for reading a rules file in it: `None`
/// when it is not there or is a file (`.git` of a worktree); unreadable
/// when it is anything else that cannot be entered (a link, another file
/// system, no permission).
fn open_dir_for_rules(
    root: &OwnedFd,
    dev: u64,
    names: &[Vec<u8>],
) -> Result<Option<OwnedFd>, Unreadable> {
    let mut cur = openat(root, b".", libc::O_RDONLY | libc::O_DIRECTORY).map_err(|_| Unreadable)?;
    for name in names {
        match openat(&cur, name, libc::O_RDONLY | libc::O_DIRECTORY) {
            Ok(next) => {
                if dev_of(&fstat(&next).map_err(|_| Unreadable)?) != dev {
                    return Err(Unreadable);
                }
                cur = next;
            }
            Err(e) if errno(&e) == libc::ENOENT => return Ok(None),
            Err(e) if errno(&e) == libc::ENOTDIR || errno(&e) == libc::ELOOP => {
                // Not a folder: fine when it is a regular file; a link is not
                // followed and its rules are not known.
                return match lstat_at(&cur, name) {
                    Ok(st) if is_type(&st, libc::S_IFREG) => Ok(None),
                    Err(e) if errno(&e) == libc::ENOENT => Ok(None),
                    _ => Err(Unreadable),
                };
            }
            Err(_) => return Err(Unreadable),
        }
    }
    Ok(Some(cur))
}

/// A rules file below `root`: `None` when it is not there; the contents when
/// it is a regular file that can be read whole; unreadable otherwise (a link,
/// anything that is not a regular file, no permission, larger than
/// [`MAX_RULES_FILE`], an error while reading). The caller hides what
/// unreadable rules would have decided. Git does not read a linked
/// `.gitignore` at all and shows what it would have hidden; in doubt, this
/// hides (the owner's rule).
fn read_rules(
    root: &OwnedFd,
    dev: u64,
    folder: &[Vec<u8>],
    name: &[u8],
) -> Result<Option<Vec<u8>>, Unreadable> {
    let Some(dir) = open_dir_for_rules(root, dev, folder)? else {
        return Ok(None);
    };
    let seen = match lstat_at(&dir, name) {
        Ok(st) => st,
        Err(e) if errno(&e) == libc::ENOENT => return Ok(None),
        Err(_) => return Err(Unreadable),
    };
    if !is_type(&seen, libc::S_IFREG) {
        return Err(Unreadable);
    }
    if dev_of(&seen) != dev || seen.st_size as u64 > MAX_RULES_FILE as u64 {
        return Err(Unreadable);
    }
    let fd = openat(
        &dir,
        name,
        libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOCTTY,
    )
    .map_err(|_| Unreadable)?;
    let st = fstat(&fd).map_err(|_| Unreadable)?;
    // The same regular file that was looked at, not something put there since.
    if !is_type(&st, libc::S_IFREG) || st.st_ino != seen.st_ino || dev_of(&st) != dev {
        return Err(Unreadable);
    }
    let mut file = std::fs::File::from(fd);
    let mut contents = Vec::new();
    io::Read::read_to_end(
        &mut io::Read::take(&mut file, MAX_RULES_FILE as u64 + 1),
        &mut contents,
    )
    .map_err(|_| Unreadable)?;
    if contents.len() > MAX_RULES_FILE {
        return Err(Unreadable);
    }
    Ok(Some(contents))
}

impl Export {
    /// Opens the folder the owner named. `show_ignored`: show what Git
    /// ignores, and `.git`. `global_rules`: the user's global ignore file.
    pub fn open(root: &Path, show_ignored: bool, global_rules: Option<&Path>) -> io::Result<Self> {
        let c = CString::new(root.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::from_raw_os_error(libc::EINVAL))?;
        // The root itself may be reached through links (it is chosen by the owner); everything
        // below it is opened without following links.
        // SAFETY: valid C string; result checked.
        let fd = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fresh descriptor, owned by nobody else.
        let root = unsafe { OwnedFd::from_raw_fd(fd) };
        let dev = dev_of(&fstat(&root)?);
        let mut paths = HashMap::new();
        let mut ids = HashMap::new();
        paths.insert(ROOT_ID, Vec::new());
        ids.insert(Vec::new(), ROOT_ID);
        Ok(Export {
            root,
            dev,
            paths,
            ids,
            order: VecDeque::new(),
            next_id: ROOT_ID + 1,
            max_ids: MAX_IDS,
            rules: Rules::new(show_ignored, global_rules),
            listings: HashMap::new(),
            bytes_read: 0,
        })
    }

    /// The same, letting go of ids sooner (tests).
    #[cfg(test)]
    pub fn with_max_ids(mut self, max_ids: usize) -> Self {
        self.max_ids = max_ids;
        self
    }

    /// The attributes of the folder itself.
    pub fn root_attr(&self) -> io::Result<Attr> {
        fstat(&self.root).map(|st| to_attr(&st))
    }

    /// The id of an entry. When as many ids are out as allowed, the oldest
    /// tenth are let go first: the device gets `stale` for them and looks the
    /// names up again, which hands out new ids. An id is never used twice.
    fn id_for(&mut self, names: Vec<Vec<u8>>) -> Result<u64, Failure> {
        if let Some(id) = self.ids.get(&names) {
            return Ok(*id);
        }
        if self.paths.len() >= self.max_ids {
            let release = (self.max_ids / 10).max(1);
            let mut released = 0;
            while released < release {
                let Some(old) = self.order.pop_front() else {
                    break;
                };
                if let Some(old_names) = self.paths.remove(&old) {
                    self.ids.remove(&old_names);
                    released += 1;
                }
            }
            if self.paths.len() >= self.max_ids {
                return Err(Failure::TooMany);
            }
        }
        let id = self.next_id;
        self.next_id += 1;
        self.paths.insert(id, names.clone());
        self.ids.insert(names, id);
        self.order.push_back(id);
        Ok(id)
    }

    fn names(&self, id: u64) -> Result<Vec<Vec<u8>>, Failure> {
        self.paths.get(&id).cloned().ok_or(Failure::Stale)
    }

    /// The sorted names in the folder `names` (open as `dir`), read again
    /// only when the folder has changed.
    fn listed(&mut self, names: &[Vec<u8>], dir: &OwnedFd) -> Result<Arc<Vec<Vec<u8>>>, Failure> {
        let now = stamp(&fstat(dir).map_err(io_failure)?);
        if let Some(listing) = self.listings.get(names) {
            if listing.stamp == now {
                return Ok(listing.names.clone());
            }
        }
        let mut listed = read_names(dir).map_err(io_failure)?;
        listed.sort();
        let listed = Arc::new(listed);
        // A folder changed within the last second may change again without its
        // times moving (file systems whose clock ticks coarsely): such a listing
        // is used once and not kept.
        let changed = std::time::UNIX_EPOCH
            + Duration::new(now.2.max(0) as u64, now.3.clamp(0, 999_999_999) as u32);
        if std::time::SystemTime::now()
            .duration_since(changed)
            .map_or(true, |age| age < Duration::from_secs(1))
        {
            return Ok(listed);
        }
        if self.listings.len() >= MAX_LISTINGS {
            self.listings.clear();
        }
        self.listings.insert(
            names.to_vec(),
            Listing {
                stamp: now,
                names: listed.clone(),
            },
        );
        Ok(listed)
    }

    /// Whether every name of `names` is still in its folder, spelled byte for
    /// byte as it is stored. On a volume that folds case or Unicode forms the
    /// file system would also find another spelling; such a name is answered
    /// as missing, so that the rules are only ever asked about stored names
    /// (in doubt, hide).
    fn check_stored(&mut self, names: &[Vec<u8>]) -> Result<(), Failure> {
        let mut dir = open_dir(&self.root, self.dev, &[])?;
        for (i, name) in names.iter().enumerate() {
            if self.listed(&names[..i], &dir)?.binary_search(name).is_err() {
                return Err(Failure::NoEnt);
            }
            if i + 1 < names.len() {
                dir = openat(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY).map_err(io_failure)?;
                if dev_of(&fstat(&dir).map_err(io_failure)?) != self.dev {
                    return Err(Failure::NoEnt);
                }
            }
        }
        Ok(())
    }

    /// An entry that is not shown, is on another file system, or is no longer
    /// stored under the name it was handed out with, is answered as one that
    /// is not there.
    fn shown(&mut self, names: &[Vec<u8>], st: &libc::stat) -> Result<(), Failure> {
        if dev_of(st) != self.dev {
            return Err(Failure::NoEnt);
        }
        self.check_stored(names)?;
        let (root, dev) = (&self.root, self.dev);
        let read = |folder: &[Vec<u8>], name: &[u8]| read_rules(root, dev, folder, name);
        if self.rules.hides(names, is_type(st, libc::S_IFDIR), &read) {
            Err(Failure::NoEnt)
        } else {
            Ok(())
        }
    }

    /// Looks at an entry without following it: the folder itself, or the
    /// last name in its folder.
    fn look(&self, names: &[Vec<u8>]) -> Result<(Option<OwnedFd>, libc::stat), Failure> {
        match names.split_last() {
            None => Ok((None, fstat(&self.root).map_err(io_failure)?)),
            Some((last, parent)) => {
                let dir = open_dir(&self.root, self.dev, parent)?;
                let st = lstat_at(&dir, last).map_err(io_failure)?;
                Ok((Some(dir), st))
            }
        }
    }

    /// Answers one question. A read answer carries its bytes in the second
    /// value.
    pub fn handle(&mut self, question: Question) -> (Answer, Vec<u8>) {
        match self.handle_inner(question) {
            Ok(answer) => answer,
            Err(error) => (Answer::Failed { error }, Vec::new()),
        }
    }

    fn handle_inner(&mut self, question: Question) -> Result<(Answer, Vec<u8>), Failure> {
        match question {
            Question::Alive => Ok((Answer::Alive, Vec::new())),
            Question::Unknown => Err(Failure::Invalid),
            Question::Stat { id } => {
                let names = self.names(id)?;
                let (_, st) = self.look(&names)?;
                self.shown(&names, &st)?;
                Ok((Answer::Attr { attr: to_attr(&st) }, Vec::new()))
            }
            Question::Lookup { dir, name } => {
                if !valid_name(&name) {
                    return Err(Failure::Invalid);
                }
                let mut names = self.names(dir)?;
                names.push(name);
                // Only a name as it is stored (checked in `shown`, before anything is
                // handed out): on a volume that folds case or Unicode forms, another
                // spelling would reach the same file (`.GIT`, `.Env`, a decomposed name).
                let (_, st) = self.look(&names)?;
                self.shown(&names, &st)?;
                let id = self.id_for(names)?;
                Ok((
                    Answer::Found {
                        id,
                        attr: to_attr(&st),
                    },
                    Vec::new(),
                ))
            }
            Question::ReadDir { id, start } => {
                let names = self.names(id)?;
                let (_, st) = self.look(&names)?;
                if !is_type(&st, libc::S_IFDIR) {
                    return Err(Failure::NotDir);
                }
                self.shown(&names, &st)?;
                let dir_fd = open_dir(&self.root, self.dev, &names)?;
                let listed = self.listed(&names, &dir_fd)?;
                // `start` is a place in the folder's own sorted listing, hidden
                // entries included, so that a page costs what it holds.
                let mut at = start as usize;
                let mut entries = Vec::new();
                let mut scanned = 0;
                while at < listed.len() && entries.len() < MAX_ENTRIES && scanned < MAX_SCANNED {
                    let name = listed[at].clone();
                    at += 1;
                    scanned += 1;
                    // An entry may vanish between the listing and its stat; skip it.
                    let Ok(st) = lstat_at(&dir_fd, &name) else {
                        continue;
                    };
                    if dev_of(&st) != self.dev {
                        continue;
                    }
                    let mut child = names.clone();
                    child.push(name.clone());
                    let (root, dev) = (&self.root, self.dev);
                    let read =
                        |folder: &[Vec<u8>], file: &[u8]| read_rules(root, dev, folder, file);
                    if self
                        .rules
                        .hides_last(&child, is_type(&st, libc::S_IFDIR), &read)
                    {
                        continue;
                    }
                    let id = self.id_for(child)?;
                    entries.push(Entry {
                        id,
                        name,
                        attr: to_attr(&st),
                    });
                }
                let next = u32::try_from(at).map_err(|_| Failure::TooMany)?;
                Ok((
                    Answer::Entries {
                        entries,
                        more: at < listed.len(),
                        next: Some(next),
                    },
                    Vec::new(),
                ))
            }
            Question::Read { id, offset, count } => {
                let names = self.names(id)?;
                let (dir, seen) = self.look(&names)?;
                let dir = dir.ok_or(Failure::IsDir)?;
                // Looked at before it is opened: a pipe or a device is never opened.
                if is_type(&seen, libc::S_IFDIR) {
                    return Err(Failure::IsDir);
                }
                if is_type(&seen, libc::S_IFLNK) {
                    return Err(Failure::Loop);
                }
                if !is_type(&seen, libc::S_IFREG) {
                    return Err(Failure::Invalid);
                }
                self.shown(&names, &seen)?;
                let last = names.last().ok_or(Failure::IsDir)?;
                let fd = openat(
                    &dir,
                    last,
                    libc::O_RDONLY | libc::O_NONBLOCK | libc::O_NOCTTY,
                )
                .map_err(io_failure)?;
                let st = fstat(&fd).map_err(io_failure)?;
                // The same regular file that was looked at, not something put there since.
                if !is_type(&st, libc::S_IFREG)
                    || st.st_ino != seen.st_ino
                    || dev_of(&st) != self.dev
                {
                    return Err(Failure::Invalid);
                }
                let count = count.min(MAX_READ) as usize;
                let mut buf = vec![0u8; count];
                let mut done = 0usize;
                while done < count {
                    let at = offset.checked_add(done as u64).ok_or(Failure::Invalid)?;
                    let at = libc::off_t::try_from(at).map_err(|_| Failure::Invalid)?;
                    // SAFETY: the buffer has `count - done` bytes left at `done`.
                    let n = unsafe {
                        libc::pread(
                            fd.as_raw_fd(),
                            buf[done..].as_mut_ptr().cast(),
                            count - done,
                            at,
                        )
                    };
                    if n < 0 {
                        return Err(io_failure(io::Error::last_os_error()));
                    }
                    if n == 0 {
                        break;
                    }
                    done += n as usize;
                }
                buf.truncate(done);
                self.bytes_read += done as u64;
                let eof = offset.saturating_add(done as u64) >= st.st_size as u64;
                Ok((Answer::Data { eof }, buf))
            }
            Question::ReadLink { id } => {
                let names = self.names(id)?;
                let (dir, seen) = self.look(&names)?;
                let dir = dir.ok_or(Failure::Invalid)?;
                if !is_type(&seen, libc::S_IFLNK) {
                    return Err(Failure::Invalid);
                }
                self.shown(&names, &seen)?;
                let last = names.last().ok_or(Failure::Invalid)?;
                let c = CString::new(last.as_slice()).map_err(|_| Failure::Invalid)?;
                let mut buf = vec![0u8; 4096];
                // SAFETY: valid descriptor, C string and buffer.
                let n = unsafe {
                    libc::readlinkat(
                        dir.as_raw_fd(),
                        c.as_ptr(),
                        buf.as_mut_ptr().cast(),
                        buf.len(),
                    )
                };
                if n < 0 {
                    return Err(io_failure(io::Error::last_os_error()));
                }
                buf.truncate(n as usize);
                Ok((Answer::Link { target: buf }, Vec::new()))
            }
        }
    }
}

fn read_names(dir: &OwnedFd) -> io::Result<Vec<Vec<u8>>> {
    // fdopendir takes ownership of the descriptor it gets, so give it a duplicate.
    // SAFETY: valid descriptor; result checked.
    let dup = unsafe { libc::dup(dir.as_raw_fd()) };
    if dup < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `dup` is ours; on failure we close it.
    let dirp = unsafe { libc::fdopendir(dup) };
    if dirp.is_null() {
        let e = io::Error::last_os_error();
        // SAFETY: still ours.
        unsafe { libc::close(dup) };
        return Err(e);
    }
    let mut names = Vec::new();
    loop {
        // SAFETY: `dirp` is a valid stream until closedir.
        let ent = unsafe { libc::readdir(dirp) };
        if ent.is_null() {
            break;
        }
        // SAFETY: d_name is a NUL-terminated array inside the entry.
        let name = unsafe { CStr::from_ptr((*ent).d_name.as_ptr()) }
            .to_bytes()
            .to_vec();
        if name != b"." && name != b".." {
            names.push(name);
        }
    }
    // SAFETY: closes the stream and `dup`.
    unsafe { libc::closedir(dirp) };
    Ok(names)
}
