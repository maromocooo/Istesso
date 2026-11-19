//! Where this device keeps its files.
//!
//! * the configuration directory holds what must not be lost: the device
//!   identity and the list of paired devices;
//! * the cache directory holds what can be rebuilt: the last known state of
//!   the other devices.
//!
//! The places are fixed: they come from the account (its home directory in
//! the system's user database, its runtime directory), never from environment
//! variables, arguments or files that a program of the same user can set. A
//! program that holds this device's key, reaches other devices or is given
//! permissions by the system must not be pointed at a peers list or a socket
//! of somebody else's choosing (ADR 0021, "the same user's other programs").
//! Development builds (the `dev-paths` feature) can move the directories
//! with `ISTESSO_CONFIG_DIR` and `ISTESSO_CACHE_DIR`, and the runtime
//! directory (where the input socket is) with `XDG_RUNTIME_DIR`; tests use
//! [`Paths::at`]. Such a build says so on standard error, carries
//! [`DEV_PATHS_MARKER`] for the signing step to refuse, and must never be
//! given permissions or a real key. The trial ("daily") is the one accepted
//! exception: it runs several identities side by side and builds with
//! `dev-paths`, knowingly leaving its signed host open to being started with
//! somebody else's directories.

use std::path::{Path, PathBuf};

/// Directory name used below the platform's standard locations.
const APP_DIR: &str = "istesso";

/// Whether this is a development build that takes its places from the
/// environment (feature `dev-paths`).
pub const DEV_PATHS: bool = cfg!(feature = "dev-paths");

/// Text that a development build carries in its binary and a release build
/// never does: the signing step refuses a binary that contains it
/// (`scripts/refuse-dev-build.sh`).
pub const DEV_PATHS_MARKER: &str = "ISTESSO-DEV-PATHS-BUILD";

/// The marker, kept in the binary and exported (`nm` finds it).
#[cfg(feature = "dev-paths")]
#[used]
#[no_mangle]
pub static ISTESSO_DEV_PATHS_BUILD: [u8; 24] = *b"ISTESSO-DEV-PATHS-BUILD\0";

/// The warning a development build gives (`istesso doctor`, and once per
/// process on standard error); `None` in any other build. The marker is in
/// the text only in development builds.
pub fn development_build() -> Option<String> {
    #[cfg(feature = "dev-paths")]
    {
        // Kept in the binary, whatever the linker removes.
        std::hint::black_box(&ISTESSO_DEV_PATHS_BUILD);
        Some(format!(
            "warning: development build [{DEV_PATHS_MARKER}]: ISTESSO_CONFIG_DIR, \
             ISTESSO_CACHE_DIR and XDG_RUNTIME_DIR move the identity, the paired devices and \
             the sockets; never give this build permissions or a real key, never sign it"
        ))
    }
    #[cfg(not(feature = "dev-paths"))]
    None
}

/// Says once per process, on standard error, that this build takes its
/// places from the environment.
#[cfg(feature = "dev-paths")]
fn warn_once() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(warning) = development_build() {
            // Never a reason to stop: standard error may be a pipe nobody
            // reads any more.
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "{warning}");
        }
    });
}

/// What this build does not guard the device key and the list of paired
/// devices against, in one line for the person (`istesso doctor`, and the
/// agent when it starts). Every form there is today has such a line.
pub fn keeping_notice() -> Option<String> {
    Paths::from_env()
        .keeping()
        .open_to_the_same_user()
        .map(|what| format!("note: {what}"))
}

#[cfg(all(feature = "login-keychain", feature = "dev-paths"))]
compile_error!(
    "`login-keychain` and `dev-paths` do not go together: a development build takes its \
     places from the environment and must never hold a real key"
);

/// The configuration directory of the account itself, as a build without
/// any override resolves it.
#[cfg(feature = "login-keychain")]
fn accounts_own_config() -> &'static PathBuf {
    static OWN: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    OWN.get_or_init(|| {
        Paths::resolve(&Environment {
            home: account_home(),
            macos: cfg!(target_os = "macos"),
            ..Environment::default()
        })
        .config
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// The account's runtime directory, which nobody else can enter
    /// (Linux: `/run/user/<uid>`); `None` where there is none.
    pub runtime: Option<PathBuf>,
}

/// What decides the locations.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// The account's home directory, from the system's user database. Not
    /// `HOME`, which whoever starts the program chooses.
    pub home: Option<PathBuf>,
    /// The account's runtime directory, when it exists, belongs to the
    /// account and nobody else can enter it.
    pub runtime: Option<PathBuf>,
    pub macos: bool,
    /// `ISTESSO_CONFIG_DIR`, read only by development builds (`dev-paths`).
    pub config_override: Option<PathBuf>,
    /// `ISTESSO_CACHE_DIR`, read only by development builds (`dev-paths`).
    pub cache_override: Option<PathBuf>,
    /// `XDG_RUNTIME_DIR`, read only by development builds (`dev-paths`).
    pub runtime_override: Option<PathBuf>,
}

impl Environment {
    pub fn current() -> Self {
        Self::gather(|name| std::env::var_os(name))
    }

    /// The environment, with `var` for looking up environment variables.
    /// Only development builds look anything up.
    fn gather(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Self {
        #[cfg(feature = "dev-paths")]
        let (config_override, cache_override, runtime_override) = {
            let var = |name: &str| var(name).filter(|v| !v.is_empty()).map(PathBuf::from);
            (
                var("ISTESSO_CONFIG_DIR"),
                var("ISTESSO_CACHE_DIR"),
                var("XDG_RUNTIME_DIR"),
            )
        };
        #[cfg(not(feature = "dev-paths"))]
        let (config_override, cache_override, runtime_override) = {
            let _ = var;
            (None, None, None)
        };
        Environment {
            home: account_home(),
            runtime: account_runtime_dir(),
            macos: cfg!(target_os = "macos"),
            config_override,
            cache_override,
            runtime_override,
        }
    }
}

/// The home directory the system's user database gives this process's
/// account.
#[cfg(unix)]
fn account_home() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;

    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let mut buffer = vec![0u8; 4096];
    loop {
        // SAFETY: a passwd is plain data; all zeroes is a valid value.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: every pointer points at memory of the length given, which
        // lives until the strings in `entry` have been copied out below.
        let error = unsafe {
            libc::getpwuid_r(
                uid,
                &mut entry,
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                &mut found,
            )
        };
        if error == libc::ERANGE && buffer.len() < 1 << 20 {
            buffer.resize(buffer.len() * 2, 0);
            continue;
        }
        if error != 0 || found.is_null() || entry.pw_dir.is_null() {
            return None;
        }
        // SAFETY: a C string inside `buffer`, by getpwuid_r's promise.
        let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
        let dir = PathBuf::from(OsStr::from_bytes(dir.to_bytes()));
        return dir.is_absolute().then_some(dir);
    }
}

#[cfg(not(unix))]
fn account_home() -> Option<PathBuf> {
    None
}

/// `/run/user/<uid>`, if it is a directory of this account that nobody else
/// can enter. Not `XDG_RUNTIME_DIR`, which whoever starts the program chooses.
#[cfg(target_os = "linux")]
fn account_runtime_dir() -> Option<PathBuf> {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    runtime_dir_if_ours(Path::new(&format!("/run/user/{uid}")), uid)
}

#[cfg(not(target_os = "linux"))]
fn account_runtime_dir() -> Option<PathBuf> {
    None
}

/// `dir`, if it is a directory itself (not a link), belongs to `uid`, and
/// nobody else can enter it.
#[cfg(any(target_os = "linux", all(test, unix)))]
fn runtime_dir_if_ours(dir: &Path, uid: u32) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let meta = std::fs::symlink_metadata(dir).ok()?;
    (meta.is_dir() && meta.uid() == uid && meta.mode() & 0o077 == 0).then(|| dir.to_path_buf())
}

/// Why a place for a socket was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotOurPlace(pub String);

impl std::fmt::Display for NotOurPlace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotOurPlace {}

impl Paths {
    /// The locations for the current account.
    pub fn from_env() -> Self {
        #[cfg(feature = "dev-paths")]
        warn_once();
        Self::resolve(&Environment::current())
    }

    pub fn resolve(env: &Environment) -> Self {
        // Without a home there is no place of the account's own: one that
        // does not exist, rather than one relative to where the program was
        // started.
        let home = env
            .home
            .clone()
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        let config = match &env.config_override {
            Some(dir) => dir.clone(),
            None if env.macos => home.join("Library/Application Support").join(APP_DIR),
            None => home.join(".config").join(APP_DIR),
        };
        let cache = match (&env.cache_override, &env.config_override) {
            (Some(dir), _) => dir.clone(),
            // Someone who moved the configuration wants everything in one
            // place (tests, portable setups).
            (None, Some(config)) => config.join("cache"),
            (None, None) if env.macos => home.join("Library/Caches").join(APP_DIR),
            (None, None) => home.join(".cache").join(APP_DIR),
        };
        let runtime = match (&env.runtime_override, &env.config_override) {
            (Some(dir), _) => Some(dir.clone()),
            // Moved configuration, and no runtime directory of its own: the
            // input socket goes next to the moved cache.
            (None, Some(_)) => None,
            (None, None) => env.runtime.clone(),
        };
        Paths {
            config,
            cache,
            runtime,
        }
    }

    /// Everything below one directory.
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        let config = dir.into();
        let cache = config.join("cache");
        Paths {
            config,
            cache,
            runtime: None,
        }
    }

    /// Where a program of this device may listen for input to pass on:
    /// `asked`, if it names a file directly in the cache directory or in
    /// `istesso/` in the runtime directory (not the runtime directory itself,
    /// where other programs' sockets are), which has to exist. Development
    /// builds (`dev-paths`) take any place.
    pub fn local_socket(&self, asked: &Path) -> Result<PathBuf, NotOurPlace> {
        if cfg!(feature = "dev-paths") {
            return Ok(asked.to_path_buf());
        }
        self.socket_in_our_places(asked)
    }

    fn socket_in_our_places(&self, asked: &Path) -> Result<PathBuf, NotOurPlace> {
        let runtime = self.runtime.as_ref().map(|dir| dir.join(APP_DIR));
        let refuse = || {
            let mut places = self.cache.display().to_string();
            if let Some(dir) = &runtime {
                places = format!("{places} or {}", dir.display());
            }
            NotOurPlace(format!(
                "{} is not a place of Istesso's: a socket goes directly into {places}",
                asked.display(),
            ))
        };
        let name = asked
            .file_name()
            .filter(|name| Path::new(name) != Path::new(".."))
            .ok_or_else(refuse)?;
        let parent = asked.parent().ok_or_else(refuse)?;
        let parent = std::fs::canonicalize(parent).map_err(|_| refuse())?;
        let ours = [Some(&self.cache), runtime.as_ref()]
            .into_iter()
            .flatten()
            .filter_map(|dir| std::fs::canonicalize(dir).ok())
            .any(|dir| dir == parent);
        if ours {
            Ok(parent.join(name))
        } else {
            Err(refuse())
        }
    }

    /// The file the file form keeps the device key in. The way to the key
    /// is [`Paths::keeping`].
    pub fn identity(&self) -> PathBuf {
        self.config.join("identity.key")
    }

    /// The one door to this device's key and its list of paired devices
    /// (`istesso_pair::Keeping`). The account's own key and list are kept in
    /// the form this build was made for; directories that are not the
    /// account's own (tests, development builds) always keep theirs in
    /// files, so that no test and no development build reaches the person's
    /// keychain.
    pub fn keeping(&self) -> istesso_pair::Keeping {
        #[cfg(feature = "login-keychain")]
        if self.config == *accounts_own_config() {
            return istesso_pair::Keeping::of_this_build(self.identity(), self.peers());
        }
        istesso_pair::Keeping::in_files(self.identity(), self.peers())
    }

    /// Where the program that shows an application of this machine to
    /// another device takes that device's keyboard and pointer input: a
    /// datagram socket the program creates, one input event per datagram,
    /// as JSON. In the account's runtime directory, which nobody else can
    /// reach; next to the cache where there is none. Never where an
    /// environment variable says: the agent passes on what paired devices
    /// type.
    pub fn input_socket(&self) -> PathBuf {
        match &self.runtime {
            Some(dir) => dir.join(APP_DIR).join("input.sock"),
            None => self.cache.join("input.sock"),
        }
    }

    /// The file the file form keeps the list of paired devices in. The way
    /// to the list is [`Paths::keeping`].
    pub fn peers(&self) -> PathBuf {
        self.config.join("peers.json")
    }

    /// Last known state of the paired device `id`. `None` when `id` does not
    /// look like a key fingerprint, so that a damaged peer list can never
    /// point outside the cache directory.
    pub fn host_snapshot(&self, id: &str) -> Option<PathBuf> {
        let plausible = !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
        plausible.then(|| self.cache.join("hosts").join(format!("{id}.json")))
    }

    /// The result of the last full inspection of this machine.
    pub fn local_manifest(&self) -> PathBuf {
        self.cache.join("local-manifest.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment {
            home: Some(PathBuf::from("/home/u")),
            ..Environment::default()
        }
    }

    #[test]
    fn linux_uses_the_account_home() {
        let paths = Paths::resolve(&env());
        assert_eq!(paths.config, PathBuf::from("/home/u/.config/istesso"));
        assert_eq!(paths.cache, PathBuf::from("/home/u/.cache/istesso"));
        assert_eq!(
            paths.input_socket(),
            PathBuf::from("/home/u/.cache/istesso/input.sock")
        );

        let paths = Paths::resolve(&Environment {
            runtime: Some(PathBuf::from("/run/user/1000")),
            ..env()
        });
        assert_eq!(
            paths.input_socket(),
            PathBuf::from("/run/user/1000/istesso/input.sock")
        );
    }

    #[test]
    fn macos_uses_the_library_folders() {
        let paths = Paths::resolve(&Environment {
            macos: true,
            ..env()
        });
        assert_eq!(
            paths.config,
            PathBuf::from("/home/u/Library/Application Support/istesso")
        );
        assert_eq!(paths.cache, PathBuf::from("/home/u/Library/Caches/istesso"));
    }

    #[test]
    fn without_a_home_nothing_is_relative() {
        let paths = Paths::resolve(&Environment::default());
        assert!(paths.config.is_absolute());
        assert!(paths.cache.is_absolute());
    }

    /// A program of the same user chooses the environment of what it starts:
    /// a build with a real key must not let it choose the peers list, the
    /// key or the input socket.
    #[cfg(not(feature = "dev-paths"))]
    #[test]
    fn no_environment_variable_moves_anything() {
        let asked = std::cell::RefCell::new(Vec::new());
        let env = Environment::gather(|name| {
            asked.borrow_mut().push(name.to_string());
            Some("/elsewhere".into())
        });
        assert!(asked.borrow().is_empty(), "looked up {:?}", asked.borrow());
        assert_eq!(env.config_override, None);
        assert_eq!(env.cache_override, None);
    }

    #[cfg(unix)]
    #[test]
    fn the_home_comes_from_the_user_database() {
        let home = account_home().expect("every account has a home");
        assert!(home.is_absolute());
        assert_eq!(Environment::current().home, Some(home));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_runtime_dir_is_the_accounts_own() {
        if let Some(dir) = account_runtime_dir() {
            // SAFETY: getuid cannot fail.
            let uid = unsafe { libc::getuid() };
            assert_eq!(dir, PathBuf::from(format!("/run/user/{uid}")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_runtime_dir_others_can_enter_is_not_ours() {
        use std::os::unix::fs::PermissionsExt;

        let root = TestDir::new("runtime");
        // SAFETY: getuid cannot fail.
        let uid = unsafe { libc::getuid() };
        let dir = root.0.join("rt");
        std::fs::create_dir(&dir).unwrap();
        let set = |mode| std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(mode));

        set(0o700).unwrap();
        assert_eq!(runtime_dir_if_ours(&dir, uid), Some(dir.clone()));
        // Another account's.
        assert_eq!(runtime_dir_if_ours(&dir, uid.wrapping_add(1)), None);
        // Others can enter or read it.
        for mode in [0o750, 0o705, 0o755, 0o701] {
            set(mode).unwrap();
            assert_eq!(runtime_dir_if_ours(&dir, uid), None, "mode {mode:o}");
        }
        set(0o700).unwrap();
        // A link to it, a file, nothing.
        let link = root.0.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert_eq!(runtime_dir_if_ours(&link, uid), None);
        let file = root.0.join("file");
        std::fs::write(&file, b"").unwrap();
        assert_eq!(runtime_dir_if_ours(&file, uid), None);
        assert_eq!(runtime_dir_if_ours(&root.0.join("missing"), uid), None);
    }

    #[cfg(feature = "dev-paths")]
    #[test]
    fn development_builds_move_the_runtime_dir_too() {
        // As the trial starts its agent: everything below its own folder.
        let env = Environment::gather(|name| match name {
            "ISTESSO_CONFIG_DIR" => Some("/run/x/config".into()),
            "ISTESSO_CACHE_DIR" => Some("/run/x/agent-cache".into()),
            "XDG_RUNTIME_DIR" => Some("/run/x/rt".into()),
            _ => None,
        });
        let paths = Paths::resolve(&env);
        assert_eq!(paths.config, PathBuf::from("/run/x/config"));
        assert_eq!(paths.cache, PathBuf::from("/run/x/agent-cache"));
        assert_eq!(
            paths.input_socket(),
            PathBuf::from("/run/x/rt/istesso/input.sock")
        );
    }

    #[cfg(feature = "dev-paths")]
    #[test]
    fn development_builds_can_move_everything() {
        let env = Environment::gather(|name| match name {
            "ISTESSO_CONFIG_DIR" => Some("/custom".into()),
            _ => None,
        });
        assert_eq!(env.config_override, Some(PathBuf::from("/custom")));
        let paths = Paths::resolve(&Environment {
            runtime: Some(PathBuf::from("/run/user/1000")),
            ..env
        });
        assert_eq!(paths, Paths::at("/custom"));

        let paths = Paths::resolve(&Environment {
            config_override: Some(PathBuf::from("/custom")),
            cache_override: Some(PathBuf::from("/tmp/c")),
            ..Environment::default()
        });
        assert_eq!(paths.cache, PathBuf::from("/tmp/c"));
    }

    /// A folder of the test's own, removed also when the test fails.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("istesso-paths-test-{}-{tag}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            TestDir(dir)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn sockets_go_directly_into_our_places() {
        let dir = TestDir::new("sockets");
        let root = &dir.0;
        let mut paths = Paths::at(root.join("config"));
        paths.runtime = Some(root.join("rt"));
        let elsewhere = root.join("elsewhere");
        // Not there yet: nothing is ours.
        assert!(paths
            .socket_in_our_places(&paths.cache.join("in.sock"))
            .is_err());
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::create_dir_all(&paths.cache).unwrap();
        std::fs::create_dir_all(root.join("rt").join(APP_DIR)).unwrap();
        assert!(paths
            .socket_in_our_places(&root.join("rt").join(APP_DIR).join("input.sock"))
            .is_ok());

        let ok = paths
            .socket_in_our_places(&paths.cache.join("probe-input.sock"))
            .unwrap();
        assert_eq!(ok.file_name().unwrap(), "probe-input.sock");
        assert_eq!(
            ok.parent().unwrap(),
            std::fs::canonicalize(&paths.cache).unwrap()
        );

        for refused in [
            elsewhere.join("in.sock"),
            paths.cache.join("sub").join("in.sock"),
            paths.cache.join(".."),
            paths.cache.join("..").join("elsewhere").join("in.sock"),
            PathBuf::from("in.sock"),
            // Beside other programs' sockets (the display's, the bus's).
            root.join("rt").join("wayland-0"),
        ] {
            assert!(
                paths.socket_in_our_places(&refused).is_err(),
                "{} was taken",
                refused.display()
            );
        }

        // A link from our place to another is not our place.
        let link = paths.cache.join("link");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(paths.socket_in_our_places(&link.join("in.sock")).is_err());
    }

    #[test]
    fn files_have_fixed_names() {
        let paths = Paths::at("/d");
        assert_eq!(paths.identity(), PathBuf::from("/d/identity.key"));
        assert_eq!(paths.peers(), PathBuf::from("/d/peers.json"));
        assert_eq!(
            paths.host_snapshot("1a2b-3c4d-5e6f-7a8b"),
            Some(PathBuf::from("/d/cache/hosts/1a2b-3c4d-5e6f-7a8b.json"))
        );
    }

    #[test]
    fn a_directory_that_is_not_the_accounts_own_keeps_its_key_in_files() {
        // Whatever the build was made for: tests and development builds
        // never reach the keychain of whoever runs them.
        let dir = crate::testing::TempDir::new("keeping");
        let keeping = dir.paths().keeping();
        assert_eq!(keeping.form(), istesso_pair::Form::Files);
        assert!(keeping.identity_if_any().unwrap().is_none());
        let made = keeping.identity().unwrap();
        assert!(dir.paths().identity().exists());
        let again = dir.paths().keeping().identity_if_any().unwrap().unwrap();
        assert_eq!(made.public_key(), again.public_key());
    }

    #[test]
    fn a_build_that_keeps_the_key_in_files_says_what_that_does_not_guard() {
        let note = keeping_notice();
        if istesso_pair::Keeping::FORM_OF_THIS_BUILD == istesso_pair::Form::Files {
            let note = note.unwrap();
            assert!(note.starts_with("note: "), "{note}");
            assert!(
                note.contains("every program of the same user can read and change"),
                "{note}"
            );
            // Where the files are is not said: the line goes into logs.
            assert!(!note.contains('/'), "{note}");
        } else {
            let note = note.unwrap();
            assert!(note.contains("login keychain"), "{note}");
            assert!(note.contains("may still be able to"), "{note}");
            assert!(!note.contains('/'), "{note}");
        }
    }

    #[test]
    fn ids_that_are_not_fingerprints_get_no_file() {
        let paths = Paths::at("/d");
        assert_eq!(paths.host_snapshot("../../etc/passwd"), None);
        assert_eq!(paths.host_snapshot("a/b"), None);
        assert_eq!(paths.host_snapshot(""), None);
        assert_eq!(paths.host_snapshot(&"a".repeat(65)), None);
    }
}
