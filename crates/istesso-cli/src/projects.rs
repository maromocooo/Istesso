use std::collections::BTreeSet;
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

fn merged(views: &[HostView]) -> Vec<ProjectRow<'_>> {
    let mut rows = Vec::new();
    for view in views {
        if let Some(manifest) = &view.manifest {
            for project in &manifest.projects {
                rows.push(ProjectRow {
                    project,
                    development_machine: &view.name,
                    reachable: view.state == HostState::Awake,
                    capabilities: Summary::for_project(project, manifest),
                });
            }
        }
    }
    rows
}

fn check_remote_name(name: &str, hosts: &[HostView]) -> Result<(), String> {
    for host in hosts.iter().filter(|host| !host.local) {
        if let Some(manifest) = &host.manifest {
            if manifest
                .projects
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(name))
            {
                return Err(format!(
                    "project {name} is already registered on {}",
                    host.name
                ));
            }
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
    let mark = if row.reachable { " " } else { "!" };
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

pub(super) fn run(action: Option<Action>, json: bool) -> Result<(), String> {
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
            let registered = projects::list(&paths)?;
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
            let rows = merged(&hosts);
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
