//! `istesso` — the command line entry point of Istesso.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, ToSocketAddrs};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use unicode_width::UnicodeWidthStr;

use istesso_agent::{cache, survey, Agent, AgentOptions, HostView, Paths, SurveyOptions};
use istesso_pair::{tcp, Identity, PairCode, Peer, PeerStore};
use istesso_proto::{
    AppEntry, AppInstance, Badge, HostApps, HostManifest, HostRef, HostState, Launch, NetKind,
    Role, UnifiedApp,
};

#[derive(Parser)]
#[command(
    name = "istesso",
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
    /// Answer paired devices (keep this running on every machine)
    Agent {
        /// TCP port to listen on
        #[arg(long, default_value_t = istesso_link::DEFAULT_PORT)]
        port: u16,
        /// Address to listen on (default: every network of this machine)
        #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::UNSPECIFIED))]
        bind: IpAddr,
        /// Do not announce this device on the local network or look for
        /// others there (for networks you do not trust)
        #[arg(long)]
        no_discovery: bool,
        /// Report every conversation with a paired device
        #[arg(short, long)]
        verbose: bool,
    },
    /// Show this machine and every paired device: state, role table, load
    Hosts {
        #[arg(long)]
        json: bool,
        /// Milliseconds to wait for a device to show up before giving up
        /// on it
        #[arg(long, default_value_t = 800)]
        timeout: u64,
    },
    /// Show this device's name and key fingerprint
    Id,
}

#[derive(Subcommand)]
enum AppsAction {
    /// List the applications installed on this machine
    List {
        /// Include every paired device: one entry per application, wherever
        /// it is installed
        #[arg(long)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Write this machine's application list to a file for `istesso apps merge`
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
        /// Files written by `istesso apps export`
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
    /// Tell this machine where a paired device can be reached
    ///
    /// Addresses are normally learnt while pairing and from the devices
    /// themselves. Use this when a device is reached another way, for example
    /// through a VPN or on a non-standard port.
    Address {
        /// Id or name of the paired device
        selector: String,
        /// Host name or IP address, optionally with a port: `studio.local`,
        /// `10.8.0.3:47801`
        address: String,
    },
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
            let manifest = istesso_doctor::detect();
            // Other commands and the agent reuse this inspection for a while.
            cache::remember_local_manifest(&Paths::from_env(), &manifest);
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
        Command::Agent {
            port,
            bind,
            no_discovery,
            verbose,
        } => run_agent(AgentOptions {
            bind,
            port,
            discovery: !no_discovery,
            verbose,
            ..AgentOptions::default()
        }),
        Command::Hosts { json, timeout } => run_hosts(json, timeout),
        Command::Id => {
            let paths = Paths::from_env();
            let identity = load_identity()?;
            println!("Name         {}", istesso_doctor::hostname());
            println!("Fingerprint  {}", identity.fingerprint());
            println!("Config       {}", paths.config.display());
            println!("Cache        {}", paths.cache.display());
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Paths and identity
// ---------------------------------------------------------------------------

fn load_identity() -> Result<Identity, String> {
    Identity::load_or_create(&Paths::from_env().identity()).map_err(|e| e.to_string())
}

fn open_peers() -> Result<PeerStore, String> {
    PeerStore::open(Paths::from_env().peers()).map_err(|e| e.to_string())
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
    if m.power.interfaces.is_empty() {
        row("Network", "no adapter found".to_string());
    }
    for interface in &m.power.interfaces {
        // Hardware addresses stay out of this view so that it can be pasted
        // into a bug report; `--json` has them.
        let kind = match interface.kind {
            NetKind::Ethernet => "Ethernet",
            NetKind::Wifi => "Wi-Fi",
            NetKind::Other => "other",
        };
        let state = if interface.addresses.is_empty() {
            "not connected".to_string()
        } else {
            interface.addresses.join(", ")
        };
        row("Network", format!("{} {kind}, {state}", interface.name));
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
        AppsAction::List { all: true, json } => {
            let options = SurveyOptions {
                local_apps: true,
                ..SurveyOptions::default()
            };
            let views = survey(&Paths::from_env(), &options)?;
            let hosts: Vec<HostApps> = views.iter().map(HostView::to_host_apps).collect();
            let merged = istesso_catalog::merge(&hosts);
            if json {
                return print_json(&merged);
            }
            if merged.is_empty() {
                println!("No applications found.");
            }
            for app in &merged {
                println!("{}", render_unified(app));
            }
            let silent: Vec<&str> = views
                .iter()
                .filter(|v| v.state != HostState::Awake)
                .map(|v| v.name.as_str())
                .collect();
            if !silent.is_empty() {
                eprintln!(
                    "\nDid not answer: {}. Their applications are listed from memory; \
                     `istesso hosts` shows why.",
                    silent.join(", ")
                );
            }
            Ok(())
        }
        AppsAction::List { all: false, json } => {
            let apps = istesso_catalog::scan_local();
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
                    name: istesso_doctor::hostname(),
                    state: state.into(),
                    local: false,
                    wake_possible,
                    load: 0.0,
                },
                apps: istesso_catalog::scan_local(),
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
            let merged = istesso_catalog::merge(&hosts);
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

/// Pads `text` with spaces to `width` terminal columns. Counting columns
/// instead of characters keeps tables straight when names contain Japanese.
fn pad(text: &str, width: usize) -> String {
    let shown = UnicodeWidthStr::width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(shown)))
}

fn render_app(app: &AppEntry) -> String {
    let target = match &app.launch {
        Launch::DesktopEntry { desktop_id, .. } => desktop_id.clone(),
        Launch::AppBundle { bundle_id, .. } => bundle_id.clone(),
    };
    format!(
        "{} {} {target}",
        pad(&app.name, 32),
        pad(app.version.as_deref().unwrap_or(""), 12)
    )
}

/// One line per application: where it would run, and where else it lives.
fn render_unified(app: &UnifiedApp) -> String {
    let label = |i: &AppInstance| -> String {
        let name = if i.local {
            "this device".to_string()
        } else {
            i.host_name.clone()
        };
        match i.host_state {
            HostState::Awake => name,
            HostState::Asleep => format!("{name} (asleep)"),
            HostState::Unknown => format!("{name} (no answer)"),
        }
    };
    let name = pad(&app.name, 32);
    match &app.preferred_host {
        Some(host_id) => {
            let (runs, others): (Vec<&AppInstance>, Vec<&AppInstance>) =
                app.instances.iter().partition(|i| &i.host_id == host_id);
            let runs = runs.first().map(|i| label(i)).unwrap_or_default();
            if others.is_empty() {
                format!("  {name} {runs}")
            } else {
                let others: Vec<String> = others.iter().map(|i| label(i)).collect();
                format!("  {name} {runs}  (also on {})", others.join(", "))
            }
        }
        None => {
            let hosts: Vec<String> = app.instances.iter().map(label).collect();
            let hint = match app.badge {
                Some(Badge::HostStopped {
                    wake_possible: true,
                }) => "  [host stopped, can be woken]",
                _ => "  [host stopped]",
            };
            format!("! {name} {}{hint}", hosts.join(", "))
        }
    }
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
            let name = name.unwrap_or_else(istesso_doctor::hostname);
            let listener = TcpListener::bind(("0.0.0.0", port))
                .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
            let code = PairCode::generate();

            println!("Pairing code: {code}");
            println!("On the other device run:");
            for line in join_commands(
                &istesso_doctor::lan_addresses(),
                port,
                &name,
                &code.to_string(),
            ) {
                println!("  {line}");
            }
            println!("Waiting for up to {timeout} seconds. The code works once.");

            let (hello, remote) = tcp::host(
                &listener,
                &code,
                &identity,
                &name,
                Duration::from_secs(timeout),
            )
            .map_err(|e| e.to_string())?;
            let peer = Peer::from_hello(&hello);
            println!("Paired with {} ({})", peer.name, peer.id);
            // Where the other device connected from is the first place to
            // look for it later.
            let hint = SocketAddr::new(remote.ip(), istesso_link::DEFAULT_PORT).to_string();
            let _ = cache::suggest_address(&Paths::from_env(), &peer.id, &peer.name, &hint);
            peers.add(peer).map_err(|e| e.to_string())?;
            println!("Run `istesso agent` on both devices, then try `istesso hosts`.");
            Ok(())
        }
        PairAction::Join {
            address,
            code,
            name,
        } => {
            let identity = load_identity()?;
            let mut peers = open_peers()?;
            let name = name.unwrap_or_else(istesso_doctor::hostname);
            let code = PairCode::parse(&code).map_err(|e| e.to_string())?;
            let addr = resolve(&address, tcp::DEFAULT_PORT)?;

            let hello = tcp::join(addr, &code, &identity, &name, Duration::from_secs(30))
                .map_err(|e| e.to_string())?;
            let peer = Peer::from_hello(&hello);
            println!("Paired with {} ({})", peer.name, peer.id);
            // Remember the name or address that was typed; a name such as
            // `studio.local` keeps working when the address changes.
            let (host, _) = split_port(&address);
            let hint = with_port(host, istesso_link::DEFAULT_PORT);
            let _ = cache::suggest_address(&Paths::from_env(), &peer.id, &peer.name, &hint);
            peers.add(peer).map_err(|e| e.to_string())?;
            println!("Run `istesso agent` on both devices, then try `istesso hosts`.");
            Ok(())
        }
    }
}

/// The commands to show while waiting for another device to join.
fn join_commands(addresses: &[String], port: u16, name: &str, code: &str) -> Vec<String> {
    let target = |address: &str| {
        if port == tcp::DEFAULT_PORT {
            address.to_string()
        } else {
            with_port(address, port)
        }
    };
    if addresses.is_empty() {
        return vec![format!(
            "istesso pair join {} {code}",
            target(&format!("<address of {name}>"))
        )];
    }
    addresses
        .iter()
        .enumerate()
        .map(|(index, address)| {
            let prefix = if index == 0 { "" } else { "or: " };
            format!("{prefix}istesso pair join {} {code}", target(address))
        })
        .collect()
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
    let (host, port) = split_port(address);
    with_port(host, port.unwrap_or(default_port))
}

/// Splits `host[:port]`. IPv6 addresses may be bare or in brackets.
fn split_port(address: &str) -> (&str, Option<u16>) {
    if let Some((host, port)) = address.rsplit_once(':') {
        // "[::1]" has colons but no port; "[::1]:47800" has one.
        let looks_like_port = !port.is_empty()
            && port.bytes().all(|b| b.is_ascii_digit())
            && (!host.contains(':') || host.ends_with(']'));
        if looks_like_port {
            if let Ok(port) = port.parse() {
                return (host, Some(port));
            }
        }
    }
    (address, None)
}

fn with_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        // Bare IPv6 address.
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
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
                println!("No paired devices yet. Start with `istesso pair host` on one device.");
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
                    cache::forget(&Paths::from_env(), &peer.id);
                    println!("Removed {} ({})", peer.name, peer.id);
                    Ok(())
                }
                None => Err(format!("no paired device matches '{selector}'")),
            }
        }
        PeersAction::Address { selector, address } => {
            let peers = open_peers()?;
            let peer = peers
                .find(&selector)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no paired device matches '{selector}'"))?;
            let address = with_default_port(address.trim(), istesso_link::DEFAULT_PORT);
            let paths = Paths::from_env();
            let mut snapshot =
                cache::load(&paths, &peer.id).unwrap_or_else(|| cache::HostSnapshot {
                    id: peer.id.clone(),
                    name: peer.name.clone(),
                    ..cache::HostSnapshot::default()
                });
            cache::promote(&mut snapshot.addresses, [address.clone()]);
            cache::store(&paths, &snapshot).map_err(|e| e.to_string())?;
            println!("{} will be tried at {address} first.", peer.name);
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// agent / hosts
// ---------------------------------------------------------------------------

fn run_agent(options: AgentOptions) -> Result<(), String> {
    let paths = Paths::from_env();
    // Everything that takes time happens before the port is opened: from
    // that moment on callers expect a prompt reaction.
    let name = istesso_doctor::hostname();
    let addresses = istesso_doctor::lan_addresses();
    let paired = open_peers()?.peers().len();

    let agent = Agent::bind(paths, &options, Arc::new(|line| eprintln!("{line}")))?;
    let addr = agent.local_addr().map_err(|e| e.to_string())?;
    println!("{name} ({})", agent.fingerprint());
    println!("Answering paired devices on {addr}. Paired devices: {paired}. Stop with Ctrl-C.");
    let reachable: Vec<String> = addresses
        .iter()
        .map(|address| with_port(address, addr.port()))
        .collect();
    if reachable.is_empty() {
        println!("This machine has no address on a local network right now.");
    } else {
        println!("Reachable at {}.", reachable.join(", "));
    }
    if paired == 0 {
        println!(
            "Nothing is paired yet: run `istesso pair host` here and `istesso pair join` on the other device."
        );
    }
    agent.run();
    Ok(())
}

fn run_hosts(json: bool, timeout_ms: u64) -> Result<(), String> {
    let options = SurveyOptions {
        timeout: Duration::from_millis(timeout_ms.clamp(50, 60_000)),
        ..SurveyOptions::default()
    };
    let views = survey(&Paths::from_env(), &options)?;
    if json {
        return print_json(&views);
    }
    print!("{}", render_hosts(&views, cache::now_unix()));
    if views.len() <= 1 {
        println!(
            "\nNo paired devices yet. Run `istesso pair host` here and `istesso pair join` on the other device."
        );
    } else if !istesso_agent::survey::local_agent_running(istesso_link::DEFAULT_PORT) {
        println!(
            "\nNothing answers on port {} of this device. Start `istesso agent` so that the other \
             devices can reach it.",
            istesso_link::DEFAULT_PORT
        );
    }
    Ok(())
}

/// How long ago something happened, in words.
fn ago(seconds: u64) -> String {
    let plural = |n: u64, unit: &str| {
        if n == 1 {
            format!("1 {unit} ago")
        } else {
            format!("{n} {unit}s ago")
        }
    };
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", seconds / 60),
        3600..=86_399 => plural(seconds / 3600, "hour"),
        _ => plural(seconds / 86_400, "day"),
    }
}

/// The table printed by `istesso hosts`. `now` is in seconds since the Unix epoch.
fn render_hosts(views: &[HostView], now: u64) -> String {
    struct Row {
        mark: &'static str,
        name: String,
        state: &'static str,
        os: String,
        load: String,
        memory: String,
        roles: String,
        note: Option<String>,
    }
    let dash = || "-".to_string();

    let rows: Vec<Row> = views
        .iter()
        .map(|view| {
            let answered = view.state == HostState::Awake;
            let state = match (view.local, view.state) {
                (true, _) => "this device",
                (false, HostState::Awake) => "awake",
                (false, HostState::Asleep) => "asleep",
                (false, HostState::Unknown) => "no answer",
            };
            let note = (!answered).then(|| {
                let mut parts = Vec::new();
                if let Some(problem) = &view.problem {
                    parts.push(problem.clone());
                }
                parts.push(match view.seen_at {
                    Some(at) => format!("last seen {}", ago(now.saturating_sub(at))),
                    None => "never reached from this device".to_string(),
                });
                if view.wake_possible() {
                    parts.push("can be woken over the network".to_string());
                }
                parts.join("; ")
            });
            Row {
                mark: if answered { " " } else { "!" },
                name: view.name.clone(),
                state,
                os: view
                    .manifest
                    .as_ref()
                    .map(|m| format!("{} {}", m.os.name, m.os.version).trim().to_string())
                    .unwrap_or_else(dash),
                load: view
                    .status
                    .as_ref()
                    .and_then(|s| s.load)
                    .map(|load| format!("{:.0}%", load * 100.0))
                    .unwrap_or_else(dash),
                memory: view
                    .status
                    .as_ref()
                    .and_then(|s| s.memory_available_bytes)
                    .map(format_bytes)
                    .unwrap_or_else(dash),
                roles: view
                    .manifest
                    .as_ref()
                    .map(|m| {
                        if m.roles.is_empty() {
                            "none".to_string()
                        } else {
                            m.roles
                                .iter()
                                .map(|r| role_label(*r))
                                .collect::<Vec<_>>()
                                .join(", ")
                        }
                    })
                    .unwrap_or_else(dash),
                note,
            }
        })
        .collect();

    let column = |title: &str, widths: &mut dyn Iterator<Item = usize>| {
        widths.max().unwrap_or(0).clamp(title.len(), 32)
    };
    let name_width = column("NAME", &mut rows.iter().map(|r| r.name.width()));
    let os_width = column("OS", &mut rows.iter().map(|r| r.os.width()));

    let mut out = format!(
        "  {} {} {} {:>5} {:>9}  ROLES\n",
        pad("NAME", name_width),
        pad("STATE", 11),
        pad("OS", os_width),
        "LOAD",
        "FREE MEM",
    );
    for row in &rows {
        out.push_str(&format!(
            "{} {} {} {} {:>5} {:>9}  {}\n",
            row.mark,
            pad(&row.name, name_width),
            pad(row.state, 11),
            pad(&row.os, os_width),
            row.load,
            row.memory,
            row.roles,
        ));
        if let Some(note) = &row.note {
            out.push_str(&format!("      {note}\n"));
        }
    }
    out
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
        let mut manifest = istesso_doctor::detect();
        manifest.warnings = vec!["something to know".to_string()];
        let text = render_manifest(&manifest);
        assert!(text.contains("Host"));
        assert!(text.contains("Roles"));
        assert!(text.contains("something to know"));
    }

    /// A made-up manifest, so that most tests do not have to inspect the
    /// machine they run on (slow on macOS).
    fn sample_manifest(name: &str) -> HostManifest {
        HostManifest {
            schema: istesso_proto::MANIFEST_SCHEMA,
            name: name.to_string(),
            os: istesso_proto::OsInfo {
                family: istesso_proto::OsFamily::Macos,
                name: "macOS".to_string(),
                version: "26.5".to_string(),
                kernel: "25.5.0".to_string(),
                arch: "aarch64".to_string(),
            },
            hardware: istesso_proto::Hardware {
                cpu_model: "Apple M4".to_string(),
                cpu_cores: 10,
                memory_bytes: 32 << 30,
                gpus: Vec::new(),
            },
            video: istesso_proto::VideoCaps {
                display_server: istesso_proto::DisplayServer::Quartz,
                encoders: vec![istesso_proto::EncoderApi::VideoToolbox],
            },
            audio: istesso_proto::AudioCaps {
                server: istesso_proto::AudioServer::CoreAudio,
                app_capture: true,
            },
            power: istesso_proto::PowerCaps::default(),
            tools: istesso_proto::Tools::default(),
            roles: vec![Role::MacApps, Role::HardwareEncode],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn manifest_rendering_shows_adapters_without_hardware_addresses() {
        let mut manifest = sample_manifest("studio");
        manifest.power.interfaces = vec![
            istesso_proto::NetInterface {
                name: "en0".to_string(),
                mac: "a0:ce:c8:11:22:33".to_string(),
                kind: NetKind::Ethernet,
                addresses: vec!["192.168.1.30".to_string()],
            },
            istesso_proto::NetInterface {
                name: "en1".to_string(),
                mac: "a0:ce:c8:44:55:66".to_string(),
                kind: NetKind::Wifi,
                addresses: Vec::new(),
            },
        ];
        let text = render_manifest(&manifest);
        assert!(
            text.contains("Network   en0 Ethernet, 192.168.1.30\n"),
            "{text}"
        );
        assert!(
            text.contains("Network   en1 Wi-Fi, not connected\n"),
            "{text}"
        );
        assert!(!text.contains("a0:ce:c8"), "{text}");

        manifest.power.interfaces.clear();
        assert!(render_manifest(&manifest).contains("Network   no adapter found\n"));
    }

    #[test]
    fn ports_are_split_from_hosts() {
        assert_eq!(split_port("studio.local"), ("studio.local", None));
        assert_eq!(
            split_port("studio.local:5000"),
            ("studio.local", Some(5000))
        );
        assert_eq!(
            split_port("192.168.1.20:47800"),
            ("192.168.1.20", Some(47800))
        );
        assert_eq!(split_port("::1"), ("::1", None));
        assert_eq!(split_port("[::1]:5000"), ("[::1]", Some(5000)));
        // Too large for a port: treated as part of the name and left to fail
        // at resolution.
        assert_eq!(split_port("host:99999"), ("host:99999", None));
        assert_eq!(with_port("::1", 47801), "[::1]:47801");
        assert_eq!(with_port("studio.local", 47801), "studio.local:47801");
    }

    #[test]
    fn pairing_shows_commands_that_can_be_typed_as_they_are() {
        let two = vec!["192.168.1.23".to_string(), "10.0.0.5".to_string()];
        assert_eq!(
            join_commands(&two, tcp::DEFAULT_PORT, "air", "ABCD-EFGH"),
            vec![
                "istesso pair join 192.168.1.23 ABCD-EFGH",
                "or: istesso pair join 10.0.0.5 ABCD-EFGH"
            ]
        );
        assert_eq!(
            join_commands(&two[..1], 5000, "air", "ABCD-EFGH"),
            vec!["istesso pair join 192.168.1.23:5000 ABCD-EFGH"]
        );
        assert_eq!(
            join_commands(&[], tcp::DEFAULT_PORT, "air", "ABCD-EFGH"),
            vec!["istesso pair join <address of air> ABCD-EFGH"]
        );
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(ago(0), "just now");
        assert_eq!(ago(59), "just now");
        assert_eq!(ago(60), "1 min ago");
        assert_eq!(ago(3599), "59 min ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }

    fn view(name: &str, local: bool, state: HostState) -> HostView {
        let mut manifest = sample_manifest(name);
        manifest.power.interfaces = vec![istesso_proto::NetInterface {
            name: "en0".to_string(),
            mac: "a0:ce:c8:11:22:33".to_string(),
            kind: NetKind::Ethernet,
            addresses: vec!["192.168.1.30".to_string()],
        }];
        let answered = state == HostState::Awake;
        HostView {
            id: format!("id-{name}"),
            name: name.to_string(),
            local,
            state,
            manifest: Some(manifest),
            status: answered.then_some(istesso_proto::HostStatus {
                load: Some(0.25),
                memory_available_bytes: Some(16 * 1024 * 1024 * 1024),
            }),
            apps: Vec::new(),
            seen_at: Some(1_000_000),
            address: None,
            problem: (!answered).then(|| "timed out waiting for the other device".to_string()),
        }
    }

    #[test]
    fn hosts_table_lines_up_and_explains_silent_devices() {
        let views = vec![
            view("はるのMacBook Air", true, HostState::Awake),
            view("ubuntu", false, HostState::Awake),
            view("studio", false, HostState::Unknown),
        ];
        let text = render_hosts(&views, 1_000_000 + 2 * 3600);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");

        // Every row starts its STATE column at the same screen column, even
        // though one name contains double-width characters.
        let column_of = |line: &str, needle: &str| {
            let index = line
                .find(needle)
                .unwrap_or_else(|| panic!("{needle} in {line}"));
            line[..index].width()
        };
        let expected = column_of(lines[0], "STATE");
        assert_eq!(column_of(lines[1], "this device"), expected);
        assert_eq!(column_of(lines[2], "awake"), expected);
        assert_eq!(column_of(lines[3], "no answer"), expected);

        assert!(lines[1].starts_with("  はるのMacBook Air"));
        assert!(lines[2].contains("25%"), "{text}");
        assert!(lines[2].contains("16.0 GiB"), "{text}");
        assert!(lines[2].contains("mac_apps, hardware_encode"), "{text}");
        assert!(lines[3].starts_with("! studio"), "{text}");
        assert_eq!(
            lines[4].trim(),
            "timed out waiting for the other device; last seen 2 hours ago; \
             can be woken over the network"
        );
    }

    #[test]
    fn a_host_on_wifi_is_not_said_to_be_wakeable() {
        // A desktop whose wired ports have no cable in them, as seen on a
        // real machine: it is on Wi-Fi only.
        let mut silent = view("ubuntu", false, HostState::Unknown);
        let manifest = silent.manifest.as_mut().unwrap();
        manifest.power.interfaces[0].addresses.clear();
        manifest.power.interfaces.push(istesso_proto::NetInterface {
            name: "wlp7s0".to_string(),
            mac: "a0:ce:c8:44:55:66".to_string(),
            kind: NetKind::Wifi,
            addresses: vec!["192.168.1.99".to_string()],
        });
        let text = render_hosts(&[silent], 1_000_000);
        assert!(text.contains("no answer"), "{text}");
        assert!(!text.contains("can be woken"), "{text}");
    }

    #[test]
    fn hosts_table_handles_devices_that_were_never_reached() {
        let mut never = view("studio", false, HostState::Unknown);
        never.manifest = None;
        never.seen_at = None;
        never.problem = Some("no address is known for this device yet".to_string());
        let text = render_hosts(&[never], 0);
        assert!(text.contains("no answer"), "{text}");
        assert!(text.contains("never reached from this device"), "{text}");
        assert!(!text.contains("can be woken"), "{text}");
    }

    fn installed(name: &str) -> AppEntry {
        AppEntry {
            id: name.to_lowercase(),
            name: name.to_string(),
            platform: istesso_proto::OsFamily::Linux,
            version: None,
            launch: Launch::DesktopEntry {
                desktop_id: format!("{}.desktop", name.to_lowercase()),
                exec: name.to_lowercase(),
            },
            icon: None,
            categories: Vec::new(),
        }
    }

    #[test]
    fn unified_apps_say_where_they_run() {
        let host = |name: &str, local: bool, state: HostState, apps: &[&str]| {
            let mut view = view(name, local, state);
            view.apps = apps.iter().map(|a| installed(a)).collect();
            view.to_host_apps()
        };
        let merged = istesso_catalog::merge(&[
            host("air", true, HostState::Awake, &["Firefox"]),
            host("ubuntu", false, HostState::Awake, &["Firefox", "Blender"]),
            host("studio", false, HostState::Unknown, &["Firefox", "Dia"]),
        ]);
        let lines: Vec<String> = merged.iter().map(render_unified).collect();
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(
            lines[0].trim_end(),
            format!("  {} ubuntu", pad("Blender", 32))
        );
        assert_eq!(
            lines[1],
            format!(
                "! {} studio (no answer)  [host stopped, can be woken]",
                pad("Dia", 32)
            )
        );
        assert_eq!(
            lines[2],
            format!(
                "  {} this device  (also on studio (no answer), ubuntu)",
                pad("Firefox", 32)
            )
        );
    }

    #[test]
    fn a_host_with_two_installs_of_an_app_is_named_once() {
        // Discord as a package and as a snap on the same machine.
        let mut ubuntu = view("ubuntu", false, HostState::Awake);
        let mut snap = installed("Discord");
        snap.id = "discord_discord.desktop".to_string();
        ubuntu.apps = vec![installed("Discord"), snap];
        let mut air = view("air", true, HostState::Awake);
        air.apps = vec![installed("Discord")];

        let merged = istesso_catalog::merge(&[air.to_host_apps(), ubuntu.to_host_apps()]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].instances.len(), 2);
        assert_eq!(
            render_unified(&merged[0]),
            format!("  {} this device  (also on ubuntu)", pad("Discord", 32))
        );
    }

    #[test]
    fn padding_counts_screen_columns() {
        assert_eq!(pad("abc", 5), "abc  ");
        assert_eq!(pad("日本", 5), "日本 ");
        assert_eq!(pad("toolong", 3), "toolong");
    }
}
