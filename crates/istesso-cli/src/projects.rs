use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use clap::Subcommand;
use istesso_agent::{projects, survey, HostView, Paths, SurveyOptions};
use istesso_proto::{HostManifest, HostState, Project, ProjectKind};
use serde::Serialize;

#[derive(Subcommand)]
pub(super) enum Action {
    /// Register a folder on this machine
    Add {
        #[arg(allow_hyphen_values = true)]
        name: String,
        folder: PathBuf,
    },
    /// Remove a registration from this machine
    Remove {
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    /// List only the projects registered on this machine
    List,
}

#[derive(Serialize)]
struct ProjectRow<'a> {
    #[serde(flatten)]
    project: &'a Project,
    development_machine: &'a str,
    reachable: bool,
    clash: bool,
    capabilities: Summary,
}

#[derive(Serialize)]
struct Summary {
    gpu: bool,
    tools: BTreeSet<&'static str>,
}

impl Summary {
    fn for_project(project: &Project, manifest: &HostManifest) -> Self {
        let mut tools = BTreeSet::new();
        for kind in &project.kinds {
            let relevant: &[&str] = match kind {
                ProjectKind::Rust => &["cargo"],
                ProjectKind::Node => &["node", "npm"],
                ProjectKind::Apple => &["swift", "xcodebuild", "simctl", "devicectl"],
                ProjectKind::Blender => &["blender"],
                ProjectKind::Unknown => &[],
            };
            for &name in relevant {
                if manifest
                    .capabilities
                    .tools
                    .get(name)
                    .is_some_and(Option::is_some)
                {
                    tools.insert(name);
                }
            }
        }
        Summary {
            gpu: !manifest.hardware.gpus.is_empty(),
            tools,
        }
    }
}

// Use the paired identity to distinguish machines with the same display name.
fn owners<'a>(
    views: impl Iterator<Item = &'a HostView>,
) -> BTreeMap<String, BTreeMap<&'a str, &'a str>> {
    let mut names = BTreeMap::new();
    for view in views {
        if let Some(manifest) = &view.manifest {
            for project in &manifest.projects {
                names
                    .entry(project.name.to_ascii_lowercase())
                    .or_insert_with(BTreeMap::new)
                    .insert(view.id.as_str(), view.name.as_str());
            }
        }
    }
    names
}

fn ambiguous(name: &str, machines: &BTreeMap<&str, &str>) -> String {
    let names = machines
        .values()
        .copied()
        .map(shown)
        .collect::<Vec<_>>()
        .join(", ");
    format!("project {name} is ambiguous on these machines: {names}")
}

fn merged<'a>(views: &'a [HostView], name: Option<&str>) -> Result<Vec<ProjectRow<'a>>, String> {
    let owners = owners(views.iter());
    if let Some(name) = name {
        match owners.get(&name.to_ascii_lowercase()) {
            None => return Err(format!("project {name} was not found")),
            Some(machines) if machines.len() > 1 => return Err(ambiguous(name, machines)),
            _ => {}
        }
    }
    let mut rows = Vec::new();
    for view in views {
        if let Some(manifest) = &view.manifest {
            for project in &manifest.projects {
                if name.is_some_and(|name| !name.eq_ignore_ascii_case(&project.name)) {
                    continue;
                }
                rows.push(ProjectRow {
                    project,
                    development_machine: &view.name,
                    reachable: view.state == HostState::Awake,
                    clash: owners[&project.name.to_ascii_lowercase()].len() > 1,
                    capabilities: Summary::for_project(project, manifest),
                });
            }
        }
    }
    Ok(rows)
}

fn check_remote_name(name: &str, hosts: &[HostView]) -> Result<(), String> {
    let owners = owners(hosts.iter().filter(|host| !host.local));
    if let Some(machines) = owners.get(&name.to_ascii_lowercase()) {
        if machines.len() > 1 {
            return Err(ambiguous(name, machines));
        }
        if let Some(machine) = machines.values().next() {
            return Err(format!(
                "project {name} is already registered on {}",
                shown(machine)
            ));
        }
    }
    Ok(())
}

/// Where `istesso shell --project-name` opens its shell: the machine the
/// project list says has the project, reached where it answered just now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ShellTarget {
    /// The machine's id, as `istesso shell` takes it.
    pub id: String,
    /// The project's name, as the machine registered it. Its folder is only
    /// known there: the machine looks it up by this name.
    pub name: String,
    pub address: Option<String>,
}

/// `--project` takes a folder: an absolute path on the device, or one that
/// starts with `~/`. Anything else is refused here, before a device could
/// take a word for the name of one of its projects.
pub(super) fn check_folder(folder: &str) -> Result<(), String> {
    if folder == "~" || folder.starts_with("~/") || folder.starts_with('/') {
        Ok(())
    } else {
        Err(
            "--project takes a folder: an absolute path on the device, or one that starts \
             with ~/ (a project by its name: --project-name)"
                .into(),
        )
    }
}

/// What another machine said, made fit for this terminal: its control
/// characters (escape sequences among them) are shown as `\u{..}` and do
/// nothing. Names and reasons from paired machines go through this.
pub(super) fn shown(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() {
                c.escape_unicode().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Asks every paired machine for its projects, as `istesso projects` does,
/// and finds where `asked` (`NAME` or `MACHINE:NAME`) is. `address`, as
/// `istesso peers address` writes it, is where the chosen machine is
/// reached when that machine is remembered there (a cable, say); otherwise
/// it is reached where it answered just now.
pub(super) fn shell_target(asked: &str, address: Option<&str>) -> Result<ShellTarget, String> {
    // A word that cannot be a name is refused before anybody is asked.
    split_asked(asked)?;
    let paths = Paths::from_env();
    let remote = {
        let paths = paths.clone();
        std::thread::spawn(move || survey::refresh_peers(&paths, &SurveyOptions::default()))
    };
    // This machine is not inspected for it: only its name and its projects
    // matter here.
    let identity = istesso_pair::Identity::load_or_create(&paths.identity())
        .map_err(|_| "cannot read this device's identity")?;
    let (id, name) = (identity.fingerprint(), istesso_doctor::hostname());
    let here = projects::list(&paths)?;
    let remote = remote
        .join()
        .map_err(|_| "internal error while asking the paired devices".to_string())?
        .map_err(|_| "cannot read the paired host list")?;
    let mut machines = vec![Machine {
        id: &id,
        name: &name,
        local: true,
        awake: true,
        address: None,
        problem: None,
        projects: Some(&here),
    }];
    machines.extend(remote.iter().map(Machine::of));
    let mut target = resolve_among(&machines, asked)?;
    let remembered = istesso_agent::cache::load(&paths, &target.id)
        .map(|snapshot| snapshot.addresses)
        .unwrap_or_default();
    prefer_address(&mut target, &remembered, address);
    Ok(target)
}

/// The chosen machine is reached at `address` when it is remembered there,
/// and where it answered otherwise: an address given for one machine (the
/// cable to it) does not send the shell of another machine's project there.
fn prefer_address(target: &mut ShellTarget, remembered: &[String], address: Option<&str>) {
    if let Some(address) = address.filter(|address| remembered.iter().any(|a| a == address)) {
        target.address = Some(address.to_string());
    }
}

/// One machine, as much of it as finding a project needs.
struct Machine<'a> {
    id: &'a str,
    name: &'a str,
    local: bool,
    awake: bool,
    address: Option<&'a str>,
    problem: Option<&'a str>,
    /// What it answered or what is remembered of it; `None` when it was
    /// never seen.
    projects: Option<&'a [Project]>,
}

impl<'a> Machine<'a> {
    fn of(view: &'a HostView) -> Self {
        Machine {
            id: &view.id,
            name: &view.name,
            local: view.local,
            awake: view.state == HostState::Awake,
            address: view.address.as_deref(),
            problem: view.problem.as_deref(),
            projects: view
                .manifest
                .as_ref()
                .map(|manifest| manifest.projects.as_slice()),
        }
    }

    /// Its projects whose names are names: anything else a machine sends
    /// can never be asked for, and is never shown or compared.
    fn projects(&self) -> impl Iterator<Item = &'a Project> {
        self.projects
            .unwrap_or_default()
            .iter()
            .filter(|project| projects::validate_name(&project.name).is_ok())
    }
}

/// `MACHINE:NAME` or `NAME`. A project's name has no `:` (nor `/`), so the
/// last `:` ends the machine.
fn split_asked(asked: &str) -> Result<(Option<&str>, &str), String> {
    let (machine, name) = match asked.rsplit_once(':') {
        Some((machine, name)) => (Some(machine), name),
        None => (None, asked),
    };
    if machine.is_some_and(str::is_empty) {
        return Err(format!(
            "{}: put the machine before the colon (MACHINE:NAME), or leave the colon out",
            shown(asked)
        ));
    }
    if projects::validate_name(name).is_err() {
        let hint = if name.contains('/') {
            "; a folder is not a project name"
        } else {
            ""
        };
        return Err(format!(
            "{name:?} is not a project name: a name is 1 to 64 ASCII letters, digits, hyphens, \
             underscores or dots, starting with a letter or digit{hint}"
        ));
    }
    Ok((machine, name))
}

/// The machines `selector` names: by id, the beginning of an id (four
/// characters or more), or display name in any case.
fn machines<'m, 'a>(all: &'m [Machine<'a>], selector: &str) -> Vec<&'m Machine<'a>> {
    if let Some(machine) = all.iter().find(|machine| machine.id == selector) {
        return vec![machine];
    }
    all.iter()
        .filter(|machine| {
            machine.name.eq_ignore_ascii_case(selector)
                || (selector.len() >= 4 && machine.id.starts_with(selector))
        })
        .collect()
}

/// [`resolve_among`] for the list as `istesso projects` has it.
#[cfg(test)]
pub(super) fn resolve(views: &[HostView], asked: &str) -> Result<ShellTarget, String> {
    let machines: Vec<Machine> = views.iter().map(Machine::of).collect();
    resolve_among(&machines, asked)
}

/// Finds the one machine that has the project `asked` (as `istesso
/// projects` shows the list). Never guesses: a name on more than one
/// machine, a machine that does not answer, or a name nobody has, ends with
/// a message saying so, and what can be typed instead.
fn resolve_among(all: &[Machine], asked: &str) -> Result<ShellTarget, String> {
    let (machine, name) = split_asked(asked)?;
    let chosen: Vec<&Machine> = match machine {
        None => all.iter().collect(),
        Some(selector) => {
            let found = machines(all, selector);
            match found.as_slice() {
                [] => {
                    let known = all
                        .iter()
                        .map(|machine| shown(machine.name))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(format!(
                        "no machine is called {selector:?}; the machines are: {known}"
                    ));
                }
                [_] => found,
                _ => {
                    let ids = found
                        .iter()
                        .map(|machine| {
                            format!("  {}:{name}\t({})", shown(machine.id), shown(machine.name))
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Err(format!(
                        "more than one machine is called {selector:?}; choose one by its id:\n{ids}"
                    ));
                }
            }
        }
    };
    let owners: Vec<(&Machine, &Project)> = chosen
        .iter()
        .flat_map(|machine| {
            machine
                .projects()
                .filter(|project| project.name.eq_ignore_ascii_case(name))
                .map(move |project| (*machine, project))
        })
        .collect();
    match owners.as_slice() {
        [] => Err(unknown(all, &chosen, machine, name)),
        [(machine, project)] if machine.local => Err(format!(
            "project {} is on this machine ({}); istesso shell opens shells on other machines",
            project.name,
            shown(machine.name)
        )),
        [(machine, project)] if !machine.awake => {
            let why = machine
                .problem
                .map(|problem| format!(" ({})", shown(problem)))
                .unwrap_or_default();
            Err(format!(
                "project {} is on {}, which is not answering now{why}",
                project.name,
                shown(machine.name)
            ))
        }
        [(machine, project)] => Ok(ShellTarget {
            id: machine.id.to_string(),
            name: project.name.clone(),
            address: machine.address.map(str::to_string),
        }),
        several => {
            // A display name shared by two machines cannot choose between
            // them: those are given by id.
            let shared = |machine: &Machine| {
                several
                    .iter()
                    .filter(|(other, _)| other.name.eq_ignore_ascii_case(machine.name))
                    .count()
                    > 1
            };
            let lines = several
                .iter()
                .map(|(machine, project)| {
                    let which = if shared(machine) {
                        machine.id
                    } else {
                        machine.name
                    };
                    let state = match (machine.local, machine.awake) {
                        (true, _) => "\t(this machine)",
                        (false, true) => "",
                        (false, false) => "\t(not answering now)",
                    };
                    format!("  {}:{}{state}", shown(which), project.name)
                })
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "project {name} is on more than one machine; choose one with MACHINE:NAME:\n{lines}"
            ))
        }
    }
}

/// The message for a name that is not in the list: the names it may have
/// been meant for, and the machines whose projects are not known.
fn unknown(all: &[Machine], chosen: &[&Machine], machine: Option<&str>, name: &str) -> String {
    let mut message = match machine {
        Some(_) => format!("{} has no project called {name}", shown(chosen[0].name)),
        None => format!("no machine has a project called {name}"),
    };
    let elsewhere = all
        .iter()
        .filter(|machine| !chosen.iter().any(|c| c.id == machine.id))
        .filter(|machine| {
            machine
                .projects()
                .any(|p| p.name.eq_ignore_ascii_case(name))
        })
        .map(|machine| shown(machine.name))
        .collect::<Vec<_>>();
    if !elsewhere.is_empty() {
        message.push_str(&format!("; it is on: {}", elsewhere.join(", ")));
    }
    let names = chosen
        .iter()
        .flat_map(|machine| machine.projects())
        .map(|p| p.name.as_str());
    let close = close_names(name, names);
    if !close.is_empty() {
        message.push_str(&format!("\ndid you mean: {}", close.join(", ")));
    }
    let silent = chosen
        .iter()
        .filter(|machine| !machine.local && machine.projects.is_none())
        .map(|machine| shown(machine.name))
        .collect::<Vec<_>>();
    if !silent.is_empty() {
        message.push_str(&format!(
            "\nnot known: the projects of {} (not answering, and never seen)",
            silent.join(", ")
        ));
    }
    // The word may have been meant as a folder below home.
    message.push_str(&format!(
        "\na folder, not a project? give its path, such as ~/{name}"
    ));
    message
}

/// The names in `names` that `asked` may have been meant for, closest
/// first: a few letters apart, or one inside the other. Five at most.
fn close_names<'a>(asked: &str, names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let asked = asked.to_ascii_lowercase();
    let mut found: Vec<(usize, String)> = Vec::new();
    for name in names {
        let lower = name.to_ascii_lowercase();
        if lower == asked
            || found
                .iter()
                .any(|(_, seen)| seen.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let within = (asked.len().max(lower.len()) / 3).max(1);
        let inside = asked.len().min(lower.len()) >= 2
            && (lower.contains(asked.as_str()) || asked.contains(lower.as_str()));
        // Lengths further apart than that are further apart in letters too.
        if !inside && asked.len().abs_diff(lower.len()) > within {
            continue;
        }
        let distance = edit_distance(&asked, &lower);
        if distance <= within || inside {
            found.push((distance, name.to_string()));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().take(5).map(|(_, name)| name).collect()
}

/// The number of letters to add, remove, change or swap with the next one
/// to make `a` into `b` (optimal string alignment; names are ASCII). A
/// swap counts once: `alpah` is one away from `alpha`.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    // d[i][j]: the distance between the first i letters of a and the first
    // j of b.
    let mut d = vec![vec![0; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let change = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + change);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[a.len()][b.len()]
}

fn kind_labels(project: &Project) -> String {
    project
        .kinds
        .iter()
        .map(|kind| kind.label())
        .collect::<Vec<_>>()
        .join(",")
}

fn render(row: &ProjectRow<'_>) -> String {
    let mark = match (row.reachable, row.clash) {
        (true, false) => " ",
        (true, true) => "?",
        (false, false) => "!",
        (false, true) => "!?",
    };
    let gpu = if row.capabilities.gpu { "yes" } else { "no" };
    let tools = row
        .capabilities
        .tools
        .iter()
        .copied()
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{mark} {}\t{}\t{}\t{gpu}\t{}",
        shown(&row.project.name),
        shown(row.development_machine),
        kind_labels(row.project),
        if tools.is_empty() { "-" } else { &tools }
    )
}

pub(super) fn run(action: Option<Action>, name: Option<&str>, json: bool) -> Result<(), String> {
    if action.is_some() && name.is_some() {
        return Err("--name applies to the merged project list".into());
    }
    let paths = Paths::from_env();
    let options = SurveyOptions::default();
    match action {
        Some(Action::Add { name, folder }) => {
            projects::validate_name(&name)?;
            let hosts = istesso_agent::survey::refresh_peers(&paths, &options)
                .map_err(|_| "cannot read the paired host list")?;
            check_remote_name(&name, &hosts)?;
            projects::add(&paths, &name, &folder)
        }
        Some(Action::Remove { name }) => projects::remove(&paths, &name),
        Some(Action::List) => {
            let list = projects::list_with_skipped(&paths)?;
            if list.skipped > 0 {
                let noun = if list.skipped == 1 {
                    "project"
                } else {
                    "projects"
                };
                eprintln!("{} {noun} skipped", list.skipped);
            }
            let registered = list.projects;
            if json {
                return super::print_json(&registered);
            }
            println!("PROJECT\tKINDS");
            for project in registered {
                println!("{}\t{}", project.name, kind_labels(&project));
            }
            Ok(())
        }
        None => {
            let hosts = survey(&paths, &options).map_err(|_| "cannot read the project list")?;
            let rows = merged(&hosts, name)?;
            if json {
                return super::print_json(&rows);
            }
            println!("  PROJECT\tDEVELOPMENT MACHINE\tKINDS\tGPU\tTOOLS");
            for row in &rows {
                println!("{}", render(row));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests;
