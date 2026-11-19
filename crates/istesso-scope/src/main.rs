//! `istesso-scope`: a prototype launcher that runs a command under an agent scope level
//! (ADR 0026, docs/research/agent-scope-proto.md). Linux only for now (Landlock); on other
//! systems only `plan` works.
//!
//! The boundary is meant to be a dedicated coding user AND these rules. Run as the owner's own
//! user, the same-user ways out (the Docker socket, the user's service manager) stay open; the
//! launcher says so when it starts.

mod ask;
mod policy;

#[cfg(target_os = "linux")]
mod linux;

use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use policy::{Level, Machine, Plan, Request};

#[derive(Parser)]
#[command(
    name = "istesso-scope",
    version,
    about = "Run a command under an agent scope level (prototype)"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show what this kernel can enforce and which same-user ways out are open.
    Check {
        /// Also send one UDP datagram to a documentation address (it reaches no one) to see
        /// whether a per-user network rule stops this user's traffic.
        #[arg(long)]
        probe_network: bool,
    },
    /// Print the rules a level would apply, without running anything.
    Plan(ScopeArgs),
    /// Run a command under a scope level.
    Run {
        #[command(flatten)]
        scope: ScopeArgs,
        #[command(flatten)]
        hold: HoldArgs,
        /// Let levels 1 to 3 listen on every address. Without it, a bind or listen on an address
        /// that is not loopback is moved to loopback (seccomp user notification; x86-64 only).
        #[arg(long)]
        listen_anywhere: bool,
        /// The command and its arguments.
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
    /// The answering device's end (a test form in a terminal, not the owner's dialog): connect
    /// to a launcher started with --ask-listen and answer its questions.
    Answer {
        /// Where the launcher listens (address:port).
        #[arg(long)]
        connect: std::net::SocketAddr,
        /// Folder with this device's test identity and the launcher's trusted key.
        #[arg(long)]
        keys: PathBuf,
        /// Answer from this list, in order, instead of asking at the prompt: once, widen-run,
        /// widen-keep, refuse (comma-separated). When it runs out: refuse.
        #[arg(long, value_delimiter = ',')]
        script: Option<Vec<String>>,
        /// Keep trying to connect for this many seconds.
        #[arg(long, default_value_t = 30)]
        retry: u64,
    },
    /// Create a test identity for the link in a folder of its own and write its public key to
    /// <dir>/public-key (never the device's real key).
    LinkKeys {
        #[arg(long)]
        dir: PathBuf,
    },
    /// Trust the device whose public key file (from link-keys) is given, in a test folder.
    LinkTrust {
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long)]
        public_key_file: PathBuf,
    },
    /// One operation with one line of result (for tests).
    #[command(hide = true)]
    Probe {
        #[command(subcommand)]
        what: ProbeCmd,
    },
}

#[derive(Args, Clone)]
struct ScopeArgs {
    /// 1 project only, 2 + development devices, 3 + relay destinations, 4 same as the user.
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=4))]
    level: u8,
    /// The project folder (read and write).
    #[arg(long)]
    project: PathBuf,
    /// The scope's own folder: its home, temporary files and tool caches (read and write).
    #[arg(long)]
    state: PathBuf,
    /// TCP port of the local relay. Levels 1 to 3 may connect only to it (and --connect-port).
    #[arg(long)]
    relay_port: Option<u16>,
    /// Further TCP ports to allow connecting to, e.g. the project's own development server
    /// (`3000` or `3000-3009`).
    #[arg(long = "connect-port", value_name = "PORT[-PORT]")]
    connect_port: Vec<String>,
    /// Extra path to allow reading.
    #[arg(long = "read", value_name = "PATH")]
    read: Vec<PathBuf>,
    /// Extra path to allow writing.
    #[arg(long = "write", value_name = "PATH")]
    write: Vec<PathBuf>,
    /// Extra device file or folder to allow (read, write, ioctl).
    #[arg(long = "device", value_name = "PATH")]
    device: Vec<PathBuf>,
    /// Do not print what is enforced and the warnings (errors are still printed).
    #[arg(long, short)]
    quiet: bool,
}

/// Holding operations at the edge and asking the owner.
#[derive(Args, Clone)]
struct HoldArgs {
    /// Hold opens of files outside the rules and ask before the kernel answers
    /// (seccomp user notification; x86-64 only).
    #[arg(long)]
    hold_files: bool,
    /// Hold TCP connections to ports outside the rules and ask.
    #[arg(long)]
    hold_connect: bool,
    /// Stand-in for the owner's dialog: a shell command run for each question, with
    /// ISTESSO_ASK_KIND, ISTESSO_ASK_PATH, ISTESSO_ASK_ACCESS or ISTESSO_ASK_ADDRESS set.
    /// Exit 0 = allow once, 10 = widen for this run, 11 = widen and keep, else refuse.
    #[arg(long, conflicts_with = "ask_listen")]
    ask_cmd: Option<String>,
    /// Ask the owner's device over the Istesso link: listen here (address:port) for it before
    /// the command starts. Use loopback or a wired address, never every address.
    #[arg(long, requires = "link_keys")]
    ask_listen: Option<std::net::SocketAddr>,
    /// Folder with the launcher's test identity and the device's trusted key (link-keys,
    /// link-trust). The confined command must not be able to read it.
    #[arg(long)]
    link_keys: Option<PathBuf>,
    /// The one device (name or id in the trusted list) that may answer for this run: the device
    /// that asked for it. Without it, the first trusted device that connects.
    #[arg(long, requires = "ask_listen")]
    ask_device: Option<String>,
    /// Seconds to wait for the answering device before refusing to start.
    #[arg(long, default_value_t = 120)]
    ask_wait: u64,
    /// Seconds a question waits for its answer; then it is refused.
    #[arg(long, default_value_t = 60)]
    ask_timeout: u64,
    /// The project's grants file: "widen and keep" writes to it, and every start reads it. The
    /// confined command must not be able to write it.
    #[arg(long)]
    grants: Option<PathBuf>,
}

#[derive(Subcommand)]
enum ProbeCmd {
    Read {
        path: PathBuf,
    },
    Write {
        path: PathBuf,
    },
    Connect {
        addr: String,
    },
    Bind {
        port: u16,
    },
    Udp {
        addr: String,
    },
    Signal {
        pid: i32,
    },
    Environ {
        pid: i32,
    },
    Unix {
        path: PathBuf,
    },
    Abstract {
        name: String,
    },
    ServeTcp {
        port: u16,
        seconds: u64,
        reply: String,
    },
    ServeAbstract {
        name: String,
        seconds: u64,
        reply: String,
    },
    /// Asks for an io_uring instance.
    IoUring,
    /// Connects with MPTCP (IPv4 address:port).
    Mptcp {
        addr: String,
    },
    /// Binds a TCP socket to "any address" (IPv6 with --v6) and prints where it was bound.
    /// It never listens, so nothing can connect to it.
    BindAny {
        port: u16,
        #[arg(long)]
        v6: bool,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Check { probe_network } => check(probe_network),
        Cmd::Plan(args) => match make_plan(&args, false) {
            Ok((plan, machine)) => {
                print_plan(&plan, &machine);
                0
            }
            Err(e) => {
                eprintln!("istesso-scope: {e}");
                125
            }
        },
        Cmd::Run {
            scope,
            hold,
            listen_anywhere,
            command,
        } => run(&scope, hold, listen_anywhere, &command),
        Cmd::Answer {
            connect,
            keys,
            script,
            retry,
        } => answer(connect, &keys, script, retry),
        Cmd::LinkKeys { dir } => match ask::make_keys(&dir) {
            Ok(id) => {
                println!("identity: {id}");
                println!("public-key-file: {}", dir.join("public-key").display());
                0
            }
            Err(e) => {
                eprintln!("istesso-scope: {e}");
                1
            }
        },
        Cmd::LinkTrust {
            dir,
            name,
            public_key_file,
        } => match ask::trust(&dir, &name, &public_key_file) {
            Ok(id) => {
                println!("trusted: {name} ({id})");
                0
            }
            Err(e) => {
                eprintln!("istesso-scope: {e}");
                1
            }
        },
        Cmd::Probe { what } => probe(what),
    };
    std::process::exit(code);
}

fn answer(
    connect: std::net::SocketAddr,
    keys: &Path,
    script: Option<Vec<String>>,
    retry: u64,
) -> i32 {
    let answers = match script {
        Some(list) => {
            let mut choices = Vec::new();
            for item in list {
                match ask::Choice::parse(&item) {
                    Some(c) => choices.push(c),
                    None => {
                        eprintln!("istesso-scope: not an answer: {item:?}");
                        return 2;
                    }
                }
            }
            ask::Answers::Script(choices)
        }
        None => ask::Answers::Prompt(std::io::stdin().lock()),
    };
    let mut out = std::io::stdout();
    match ask::answer(
        connect,
        keys,
        std::time::Duration::from_secs(retry),
        answers,
        &mut out,
    ) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("istesso-scope: {e}");
            125
        }
    }
}

fn absolute(p: &Path) -> Result<PathBuf, String> {
    std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}

fn discover_machine() -> Machine {
    let path_dirs = std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p)
                .filter(|d| d.is_absolute() && d.is_dir())
                .collect()
        })
        .unwrap_or_default();
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".rustup")))
        .filter(|p| p.is_dir());
    #[cfg(target_os = "linux")]
    let nvidia_devices = linux::nvidia_devices();
    #[cfg(not(target_os = "linux"))]
    let nvidia_devices = Vec::new();
    Machine {
        path_dirs,
        rustup_home,
        nvidia_devices,
    }
}

fn make_plan(args: &ScopeArgs, create_state: bool) -> Result<(Plan, Machine), String> {
    let level = Level::from_number(args.level).ok_or("level must be 1 to 4")?;
    let project = absolute(&args.project)?;
    if create_state {
        for sub in ["", "home", "tmp"] {
            let dir = args.state.join(sub);
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
    }
    let state = if create_state || args.state.exists() {
        absolute(&args.state)?
    } else {
        args.state.clone()
    };
    if project == Path::new("/") || state == Path::new("/") {
        return Err("the project and state folders cannot be the root folder".into());
    }
    let mut connect_ports = Vec::new();
    for text in &args.connect_port {
        connect_ports.extend(policy::parse_ports(text)?);
    }
    let canon =
        |v: &[PathBuf]| -> Result<Vec<PathBuf>, String> { v.iter().map(|p| absolute(p)).collect() };
    let req = Request {
        level,
        project,
        state,
        relay_port: args.relay_port,
        connect_ports,
        extra_read: canon(&args.read)?,
        extra_write: canon(&args.write)?,
        extra_devices: canon(&args.device)?,
    };
    let machine = discover_machine();
    Ok((Plan::new(&req, &machine), machine))
}

fn print_plan(plan: &Plan, machine: &Machine) {
    println!("level: {} ({})", plan.level.number(), plan.level.describe());
    if !plan.confined() {
        println!("confinement: none");
        return;
    }
    for r in &plan.paths {
        println!("path: {:<6} {}", r.access.name(), r.path.display());
    }
    let ports: Vec<String> = plan.connect_ports.iter().map(|p| p.to_string()).collect();
    println!(
        "tcp-connect: {}",
        if ports.is_empty() {
            "none".into()
        } else {
            ports.join(",")
        }
    );
    println!("tcp-bind: not limited");
    let outside: Vec<(String, String)> = std::env::vars().collect();
    let names: Vec<String> = plan
        .environment(&outside, machine)
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    println!("environment: {}", names.join(" "));
}

#[cfg(not(target_os = "linux"))]
fn check(_: bool) -> i32 {
    println!("platform: not Linux; this prototype confines on Linux only");
    1
}

#[cfg(target_os = "linux")]
fn check(probe_network: bool) -> i32 {
    let abi = linux::landlock_abi();
    let yes = |b: bool| if b { "yes" } else { "no" };
    println!("landlock-abi: {abi}");
    println!("landlock-files: {}", yes(abi >= 1));
    println!("landlock-tcp-ports: {}", yes(abi >= 4));
    println!(
        "landlock-signal-and-abstract-socket-scopes: {}",
        yes(abi >= 6)
    );
    println!("landlock-named-unix-sockets: {}", yes(abi >= 9));
    println!("landlock-udp: {}", yes(abi >= 10));
    println!(
        "seccomp-user-notification: {}",
        yes(linux::seccomp_user_notification())
    );
    println!(
        "launcher-accepts-this-kernel: {}",
        yes(abi >= linux::sandbox::MIN_ABI)
    );
    let groups = linux::group_names();
    let risky: Vec<&str> = linux::ROOT_EQUIVALENT_GROUPS
        .iter()
        .copied()
        .filter(|g| groups.iter().any(|n| n == g))
        .collect();
    println!(
        "root-equivalent-groups-of-this-user: {}",
        if risky.is_empty() {
            "none".to_string()
        } else {
            risky.join(",")
        }
    );
    if probe_network {
        println!(
            "network-rule-for-this-user: {}",
            match linux::network_rule_present() {
                Some(true) => "present (traffic leaving this machine is refused)",
                Some(false) => "absent (a datagram for another machine was sent)",
                None => "unknown",
            }
        );
    }
    0
}

#[cfg(not(target_os = "linux"))]
fn run(_: &ScopeArgs, _: HoldArgs, _: bool, _: &[String]) -> i32 {
    eprintln!("istesso-scope: this prototype confines on Linux only");
    125
}

#[cfg(target_os = "linux")]
fn run(args: &ScopeArgs, hold: HoldArgs, listen_anywhere: bool, command: &[String]) -> i32 {
    use std::os::unix::process::CommandExt;

    let (mut plan, machine) = match make_plan(args, true) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("istesso-scope: {e}");
            return 125;
        }
    };
    let kept_connects = match apply_grants(&mut plan, &hold, !args.quiet) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("istesso-scope: {e}");
            return 125;
        }
    };
    if let Err(e) = check_private_places(&plan, &hold) {
        eprintln!("istesso-scope: {e}");
        return 125;
    }
    let outside: Vec<(String, String)> = std::env::vars().collect();
    let mut cmd = std::process::Command::new(&command[0]);
    cmd.args(&command[1..]);
    if !plan.confined() {
        eprintln!("istesso-scope: level 4: no confinement");
        let e = cmd.exec();
        eprintln!("istesso-scope: could not start {}: {e}", command[0]);
        return if e.kind() == std::io::ErrorKind::NotFound {
            127
        } else {
            126
        };
    }

    let abi = linux::landlock_abi();
    if abi < linux::sandbox::MIN_ABI {
        eprintln!(
            "istesso-scope: this kernel's Landlock (ABI {abi}) cannot enforce level {}; \
             refusing to start (needs ABI {})",
            plan.level.number(),
            linux::sandbox::MIN_ABI
        );
        return 125;
    }
    let say = !args.quiet;
    if say {
        for line in linux::sandbox::report(&plan, abi) {
            eprintln!("istesso-scope: {line}");
        }
    }
    let groups = linux::group_names();
    for g in linux::ROOT_EQUIVALENT_GROUPS {
        if say && groups.iter().any(|n| n == g) {
            eprintln!(
                "istesso-scope: WARNING: this user is in the {g} group; a confined process can \
                 reach that daemon and act as root. Use a dedicated user without it."
            );
        }
    }

    let env = plan.environment(&outside, &machine);
    cmd.env_clear().envs(env);
    let cwd = std::env::current_dir()
        .ok()
        .filter(|d| plan.covers(d, false));
    cmd.current_dir(cwd.unwrap_or_else(|| plan.project.clone()));

    let built = match linux::sandbox::build(&plan) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("istesso-scope: {e}");
            return 125;
        }
    };
    for p in &built.skipped {
        if say && !policy::is_default_path(p) {
            eprintln!("istesso-scope: no rule for missing path {}", p.display());
        }
    }

    let loopback = !listen_anywhere;
    // Kept addresses are connected by the supervisor, so connections are held whenever there are
    // any.
    let hold_connect = hold.hold_connect || !kept_connects.is_empty();
    if hold.hold_files || hold_connect || loopback {
        if !cfg!(target_arch = "x86_64") {
            eprintln!(
                "istesso-scope: the supervisor (--hold-files, --hold-connect, loopback-only \
                 listening) is written for x86-64 only; use --listen-anywhere elsewhere"
            );
            return 125;
        }
        if !linux::seccomp_user_notification() {
            eprintln!("istesso-scope: this kernel has no seccomp user notification");
            return 125;
        }
        let link = match (&hold.ask_listen, &hold.link_keys) {
            (Some(addr), Some(keys)) => {
                match ask::Asker::wait_for_device(
                    *addr,
                    keys,
                    hold.ask_device.as_deref(),
                    std::time::Duration::from_secs(hold.ask_wait),
                ) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        eprintln!("istesso-scope: {e}; not starting");
                        return 125;
                    }
                }
            }
            _ => None,
        };
        let run_info = link.as_ref().map(|_| ask::HostSays::Run {
            project: plan.project.display().to_string(),
            level: plan.level.number(),
            command: command.to_vec(),
            ask_timeout_secs: hold.ask_timeout,
        });
        let opts = linux::hold::Options {
            hold_files: hold.hold_files,
            hold_connect,
            ask_cmd: hold.ask_cmd,
            link,
            ask_timeout: std::time::Duration::from_secs(hold.ask_timeout),
            grants: hold.grants,
            kept_connects,
            loopback,
            run_info,
        };
        return linux::hold::run(&plan, built.ruleset, cmd, opts);
    }

    if let Err(e) = linux::sandbox::restrict(built.ruleset) {
        eprintln!("istesso-scope: {e}");
        return 125;
    }
    if cfg!(target_arch = "x86_64") {
        if let Err(e) = linux::hold::install_plain() {
            eprintln!("istesso-scope: seccomp: {e}");
            return 125;
        }
    } else if say {
        eprintln!(
            "istesso-scope: NOT enforced on this machine: TCP Fast Open (sendto with \
             MSG_FASTOPEN) is not seen by Landlock's connect rule"
        );
    }
    let e = cmd.exec();
    eprintln!("istesso-scope: could not start {}: {e}", command[0]);
    if e.kind() == std::io::ErrorKind::NotFound {
        127
    } else {
        126
    }
}

/// Reads the grants file (when given): kept files and folders become rules of the plan (the
/// kernel enforces them from this start on); kept addresses are returned for the supervisor.
#[cfg(target_os = "linux")]
fn apply_grants(
    plan: &mut Plan,
    hold: &HoldArgs,
    say: bool,
) -> Result<Vec<std::net::SocketAddr>, String> {
    let Some(file) = &hold.grants else {
        return Ok(Vec::new());
    };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", file.display())),
    };
    let mut connects = Vec::new();
    for g in ask::parse_grants(&text)? {
        match g {
            ask::Grant::Read(p) | ask::Grant::Write(p) if !p.exists() => {
                if say {
                    eprintln!("istesso-scope: kept grant for missing path {}", p.display());
                }
            }
            // The rule would be put on whatever the path leads to now: a link put in its place
            // since (or in a folder above) would move the grant elsewhere.
            ask::Grant::Read(p) | ask::Grant::Write(p)
                if std::fs::canonicalize(&p).ok().as_deref() != Some(p.as_path()) =>
            {
                eprintln!(
                    "istesso-scope: kept grant for {} skipped: the path leads elsewhere now",
                    p.display()
                );
            }
            ask::Grant::Read(p) => plan.add_rule(p, policy::Access::Read),
            ask::Grant::Write(p) => plan.add_rule(p, policy::Access::Write),
            ask::Grant::Connect(a) => connects.push(a),
        }
    }
    if say && !text.is_empty() {
        eprintln!("istesso-scope: read the kept grants in {}", file.display());
    }
    Ok(connects)
}

/// The link keys must stay unreadable and the grants file unwritable for the confined command,
/// or it could answer its own questions or widen its own scope.
#[cfg(target_os = "linux")]
fn check_private_places(plan: &Plan, hold: &HoldArgs) -> Result<(), String> {
    if let Some(keys) = &hold.link_keys {
        let keys = absolute(keys)?;
        if plan.covers(&keys, false) || plan.covers(&keys.join("identity"), false) {
            return Err(format!(
                "the confined command could read the link keys in {}; keep them outside its rules",
                keys.display()
            ));
        }
    }
    if let Some(file) = &hold.grants {
        let parent = file
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = absolute(parent)?;
        let name = file.file_name().ok_or("the grants file needs a name")?;
        if plan.covers(&parent, true) || plan.covers(&parent.join(name), true) {
            return Err(format!(
                "the confined command could write the grants file in {}; keep it outside its rules",
                parent.display()
            ));
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn probe(_: ProbeCmd) -> i32 {
    eprintln!("istesso-scope: probes are Linux only");
    125
}

#[cfg(target_os = "linux")]
fn probe(what: ProbeCmd) -> i32 {
    use linux::probe as p;
    match what {
        ProbeCmd::Read { path } => p::read(&path),
        ProbeCmd::Write { path } => p::write(&path),
        ProbeCmd::Connect { addr } => p::connect(&addr),
        ProbeCmd::Bind { port } => p::bind(port),
        ProbeCmd::Udp { addr } => p::udp(&addr),
        ProbeCmd::Signal { pid } => p::signal(pid),
        ProbeCmd::Environ { pid } => p::environ(pid),
        ProbeCmd::Unix { path } => p::unix(&path),
        ProbeCmd::Abstract { name } => p::abstract_connect(&name),
        ProbeCmd::ServeTcp {
            port,
            seconds,
            reply,
        } => p::serve_tcp(port, seconds, &reply),
        ProbeCmd::BindAny { port, v6 } => p::bind_any(port, v6),
        ProbeCmd::IoUring => p::io_uring(),
        ProbeCmd::Mptcp { addr } => p::mptcp(&addr),
        ProbeCmd::ServeAbstract {
            name,
            seconds,
            reply,
        } => p::serve_abstract(&name, seconds, &reply),
    }
}
