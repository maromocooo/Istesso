//! The development machine owns the register. Only its public projection
//! travels with the host manifest.
mod kinds;

use std::collections::hash_map::RandomState;
use std::fs;
use std::hash::BuildHasher;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Instant;

use istesso_proto::Project;
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

/// The folder registered on this machine under `name` (in any case), for
/// a shell asked to start in a project by its name. `None` when no sound
/// registration has that name.
pub fn folder(paths: &Paths, name: &str) -> Option<PathBuf> {
    let path = entry_path(paths, name).ok()?;
    let data = fs::read(&path).ok()?;
    read_record(paths, &path, &data).map(|record| record.folder)
}

#[derive(Default)]
pub struct ProjectList {
    pub projects: Vec<Project>,
    pub skipped: usize,
}

pub fn list(paths: &Paths) -> Result<Vec<Project>, String> {
    Ok(list_with_skipped(paths)?.projects)
}

pub fn list_with_skipped(paths: &Paths) -> Result<ProjectList, String> {
    let mut list = ProjectList::default();
    let entries = match fs::read_dir(paths.config.join("projects")) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(list),
        Err(_) => return Err("cannot read the local project register".into()),
    };
    let mut found = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            list.skipped += 1;
            continue;
        };
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        if !entry.file_type().is_ok_and(|ty| ty.is_file()) {
            list.skipped += 1;
            continue;
        }
        let data = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == ErrorKind::NotFound => continue, // Removed during the listing.
            Err(_) => {
                list.skipped += 1;
                continue;
            }
        };
        if let Some(record) = read_record(paths, &path, &data) {
            let scan = kinds::start(record.folder.clone());
            found.push((record, scan));
        } else {
            list.skipped += 1;
        }
    }
    // Every folder is being looked at by now: the answer waits once at most.
    let deadline = Instant::now() + kinds::TIMEOUT;
    for (record, scan) in found {
        match scan.and_then(|scan| kinds::wait(&scan, deadline)) {
            Some(kinds) => list.projects.push(Project {
                kinds,
                name: record.name,
                id: record.id,
            }),
            None => list.skipped += 1,
        }
    }
    list.projects
        .sort_by_cached_key(|p| p.name.to_ascii_lowercase());
    Ok(list)
}

fn read_record(paths: &Paths, path: &Path, data: &[u8]) -> Option<Registration> {
    let record: Registration = serde_json::from_slice(data).ok()?;
    let sound = entry_path(paths, &record.name).ok()? == path
        && record.folder.is_absolute()
        && record.id.len() == 32
        && record.id.bytes().all(|b| b.is_ascii_hexdigit());
    sound.then_some(record)
}
