//! `ud` — command line entry point for the unified desktop.

use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};

use ud_pair::{tcp, Identity, PairCode, Peer, PeerStore};
use ud_proto::{
    AppEntry, Badge, HostApps, HostManifest, HostRef, HostState, Launch, Role, UnifiedApp,
};

#[derive(Parser)]
#[command(
    name = "ud",
    version,
    about = "One desktop across all of your computers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show what this machine can do (the host's entry in the role table)
    Doctor {
        /// Print the manifest as JSON
        #[arg(long)]
        json: bool,
    },
    /// Work with the application catalogue
    Apps {
        #[command(subcommand)]
        action: AppsAction,
    },
    /// Pair this device with another one
    Pair {
        #[command(subcommand)]
        action: PairAction,
    },
    /// Manage the devices this machine trusts
    Peers {
        #[command(subcommand)]
        action: PeersAction,
    },
    /// Show this device's name and key fingerprint
    Id,
}

#[derive(Subcommand)]
enum AppsAction {
    /// List the applications installed on this machine
    List {
        #[arg(long)]
        json: bool,
    },
    /// Write this machine's application list to a file for `ud apps merge`
    Export {
        /// File to write (standard output when omitted)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// State to record for this host
        #[arg(long, value_enum, default_value_t = StateArg::Awake)]
        state: StateArg,
        /// Record that this host can be woken over the network
        #[arg(long)]
        wake_possible: bool,
    },
    /// Merge exported lists into one catalogue (one icon per application)
    Merge {
        /// Files written by `ud apps export`
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Id or name of the host the user is sitting at (default: this machine)
        #[arg(long)]
        local: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum StateArg {
    Awake,
    Asleep,
    Unknown,
}

impl From<StateArg> for HostState {
    fn from(state: StateArg) -> Self {
        match state {
            StateArg::Awake => HostState::Awake,
            StateArg::Asleep => HostState::Asleep,
            StateArg::Unknown => HostState::Unknown,
        }
    }
}

#[derive(Subcommand)]
enum PairAction {
    /// Show a one-time code and wait for another device to join
    Host {
        /// TCP port to listen on
        #[arg(long, default_value_t = tcp::DEFAULT_PORT)]
        port: u16,
        /// Seconds to wait before the code expires
        #[arg(long, default_value_t = 300)]
        timeout: u64,
        /// Name to present to the other device (default: the host name)
        #[arg(long)]
        name: Option<String>,
    },
    /// Join a device that is showing a code
    Join {
        /// Address of the waiting device, e.g. `studio.local` or `192.168.1.20:47800`
        address: String,
        /// The code shown on the other device
        code: String,
        /// Name to present to the other device (default: the host name)
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Subcommand)]
enum PeersAction {
    /// List trusted devices
    List {
        #[arg(long)]
        json: bool,
    },
    /// Stop trusting a device (by id or name)
    Remove { selector: String },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Doctor { json } => {
            let manifest = ud_doctor::detect();
            if json {
                print_json(&manifest)
            } else {
                print!("{}", render_manifest(&manifest));
                Ok(())
            }
        }
        Command::Apps { action } => run_apps(action),
        Command::Pair { action } => run_pair(action),
        Command::Peers { action } => run_peers(action),
        Command::Id => {
            let identity = load_identity()?;
            println!("Name         {}", ud_doctor::hostname());
            println!("Fingerprint  {}", identity.fingerprint());
            println!("Config       {}", config_dir().display());
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Paths and identity
// ---------------------------------------------------------------------------

/// Where this device keeps its identity and its list of trusted peers.
fn config_dir() -> PathBuf {
    config_dir_from(
        std::env::var_os("UD_CONFIG_DIR").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
        cfg!(target_os = "macos"),
    )
}

fn config_dir_from(
    explicit: Option<PathBuf>,
    xdg_config_home: Option<PathBuf>,
    home: Option<PathBuf>,
    macos: bool,
) -> PathBuf {
    if let Some(dir) = explicit.filter(|d| !d.as_os_str().is_empty()) {
        return dir;
    }
    let home = home.unwrap_or_else(|| PathBuf::from("."));
    if macos {
        return home.join("Library/Application Support/ud");
    }
    xdg_config_home
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".config"))
        .join("ud")
}

fn load_identity() -> Result<Identity, String> {
    Identity::load_or_create(&config_dir().join("identity.key")).map_err(|e| e.to_string())
}

fn open_peers() -> Result<PeerStore, String> {
    PeerStore::open(config_dir().join("peers.json")).map_err(|e| e.to_string())
}

fn print_json<T: serde::Serialize + ?Sized>(value: &T) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

fn role_label(role: Role) -> &'static str {
    match role {
        Role::Gpu => "gpu",
        Role::MacApps => "mac_apps",
        Role::XcodeBuild => "xcode_build",
        Role::AlwaysOnCandidate => "always_on_candidate",
        Role::HardwareEncode => "hardware_encode",
        Role::AppAudioCapture => "app_audio_capture",
    }
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.1} GiB", bytes as f64 / GIB)
}

fn render_manifest(m: &HostManifest) -> String {
    let mut out = String::new();
    let mut row = |label: &str, value: String| {
        out.push_str(&format!("{label:<10}{value}\n"));
    };

    row("Host", m.name.clone());
    row(
        "OS",
        format!(
            "{} {} ({}, kernel {})",
            m.os.name, m.os.version, m.os.arch, m.os.kernel
        ),
    );
    row(
        "CPU",
        format!(
            "{} ({} cores)",
            if m.hardware.cpu_model.is_empty() {
                "unknown"
            } else {
                &m.hardware.cpu_model
            },
            m.hardware.cpu_cores
        ),
    );
    row("Memory", format_bytes(m.hardware.memory_bytes));
    if m.hardware.gpus.is_empty() {
        row("GPU", "none found".to_string());
    }
    for gpu in &m.hardware.gpus {
        let mut details = Vec::new();
        if let Some(vram) = gpu.vram_bytes {
            details.push(format_bytes(vram));
        }
        if let Some(driver) = &gpu.driver {
            details.push(format!("driver {driver}"));
        }
        let suffix = if details.is_empty() {
            String::new()
        } else {
            format!(" ({})", details.join(", "))
        };
        row("GPU", format!("{}{suffix}", gpu.name));
    }
    row(
        "Display",
        format!("{:?}", m.video.display_server).to_lowercase(),
    );
    row(
        "Encoders",
        m.video
            .encoders
            .iter()
            .map(|e| format!("{e:?}").to_lowercase())
            .collect::<Vec<_>>()
            .join(", "),
    );
    row(
        "Audio",
        format!(
            "{} (app capture: {})",
            format!("{:?}", m.audio.server).to_lowercase(),
            if m.audio.app_capture { "yes" } else { "no" }
        ),
    );
    row(
        "Power",
        if m.power.portable {
            "battery powered".to_string()
        } else {
            "stationary".to_string()
        },
    );
    for interface in &m.power.interfaces {
        row("Network", format!("{} {}", interface.name, interface.mac));
    }
    let mut tools = Vec::new();
    if let Some(xcode) = &m.tools.xcode {
        tools.push(format!("Xcode {xcode}"));
    }
    if m.tools.cargo {
        tools.push("cargo".to_string());
    }
    if m.tools.docker {
        tools.push("docker".to_string());
    }
    if m.tools.gstreamer {
        tools.push("gstreamer".to_string());
    }
    row(
        "Tools",
        if tools.is_empty() {
            "none found".to_string()
        } else {
            tools.join(", ")
        },
    );
    row(
        "Roles",
        if m.roles.is_empty() {
            "none".to_string()
        } else {
            m.roles
                .iter()
                .map(|r| role_label(*r))
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    if !m.warnings.is_empty() {
        out.push_str("Warnings\n");
        for warning in &m.warnings {
            out.push_str(&format!("  - {warning}\n"));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// apps
// ---------------------------------------------------------------------------

fn run_apps(action: AppsAction) -> Result<(), String> {
    match action {
        AppsAction::List { json } => {
            let apps = ud_catalog::scan_local();
            if json {
                return print_json(&apps);
            }
            if apps.is_empty() {
                println!("No applications found on this machine.");
            }
            for app in &apps {
                println!("{}", render_app(app));
            }
            Ok(())
        }
        AppsAction::Export {
            output,
            state,
            wake_possible,
        } => {
            let identity = load_identity()?;
            let export = HostApps {
                host: HostRef {
                    id: identity.fingerprint(),
                    name: ud_doctor::hostname(),
                    state: state.into(),
                    local: false,
                    wake_possible,
                    load: 0.0,
                },
                apps: ud_catalog::scan_local(),
            };
            let text = serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?;
            match output {
                Some(path) => std::fs::write(&path, text)
                    .map_err(|e| format!("cannot write {}: {e}", path.display())),
                None => {
                    println!("{text}");
                    Ok(())
                }
            }
        }
        AppsAction::Merge { files, local, json } => {
            let local = match local {
                Some(selector) => selector,
                None => load_identity()?.fingerprint(),
            };
            let mut hosts = Vec::new();
            for path in &files {
                let text = std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                let mut host_apps: HostApps = serde_json::from_str(&text)
                    .map_err(|e| format!("{} is not an app export: {e}", path.display()))?;
                host_apps.host.local =
                    host_apps.host.id == local || host_apps.host.name.eq_ignore_ascii_case(&local);
                hosts.push(host_apps);
            }
            let merged = ud_catalog::merge(&hosts);
            if json {
                return print_json(&merged);
            }
            for app in &merged {
                println!("{}", render_unified(app));
            }
            Ok(())
        }
    }
}

fn render_app(app: &AppEntry) -> String {
    let target = match &app.launch {
        Launch::DesktopEntry { desktop_id, .. } => desktop_id.clone(),
        Launch::AppBundle { bundle_id, .. } => bundle_id.clone(),
    };
    match &app.version {
        Some(version) => format!("{:<32} {version:<12} {target}", app.name),
        None => format!("{:<32} {:<12} {target}", app.name, ""),
    }
}

fn render_unified(app: &UnifiedApp) -> String {
    let mark = match app.badge {
        Some(Badge::HostStopped { .. }) => "!",
        None => " ",
    };
    let hosts = app
        .instances
        .iter()
        .map(|i| {
            let mut notes = Vec::new();
            if i.local {
                notes.push("here");
            }
            match i.host_state {
                HostState::Awake => {}
                HostState::Asleep => notes.push("stopped"),
                HostState::Unknown => notes.push("unreachable"),
            }
            if app.preferred_host.as_deref() == Some(i.host_id.as_str()) {
                notes.push("runs here");
            }
            if notes.is_empty() {
                i.host_name.clone()
            } else {
                format!("{} ({})", i.host_name, notes.join(", "))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let hint = match app.badge {
        Some(Badge::HostStopped {
            wake_possible: true,
        }) => "  [host stopped, can be woken]",
        Some(Badge::HostStopped {
            wake_possible: false,
        }) => "  [host stopped]",
        None => "",
    };
    format!("{mark} {:<32} {hosts}{hint}", app.name)
}

// ---------------------------------------------------------------------------
// pair / peers
// ---------------------------------------------------------------------------

fn run_pair(action: PairAction) -> Result<(), String> {
    match action {
        PairAction::Host {
            port,
            timeout,
            name,
        } => {
            let identity = load_identity()?;
            let mut peers = open_peers()?;
            let name = name.unwrap_or_else(ud_doctor::hostname);
            let listener = TcpListener::bind(("0.0.0.0", port))
                .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
            let code = PairCode::generate();

            println!("Pairing code: {code}");
            println!("On the other device run:");
            println!("  ud pair join <address of {name}>:{port} {code}");
            println!("Waiting for up to {timeout} seconds. The code works once.");

            let hello = tcp::host(
                &listener,
                &code,
                &identity,
                &name,
                Duration::from_secs(timeout),
            )
            .map_err(|e| e.to_string())?;
            let peer = Peer::from_hello(&hello);
            println!("Paired with {} ({})", peer.name, peer.id);
            peers.add(peer).map_err(|e| e.to_string())
        }
        PairAction::Join {
            address,
            code,
            name,
        } => {
            let identity = load_identity()?;
            let mut peers = open_peers()?;
            let name = name.unwrap_or_else(ud_doctor::hostname);
            let code = PairCode::parse(&code).map_err(|e| e.to_string())?;
            let addr = resolve(&address, tcp::DEFAULT_PORT)?;

            let hello = tcp::join(addr, &code, &identity, &name, Duration::from_secs(30))
                .map_err(|e| e.to_string())?;
            let peer = Peer::from_hello(&hello);
            println!("Paired with {} ({})", peer.name, peer.id);
            peers.add(peer).map_err(|e| e.to_string())
        }
    }
}

/// Turns `host`, `host:port` or an IP address into a socket address.
fn resolve(address: &str, default_port: u16) -> Result<SocketAddr, String> {
    let with_port = with_default_port(address, default_port);
    with_port
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {address}: {e}"))?
        .next()
        .ok_or_else(|| format!("{address} did not resolve to an address"))
}

fn with_default_port(address: &str, default_port: u16) -> String {
    let has_port = match address.rsplit_once(':') {
        // "[::1]" has colons but no port; "[::1]:47800" has one.
        Some((host, port)) => {
            !port.is_empty()
                && port.chars().all(|c| c.is_ascii_digit())
                && (!host.contains(':') || host.ends_with(']'))
        }
        None => false,
    };
    if has_port {
        address.to_string()
    } else if address.contains(':') && !address.starts_with('[') {
        // Bare IPv6 address.
        format!("[{address}]:{default_port}")
    } else {
        format!("{address}:{default_port}")
    }
}

fn run_peers(action: PeersAction) -> Result<(), String> {
    match action {
        PeersAction::List { json } => {
            let peers = open_peers()?;
            if json {
                return print_json(&peers.peers());
            }
            if peers.peers().is_empty() {
                println!("No paired devices yet. Start with `ud pair host` on one device.");
            }
            for peer in peers.peers() {
                println!("{:<24} {}", peer.name, peer.id);
            }
            Ok(())
        }
        PeersAction::Remove { selector } => {
            let mut peers = open_peers()?;
            match peers.remove(&selector).map_err(|e| e.to_string())? {
                Some(peer) => {
                    println!("Removed {} ({})", peer.name, peer.id);
                    Ok(())
                }
                None => Err(format!("no paired device matches '{selector}'")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn config_dir_follows_platform_conventions() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            config_dir_from(Some(PathBuf::from("/custom")), None, home.clone(), false),
            PathBuf::from("/custom")
        );
        assert_eq!(
            config_dir_from(None, None, home.clone(), false),
            PathBuf::from("/home/u/.config/ud")
        );
        assert_eq!(
            config_dir_from(None, Some(PathBuf::from("/xdg")), home.clone(), false),
            PathBuf::from("/xdg/ud")
        );
        assert_eq!(
            config_dir_from(None, Some(PathBuf::from("/xdg")), home, true),
            PathBuf::from("/home/u/Library/Application Support/ud")
        );
    }

    #[test]
    fn default_port_is_added_only_when_missing() {
        assert_eq!(
            with_default_port("studio.local", 47800),
            "studio.local:47800"
        );
        assert_eq!(
            with_default_port("studio.local:5000", 47800),
            "studio.local:5000"
        );
        assert_eq!(
            with_default_port("192.168.1.20", 47800),
            "192.168.1.20:47800"
        );
        assert_eq!(with_default_port("::1", 47800), "[::1]:47800");
        assert_eq!(with_default_port("[::1]", 47800), "[::1]:47800");
        assert_eq!(with_default_port("[::1]:5000", 47800), "[::1]:5000");
    }

    #[test]
    fn formats_memory_sizes() {
        assert_eq!(format_bytes(24 * 1024 * 1024 * 1024), "24.0 GiB");
        assert_eq!(format_bytes(0), "0.0 GiB");
    }

    #[test]
    fn manifest_rendering_mentions_roles_and_warnings() {
        let mut manifest = ud_doctor::detect();
        manifest.warnings = vec!["something to know".to_string()];
        let text = render_manifest(&manifest);
        assert!(text.contains("Host"));
        assert!(text.contains("Roles"));
        assert!(text.contains("something to know"));
    }
}
