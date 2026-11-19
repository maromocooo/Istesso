//! Makes two configuration directories of devices that are paired with each other, for checks
//! on one machine with keys of their own (development only).
//!
//! ```text
//! cargo run -p istesso-media --example paired-dirs -- HOST_DIR VIEWER_DIR
//! ```
//!
//! Each directory is what the C interface's `config_dir` takes: an identity of its own, made
//! afresh here, and a list of paired devices that trusts the other one, called "check-viewer"
//! in HOST_DIR and "check-host" in VIEWER_DIR, with the rights a newly paired device has. The
//! directories are made readable by their owner only, the files as the identity and the list
//! of paired devices are always written. Neither directory may exist yet: nothing is
//! overwritten, and the machine's own identity and pairings are left alone.
//!
//! Prints the two directories and nothing else; never a key.

use std::path::{Path, PathBuf};

use istesso_agent::Paths;
use istesso_pair::{Hello, Identity, Peer, PeerStore};

type Failure = Box<dyn std::error::Error>;

fn main() {
    if let Err(err) = run(std::env::args().skip(1).collect()) {
        eprintln!("paired-dirs: {err}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), Failure> {
    let [host, viewer] = <[String; 2]>::try_from(args)
        .map_err(|_| "give two directories: HOST_DIR VIEWER_DIR")?
        .map(PathBuf::from);
    if host == viewer {
        return Err("the two directories have to be different".into());
    }
    // Both are looked at before either is made, so that a refusal leaves nothing behind.
    for dir in [&host, &viewer] {
        if dir.symlink_metadata().is_ok() {
            return Err(format!("{} exists already; nothing was made", dir.display()).into());
        }
    }
    make_private_dir(&host)?;
    make_private_dir(&viewer)?;
    let (host_paths, viewer_paths) = (Paths::at(&host), Paths::at(&viewer));
    let host_identity = Identity::load_or_create(&host_paths.identity())?;
    let viewer_identity = Identity::load_or_create(&viewer_paths.identity())?;
    for (paths, name, other) in [
        (&host_paths, "check-viewer", &viewer_identity),
        (&viewer_paths, "check-host", &host_identity),
    ] {
        // As a pairing that has just been completed records it: the default rights.
        PeerStore::open(paths.peers())?.add(Peer::from_hello(&Hello {
            device_name: name.into(),
            public_key: other.public_key(),
        }))?;
    }
    println!("{}", host.display());
    println!("{}", viewer.display());
    Ok(())
}

/// Makes `dir`, which must not exist yet, readable by its owner only. Its parent must exist.
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}
