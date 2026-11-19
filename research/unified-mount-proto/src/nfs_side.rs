//! The viewing side: a read-only NFSv3 server on 127.0.0.1 that answers the operating system's
//! own NFS client by asking the machine that owns the folder, over Istesso's control channel
//! ([`Remote`], the questions of `istesso_link::mount`).
//!
//! The loopback port has no authentication of its own: any process on this machine can connect
//! to it (docs/research/unified-mount.md, 5.1). Three things narrow what such a process can get
//! (ADR 0007, proposal "フォルダーを読むだけで見せる"); none of them is authentication:
//!
//! 1. Only the first MNT is answered with the folder's handle. Later ones are told there is no
//!    such export.
//! 2. A handle is an id and 16 random bytes made for that id when it was first handed out. A
//!    handle that was not handed out is refused (`BADHANDLE`), so ids cannot be guessed into
//!    handles.
//! 3. Nothing but attributes is answered until this program's own `mount_nfs` has succeeded. If
//!    another process took the first MNT, that mount fails, the program stops, and the other
//!    process never got a name or a byte.

use async_trait::async_trait;
use istesso_agent::mount::client::{Gone, Remote};
use istesso_link::mount::{Answer, Attr, Entry, Failure, Kind, Question, ROOT_ID};
use nfsserve::nfs::{
    fattr3, fileid3, filename3, ftype3, nfs_fh3, nfspath3, nfsstat3, nfsstring, nfstime3, sattr3,
    specdata3,
};
use nfsserve::vfs::{DirEntry, NFSFileSystem, ReadDirResult, VFSCapabilities};
use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The most entries of one folder that are listed.
const MAX_LISTED: usize = 200_000;
/// Bytes of a handle: the id, then its random token.
const HANDLE_LEN: usize = 8 + 16;

/// What the server and the program that started it share.
#[derive(Default)]
pub struct Gate {
    /// The first MNT was answered.
    mount_asked: AtomicBool,
    /// This program's own mount succeeded: contents may be answered.
    open: AtomicBool,
    /// MNT requests after the first, refused.
    pub refused_mounts: AtomicU64,
    /// Handles that were not handed out, refused.
    pub refused_handles: AtomicU64,
    /// Questions about contents before the mount succeeded, refused.
    pub refused_early: AtomicU64,
}

impl Gate {
    /// Lets contents be answered: this program's own mount succeeded.
    pub fn open(&self) {
        self.open.store(true, Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), nfsstat3> {
        if self.open.load(Ordering::SeqCst) {
            Ok(())
        } else {
            self.refused_early.fetch_add(1, Ordering::Relaxed);
            Err(nfsstat3::NFS3ERR_ACCES)
        }
    }
}

fn to_nfs_error(e: Failure) -> nfsstat3 {
    match e {
        Failure::NoEnt => nfsstat3::NFS3ERR_NOENT,
        Failure::NotDir => nfsstat3::NFS3ERR_NOTDIR,
        Failure::IsDir => nfsstat3::NFS3ERR_ISDIR,
        Failure::Access => nfsstat3::NFS3ERR_ACCES,
        Failure::Invalid | Failure::Loop => nfsstat3::NFS3ERR_INVAL,
        Failure::Stale => nfsstat3::NFS3ERR_STALE,
        Failure::TooMany => nfsstat3::NFS3ERR_SERVERFAULT,
        Failure::Io => nfsstat3::NFS3ERR_IO,
    }
}

fn time(secs: i64, nsec: u32) -> nfstime3 {
    nfstime3 {
        seconds: secs.clamp(0, u32::MAX as i64) as u32,
        nseconds: nsec,
    }
}

/// Presents the owner's files as owned by the local user, without write bits and without the
/// set-user, set-group and sticky bits: the mount is read-only, and the owner's user number
/// means nothing on this machine.
pub fn to_fattr(id: u64, a: &Attr, uid: u32, gid: u32) -> fattr3 {
    let ftype = match a.kind {
        Kind::File => ftype3::NF3REG,
        Kind::Dir => ftype3::NF3DIR,
        Kind::Symlink => ftype3::NF3LNK,
        Kind::Other => ftype3::NF3FIFO,
    };
    fattr3 {
        ftype,
        mode: a.mode & 0o555,
        nlink: a.nlink,
        uid,
        gid,
        size: a.size,
        used: a.size,
        rdev: specdata3 {
            specdata1: 0,
            specdata2: 0,
        },
        fsid: 0x1575_e550,
        fileid: id,
        atime: time(a.mtime, a.mtime_nsec),
        mtime: time(a.mtime, a.mtime_nsec),
        ctime: time(a.ctime, a.ctime_nsec),
    }
}

/// Whether two byte strings are equal, looking at every byte whatever the first difference.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The most handles kept. The host lets its oldest ids go at 100 000; when more handles than
/// this are out, the oldest tenth are forgotten here too (the kernel then hears `STALE` and
/// looks the names up again).
const MAX_HANDLES: usize = 250_000;

/// Each id's random token, and the ids in the order their handles were made.
type Tokens = (HashMap<u64, [u8; 16]>, std::collections::VecDeque<u64>);

/// The random tokens of the handles handed out so far, and the order they were made in.
pub struct Handles {
    tokens: Mutex<Tokens>,
    random: Mutex<std::fs::File>,
}

impl Handles {
    pub fn new() -> std::io::Result<Handles> {
        Ok(Handles {
            tokens: Mutex::new((HashMap::new(), std::collections::VecDeque::new())),
            random: Mutex::new(std::fs::File::open("/dev/urandom")?),
        })
    }

    /// The handle of `id`, made the first time it is asked for.
    pub fn handle(&self, id: u64) -> Vec<u8> {
        self.handle_within(id, MAX_HANDLES)
    }

    fn handle_within(&self, id: u64, max: usize) -> Vec<u8> {
        let mut guard = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        let (tokens, order) = &mut *guard;
        if !tokens.contains_key(&id) {
            if tokens.len() >= max {
                let mut forget = (max / 10).max(1);
                while forget > 0 {
                    let Some(old) = order.pop_front() else { break };
                    // The folder's own handle is never forgotten.
                    if old != ROOT_ID && tokens.remove(&old).is_some() {
                        forget -= 1;
                    } else if old == ROOT_ID {
                        order.push_back(old);
                    }
                }
            }
            let mut token = [0u8; 16];
            let mut random = self.random.lock().unwrap_or_else(|e| e.into_inner());
            // Without randomness there is no handle worth handing out.
            random
                .read_exact(&mut token)
                .expect("cannot read /dev/urandom");
            tokens.insert(id, token);
            order.push_back(id);
        }
        let token = tokens[&id];
        let mut handle = Vec::with_capacity(HANDLE_LEN);
        handle.extend_from_slice(&id.to_le_bytes());
        handle.extend_from_slice(&token);
        handle
    }

    /// The id of a handle that was handed out. Anything that is not one is `BADHANDLE`,
    /// except a well-formed handle whose id is not known (any more): `STALE`, so that the
    /// kernel looks the name up again.
    pub fn id(&self, handle: &[u8]) -> Result<u64, nfsstat3> {
        if handle.len() != HANDLE_LEN {
            return Err(nfsstat3::NFS3ERR_BADHANDLE);
        }
        let id = u64::from_le_bytes(
            handle[..8]
                .try_into()
                .map_err(|_| nfsstat3::NFS3ERR_BADHANDLE)?,
        );
        let guard = self.tokens.lock().unwrap_or_else(|e| e.into_inner());
        match guard.0.get(&id) {
            None => Err(nfsstat3::NFS3ERR_STALE),
            Some(token) if same(token, &handle[8..]) => Ok(id),
            Some(_) => Err(nfsstat3::NFS3ERR_BADHANDLE),
        }
    }
}

/// Whether an entry the host listed in the folder `dir` can be shown here: a name of one
/// entry (no `/`, no NUL, not `.` or `..`, at most 255 bytes) and an id that is not 0 (taken
/// by the NFS library) and not the folder's root or the folder itself.
fn valid_entry(entry: &Entry, dir: u64) -> bool {
    istesso_link::mount::valid_name(&entry.name) && valid_child(entry.id, dir)
}

fn valid_child(id: u64, dir: u64) -> bool {
    id != 0 && id != ROOT_ID && id != dir
}

/// The longest link text taken from the host (`PATH_MAX`).
const MAX_LINK: usize = 1024;

pub struct RemoteFs {
    remote: Arc<Remote>,
    gate: Arc<Gate>,
    handles: Handles,
    uid: u32,
    gid: u32,
    /// Short-lived copy of the last listing of each folder, so that the NFS client's paged
    /// READDIRPLUS calls do not each ask the owner again. Kept in memory only, for one second.
    listings: Mutex<HashMap<u64, (Instant, Vec<Entry>)>>,
}

impl RemoteFs {
    pub fn new(remote: Arc<Remote>, gate: Arc<Gate>) -> std::io::Result<RemoteFs> {
        // SAFETY: getuid and getgid cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Ok(RemoteFs {
            remote,
            gate,
            handles: Handles::new()?,
            uid,
            gid,
            listings: Mutex::new(HashMap::new()),
        })
    }

    /// One question to the owner. A lost connection is an input/output error, at once.
    fn ask(&self, question: Question) -> Result<(Answer, Vec<u8>), nfsstat3> {
        let remote = self.remote.clone();
        tokio::task::block_in_place(move || remote.call(&question))
            .map_err(|Gone| nfsstat3::NFS3ERR_IO)
            .and_then(|(answer, data)| match answer {
                Answer::Failed { error } => Err(to_nfs_error(error)),
                answer => Ok((answer, data)),
            })
    }

    fn listing(&self, id: u64) -> Result<Vec<Entry>, nfsstat3> {
        if let Some((at, entries)) = self
            .listings
            .lock()
            .map_err(|_| nfsstat3::NFS3ERR_SERVERFAULT)?
            .get(&id)
        {
            if at.elapsed() < Duration::from_secs(1) {
                return Ok(entries.clone());
            }
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut names = std::collections::HashSet::new();
        let mut start = 0u32;
        loop {
            match self.ask(Question::ReadDir { id, start })? {
                (
                    Answer::Entries {
                        entries: page,
                        more,
                        next,
                    },
                    _,
                ) => {
                    let shown = page.len() as u32;
                    // What the host says becomes names and file ids in this machine's
                    // kernel: only entries that can be one are taken.
                    for entry in page {
                        if valid_entry(&entry, id) && names.insert(entry.name.clone()) {
                            entries.push(entry);
                        }
                    }
                    let next = next.unwrap_or(start.saturating_add(shown));
                    if !more || next <= start || entries.len() >= MAX_LISTED {
                        break;
                    }
                    start = next;
                }
                _ => return Err(nfsstat3::NFS3ERR_SERVERFAULT),
            }
        }
        self.listings
            .lock()
            .map_err(|_| nfsstat3::NFS3ERR_SERVERFAULT)?
            .insert(id, (Instant::now(), entries.clone()));
        Ok(entries)
    }
}

#[async_trait]
impl NFSFileSystem for RemoteFs {
    fn capabilities(&self) -> VFSCapabilities {
        VFSCapabilities::ReadOnly
    }

    fn root_dir(&self) -> fileid3 {
        ROOT_ID
    }

    fn id_to_fh(&self, id: fileid3) -> nfs_fh3 {
        nfs_fh3 {
            data: self.handles.handle(id),
        }
    }

    fn fh_to_id(&self, fh: &nfs_fh3) -> Result<fileid3, nfsstat3> {
        self.handles.id(&fh.data).inspect_err(|_| {
            self.gate.refused_handles.fetch_add(1, Ordering::Relaxed);
        })
    }

    /// Called for MNT only. The first one gets the folder; every later one is refused.
    async fn path_to_id(&self, path: &[u8]) -> Result<fileid3, nfsstat3> {
        if self.gate.mount_asked.swap(true, Ordering::SeqCst) {
            self.gate.refused_mounts.fetch_add(1, Ordering::Relaxed);
            return Err(nfsstat3::NFS3ERR_ACCES);
        }
        if path.iter().all(|&b| b == b'/') {
            Ok(ROOT_ID)
        } else {
            Err(nfsstat3::NFS3ERR_NOENT)
        }
    }

    async fn lookup(&self, dirid: fileid3, filename: &filename3) -> Result<fileid3, nfsstat3> {
        self.gate.check()?;
        let name = filename.0.clone();
        if name == b"." {
            return Ok(dirid);
        }
        if !istesso_link::mount::valid_name(&name) {
            return Err(nfsstat3::NFS3ERR_INVAL);
        }
        match self.ask(Question::Lookup { dir: dirid, name })? {
            (Answer::Found { id, .. }, _) if valid_child(id, dirid) => Ok(id),
            _ => Err(nfsstat3::NFS3ERR_IO),
        }
    }

    async fn getattr(&self, id: fileid3) -> Result<fattr3, nfsstat3> {
        match self.ask(Question::Stat { id })? {
            (Answer::Attr { attr }, _) => Ok(to_fattr(id, &attr, self.uid, self.gid)),
            _ => Err(nfsstat3::NFS3ERR_SERVERFAULT),
        }
    }

    async fn setattr(&self, _id: fileid3, _setattr: sattr3) -> Result<fattr3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn read(
        &self,
        id: fileid3,
        offset: u64,
        count: u32,
    ) -> Result<(Vec<u8>, bool), nfsstat3> {
        self.gate.check()?;
        match self.ask(Question::Read { id, offset, count })? {
            // Never more than was asked for.
            (Answer::Data { eof }, data) if data.len() <= count as usize => Ok((data, eof)),
            _ => Err(nfsstat3::NFS3ERR_IO),
        }
    }

    async fn write(&self, _id: fileid3, _offset: u64, _data: &[u8]) -> Result<fattr3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn create(
        &self,
        _d: fileid3,
        _f: &filename3,
        _a: sattr3,
    ) -> Result<(fileid3, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn create_exclusive(&self, _d: fileid3, _f: &filename3) -> Result<fileid3, nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn mkdir(&self, _d: fileid3, _f: &filename3) -> Result<(fileid3, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn remove(&self, _d: fileid3, _f: &filename3) -> Result<(), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn rename(
        &self,
        _a: fileid3,
        _b: &filename3,
        _c: fileid3,
        _d: &filename3,
    ) -> Result<(), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn readdir(
        &self,
        dirid: fileid3,
        start_after: fileid3,
        max_entries: usize,
    ) -> Result<ReadDirResult, nfsstat3> {
        self.gate.check()?;
        let entries = self.listing(dirid)?;
        let start = if start_after == 0 {
            0
        } else {
            match entries.iter().position(|e| e.id == start_after) {
                Some(i) => i + 1,
                None => return Err(nfsstat3::NFS3ERR_BAD_COOKIE),
            }
        };
        let page: Vec<DirEntry> = entries
            .iter()
            .skip(start)
            .take(max_entries)
            .map(|e| DirEntry {
                fileid: e.id,
                name: nfsstring(e.name.clone()),
                attr: to_fattr(e.id, &e.attr, self.uid, self.gid),
            })
            .collect();
        let end = start + page.len() >= entries.len();
        Ok(ReadDirResult { entries: page, end })
    }

    async fn symlink(
        &self,
        _d: fileid3,
        _l: &filename3,
        _s: &nfspath3,
        _a: &sattr3,
    ) -> Result<(fileid3, fattr3), nfsstat3> {
        Err(nfsstat3::NFS3ERR_ROFS)
    }

    async fn readlink(&self, id: fileid3) -> Result<nfspath3, nfsstat3> {
        self.gate.check()?;
        match self.ask(Question::ReadLink { id })? {
            (Answer::Link { target }, _)
                if !target.is_empty() && target.len() <= MAX_LINK && !target.contains(&0) =>
            {
                Ok(nfsstring(target))
            }
            _ => Err(nfsstat3::NFS3ERR_IO),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attributes_lose_write_bits_and_take_the_local_owner() {
        let a = Attr {
            kind: Kind::File,
            mode: 0o4766,
            nlink: 1,
            size: 5,
            mtime: 10,
            mtime_nsec: 1,
            ctime: 11,
            ctime_nsec: 2,
        };
        let f = to_fattr(7, &a, 501, 20);
        assert_eq!(f.mode, 0o544);
        assert_eq!((f.uid, f.gid, f.fileid, f.size), (501, 20, 7, 5));
        assert!(matches!(f.ftype, ftype3::NF3REG));
    }

    #[test]
    fn only_handles_that_were_handed_out_are_taken() {
        let handles = Handles::new().unwrap();
        let root = handles.handle(ROOT_ID);
        assert_eq!(root.len(), HANDLE_LEN);
        assert!(matches!(handles.id(&root), Ok(ROOT_ID)));
        // The same id gets the same handle while the server runs.
        assert_eq!(handles.handle(ROOT_ID), root);
        let other = handles.handle(2);
        assert_ne!(other[8..], root[8..]);
        // A wrong token, an id never handed out, the library's own format, a short handle.
        let mut forged = root.clone();
        forged[HANDLE_LEN - 1] ^= 1;
        assert!(matches!(
            handles.id(&forged),
            Err(nfsstat3::NFS3ERR_BADHANDLE)
        ));
        let mut guessed = 3u64.to_le_bytes().to_vec();
        guessed.extend_from_slice(&root[8..]);
        assert!(matches!(handles.id(&guessed), Err(nfsstat3::NFS3ERR_STALE)));
        let mut plain = 0u64.to_le_bytes().to_vec();
        plain.extend_from_slice(&ROOT_ID.to_le_bytes());
        assert!(matches!(
            handles.id(&plain),
            Err(nfsstat3::NFS3ERR_BADHANDLE)
        ));
        assert!(matches!(
            handles.id(&root[..8]),
            Err(nfsstat3::NFS3ERR_BADHANDLE)
        ));
        // A token taken from one handle does not open another id.
        let mut swapped = 2u64.to_le_bytes().to_vec();
        swapped.extend_from_slice(&root[8..]);
        assert!(matches!(
            handles.id(&swapped),
            Err(nfsstat3::NFS3ERR_BADHANDLE)
        ));
    }

    #[test]
    fn only_entries_that_can_be_names_and_ids_here_are_taken() {
        let attr = Attr {
            kind: Kind::File,
            mode: 0o644,
            nlink: 1,
            size: 0,
            mtime: 0,
            mtime_nsec: 0,
            ctime: 0,
            ctime_nsec: 0,
        };
        let entry = |id: u64, name: &[u8]| Entry {
            id,
            name: name.to_vec(),
            attr,
        };
        assert!(valid_entry(&entry(5, b"main.rs"), 2));
        for (id, name) in [
            (5, &b"a/b"[..]),
            (5, b".."),
            (5, b"."),
            (5, b""),
            (5, b"a\0b"),
            (0, b"zero"),
            (ROOT_ID, b"root"),
            (2, b"itself"),
        ] {
            assert!(!valid_entry(&entry(id, name), 2), "{id} {name:?}");
        }
        assert!(!valid_entry(&entry(5, &[b'a'; 256]), 2));
    }

    #[test]
    fn the_oldest_handles_are_forgotten_but_never_the_folders_own() {
        let handles = Handles::new().unwrap();
        let root = handles.handle_within(ROOT_ID, 10);
        let first = handles.handle_within(2, 10);
        for id in 3..40 {
            handles.handle_within(id, 10);
        }
        assert!(matches!(handles.id(&root), Ok(ROOT_ID)));
        assert!(matches!(handles.id(&first), Err(nfsstat3::NFS3ERR_STALE)));
        assert!(handles.tokens.lock().unwrap().0.len() <= 10);
        let last = handles.handle_within(39, 10);
        assert!(matches!(handles.id(&last), Ok(39)));
    }

    #[test]
    fn contents_wait_for_the_gate() {
        let gate = Gate::default();
        assert!(!gate.mount_asked.swap(true, Ordering::SeqCst));
        assert!(gate.mount_asked.swap(true, Ordering::SeqCst));
        assert!(matches!(gate.check(), Err(nfsstat3::NFS3ERR_ACCES)));
        gate.open();
        assert!(gate.check().is_ok());
        assert_eq!(gate.refused_early.load(Ordering::Relaxed), 1);
    }
}
