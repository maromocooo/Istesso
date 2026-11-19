//! `istesso` — the command line entry point of Istesso.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, ToSocketAddrs, UdpSocket};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use unicode_width::UnicodeWidthStr;

use istesso_agent::{cache, survey, Agent, AgentOptions, HostView, Paths, SurveyOptions};
use istesso_measure::{Clock, Comparison, Jitter, ProbeSummary, Report};
use istesso_pair::{tcp, Identity, PairCode, Peer, PeerStore};
use istesso_proto::{
    AppEntry, AppInstance, Badge, HostApps, HostManifest, HostRef, HostState, InputEvent, Launch,
    NetKind, Role, UnifiedApp,
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
    /// Inspect devices connected to this machine
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
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
        /// For measuring: write to this file when input from a paired
        /// device was passed on. Times only, never what the input was; the
        /// file can be read by its owner only
        #[arg(long, value_name = "PATH")]
        input_timings: Option<PathBuf>,
        /// Hand input over on the socket this program was born with at this
        /// descriptor number, instead of at a socket with a name: for the
        /// program that shows an application and starts an agent of its own
        /// with one end of a pair of sockets. No other program can write to
        /// such a pair or take its place
        #[arg(long, value_name = "NUMBER")]
        input_fd: Option<i32>,
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
    /// Work with latency measurements (see docs/measurements/README.md)
    Measure {
        #[command(subcommand)]
        action: MeasureAction,
    },
    /// Pass keyboard and pointer input on to a paired device
    Input {
        #[command(subcommand)]
        action: InputAction,
    },
}

#[derive(Subcommand)]
enum DevicesAction {
    /// List local devices, their ports and how they can be used
    List {
        /// Print the inventory as JSON with machine-readable classifications
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum InputAction {
    /// Take input from a local socket and send it to a paired device
    ///
    /// The program that shows the device's application in a window writes
    /// what the person does there to the socket: one event per datagram,
    /// as JSON, for example {"kind":"key","code":30,"pressed":true}. This
    /// command sends the events over the control channel, to the device
    /// only, encrypted. The device has to run `istesso agent`, to show an
    /// application that takes input, and to have given this device the
    /// right to control it (it has by default after pairing).
    Relay {
        /// The paired device: its name or the beginning of its id
        #[arg(long)]
        to: String,
        /// The socket to listen on; one left by an earlier run is replaced.
        /// It has to be directly in Istesso's cache directory (`istesso id`
        /// shows it). Every program of the same user can write to a socket
        /// with a name, and can take its place: see --input-fd
        #[arg(long, value_name = "PATH", required_unless_present = "input_fd")]
        socket: Option<PathBuf>,
        /// Instead of --socket: take input on the socket this program was
        /// born with at this descriptor number. The program that shows the
        /// window makes a pair of sockets, starts this command with one end
        /// and writes to the other; no other program can write to the pair
        /// or take its place. The command ends when that program is gone
        #[arg(long, value_name = "NUMBER", conflicts_with = "socket")]
        input_fd: Option<i32>,
        /// Reach the device at this address only, instead of at whichever
        /// of its remembered addresses answers first: for when the way the
        /// input takes matters, as in a measurement over a cable
        #[arg(long, value_name = "ADDRESS[:PORT]")]
        address: Option<String>,
        /// Keep trying to reach the device for this many seconds instead of
        /// giving up at once. Started as an application for the first
        /// time, the program may not use the local network until the
        /// person has answered the system's question
        #[arg(long, value_name = "SECONDS", default_value_t = 0)]
        keep_trying: u64,
        /// For measuring: write to this file when input was taken from the
        /// socket and how long it was until it had been sent. Times only,
        /// never what the input was; the file can be read by its owner only
        #[arg(long, value_name = "PATH")]
        timings: Option<PathBuf>,
    },
}

/// How a measuring client's inputs reached the test application on the host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
enum SentAs {
    Network,
    Keys,
}

#[derive(Subcommand)]
enum MeasureAction {
    /// Compute the added latency from the logs of both sides
    ///
    /// Added latency is the round trip on the client's clock minus the
    /// application's own response time on the host's clock. Inputs are
    /// paired by their number, so the two clocks need not agree.
    Report {
        /// Log written on the host, for example by `istesso-flash --log`
        #[arg(long, value_name = "FILE")]
        host: PathBuf,
        /// Log written by the measuring client
        #[arg(long, value_name = "FILE")]
        client: PathBuf,
        /// Print the report as JSON
        #[arg(long)]
        json: bool,
    },
    /// Compare a relayed window with a local one watched on the same screen
    ///
    /// Takes the two logs of the screen probe (macos/screen-probe): for
    /// every input, when the local window and when the relayed window
    /// first showed it. With the logs of both copies of the test
    /// application it prints the added latency of each window and how
    /// much more the relayed one has, percentile by percentile, next to
    /// the pass line (ADR 0002).
    Compare {
        /// The probe's log for the window of the local test application
        #[arg(long, value_name = "FILE")]
        local: PathBuf,
        /// The probe's log for the window relayed from the host
        #[arg(long, value_name = "FILE")]
        relayed: PathBuf,
        /// Log written by the local test application (`istesso-flash --log`)
        #[arg(long, value_name = "FILE", requires = "relayed_app")]
        local_app: Option<PathBuf>,
        /// Log written by the test application on the host
        #[arg(long, value_name = "FILE", requires = "local_app")]
        relayed_app: Option<PathBuf>,
        /// What the local window's median added latency usually is on this
        /// machine, in milliseconds. A run that is 2 ms or more away from
        /// it is flagged to be measured again
        #[arg(long, value_name = "MS", requires = "local_app")]
        local_usual: Option<f64>,
        /// How the probe's inputs reached the test application on the host:
        /// `network`, straight to the application with their number, or
        /// `keys`, through the host's input path as key presses (the probe's
        /// `--relayed-keys`): control channel, compositor, keyboard
        #[arg(long, value_enum, default_value_t = SentAs::Network, requires = "relayed_app")]
        relayed_inputs: SentAs,
        /// Refresh rate of the display both windows were on, to count the
        /// difference in refreshes
        #[arg(long, value_name = "HZ")]
        refresh_hz: Option<f64>,
        /// Also show the run cut into this many parts of equal length, in
        /// the order the inputs were sent: both windows answer every input
        /// at the same moment, and the parts show whether either of them
        /// drifted while the run lasted
        #[arg(long, value_name = "COUNT", default_value_t = 1, value_parser = clap::value_parser!(u64).range(1..=100), requires = "relayed_app")]
        parts: u64,
        /// Print the comparison as JSON
        #[arg(long)]
        json: bool,
    },
    /// Write the frames the test application shows, as raw video
    ///
    /// The moving text of `istesso-flash`, frame after frame, as raw video
    /// on standard output: four bytes a pixel (blue, green, red, unused).
    /// Put an encoder and a decoder behind it and hand the result to
    /// `istesso measure quality`.
    Frames {
        #[command(flatten)]
        film: FilmArgs,
        /// How many frames to write
        #[arg(long, default_value_t = 255)]
        count: u64,
    },
    /// Say how much of the test application's frames came through a codec
    ///
    /// Reads raw video (four bytes a pixel: blue, green, red, unused) from
    /// standard input: what an encoder and a decoder made of the frames
    /// of `istesso measure frames`. Makes those frames again, with the
    /// same options, and compares frame by frame: the structural
    /// similarity (SSIM) of the brightness and of the two colour
    /// differences, and the signal-to-noise ratio of the brightness.
    Quality {
        #[command(flatten)]
        film: FilmArgs,
        /// Leave the first so many frames out of the figures: an encoder
        /// needs a moment to settle after it starts
        #[arg(long, value_name = "FRAMES", default_value_t = 0)]
        skip: usize,
        /// The frames are not the film's in their order but whatever a
        /// test application with `--stamp` showed: each is compared with
        /// the frame its stamp says it is. For frames caught on a screen
        #[arg(long)]
        stamped: bool,
        /// Print the result as JSON, with the figures of every frame
        #[arg(long)]
        json: bool,
    },
    /// Send synthetic inputs to the test application and time its answers
    ///
    /// Measures the path of an input without any video: to the host,
    /// through the application until its frame is committed, and back.
    /// The application's own response time is taken off, which leaves the
    /// transfer in both directions. Run `istesso-flash --listen` on the
    /// host first. The inputs are neither encrypted nor authenticated:
    /// use this on a trusted network only.
    Inputs {
        /// Where the test application listens: address, optionally with a port
        #[arg(long, value_name = "ADDRESS[:PORT]")]
        to: String,
        /// Number of inputs to send
        #[arg(long, default_value_t = istesso_measure::MIN_SAMPLES as u64)]
        count: u64,
        /// Shortest interval between two inputs, in milliseconds
        #[arg(long, default_value_t = 200)]
        min_ms: u64,
        /// Longest interval between two inputs, in milliseconds
        #[arg(long, default_value_t = 400)]
        max_ms: u64,
        /// Seed of the intervals. The same seed repeats the same run
        #[arg(long)]
        seed: Option<u64>,
        /// Milliseconds to wait for the last answers
        #[arg(long, default_value_t = 1000)]
        timeout_ms: u64,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
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
    /// Allow a paired device to do one more thing on this machine
    ///
    /// A newly paired device may look at this machine and control it.
    /// Installing applications, attaching USB devices and receiving this
    /// machine's sound each have to be allowed here, on the machine that
    /// grants them.
    Allow {
        /// Id or name of the paired device
        selector: String,
        #[arg(value_enum)]
        permission: PermissionArg,
    },
    /// Take one right away from a paired device
    Deny {
        /// Id or name of the paired device
        selector: String,
        #[arg(value_enum)]
        permission: PermissionArg,
    },
}

/// What a paired device can be allowed to do, as typed on the command line.
#[derive(Clone, Copy, ValueEnum)]
enum PermissionArg {
    /// See windows and listings
    View,
    /// Send keyboard and pointer input
    Control,
    /// Install or remove applications
    #[value(name = "install_apps", alias = "install-apps")]
    InstallApps,
    /// Attach USB devices
    Usb,
    /// Receive this machine's sound
    Audio,
}

impl From<PermissionArg> for istesso_pair::Permission {
    fn from(permission: PermissionArg) -> Self {
        match permission {
            PermissionArg::View => istesso_pair::Permission::View,
            PermissionArg::Control => istesso_pair::Permission::Control,
            PermissionArg::InstallApps => istesso_pair::Permission::InstallApps,
            PermissionArg::Usb => istesso_pair::Permission::Usb,
            PermissionArg::Audio => istesso_pair::Permission::Audio,
        }
    }
}

/// The rights of a paired device as one line of text, or `nothing`.
fn permission_names(permissions: &[istesso_pair::Permission]) -> String {
    if permissions.is_empty() {
        return "nothing".to_string();
    }
    permissions
        .iter()
        .map(|permission| permission.name())
        .collect::<Vec<_>>()
        .join(", ")
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
            if let Some(warning) = istesso_agent::paths::development_build() {
                eprintln!("{warning}");
            }
            // What this build does not guard; on standard error, so that the
            // JSON stays what paired devices are sent.
            if let Some(note) = istesso_agent::paths::keeping_notice() {
                eprintln!("{note}");
            }
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
        Command::Devices {
            action: DevicesAction::List { json },
        } => {
            let devices = istesso_devices::inventory::local()?;
            if json {
                print_json(&devices)
            } else {
                if devices.is_empty() {
                    println!("No local devices found.");
                }
                for device in devices {
                    println!("{device}");
                }
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
            input_timings,
            input_fd,
        } => {
            // Before the key is used, and before anything else could open a
            // descriptor.
            let input_end = input_fd.map(born_with_input).transpose()?;
            let mut options = AgentOptions {
                bind,
                port,
                discovery: !no_discovery,
                verbose,
                input_timings,
                ..AgentOptions::default()
            };
            #[cfg(unix)]
            {
                options.input_end = input_end.map(Arc::new);
            }
            #[cfg(not(unix))]
            let _ = input_end;
            run_agent(options)
        }
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
        Command::Measure { action } => run_measure(action),
        Command::Input { action } => match action {
            InputAction::Relay {
                to,
                socket,
                input_fd,
                address,
                keep_trying,
                timings,
            } => relay_input(
                &to,
                match (input_fd, socket) {
                    (Some(number), _) => InputFrom::BornWith(number),
                    (None, Some(socket)) => InputFrom::Socket(socket),
                    (None, None) => return Err("give --socket or --input-fd".to_string()),
                },
                address.as_deref(),
                Duration::from_secs(keep_trying),
                timings.as_deref(),
            ),
        },
    }
}

/// The comparison of a run, cut into parts of equal length in the order the
/// inputs were sent. Every input goes to both windows at the same moment, so
/// each part compares the two windows over the same stretch of time.
fn in_parts(
    local: &[istesso_measure::ClientSample],
    relayed: &[istesso_measure::ClientSample],
    local_app: Option<&[istesso_measure::HostSample]>,
    relayed_app: Option<&[istesso_measure::HostSample]>,
    parts: u64,
) -> String {
    use std::fmt::Write;

    let inputs = local
        .iter()
        .chain(relayed)
        .map(|sample| sample.id + 1)
        .max()
        .unwrap_or(0);
    let mut out = String::from(
        "\nThe run in parts, in the order the inputs were sent (added latency, milliseconds)\n",
    );
    let _ = writeln!(
        out,
        "{:<16}{:>8}{:>14}{:>16}{:>12}{:>9}",
        "inputs", "shown", "local median", "relayed median", "difference", "95%"
    );
    for part in 0..parts {
        let (from, to) = (inputs * part / parts, inputs * (part + 1) / parts);
        let within = |samples: &[istesso_measure::ClientSample]| -> Vec<_> {
            samples
                .iter()
                .filter(|sample| (from..to).contains(&sample.id))
                .cloned()
                .collect()
        };
        let comparison = Comparison::build(
            &within(local),
            &within(relayed),
            local_app,
            relayed_app,
            None,
        );
        let label = format!("{from} to {}", to.saturating_sub(1));
        match (
            &comparison.local_added,
            &comparison.relayed_added,
            &comparison.gap,
        ) {
            (Some(local), Some(relayed), Some(gap)) => {
                let _ = writeln!(
                    out,
                    "{label:<16}{:>8}{:>14.2}{:>16.2}{:>12.2}{:>9.2}",
                    comparison.paired,
                    local.median_ms,
                    relayed.median_ms,
                    gap.median_ms,
                    gap.p95_ms
                );
            }
            _ => {
                let _ = writeln!(out, "{label:<16}{:>8}  (too few inputs)", comparison.paired);
            }
        }
    }
    out
}

/// Where `istesso input relay` takes the input it passes on.
enum InputFrom {
    /// A socket with a name, which every program of the same user can write
    /// to and replace: the form the measuring tools use.
    Socket(PathBuf),
    /// The end of a pair without a name this program was born with, at this
    /// descriptor number.
    BornWith(i32),
}

/// The end of a pair of sockets this program was born with at descriptor
/// `number`: taken first thing, and refused unless nothing else can reach it.
#[cfg(unix)]
fn born_with_input(number: i32) -> Result<std::os::unix::net::UnixDatagram, String> {
    // SAFETY: called at the start, before this program has opened anything
    // it could own at that number; whoever started the program gave it.
    let end = unsafe { istesso_agent::endpoint::inherited(number) }
        .map_err(|e| format!("cannot take input there: {e}"))?;
    // The end is worth what the process is: no other program of the same
    // user gets to look into this one, where the system offers that.
    if !istesso_agent::endpoint::keep_others_out_of_this_process() {
        eprintln!(
            "note: on this system a program is kept from looking into this one only by how \
             it is signed"
        );
    }
    Ok(end)
}

#[cfg(not(unix))]
fn born_with_input(_number: i32) -> Result<(), String> {
    Err("taking input on an inherited socket needs a Unix system".to_string())
}

/// Takes input events from a local socket and sends them to a paired device.
#[cfg(unix)]
fn relay_input(
    to: &str,
    from: InputFrom,
    address: Option<&str>,
    keep_trying: Duration,
    timings: Option<&std::path::Path>,
) -> Result<(), String> {
    use std::io::ErrorKind;
    use std::os::unix::net::UnixDatagram;

    // Before anything else is done, and before the key is used: the relay
    // speaks for this device to another one. Either the end of a pair it was
    // born with, which nothing else can reach, or a name in a place of
    // Istesso's own, so that whoever starts it does not choose where it
    // listens.
    let (born_with, socket) = match from {
        InputFrom::BornWith(number) => (Some(born_with_input(number)?), None),
        InputFrom::Socket(asked) => (
            None,
            Some(
                Paths::from_env()
                    .local_socket(&asked)
                    .map_err(|e| e.to_string())?,
            ),
        ),
    };
    // The program that holds the other end of the pair. When it is gone the
    // system gives this one another parent, and nothing can arrive any more.
    let started_by = std::os::unix::process::parent_id();
    let passing = match timings {
        Some(path) => Some(
            istesso_agent::PassLog::create(path)
                .map_err(|e| format!("cannot write to {}: {e}", path.display()))?,
        ),
        None => None,
    };
    use istesso_agent::relay::{Happened, InputLine, LISTEN_EVERY};
    // Opens a conversation with the device: now, and again whenever it is cut. The
    // list of paired devices is read each time, so a pairing that was removed
    // meanwhile is not used.
    let mut connect: istesso_agent::relay::Connect = {
        let (to, address) = (to.to_string(), address.map(str::to_string));
        Box::new(move |within| {
            istesso_agent::connect_peer_or_say_why(
                &Paths::from_env(),
                &to,
                address.as_deref(),
                within,
            )
        })
    };
    let began = std::time::Instant::now();
    let mut said = false;
    let client = loop {
        match connect(Duration::from_secs(3)) {
            Ok(client) => break client,
            Err(why) if began.elapsed() < keep_trying => {
                if !said {
                    eprintln!("cannot reach the device yet ({why}); trying again");
                    said = true;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            Err(why) => return Err(why.to_string()),
        }
    };
    let mut line = InputLine::open(client, connect)
        .map_err(|(_, why)| format!("the device does not take input from here: {why}"))?;
    let listener: UnixDatagram = match (born_with, &socket) {
        (Some(end), _) => {
            println!("Input from the program that started this one goes to the paired device.");
            end
        }
        (None, Some(socket)) => {
            // A socket left by an earlier run is in the way of a new one.
            match std::fs::remove_file(socket) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => return Err(format!("cannot replace {}: {e}", socket.display())),
            }
            let listener = UnixDatagram::bind(socket)
                .map_err(|e| format!("cannot listen on {}: {e}", socket.display()))?;
            // For the owner only, whatever the starter's umask says. This
            // keeps other accounts out, not other programs of this one.
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))
                    .map_err(|e| format!("cannot close {} to others: {e}", socket.display()))?;
            }
            println!(
                "Input written to {} goes to the paired device.",
                socket.display()
            );
            eprintln!(
                "note: any program of this account can write to a socket with a name or take \
                 its place (a pair without a name: --input-fd)"
            );
            listener
        }
        (None, None) => return Err("there is nowhere to take input from".to_string()),
    };
    let failed = |e: std::io::Error| format!("cannot read the input: {e}");
    // How often the conversation was cut is always said, also when it is 0: on standard
    // output, in the log of times if there is one, and in a small file next to the socket
    // for the program that shows the window. A pair without a name has no such file: the
    // program that started this one reads standard output.
    let state = socket
        .as_ref()
        .map(|socket| std::path::PathBuf::from(format!("{}.state", socket.display())));
    let tell = |event: &str, cuts: u64, after: Option<Duration>| {
        let unix_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let after = after.map_or(String::new(), |after| {
            format!(",\"after_ms\":{:.1}", after.as_secs_f64() * 1e3)
        });
        println!(
            "{{\"istesso\":\"input\",\"event\":\"{event}\",\"reconnections\":{cuts},\"unix_ns\":{unix_ns}{after}}}"
        );
        if let Some(passing) = &passing {
            passing.reconnection(event, cuts);
        }
        // Written beside and moved over, so that nobody reads half of it. Whoever reads
        // it checks that the process named in it still runs.
        // Made anew each time (never through a link left there), readable by
        // the owner only.
        let Some(state) = &state else {
            return;
        };
        let beside = state.with_extension("state.new");
        let line = format!("{{\"pid\":{},\"input\":{cuts}}}\n", std::process::id());
        let _ = std::fs::remove_file(&beside);
        let written = {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&beside)
                .and_then(|mut file| file.write_all(line.as_bytes()))
        };
        if written.is_ok() {
            let _ = std::fs::rename(&beside, state);
        }
    };
    tell("open", 0, None);
    if !line.is_answered() {
        eprintln!(
            "note: the device is of a version that does not answer \"still here\"; a connection \
             that goes silent is only noticed when sending fails"
        );
    }

    let mut buffer = [0u8; 2048];
    let take = |events: &mut Vec<InputEvent>, datagram: &[u8]| {
        let Ok(event) = serde_json::from_slice::<InputEvent>(datagram) else {
            return;
        };
        // Of pointer positions in a row only the last one matters.
        match (events.last_mut(), &event) {
            (Some(last @ InputEvent::PointerAt { .. }), InputEvent::PointerAt { .. }) => {
                *last = event;
            }
            _ => events.push(event),
        }
    };
    loop {
        if socket.is_none() && std::os::unix::process::parent_id() != started_by {
            eprintln!("note: the program that started this one is gone; ending");
            return Ok(());
        }
        let mut events = Vec::new();
        // Wait for something to happen, but not for long: the conversation is looked
        // after in between (saying "still here", hearing the answers, opening it again
        // when it was cut).
        // While it is being opened again, more often: the attempt is due at once.
        let wait = if line.is_open() {
            LISTEN_EVERY
        } else {
            Duration::from_millis(5)
        };
        listener.set_read_timeout(Some(wait)).map_err(failed)?;
        match listener.recv(&mut buffer) {
            Ok(length) => take(&mut events, &buffer[..length]),
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(e) => return Err(failed(e)),
        }
        let in_hand = istesso_agent::PassLog::mark();
        // Whatever else has arrived meanwhile goes into the same message.
        listener.set_nonblocking(true).map_err(failed)?;
        while let Ok(length) = listener.recv(&mut buffer) {
            take(&mut events, &buffer[..length]);
        }
        listener.set_nonblocking(false).map_err(failed)?;
        let now = std::time::Instant::now();
        let mut happened = None;
        if !events.is_empty() {
            // Without a conversation the events are gone: they are not sent later.
            let passed = line.pass(&events, now);
            happened = passed.happened;
            if let (Some(passing), true) = (&passing, passed.sent) {
                passing.passed(in_hand, events.len());
            }
        }
        if happened.is_none() {
            happened = line.turn(now).map_err(|why| {
                format!("the device does not take input from here any more: {why}")
            })?;
        }
        match happened {
            Some(Happened::Cut) => {
                eprintln!("note: the conversation with the device was cut; opening it again");
                tell("cut", line.cuts(), None);
            }
            Some(Happened::Back { after }) => {
                eprintln!(
                    "note: the conversation with the device is back after {:.2} s",
                    after.as_secs_f64()
                );
                tell("back", line.cuts(), Some(after));
            }
            None => {}
        }
    }
}

#[cfg(not(unix))]
fn relay_input(
    _to: &str,
    _from: InputFrom,
    _address: Option<&str>,
    _keep_trying: Duration,
    _timings: Option<&std::path::Path>,
) -> Result<(), String> {
    Err("relaying input is not available on this system yet".into())
}

// ---------------------------------------------------------------------------
// Paths and identity
// ---------------------------------------------------------------------------

fn load_identity() -> Result<Identity, String> {
    Paths::from_env()
        .keeping()
        .identity()
        .map_err(|e| e.to_string())
}

fn open_peers() -> Result<PeerStore, String> {
    Paths::from_env()
        .keeping()
        .peers()
        .map_err(|e| e.to_string())
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
            // Opened before the wait so that a damaged list is reported before
            // anyone types the code. `add` reads the file again under its lock,
            // so changes made with `istesso peers` while this waits are kept.
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
            // As above: `add` reads the file again before it writes.
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
                println!(
                    "{:<24} {}  {}",
                    peer.name,
                    peer.id,
                    permission_names(&peer.permissions)
                );
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
        PeersAction::Allow {
            selector,
            permission,
        } => set_permission(&selector, permission.into(), true),
        PeersAction::Deny {
            selector,
            permission,
        } => set_permission(&selector, permission.into(), false),
    }
}

/// Grants or withdraws one right of a paired device and says what the device
/// may do from now on. Only this machine's record changes; nothing is sent to
/// the device.
fn set_permission(
    selector: &str,
    permission: istesso_pair::Permission,
    allowed: bool,
) -> Result<(), String> {
    let mut peers = open_peers()?;
    let peer = peers
        .set_permission(selector, permission, allowed)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no paired device matches '{selector}'"))?;
    println!(
        "{} ({}) may now: {}",
        peer.name,
        peer.id,
        permission_names(&peer.permissions)
    );
    println!("This applies from the device's next connection at the latest.");
    Ok(())
}

// ---------------------------------------------------------------------------
// agent / hosts
// ---------------------------------------------------------------------------

fn run_agent(options: AgentOptions) -> Result<(), String> {
    let paths = Paths::from_env();
    if let Some(note) = istesso_agent::paths::keeping_notice() {
        eprintln!("{note}");
    }
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

// ---------------------------------------------------------------------------
// measure
// ---------------------------------------------------------------------------

/// Which frames of the test application's moving text are meant.
#[derive(clap::Args)]
struct FilmArgs {
    /// Size of a frame in pixels
    #[arg(long, value_name = "WIDTHxHEIGHT", value_parser = parse_frame_size)]
    size: (usize, usize),
    /// Frames a second the text is drawn at
    #[arg(long, default_value_t = 85, value_parser = clap::value_parser!(u32).range(1..=1000))]
    fps: u32,
    /// How fast the text moves, in rows a second
    #[arg(long, default_value_t = 240)]
    scroll: u32,
    /// Size of a font pixel in pixels of the frame: 2 is what the test
    /// application shows on a display that is not scaled, 1 gives letters
    /// with strokes one pixel wide
    #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u32).range(1..=16))]
    scale: u32,
    /// The shade in the middle switches every so many frames, as it does
    /// when inputs arrive. 0 leaves it as it is
    #[arg(long, value_name = "FRAMES", default_value_t = 36)]
    toggle_every: u64,
    /// Which of the test application's moving pictures
    #[arg(long, value_enum, default_value_t = FilmPicture::Text)]
    picture: FilmPicture,
    /// The frames carry a stamp in their top left corner that says how far
    /// the picture has moved (`istesso-flash --stamp`)
    #[arg(long)]
    stamp: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum FilmPicture {
    /// Scrolling text: what measurements are judged with
    Text,
    /// Something like a photograph in motion, with grain: hard on an encoder
    Hard,
}

impl FilmArgs {
    fn film(&self) -> istesso_measure::quality::Film {
        let mut film = istesso_measure::quality::Film::new(
            self.size.0,
            self.size.1,
            self.scale as usize,
            self.fps,
            self.scroll,
            self.toggle_every,
        );
        if self.picture == FilmPicture::Hard {
            film = film.hard();
        }
        if self.stamp {
            film = film.stamped();
        }
        film
    }
}

fn parse_frame_size(text: &str) -> Result<(usize, usize), String> {
    let parsed = text.split_once(['x', 'X']).and_then(|(width, height)| {
        Some((width.parse::<usize>().ok()?, height.parse::<usize>().ok()?))
    });
    match parsed {
        // Eight pixels each way is the smallest picture the comparison can look at.
        Some((width, height)) if (8..=16384).contains(&width) && (8..=16384).contains(&height) => {
            Ok((width, height))
        }
        _ => Err(format!(
            "expected WIDTHxHEIGHT, for example 3440x1440, not {text:?}"
        )),
    }
}

/// Writes `count` frames of the film to standard output.
fn write_frames(film: &istesso_measure::quality::Film, count: u64) -> Result<(), String> {
    use std::io::{IsTerminal, Write};
    let stdout = std::io::stdout();
    if stdout.is_terminal() {
        return Err(
            "this writes raw video: send it into a pipe or a file, not to the terminal".into(),
        );
    }
    let mut out = stdout.lock();
    // A few frames are painted side by side and written in their order.
    let batch = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8) as u64;
    let mut number = 0;
    while number < count {
        let frames: Vec<Vec<u8>> = std::thread::scope(|scope| {
            let painters: Vec<_> = (number..count.min(number + batch))
                .map(|number| {
                    scope.spawn(move || {
                        let mut frame = vec![0u8; film.frame_len()];
                        film.paint(number, &mut frame);
                        frame
                    })
                })
                .collect();
            painters
                .into_iter()
                .map(|painter| painter.join().expect("painting a frame does not panic"))
                .collect()
        });
        number += frames.len() as u64;
        for frame in &frames {
            match out.write_all(frame) {
                Ok(()) => {}
                // Whoever reads has had enough.
                Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
                Err(e) => return Err(format!("cannot write a frame: {e}")),
            }
        }
    }
    out.flush()
        .map_err(|e| format!("cannot write a frame: {e}"))
}

/// Reads frames from standard input and compares each with the film's frame
/// of the same number. A few frames at a time are compared side by side.
fn read_quality(
    film: &istesso_measure::quality::Film,
    width: usize,
    height: usize,
    skip: usize,
    stamped: bool,
) -> Result<istesso_measure::quality::Quality, String> {
    use istesso_measure::quality::{FrameQuality, Quality};
    use std::io::Read;
    let mut input = std::io::stdin().lock();
    let batch = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(8);
    let mut per_frame = Vec::new();
    let mut unreadable = 0;
    let mut ended = false;
    while !ended {
        let mut received = Vec::with_capacity(batch);
        while received.len() < batch {
            let mut frame = vec![0u8; film.frame_len()];
            let mut filled = 0;
            while filled < frame.len() {
                match input.read(&mut frame[filled..]) {
                    Ok(0) => break,
                    Ok(n) => filled += n,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(format!("cannot read a frame: {e}")),
                }
            }
            if filled < frame.len() {
                if filled > 0 {
                    return Err(format!(
                        "the video ends in the middle of a frame ({filled} of {} bytes): is the size right?",
                        frame.len()
                    ));
                }
                ended = true;
                break;
            }
            // A stamped frame says which frame it is; the others come in the film's order.
            let moment = if stamped {
                match film.read(&frame) {
                    Some(moment) => Some(moment),
                    None => {
                        unreadable += 1;
                        continue;
                    }
                }
            } else {
                None
            };
            received.push((frame, moment));
        }
        let first = per_frame.len() as u64;
        let compared: Vec<FrameQuality> = std::thread::scope(|scope| {
            let workers: Vec<_> = received
                .iter()
                .enumerate()
                .map(|(i, (frame, moment))| {
                    scope.spawn(move || {
                        let mut sent = vec![0u8; film.frame_len()];
                        match moment {
                            Some((rows, shade)) => film.paint_at(*rows, *shade, &mut sent),
                            None => film.paint(first + i as u64, &mut sent),
                        }
                        FrameQuality::of(&sent, frame, width, height)
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("comparing a frame does not panic"))
                .collect()
        });
        per_frame.extend(compared);
    }
    if per_frame.is_empty() {
        return Err(if unreadable > 0 {
            format!(
                "none of the {unreadable} frames carries a stamp that can be read: are the size and the picture right?"
            )
        } else {
            "no video on standard input".into()
        });
    }
    let mut quality = Quality::of(per_frame, skip, width, height);
    quality.unreadable = unreadable;
    Ok(quality)
}

fn run_measure(action: MeasureAction) -> Result<(), String> {
    match action {
        MeasureAction::Report { host, client, json } => {
            let host = istesso_measure::host_samples(&read_measurement_log(&host)?);
            let client = istesso_measure::client_samples(&read_measurement_log(&client)?);
            let report = Report::build(&host, &client);
            if json {
                print_json(&report)
            } else {
                print!("{}", report.render());
                Ok(())
            }
        }
        MeasureAction::Compare {
            local,
            relayed,
            local_app,
            relayed_app,
            relayed_inputs,
            local_usual,
            refresh_hz,
            parts,
            json,
        } => {
            let local = istesso_measure::client_samples(&read_measurement_log(&local)?);
            let relayed = istesso_measure::client_samples(&read_measurement_log(&relayed)?);
            // An application's log: its samples, and whether it shifted its beat.
            let app_log = |path: Option<PathBuf>| -> Result<(Option<Vec<_>>, Option<u64>), String> {
                let Some(path) = path else {
                    return Ok((None, None));
                };
                let records = read_measurement_log(&path)?;
                Ok((
                    Some(istesso_measure::host_samples(&records)),
                    istesso_measure::beat_shift_seed(&records),
                ))
            };
            let (local_app, local_shift) = app_log(local_app)?;
            let (mut relayed_app, relayed_shift) = app_log(relayed_app)?;
            if let (SentAs::Keys, Some(app)) = (relayed_inputs, relayed_app.as_mut()) {
                *app = istesso_measure::sent_as_keys(app);
                if !json {
                    println!(
                        "The inputs reached the application on the host as key presses, through its input path: {} in its log.",
                        app.len()
                    );
                }
            }
            let comparison = Comparison::build(
                &local,
                &relayed,
                local_app.as_deref(),
                relayed_app.as_deref(),
                refresh_hz,
            )
            .with_beat_shifts(local_shift, relayed_shift)
            .with_local_usual(local_usual);
            if json {
                print_json(&comparison)
            } else {
                print!("{}", comparison.render());
                if parts > 1 {
                    print!(
                        "{}",
                        in_parts(
                            &local,
                            &relayed,
                            local_app.as_deref(),
                            relayed_app.as_deref(),
                            parts
                        )
                    );
                }
                Ok(())
            }
        }
        MeasureAction::Frames { film, count } => write_frames(&film.film(), count),
        MeasureAction::Quality {
            film,
            skip,
            stamped,
            json,
        } => {
            let pictures = if stamped {
                film.film().stamped()
            } else {
                film.film()
            };
            let quality = read_quality(&pictures, film.size.0, film.size.1, skip, stamped)?;
            if json {
                print_json(&quality)
            } else {
                print!("{}", quality.render());
                Ok(())
            }
        }
        MeasureAction::Inputs {
            to,
            count,
            min_ms,
            max_ms,
            seed,
            timeout_ms,
            json,
        } => {
            let target = resolve(&to, istesso_measure::DEFAULT_FLASH_PORT)?;
            let local: SocketAddr = if target.is_ipv4() {
                (Ipv4Addr::UNSPECIFIED, 0).into()
            } else {
                (Ipv6Addr::UNSPECIFIED, 0).into()
            };
            let socket =
                UdpSocket::bind(local).map_err(|e| format!("cannot open a socket: {e}"))?;
            socket
                .connect(target)
                .map_err(|e| format!("cannot reach {to}: {e}"))?;

            let seed = seed.unwrap_or_else(seed_from_time);
            let mut intervals = Jitter::new(
                seed,
                Duration::from_millis(min_ms),
                Duration::from_millis(max_ms),
            );
            let samples = istesso_measure::probe(
                &socket,
                &Clock::new(),
                count,
                &mut intervals,
                Duration::from_millis(timeout_ms),
            )
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::ConnectionRefused => format!(
                    "nothing takes synthetic inputs at {to}. Start `istesso-flash --listen` there first"
                ),
                _ => format!("the measurement stopped: {e}"),
            })?;
            let summary = ProbeSummary::of(&samples);
            if json {
                print_json(&summary)
            } else {
                print!("{}", render_probe(&summary, seed));
                Ok(())
            }
        }
    }
}

fn read_measurement_log(path: &std::path::Path) -> Result<Vec<istesso_measure::Record>, String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    istesso_measure::read_log(std::io::BufReader::new(file))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// A seed that differs from run to run. It is printed, so a run can be repeated.
fn seed_from_time() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Never names the target: results get pasted into measurement records,
/// which must not contain addresses.
fn render_probe(summary: &ProbeSummary, seed: u64) -> String {
    let mut out = format!(
        "Synthetic inputs: {} sent, {} answered, {} lost (seed {seed})\n\n",
        summary.sent,
        summary.answered,
        summary.sent - summary.answered
    );
    out.push_str(&istesso_measure::table(&[
        ("transfer, both ways", summary.transfer),
        ("round trip (client)", summary.round_trip),
        ("app response (host)", summary.app_response),
    ]));
    if summary.answered < istesso_measure::MIN_SAMPLES {
        out.push_str(&format!(
            "\nOnly {} answers: the measurement guide asks for {} or more.\n",
            summary.answered,
            istesso_measure::MIN_SAMPLES
        ));
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
    fn frame_sizes_are_width_by_height() {
        assert_eq!(parse_frame_size("3440x1440"), Ok((3440, 1440)));
        assert_eq!(parse_frame_size("8X8"), Ok((8, 8)));
        for wrong in [
            "", "3440", "3440x", "x1440", "7x8", "8x7", "0x0", "20000x8", "a×b",
        ] {
            assert!(parse_frame_size(wrong).is_err(), "{wrong:?} was accepted");
        }
    }

    #[test]
    fn frames_and_quality_take_the_same_description_of_the_film() {
        for command in ["frames", "quality"] {
            let parsed = Cli::try_parse_from([
                "istesso",
                "measure",
                command,
                "--size",
                "1720x720",
                "--scale",
                "1",
                "--toggle-every",
                "0",
            ]);
            assert!(parsed.is_ok(), "{command}");
            assert!(Cli::try_parse_from(["istesso", "measure", command]).is_err());
        }
    }

    #[test]
    fn compare_takes_the_applications_logs_as_a_pair() {
        let parse = |extra: &[&str]| {
            let mut args = vec![
                "istesso",
                "measure",
                "compare",
                "--local",
                "probe-local.jsonl",
                "--relayed",
                "probe-relayed.jsonl",
            ];
            args.extend_from_slice(extra);
            Cli::try_parse_from(args)
        };
        assert!(parse(&[]).is_ok());
        assert!(parse(&["--local-app", "local.jsonl", "--relayed-app", "host.jsonl"]).is_ok());
        assert!(parse(&[
            "--local-app",
            "local.jsonl",
            "--relayed-app",
            "host.jsonl",
            "--local-usual",
            "17.3"
        ])
        .is_ok());
        // What is usual for the local window means nothing without its application's log.
        assert!(parse(&["--local-usual", "17.3"]).is_err());
        // Added latency of one window alone says nothing about the pass line.
        assert!(parse(&["--local-app", "local.jsonl"]).is_err());
        assert!(parse(&["--relayed-app", "host.jsonl"]).is_err());
    }

    #[test]
    fn permissions_are_named_as_they_are_stored() {
        use istesso_pair::Permission;
        assert_eq!(permission_names(&[]), "nothing");
        assert_eq!(
            permission_names(&[Permission::View, Permission::Audio]),
            "view, audio"
        );
        // Every name the command line takes leads to the permission that is
        // stored under that name.
        for arg in PermissionArg::value_variants() {
            let name = arg.to_possible_value().unwrap().get_name().to_string();
            assert_eq!(Permission::from(*arg).name(), name);
        }
        // Both spellings of the one name that has two words are understood.
        for spelling in ["install_apps", "install-apps"] {
            let cli = Cli::try_parse_from(["istesso", "peers", "allow", "studio", spelling]);
            assert!(matches!(
                cli.map(|cli| cli.command),
                Ok(Command::Peers {
                    action: PeersAction::Allow {
                        permission: PermissionArg::InstallApps,
                        ..
                    }
                })
            ));
        }
        assert!(
            Cli::try_parse_from(["istesso", "peers", "allow", "studio", "everything"]).is_err()
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
    fn probe_results_name_no_address_and_flag_short_runs() {
        let samples: Vec<istesso_measure::ProbeSample> = (0..3)
            .map(|id| istesso_measure::ProbeSample {
                id,
                sent_ns: id * 300_000_000,
                answer: (id != 1).then_some(istesso_measure::Answer {
                    received_ns: id * 300_000_000 + 9_000_000,
                    input_ns: 5_000_000,
                    commit_ns: 6_000_000,
                }),
            })
            .collect();
        let text = render_probe(&ProbeSummary::of(&samples), 42);
        assert!(text.starts_with("Synthetic inputs: 3 sent, 2 answered, 1 lost (seed 42)"));
        // 9 ms round trip, 1 ms in the application.
        let row = |label: &str| -> Vec<String> {
            let line = text.lines().find(|l| l.starts_with(label)).unwrap();
            line[label.len()..]
                .split_whitespace()
                .map(str::to_string)
                .collect()
        };
        assert_eq!(
            row("transfer, both ways"),
            ["8.00", "8.00", "8.00", "8.00", "2"]
        );
        assert_eq!(
            row("round trip (client)"),
            ["9.00", "9.00", "9.00", "9.00", "2"]
        );
        assert_eq!(
            row("app response (host)"),
            ["1.00", "1.00", "1.00", "1.00", "2"]
        );
        assert!(text.contains("Only 2 answers"), "{text}");
        assert!(!text.contains("192.") && !text.contains("127."), "{text}");
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
