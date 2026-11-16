//! Where this device keeps its files.
//!
//! * the configuration directory holds what must not be lost: the device
//!   identity and the list of paired devices;
//! * the cache directory holds what can be rebuilt: the last known state of
//!   the other devices;
//! * the runtime directory holds what only means something while a program
//!   runs: sockets.

use std::path::PathBuf;

use istesso_proto::SessionId;

/// Directory name used below the platform's standard locations.
const APP_DIR: &str = "istesso";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub config: PathBuf,
    pub cache: PathBuf,
    /// Only the user can reach it, and it is emptied when they log out.
    pub runtime: PathBuf,
}

/// The settings that decide the locations.
#[derive(Debug, Clone, Default)]
pub struct Environment {
    /// `ISTESSO_CONFIG_DIR`
    pub config_override: Option<PathBuf>,
    /// `ISTESSO_CACHE_DIR`
    pub cache_override: Option<PathBuf>,
    /// `ISTESSO_RUNTIME_DIR`
    pub runtime_override: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    pub xdg_cache_home: Option<PathBuf>,
    pub xdg_runtime_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    pub macos: bool,
}

impl Environment {
    pub fn current() -> Self {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Environment {
            config_override: var("ISTESSO_CONFIG_DIR"),
            cache_override: var("ISTESSO_CACHE_DIR"),
            runtime_override: var("ISTESSO_RUNTIME_DIR"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
            xdg_cache_home: var("XDG_CACHE_HOME"),
            xdg_runtime_dir: var("XDG_RUNTIME_DIR"),
            home: var("HOME"),
            macos: cfg!(target_os = "macos"),
        }
    }
}

impl Paths {
    /// The locations for the current user.
    pub fn from_env() -> Self {
        Self::resolve(&Environment::current())
    }

    pub fn resolve(env: &Environment) -> Self {
        let home = env.home.clone().unwrap_or_else(|| PathBuf::from("."));
        let config = match &env.config_override {
            Some(dir) => dir.clone(),
            None if env.macos => home.join("Library/Application Support").join(APP_DIR),
            None => env
                .xdg_config_home
                .clone()
                .unwrap_or_else(|| home.join(".config"))
                .join(APP_DIR),
        };
        let cache = match (&env.cache_override, &env.config_override) {
            (Some(dir), _) => dir.clone(),
            // Someone who moved the configuration wants everything in one
            // place (tests, portable setups).
            (None, Some(config)) => config.join("cache"),
            (None, None) if env.macos => home.join("Library/Caches").join(APP_DIR),
            (None, None) => env
                .xdg_cache_home
                .clone()
                .unwrap_or_else(|| home.join(".cache"))
                .join(APP_DIR),
        };
        let runtime = match (
            &env.runtime_override,
            &env.config_override,
            &env.xdg_runtime_dir,
        ) {
            // A second agent of the same user needs sockets of its own.
            (Some(dir), _, _) => dir.clone(),
            // Everything in one place, as for the cache.
            (None, Some(config), _) => config.join("run"),
            (None, None, Some(dir)) => dir.join(APP_DIR),
            // No such directory on this system (macOS): next to the cache,
            // which is the user's alone as well.
            (None, None, None) => cache.join("run"),
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
        let runtime = config.join("run");
        Paths {
            config,
            cache,
            runtime,
        }
    }

    pub fn identity(&self) -> PathBuf {
        self.config.join("identity.key")
    }

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

    /// Where the runner of one streaming session takes the keyboard and
    /// pointer input of the device that is shown the session: a datagram
    /// socket the runner creates (ADR 0010). The agent only passes ids it
    /// made up itself; the name is built from nothing else.
    pub fn session_input_socket(&self, session: &SessionId) -> PathBuf {
        self.runtime.join(format!("input-{session}.sock"))
    }

    /// Where the agent answers the person at this machine: a socket that
    /// only this user can reach, because the directory it is in is theirs
    /// alone (ADR 0010).
    pub fn agent_socket(&self) -> PathBuf {
        self.runtime.join("agent.sock")
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
    fn linux_follows_the_xdg_directories() {
        let paths = Paths::resolve(&env());
        assert_eq!(paths.config, PathBuf::from("/home/u/.config/istesso"));
        assert_eq!(paths.cache, PathBuf::from("/home/u/.cache/istesso"));

        let paths = Paths::resolve(&Environment {
            xdg_config_home: Some(PathBuf::from("/xdg/config")),
            xdg_cache_home: Some(PathBuf::from("/xdg/cache")),
            ..env()
        });
        assert_eq!(paths.config, PathBuf::from("/xdg/config/istesso"));
        assert_eq!(paths.cache, PathBuf::from("/xdg/cache/istesso"));
    }

    #[test]
    fn sockets_go_where_only_the_user_can_reach() {
        // The runtime directory of the login session, where there is one.
        let paths = Paths::resolve(&Environment {
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..env()
        });
        assert_eq!(paths.runtime, PathBuf::from("/run/user/1000/istesso"));
        assert_eq!(
            paths.session_input_socket(&SessionId("5b0c9e3a7f21d684".into())),
            PathBuf::from("/run/user/1000/istesso/input-5b0c9e3a7f21d684.sock")
        );
        // Next to the cache where there is none.
        let paths = Paths::resolve(&Environment {
            macos: true,
            ..env()
        });
        assert_eq!(
            paths.runtime,
            PathBuf::from("/home/u/Library/Caches/istesso/run")
        );
        // Someone who moved the configuration gets everything in one place.
        let paths = Paths::resolve(&Environment {
            config_override: Some(PathBuf::from("/custom")),
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..env()
        });
        assert_eq!(paths.runtime, PathBuf::from("/custom/run"));
        assert_eq!(paths, Paths::at("/custom"));
        assert_eq!(
            paths.agent_socket(),
            PathBuf::from("/custom/run/agent.sock")
        );
        // A second agent of the same user is given a place of its own.
        let paths = Paths::resolve(&Environment {
            runtime_override: Some(PathBuf::from("/second/run")),
            config_override: Some(PathBuf::from("/custom")),
            xdg_runtime_dir: Some(PathBuf::from("/run/user/1000")),
            ..env()
        });
        assert_eq!(paths.runtime, PathBuf::from("/second/run"));
        assert_eq!(paths.config, PathBuf::from("/custom"));
    }

    #[test]
    fn macos_uses_the_library_folders() {
        let paths = Paths::resolve(&Environment {
            macos: true,
            // XDG variables are ignored on macOS.
            xdg_config_home: Some(PathBuf::from("/xdg/config")),
            ..env()
        });
        assert_eq!(
            paths.config,
            PathBuf::from("/home/u/Library/Application Support/istesso")
        );
        assert_eq!(paths.cache, PathBuf::from("/home/u/Library/Caches/istesso"));
    }

    #[test]
    fn overrides_win() {
        let paths = Paths::resolve(&Environment {
            config_override: Some(PathBuf::from("/custom")),
            macos: true,
            ..env()
        });
        assert_eq!(paths.config, PathBuf::from("/custom"));
        assert_eq!(paths.cache, PathBuf::from("/custom/cache"));
        assert_eq!(paths, Paths::at("/custom"));

        let paths = Paths::resolve(&Environment {
            config_override: Some(PathBuf::from("/custom")),
            cache_override: Some(PathBuf::from("/tmp/c")),
            ..env()
        });
        assert_eq!(paths.cache, PathBuf::from("/tmp/c"));
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
    fn ids_that_are_not_fingerprints_get_no_file() {
        let paths = Paths::at("/d");
        assert_eq!(paths.host_snapshot("../../etc/passwd"), None);
        assert_eq!(paths.host_snapshot("a/b"), None);
        assert_eq!(paths.host_snapshot(""), None);
        assert_eq!(paths.host_snapshot(&"a".repeat(65)), None);
    }
}
