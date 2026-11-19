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
    let names = machines.values().copied().collect::<Vec<_>>().join(", ");
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
            return Err(format!("project {name} is already registered on {machine}"));
        }
    }
    Ok(())
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
        row.project.name,
        row.development_machine,
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
