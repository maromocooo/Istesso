//! The host's end of a shown folder: answers questions about it, read-only,
//! and never leaves it.
//!
//! Every path is opened one name at a time from the folder's own
//! descriptor, with symbolic links refused (`O_NOFOLLOW`); names such as
//! `..` or names with `/` are refused. Links inside the folder are listed as
//! links; their text is returned and resolved (if at all) on the device,
//! never here. Pipes, sockets and devices are listed and never opened.
//!
//! This began as `export.rs` of the research prototype (branch
//! `research-unified-mount`, `research/unified-mount-proto`), which answered
//! the same questions on standard input and output; it now answers the
//! questions of [`istesso_link::mount`] and hides what [`super::rules`]
//! says.

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

use istesso_link::mount::{
    valid_name, Answer, Attr, Entry, Failure, Kind, Question, MAX_ENTRIES, MAX_READ, ROOT_ID,
};

use super::rules::{Rules, MAX_RULES_FILE};
use super::MAX_IDS;

pub struct Export {
    root: OwnedFd,
    /// id -> names from the root. The root is the empty list.
    paths: HashMap<u64, Vec<Vec<u8>>>,
    ids: HashMap<Vec<Vec<u8>>, u64>,
    next_id: u64,
    rules: Rules,
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

/// Opens a folder below `root`, one name at a time, refusing links.
fn open_dir(root: &OwnedFd, names: &[Vec<u8>]) -> io::Result<OwnedFd> {
    let mut cur = openat(root, b".", libc::O_RDONLY | libc::O_DIRECTORY)?;
    for name in names {
        cur = openat(&cur, name, libc::O_RDONLY | libc::O_DIRECTORY)?;
    }
    Ok(cur)
}

/// Whether every name of `names` is still in its folder, spelled byte for
/// byte as it is stored. On a volume that folds case or Unicode forms the
/// file system would also find another spelling; such a name is answered as
/// missing, so that the rules are only ever asked about stored names (in
/// doubt, hide).
fn stored_as_named(root: &OwnedFd, names: &[Vec<u8>]) -> Result<(), Failure> {
    let mut dir = openat(root, b".", libc::O_RDONLY | libc::O_DIRECTORY).map_err(io_failure)?;
    for (i, name) in names.iter().enumerate() {
        if !read_names(&dir).map_err(io_failure)?.contains(name) {
            return Err(Failure::NoEnt);
        }
        if i + 1 < names.len() {
            dir = openat(&dir, name, libc::O_RDONLY | libc::O_DIRECTORY).map_err(io_failure)?;
        }
    }
    Ok(())
}

/// The contents of a small regular file below `root`, never through a link.
fn read_small(root: &OwnedFd, folder: &[Vec<u8>], name: &[u8]) -> Option<Vec<u8>> {
    let dir = open_dir(root, folder).ok()?;
    let fd = openat(&dir, name, libc::O_RDONLY | libc::O_NONBLOCK).ok()?;
    let st = fstat(&fd).ok()?;
    if !is_type(&st, libc::S_IFREG) || st.st_size as u64 > MAX_RULES_FILE as u64 {
        return None;
    }
    let mut file = std::fs::File::from(fd);
    let mut contents = Vec::new();
    io::Read::read_to_end(
        &mut io::Read::take(&mut file, MAX_RULES_FILE as u64 + 1),
        &mut contents,
    )
    .ok()?;
    (contents.len() <= MAX_RULES_FILE).then_some(contents)
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
        let mut paths = HashMap::new();
        let mut ids = HashMap::new();
        paths.insert(ROOT_ID, Vec::new());
        ids.insert(Vec::new(), ROOT_ID);
        // SAFETY: fresh descriptor, owned by nobody else.
        let root_fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Export {
            paths,
            ids,
            next_id: ROOT_ID + 1,
            rules: Rules::new(show_ignored, global_rules),
            root: root_fd,
            bytes_read: 0,
        })
    }

    /// The attributes of the folder itself.
    pub fn root_attr(&self) -> io::Result<Attr> {
        fstat(&self.root).map(|st| to_attr(&st))
    }

    fn id_for(&mut self, names: Vec<Vec<u8>>) -> Result<u64, Failure> {
        if let Some(id) = self.ids.get(&names) {
            return Ok(*id);
        }
        if self.paths.len() >= MAX_IDS {
            return Err(Failure::TooMany);
        }
        let id = self.next_id;
        self.next_id += 1;
        self.paths.insert(id, names.clone());
        self.ids.insert(names, id);
        Ok(id)
    }

    fn stat_path(&self, names: &[Vec<u8>]) -> io::Result<libc::stat> {
        match names.split_last() {
            None => fstat(&self.root),
            Some((last, parent)) => lstat_at(&open_dir(&self.root, parent)?, last),
        }
    }

    fn names(&self, id: u64) -> Result<Vec<Vec<u8>>, Failure> {
        self.paths.get(&id).cloned().ok_or(Failure::Stale)
    }

    /// An entry that is not shown, or no longer stored under the name it was
    /// handed out with, is answered as one that is not there.
    fn shown(&mut self, names: &[Vec<u8>], is_dir: bool) -> Result<(), Failure> {
        stored_as_named(&self.root, names)?;
        let root = &self.root;
        let read = |folder: &[Vec<u8>], name: &[u8]| read_small(root, folder, name);
        if self.rules.hides(names, is_dir, &read) {
            Err(Failure::NoEnt)
        } else {
            Ok(())
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
                let st = self.stat_path(&names).map_err(io_failure)?;
                self.shown(&names, is_type(&st, libc::S_IFDIR))?;
                Ok((Answer::Attr { attr: to_attr(&st) }, Vec::new()))
            }
            Question::Lookup { dir, name } => {
                if !valid_name(&name) {
                    return Err(Failure::Invalid);
                }
                let mut names = self.names(dir)?;
                let dir_fd = open_dir(&self.root, &names).map_err(io_failure)?;
                // Only a name as it is stored (checked in `shown`, before anything
                // is handed out): on a volume that folds case or Unicode forms,
                // another spelling would reach the same file (`.GIT`, `.Env`, a
                // decomposed name).
                let st = lstat_at(&dir_fd, &name).map_err(io_failure)?;
                names.push(name);
                self.shown(&names, is_type(&st, libc::S_IFDIR))?;
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
                let dir_fd = open_dir(&self.root, &names).map_err(io_failure)?;
                self.shown(&names, true)?;
                let mut listed = read_names(&dir_fd).map_err(io_failure)?;
                listed.sort();
                let mut shown = Vec::with_capacity(listed.len());
                for name in listed {
                    // An entry may vanish between the listing and its stat; skip it.
                    let Ok(st) = lstat_at(&dir_fd, &name) else {
                        continue;
                    };
                    let mut child = names.clone();
                    child.push(name);
                    let root = &self.root;
                    let read = |folder: &[Vec<u8>], file: &[u8]| read_small(root, folder, file);
                    if self
                        .rules
                        .hides_last(&child, is_type(&st, libc::S_IFDIR), &read)
                    {
                        continue;
                    }
                    shown.push((child, st));
                }
                let start = start as usize;
                let more = shown.len() > start.saturating_add(MAX_ENTRIES);
                let mut entries = Vec::new();
                for (child, st) in shown.into_iter().skip(start).take(MAX_ENTRIES) {
                    let name = child.last().cloned().unwrap_or_default();
                    let id = self.id_for(child)?;
                    entries.push(Entry {
                        id,
                        name,
                        attr: to_attr(&st),
                    });
                }
                Ok((Answer::Entries { entries, more }, Vec::new()))
            }
            Question::Read { id, offset, count } => {
                let names = self.names(id)?;
                let (last, parent) = names.split_last().ok_or(Failure::IsDir)?;
                let dir_fd = open_dir(&self.root, parent).map_err(io_failure)?;
                // O_NONBLOCK: a pipe in the folder must not make us wait for a writer.
                let fd =
                    openat(&dir_fd, last, libc::O_RDONLY | libc::O_NONBLOCK).map_err(io_failure)?;
                let st = fstat(&fd).map_err(io_failure)?;
                if !is_type(&st, libc::S_IFREG) {
                    return Err(Failure::Invalid);
                }
                self.shown(&names, false)?;
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
                let (last, parent) = names.split_last().ok_or(Failure::Invalid)?;
                let dir_fd = open_dir(&self.root, parent).map_err(io_failure)?;
                self.shown(&names, false)?;
                let c = CString::new(last.as_slice()).map_err(|_| Failure::Invalid)?;
                let mut buf = vec![0u8; 4096];
                // SAFETY: valid descriptor, C string and buffer.
                let n = unsafe {
                    libc::readlinkat(
                        dir_fd.as_raw_fd(),
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
