use std::collections::HashSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use clap::Subcommand;
use serde::Serialize;

const LIST_BYTES: usize = 64 * 1024;
const LIST_ENTRIES: usize = 200;
const LABEL_BYTES: usize = 4 * 1024;
const LIST_TIMEOUT: Duration = Duration::from_secs(5);
const LIST_FAILED: &str = "the trial's Ubuntu app list failed";
const LIST_TIMEOUT_MESSAGE: &str = "the trial's Ubuntu app list did not answer in time";
const DAILY: &str = "scripts/daily.sh";

#[derive(Subcommand)]
pub(super) enum Action {
    /// List applications in launcher order
    Apps {
        #[arg(long, value_name = "DIR")]
        trial: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Open a listed application
    Open {
        #[arg(allow_hyphen_values = true)]
        id: String,
        #[arg(long, value_name = "DIR")]
        trial: Option<PathBuf>,
        /// Print the command that would open the application
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Default, Serialize)]
struct Catalog {
    apps: Vec<App>,
    problems: Vec<String>,
}

#[derive(Serialize)]
struct App {
    id: String,
    name: String,
    machine: String,
    host: &'static str,
    #[serde(skip)]
    open: OpenCommand,
}

#[derive(Serialize)]
struct OpenCommand {
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
}

impl OpenCommand {
    fn status(&self) -> io::Result<ExitStatus> {
        Command::new(&self.program)
            .args(&self.args)
            .current_dir(&self.cwd)
            .env_remove("DAILY_OWNER")
            .env_remove("DAILY_OPEN_HANDLER")
            .env_remove("ISTESSO_APP_ICON")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    }
}

fn read_limited(reader: impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit as u64).read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn lines(bytes: &[u8]) -> impl Iterator<Item = Result<&str, std::str::Utf8Error>> {
    bytes.split_inclusive(|&b| b == b'\n').map(|line| {
        let content = match line.strip_suffix(b"\n") {
            Some(body) => body.strip_suffix(b"\r").unwrap_or(body),
            None => line,
        };
        std::str::from_utf8(content)
    })
}

struct Labels {
    ubuntu: String,
    mac: String,
}

impl Default for Labels {
    fn default() -> Self {
        Self {
            ubuntu: "Ubuntu".into(),
            mac: "Studio".into(),
        }
    }
}

fn parse_labels(bytes: &[u8]) -> Result<Labels, usize> {
    let mut labels = Labels::default();
    for (index, line) in lines(bytes).enumerate() {
        let number = index + 1;
        let line = line.map_err(|_| number)?;
        if line.trim().is_empty() {
            continue;
        }
        let (host, value) = line.split_once('=').ok_or(number)?;
        let label = value.trim();
        if !(1..=12).contains(&label.chars().count()) || label.chars().any(char::is_control) {
            return Err(number);
        }
        match host {
            "ubuntu" => labels.ubuntu = label.into(),
            "mac" => labels.mac = label.into(),
            _ => return Err(number),
        }
    }
    Ok(labels)
}

fn read_labels(home: Option<&Path>) -> Result<Labels, usize> {
    let Some(file) = home
        .and_then(|home| File::open(home.join(".config/istesso/daily-machine-labels.txt")).ok())
    else {
        return Ok(Labels::default());
    };
    let too_large = file.metadata().is_ok_and(|m| m.len() > LABEL_BYTES as u64);
    let bytes = read_limited(file, LABEL_BYTES).map_err(|_| 1usize)?;
    let labels = parse_labels(&bytes)?;
    if too_large && !bytes.ends_with(b"\n") {
        return Err(1 + bytes.iter().filter(|&&b| b == b'\n').count());
    }
    Ok(labels)
}

fn valid_name(name: &str) -> bool {
    if name.is_empty()
        || name != name.trim()
        || name.starts_with(['-', '.'])
        || name.contains(['/', ':', '\\'])
        || name.chars().any(char::is_control)
    {
        return false;
    }
    let lower = name.to_lowercase();
    !matches!(
        lower.strip_suffix(".app").unwrap_or(&lower),
        "desktop" | "safari" | "notes" | "finder" | "preview" | "dia"
    )
}

#[derive(Clone, Copy)]
enum TrialHost {
    Ubuntu,
    Studio,
}

impl TrialHost {
    fn key(self) -> &'static str {
        match self {
            Self::Ubuntu => "ubuntu",
            Self::Studio => "mac",
        }
    }

    fn list_name(self) -> &'static str {
        match self {
            Self::Ubuntu => "Ubuntu",
            Self::Studio => "Studio",
        }
    }

    fn parse_line(self, line: &str) -> Option<(&str, &str)> {
        let (name, title) = match self {
            Self::Ubuntu => {
                let (name, title) = line.split_once('\t')?;
                let trimmed = title.trim();
                if title.chars().any(char::is_control)
                    || trimmed.is_empty()
                    || title.chars().count() > 64
                {
                    return None;
                }
                (name, trimmed)
            }
            Self::Studio => (line, line),
        };
        valid_name(name).then_some((name, title))
    }
}

impl Catalog {
    fn parse_trial(&mut self, host: TrialHost, machine: &str, dir: &Path, bytes: &[u8]) {
        let mut names = HashSet::new();
        let mut skipped = 0;
        for line in lines(bytes) {
            let Ok(line) = line else {
                skipped += 1;
                continue;
            };
            if line.trim().is_empty()
                || (matches!(host, TrialHost::Studio) && line.starts_with('#'))
            {
                continue;
            }
            let Some((name, title)) = host.parse_line(line) else {
                skipped += 1;
                continue;
            };
            if names.len() == LIST_ENTRIES || !names.insert(name) {
                continue;
            }
            let host = host.key();
            self.apps.push(App {
                id: format!("{host}/{name}"),
                name: title.into(),
                host,
                machine: machine.into(),
                open: OpenCommand {
                    program: dir.join(DAILY),
                    args: vec!["open".into(), name.into(), "--host".into(), host.into()],
                    cwd: dir.into(),
                },
            });
        }
        if skipped > 0 {
            let noun = if skipped == 1 { "line" } else { "lines" };
            self.problems.push(format!(
                "the trial's {} app list: {skipped} {noun} skipped",
                host.list_name()
            ));
        }
    }

    fn add_trial(&mut self, host: TrialHost, dir: &Path, machine: &str) {
        let result = match host {
            TrialHost::Ubuntu => ubuntu_list(dir),
            TrialHost::Studio => File::open(dir.join("macos/daily-shortcuts/mac-apps.txt"))
                .and_then(|file| read_limited(file, LIST_BYTES))
                .map_err(|_| "the trial has no Studio app list"),
        };
        match result {
            Ok(bytes) => {
                // At the limit a partial name could identify a different app.
                let complete = if bytes.len() == LIST_BYTES && !bytes.ends_with(b"\n") {
                    &bytes[..bytes.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1)]
                } else {
                    &bytes
                };
                self.parse_trial(host, machine, dir, complete);
            }
            Err(message) => self.problems.push(message.into()),
        }
    }

    #[cfg(any(target_os = "macos", test))]
    fn add_local(&mut self, dirs: &[PathBuf]) {
        for entry in istesso_catalog::macos::scan(dirs) {
            if entry.id.starts_with("com.maromoco.istesso-dev.") {
                continue;
            }
            if let istesso_proto::Launch::AppBundle { bundle_id, path } = entry.launch {
                self.apps.push(App {
                    id: format!("local/{bundle_id}"),
                    name: entry.name,
                    machine: "This Mac".into(),
                    host: "local",
                    open: OpenCommand {
                        program: "/usr/bin/open".into(),
                        args: vec!["-a".into(), path],
                        cwd: "/".into(),
                    },
                });
            }
        }
    }

    fn find(&self, id: &str) -> Option<&App> {
        self.apps.iter().find(|app| app.id == id)
    }
}

// Read while the child runs so a full pipe cannot stall the list command.
// Both waits share one deadline, including if a descendant inherits stdout.
fn ubuntu_list(dir: &Path) -> Result<Vec<u8>, &'static str> {
    let script = dir.join(DAILY);
    if !script.exists() {
        return Err("the trial has no scripts/daily.sh");
    }
    let mut child = Command::new(script)
        .arg("apps")
        .current_dir(dir)
        .env_remove("DAILY_OWNER")
        .env_remove("DAILY_OPEN_HANDLER")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| LIST_FAILED)?;
    let deadline = Instant::now() + LIST_TIMEOUT;
    let stdout = child.stdout.take().expect("piped stdout");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(read_limited(stdout, LIST_BYTES));
    });
    loop {
        let problem = match child.try_wait() {
            Ok(Some(status)) if status.success() => break,
            Ok(Some(_)) => return Err(LIST_FAILED),
            Ok(None) if Instant::now() >= deadline => LIST_TIMEOUT_MESSAGE,
            Ok(None) => {
                thread::sleep(Duration::from_millis(10));
                continue;
            }
            Err(_) => LIST_FAILED,
        };
        let _ = child.kill();
        let _ = child.wait();
        return Err(problem);
    }
    receiver
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|_| LIST_TIMEOUT_MESSAGE)?
        .map_err(|_| LIST_FAILED)
}

fn catalog(
    trial: Option<PathBuf>,
    home: Option<&Path>,
    source: Option<&str>,
) -> Result<Catalog, String> {
    let mut catalog = Catalog::default();
    let wants = |key| source.is_none_or(|s| s == key);
    if wants("ubuntu") || wants("mac") {
        let labels = read_labels(home).unwrap_or_else(|line| {
            catalog.problems.push(format!(
                "the machine labels file has an invalid line {line}"
            ));
            Labels::default()
        });
        let trial = trial.or_else(|| env::var_os("ISTESSO_LAUNCHER_TRIAL").map(PathBuf::from));
        if let Some(trial) = trial.filter(|dir| !dir.as_os_str().is_empty()) {
            let dir =
                std::path::absolute(trial).map_err(|_| "cannot resolve the trial checkout")?;
            for (host, machine) in [
                (TrialHost::Ubuntu, labels.ubuntu),
                (TrialHost::Studio, labels.mac),
            ] {
                if wants(host.key()) {
                    catalog.add_trial(host, &dir, &machine);
                }
            }
        } else {
            catalog
                .problems
                .push("no trial checkout was given (--trial)".into());
        }
    }
    #[cfg(target_os = "macos")]
    if wants("local") {
        catalog.add_local(&istesso_catalog::macos::default_dirs());
    }
    Ok(catalog)
}

fn measuring(home: Option<&Path>) -> Result<bool, String> {
    let Some(home) = home else { return Ok(false) };
    match fs::read_to_string(home.join(".local/state/istesso/measuring")) {
        Ok(signal) => Ok(signal.split_whitespace().next() == Some("busy")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err("cannot read the measuring signal".into()),
    }
}

fn exit_number(status: ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    status.code().unwrap_or(1)
}

pub(super) fn run(action: Action) -> Result<ExitCode, String> {
    let home = env::var_os("HOME").map(PathBuf::from);
    match action {
        Action::Apps { trial, json } => {
            let catalog = catalog(trial, home.as_deref(), None)?;
            if json {
                super::print_json(&catalog)?;
            } else {
                println!("APP\tMACHINE\tID");
                for app in catalog.apps {
                    println!("{}\t{}\t{}", app.name, app.machine, app.id);
                }
                for problem in catalog.problems {
                    eprintln!("warning: {problem}");
                }
            }
        }
        Action::Open { id, trial, dry_run } => {
            if measuring(home.as_deref())? {
                eprintln!("a latency measurement is running; open it again when it is over");
                return Ok(ExitCode::from(3));
            }
            let source = id.split_once('/').map_or("", |(prefix, _)| prefix);
            let catalog = catalog(trial, home.as_deref(), Some(source))?;
            let Some(app) = catalog.find(&id) else {
                eprintln!("unknown launcher entry: {id}");
                return Ok(ExitCode::FAILURE);
            };
            if dry_run {
                super::print_json(&app.open)?;
            } else {
                let code = app.open.status().map(exit_number).unwrap_or(1);
                if code != 0 {
                    eprintln!(
                        "could not open {} on {} (exit {code})",
                        app.name, app.machine
                    );
                    return Ok(ExitCode::from(code as u8));
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests;
