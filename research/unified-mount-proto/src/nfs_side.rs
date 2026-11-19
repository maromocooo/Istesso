//! The viewing side: a read-only NFSv3 server on 127.0.0.1 that answers the operating system's
//! own NFS client by asking the machine that owns the folder (through `Remote`).

use crate::wire::{self, Attr, Kind, Request, Response};
use async_trait::async_trait;
use nfsserve::nfs::{
    fattr3, fileid3, filename3, ftype3, nfspath3, nfsstat3, nfsstring, nfstime3, sattr3, specdata3,
};
use nfsserve::vfs::{DirEntry, NFSFileSystem, ReadDirResult, VFSCapabilities};
use std::collections::HashMap;
use std::io;
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// The connection to the owning machine: here, a child process's standard input and output.
pub struct Remote {
    io: Mutex<(ChildStdin, ChildStdout)>,
}

impl Remote {
    pub fn from_child(child: &mut Child) -> io::Result<Remote> {
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("child has no stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("child has no stdout"))?;
        Ok(Remote {
            io: Mutex::new((stdin, stdout)),
        })
    }

    /// One request, one answer. Requests are sent one at a time.
    pub fn call(&self, req: &Request) -> Result<(Response, Vec<u8>), nfsstat3> {
        let mut guard = self.io.lock().map_err(|_| nfsstat3::NFS3ERR_SERVERFAULT)?;
        let (stdin, stdout) = &mut *guard;
        wire::write_frame(stdin, req, b"").map_err(|_| nfsstat3::NFS3ERR_IO)?;
        match wire::read_frame::<_, Response>(stdout) {
            Ok(Some(r)) => Ok(r),
            _ => Err(nfsstat3::NFS3ERR_IO),
        }
    }
}

fn to_nfs_error(e: wire::Error) -> nfsstat3 {
    match e {
        wire::Error::NoEnt => nfsstat3::NFS3ERR_NOENT,
        wire::Error::NotDir => nfsstat3::NFS3ERR_NOTDIR,
        wire::Error::IsDir => nfsstat3::NFS3ERR_ISDIR,
        wire::Error::Access => nfsstat3::NFS3ERR_ACCES,
        wire::Error::Invalid | wire::Error::Loop => nfsstat3::NFS3ERR_INVAL,
        wire::Error::Stale => nfsstat3::NFS3ERR_STALE,
        wire::Error::Io => nfsstat3::NFS3ERR_IO,
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

pub struct RemoteFs {
    remote: Remote,
    uid: u32,
    gid: u32,
    /// Short-lived copy of the last listing of each folder, so that the NFS client's paged
    /// READDIRPLUS calls do not each ask the owner again. Kept in memory only, for one second.
    listings: Mutex<HashMap<u64, (Instant, Vec<wire::Entry>)>>,
}

impl RemoteFs {
    pub fn new(remote: Remote) -> RemoteFs {
        // SAFETY: getuid and getgid cannot fail.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        RemoteFs {
            remote,
            uid,
            gid,
            listings: Mutex::new(HashMap::new()),
        }
    }

    fn listing(&self, id: u64) -> Result<Vec<wire::Entry>, nfsstat3> {
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
        match self.remote.call(&Request::Readdir { id })? {
            (Response::Entries { entries }, _) => {
                self.listings
                    .lock()
                    .map_err(|_| nfsstat3::NFS3ERR_SERVERFAULT)?
                    .insert(id, (Instant::now(), entries.clone()));
                Ok(entries)
            }
            (Response::Failed { error }, _) => Err(to_nfs_error(error)),
            _ => Err(nfsstat3::NFS3ERR_SERVERFAULT),
        }
    }
}

#[async_trait]
impl NFSFileSystem for RemoteFs {
    fn capabilities(&self) -> VFSCapabilities {
        VFSCapabilities::ReadOnly
    }

    fn root_dir(&self) -> fileid3 {
        crate::export::ROOT_ID
    }

    async fn lookup(&self, dirid: fileid3, filename: &filename3) -> Result<fileid3, nfsstat3> {
        let name = filename.0.clone();
        if name == b"." {
            return Ok(dirid);
        }
        match self.remote.call(&Request::Lookup { dir: dirid, name })? {
            (Response::Found { id, .. }, _) => Ok(id),
            (Response::Failed { error }, _) => Err(to_nfs_error(error)),
            _ => Err(nfsstat3::NFS3ERR_SERVERFAULT),
        }
    }

    async fn getattr(&self, id: fileid3) -> Result<fattr3, nfsstat3> {
        match self.remote.call(&Request::Getattr { id })? {
            (Response::Attr { attr }, _) => Ok(to_fattr(id, &attr, self.uid, self.gid)),
            (Response::Failed { error }, _) => Err(to_nfs_error(error)),
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
        match self.remote.call(&Request::Read { id, offset, count })? {
            (Response::Data { eof }, data) => Ok((data, eof)),
            (Response::Failed { error }, _) => Err(to_nfs_error(error)),
            _ => Err(nfsstat3::NFS3ERR_SERVERFAULT),
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
        match self.remote.call(&Request::Readlink { id })? {
            (Response::Link { target }, _) => Ok(nfsstring(target)),
            (Response::Failed { error }, _) => Err(to_nfs_error(error)),
            _ => Err(nfsstat3::NFS3ERR_SERVERFAULT),
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
}
