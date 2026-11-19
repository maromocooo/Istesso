//! The side that owns the folder. It answers requests about one folder, read-only, and never
//! leaves it: every path is opened one name at a time from the folder's own descriptor, with
//! symbolic links refused (`O_NOFOLLOW`), and names such as `..` or names with `/` are refused.
//! Links inside the folder are listed as links; their targets are returned as text and resolved
//! (if at all) by the viewing machine, never here.

use crate::wire::{self, Attr, Entry, Error, Kind, Request, Response};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::Path;

pub const ROOT_ID: u64 = 1;

pub struct Export {
    root: OwnedFd,
    /// id -> names from the root. The root is the empty list.
    paths: HashMap<u64, Vec<Vec<u8>>>,
    ids: HashMap<Vec<Vec<u8>>, u64>,
    next_id: u64,
}

fn errno_to_error(e: &io::Error) -> Error {
    match e.raw_os_error() {
        Some(libc::ENOENT) => Error::NoEnt,
        Some(libc::ENOTDIR) => Error::NotDir,
        Some(libc::EISDIR) => Error::IsDir,
        Some(libc::EACCES) | Some(libc::EPERM) => Error::Access,
        Some(libc::ELOOP) => Error::Loop,
        Some(libc::EINVAL) | Some(libc::ENAMETOOLONG) => Error::Invalid,
        _ => Error::Io,
    }
}

/// A single name inside a folder: not empty, not `.` or `..`, no `/`, no NUL byte.
pub fn valid_name(name: &[u8]) -> bool {
    !name.is_empty()
        && name != b"."
        && name != b".."
        && !name.contains(&b'/')
        && !name.contains(&0)
        && name.len() <= 255
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
    // SAFETY: as above.
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
fn to_attr(st: &libc::stat) -> Attr {
    let fmt = st.st_mode as u32 & libc::S_IFMT as u32;
    let kind = if fmt == libc::S_IFREG as u32 {
        Kind::File
    } else if fmt == libc::S_IFDIR as u32 {
        Kind::Dir
    } else if fmt == libc::S_IFLNK as u32 {
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

impl Export {
    pub fn open(root: &Path) -> io::Result<Self> {
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
        // SAFETY: fresh descriptor.
        Ok(Export {
            root: unsafe { OwnedFd::from_raw_fd(fd) },
            paths,
            ids,
            next_id: ROOT_ID + 1,
        })
    }

    fn id_for(&mut self, comps: Vec<Vec<u8>>) -> u64 {
        if let Some(id) = self.ids.get(&comps) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.paths.insert(id, comps.clone());
        self.ids.insert(comps, id);
        id
    }

    /// Opens a folder below the root, one name at a time, refusing links.
    fn open_dir(&self, comps: &[Vec<u8>]) -> io::Result<OwnedFd> {
        let mut cur = openat(&self.root, b".", libc::O_RDONLY | libc::O_DIRECTORY)?;
        for c in comps {
            cur = openat(&cur, c, libc::O_RDONLY | libc::O_DIRECTORY)?;
        }
        Ok(cur)
    }

    fn stat_path(&self, comps: &[Vec<u8>]) -> io::Result<libc::stat> {
        match comps.split_last() {
            None => fstat(&self.root),
            Some((last, parent)) => lstat_at(&self.open_dir(parent)?, last),
        }
    }

    fn comps(&self, id: u64) -> Result<Vec<Vec<u8>>, Error> {
        self.paths.get(&id).cloned().ok_or(Error::Stale)
    }

    /// Answers one request. Read answers carry their bytes in the second value.
    pub fn handle(&mut self, req: Request) -> (Response, Vec<u8>) {
        match self.handle_inner(req) {
            Ok(r) => r,
            Err(error) => (Response::Failed { error }, Vec::new()),
        }
    }

    fn handle_inner(&mut self, req: Request) -> Result<(Response, Vec<u8>), Error> {
        let io = |e: io::Error| errno_to_error(&e);
        match req {
            Request::Getattr { id } => {
                let st = self.stat_path(&self.comps(id)?).map_err(io)?;
                Ok((Response::Attr { attr: to_attr(&st) }, Vec::new()))
            }
            Request::Lookup { dir, name } => {
                if !valid_name(&name) {
                    return Err(Error::Invalid);
                }
                let mut comps = self.comps(dir)?;
                let dir_fd = self.open_dir(&comps).map_err(io)?;
                let st = lstat_at(&dir_fd, &name).map_err(io)?;
                comps.push(name);
                let id = self.id_for(comps);
                Ok((
                    Response::Found {
                        id,
                        attr: to_attr(&st),
                    },
                    Vec::new(),
                ))
            }
            Request::Readdir { id } => {
                let comps = self.comps(id)?;
                let dir_fd = self.open_dir(&comps).map_err(io)?;
                let mut names = read_names(&dir_fd).map_err(io)?;
                names.sort();
                let mut entries = Vec::with_capacity(names.len());
                for name in names {
                    // An entry may vanish between the listing and its stat; skip it.
                    let Ok(st) = lstat_at(&dir_fd, &name) else {
                        continue;
                    };
                    let mut child = comps.clone();
                    child.push(name.clone());
                    let id = self.id_for(child);
                    entries.push(Entry {
                        id,
                        name,
                        attr: to_attr(&st),
                    });
                }
                Ok((Response::Entries { entries }, Vec::new()))
            }
            Request::Read { id, offset, count } => {
                let comps = self.comps(id)?;
                let (last, parent) = comps.split_last().ok_or(Error::IsDir)?;
                let dir_fd = self.open_dir(parent).map_err(io)?;
                // O_NONBLOCK: a pipe in the folder must not make us wait for a writer.
                let fd = openat(&dir_fd, last, libc::O_RDONLY | libc::O_NONBLOCK).map_err(io)?;
                let st = fstat(&fd).map_err(io)?;
                if st.st_mode as u32 & libc::S_IFMT as u32 != libc::S_IFREG as u32 {
                    return Err(Error::Invalid);
                }
                let count = count.min(wire::MAX_DATA) as usize;
                let mut buf = vec![0u8; count];
                let mut done = 0usize;
                while done < count {
                    // SAFETY: the buffer has `count - done` bytes left at `done`.
                    let n = unsafe {
                        libc::pread(
                            fd.as_raw_fd(),
                            buf[done..].as_mut_ptr().cast(),
                            count - done,
                            (offset + done as u64) as libc::off_t,
                        )
                    };
                    if n < 0 {
                        return Err(io(io::Error::last_os_error()));
                    }
                    if n == 0 {
                        break;
                    }
                    done += n as usize;
                }
                buf.truncate(done);
                let eof = offset + done as u64 >= st.st_size as u64;
                Ok((Response::Data { eof }, buf))
            }
            Request::Readlink { id } => {
                let comps = self.comps(id)?;
                let (last, parent) = comps.split_last().ok_or(Error::Invalid)?;
                let dir_fd = self.open_dir(parent).map_err(io)?;
                let c = CString::new(last.as_slice()).map_err(|_| Error::Invalid)?;
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
                    return Err(io(io::Error::last_os_error()));
                }
                buf.truncate(n as usize);
                Ok((Response::Link { target: buf }, Vec::new()))
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

/// Serves requests from `input` until it closes. Used as `unified-mount-proto export <folder>`.
pub fn serve<R: Read, W: Write>(
    export: &mut Export,
    input: &mut R,
    output: &mut W,
) -> io::Result<()> {
    while let Some((req, _data)) = wire::read_frame::<_, Request>(input)? {
        let (resp, data) = export.handle(req);
        wire::write_frame(output, &resp, &data)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn scratch(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "unified-mount-proto-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::create_dir(&p);
        p
    }

    fn found(r: (Response, Vec<u8>)) -> (u64, Attr) {
        match r.0 {
            Response::Found { id, attr } => (id, attr),
            other => panic!("expected Found, got {other:?}"),
        }
    }

    fn failed(r: (Response, Vec<u8>)) -> Error {
        match r.0 {
            Response::Failed { error } => error,
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn lists_and_reads_inside_the_folder() {
        let base = scratch("list");
        let root = base.join("project");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/main.rs"), b"fn main() {}\n").unwrap();
        let mut ex = Export::open(&root).unwrap();
        let (src, attr) = found(ex.handle(Request::Lookup {
            dir: ROOT_ID,
            name: b"src".to_vec(),
        }));
        assert_eq!(attr.kind, Kind::Dir);
        let (resp, _) = ex.handle(Request::Readdir { id: src });
        let Response::Entries { entries } = resp else {
            panic!()
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, b"main.rs");
        let (resp, data) = ex.handle(Request::Read {
            id: entries[0].id,
            offset: 3,
            count: 4,
        });
        assert_eq!(resp, Response::Data { eof: false });
        assert_eq!(data, b"main");
        let (resp, data) = ex.handle(Request::Read {
            id: entries[0].id,
            offset: 3,
            count: 100,
        });
        assert_eq!(resp, Response::Data { eof: true });
        assert_eq!(data, b"main() {}\n");
        fs::remove_file(root.join("src/main.rs")).unwrap();
        fs::remove_dir(root.join("src")).unwrap();
        fs::remove_dir(&root).unwrap();
        fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn refuses_dot_dot_slash_and_empty_names() {
        let base = scratch("names");
        let mut ex = Export::open(&base).unwrap();
        for bad in [&b".."[..], b".", b"", b"a/b", b"/etc", b"a\0b"] {
            assert_eq!(
                failed(ex.handle(Request::Lookup {
                    dir: ROOT_ID,
                    name: bad.to_vec()
                })),
                Error::Invalid
            );
        }
        fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn links_are_listed_but_never_followed() {
        let base = scratch("links");
        let root = base.join("project");
        let outside = base.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), b"outside").unwrap();
        symlink(&outside, root.join("dir-link")).unwrap();
        symlink(outside.join("secret.txt"), root.join("file-link")).unwrap();
        let mut ex = Export::open(&root).unwrap();

        let (file_link, attr) = found(ex.handle(Request::Lookup {
            dir: ROOT_ID,
            name: b"file-link".to_vec(),
        }));
        assert_eq!(attr.kind, Kind::Symlink);
        // Reading through the link is refused; only its text is returned.
        assert_eq!(
            failed(ex.handle(Request::Read {
                id: file_link,
                offset: 0,
                count: 10
            })),
            Error::Loop
        );
        let (resp, _) = ex.handle(Request::Readlink { id: file_link });
        assert_eq!(
            resp,
            Response::Link {
                target: outside
                    .join("secret.txt")
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec()
            }
        );

        let (dir_link, attr) = found(ex.handle(Request::Lookup {
            dir: ROOT_ID,
            name: b"dir-link".to_vec(),
        }));
        assert_eq!(attr.kind, Kind::Symlink);
        // A link to a folder cannot be listed or walked through (macOS answers ENOTDIR, Linux ELOOP).
        let err = failed(ex.handle(Request::Readdir { id: dir_link }));
        assert!(matches!(err, Error::Loop | Error::NotDir), "{err:?}");
        let err = failed(ex.handle(Request::Lookup {
            dir: dir_link,
            name: b"secret.txt".to_vec(),
        }));
        assert!(matches!(err, Error::Loop | Error::NotDir), "{err:?}");

        fs::remove_file(root.join("dir-link")).unwrap();
        fs::remove_file(root.join("file-link")).unwrap();
        fs::remove_file(outside.join("secret.txt")).unwrap();
        fs::remove_dir(&outside).unwrap();
        fs::remove_dir(&root).unwrap();
        fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn a_pipe_is_not_opened() {
        let base = scratch("fifo");
        let c = CString::new(base.join("pipe").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let mut ex = Export::open(&base).unwrap();
        let (id, attr) = found(ex.handle(Request::Lookup {
            dir: ROOT_ID,
            name: b"pipe".to_vec(),
        }));
        assert_eq!(attr.kind, Kind::Other);
        assert_eq!(
            failed(ex.handle(Request::Read {
                id,
                offset: 0,
                count: 1
            })),
            Error::Invalid
        );
        fs::remove_file(base.join("pipe")).unwrap();
        fs::remove_dir(&base).unwrap();
    }

    #[test]
    fn unknown_ids_are_stale() {
        let base = scratch("stale");
        let mut ex = Export::open(&base).unwrap();
        assert_eq!(
            failed(ex.handle(Request::Getattr { id: 999 })),
            Error::Stale
        );
        fs::remove_dir(&base).unwrap();
    }
}
