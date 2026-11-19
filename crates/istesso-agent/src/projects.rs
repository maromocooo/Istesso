//! The development machine owns the register. Only its public projection
//! travels with the host manifest.
use std::collections::{hash_map::RandomState, BTreeSet};
use std::fs;
use std::hash::BuildHasher;
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};

use istesso_proto::{Project, ProjectKind};
use serde::{Deserialize, Serialize};

use crate::{cache::write_private, Paths};

#[derive(Serialize, Deserialize)]
struct Registration {
    id: String,
    name: String,
    folder: PathBuf,
}

/// A project name is typed as one word in later commands, so it must not
/// look like an option (`-x`) or a path (`.`, `..`, `.hidden`).
pub fn validate_name(name: &str) -> Result<(), String> {
    if (1..=64).contains(&name.len())
        && name.as_bytes()[0].is_ascii_alphanumeric()
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
    {
        Ok(())
    } else {
        Err(
            "use 1 to 64 ASCII letters, digits, hyphens, underscores or dots for the project \
             name, starting with a letter or digit"
                .into(),
        )
    }
}

fn entry_path(paths: &Paths, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    Ok(paths
        .config
        .join("projects")
        .join(format!("project-{}.json", name.to_ascii_lowercase())))
}

fn kinds(folder: &Path) -> io::Result<Vec<ProjectKind>> {
    let mut kinds = BTreeSet::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        let ty = entry.file_type()?;
        let kind = match name {
            "Cargo.toml" if ty.is_file() => ProjectKind::Rust,
            "package.json" if ty.is_file() => ProjectKind::Node,
            "Package.swift" if ty.is_file() => ProjectKind::Apple,
            _ if name.ends_with(".xcodeproj") && ty.is_dir() => ProjectKind::Apple,
            _ if name.ends_with(".blend") && ty.is_file() => ProjectKind::Blender,
            _ => continue,
        };
        kinds.insert(kind);
    }
    if kinds.is_empty() {
        kinds.insert(ProjectKind::Unknown);
    }
    Ok(kinds.into_iter().collect())
}

pub fn add(paths: &Paths, name: &str, folder: &Path) -> Result<(), String> {
    let path = entry_path(paths, name)?;
    let folder = folder
        .canonicalize()
        .map_err(|_| "cannot open the project folder")?;
    fs::read_dir(&folder).map_err(|_| "the project folder must be a readable directory")?;
    // This public label needs no secret or machine-specific input. A freshly
    // seeded standard hasher supplies two words for the opaque project ID.
    let seed = RandomState::new();
    let id = format!("{:016x}{:016x}", seed.hash_one(0u8), seed.hash_one(1u8));
    let staged = path.with_file_name(format!("pending-{id}"));
    let record = Registration {
        id,
        name: name.into(),
        folder,
    };
    let json = serde_json::to_vec(&record).map_err(|_| "cannot encode the project registration")?;
    write_private(&staged, &json).map_err(|_| "cannot save the project registration")?;
    // Publish a complete record only if the name is still free. Concurrent
    // adds never overwrite one another, even when the names differ by case.
    let result = fs::hard_link(&staged, path);
    let _ = fs::remove_file(staged);
    match result {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => {
            Err("project name already registered on this machine".into())
        }
        Err(_) => Err("cannot save the project registration".into()),
    }
}

pub fn remove(paths: &Paths, name: &str) -> Result<(), String> {
    match fs::remove_file(entry_path(paths, name)?) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => {
            Err("project is not registered on this machine".into())
        }
        Err(_) => Err("cannot remove the project registration".into()),
    }
}

pub fn list(paths: &Paths) -> Result<Vec<Project>, String> {
    const READ_ERROR: &str = "cannot read the local project register";
    let entries = match fs::read_dir(paths.config.join("projects")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(READ_ERROR.into()),
    };
    let mut projects = Vec::new();
    for entry in entries {
        let path = entry.map_err(|_| READ_ERROR)?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let data = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => continue, // Removed during the listing.
            Err(_) => return Err(READ_ERROR.into()),
        };
        let record: Registration = serde_json::from_slice(&data).map_err(|_| READ_ERROR)?;
        validate_name(&record.name)?;
        projects.push(Project {
            kinds: kinds(&record.folder).unwrap_or_else(|_| vec![ProjectKind::Unknown]),
            name: record.name,
            id: record.id,
        });
    }
    projects.sort_by_cached_key(|p| p.name.to_ascii_lowercase());
    Ok(projects)
}
