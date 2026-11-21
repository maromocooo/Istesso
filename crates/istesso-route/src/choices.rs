//! Remembered choices: the machine a person chose for a deploy task.
//!
//! The choice belongs to the person on this device, not to the project, so
//! it lives in a settings file per device and user, never in the project's
//! `.istesso/` (that file is shared and may be public). It is not kept next
//! to Istesso's keys and pairing list either: a coding agent may be fenced
//! off from that folder.
//!
//! A machine is remembered by the id the role table uses; the name is kept
//! for display only. A task is remembered by its project and its name, so
//! that a task of the same name in another project is not mistaken for it.
//! The project is the name the task file gives (`project`), so that every
//! worktree of the project shares the choice; a task file without a name is
//! told apart by the project's location instead.
//!
//! ```json
//! { "version": 1,
//!   "choices": [ { "project_name": "example", "task": "site",
//!                  "host": "9f8e-7d6c-5b4a-3921", "name": "tower",
//!                  "chosen_at": 1794879000 },
//!                { "project": "/path/to/project", "task": "site",
//!                  "host": "5d4c-3b2a-1f0e-9d8c", "name": "mini",
//!                  "chosen_at": 1794879000 } ] }
//! ```
//!
//! Unknown items are kept when the file is written again. A file of an
//! unknown version, or one that cannot be read, is never overwritten.
//!
//! A remembered choice sends a deploy task to its machine without asking, so
//! a file that someone else could have written is not used either (and not
//! overwritten). On Unix the folder that holds it must belong to this user
//! or to root, the file to this user, and neither may be writable by others,
//! nor by their group unless that group cannot hold anyone but this user
//! (the rule of the task file, see `accounts`). The folder is checked before
//! the file is opened, and the file on the open handle it is read from.
//! Folders above that one, and access control lists, are not looked at: this
//! command makes the folder 0700 and the file 0600 itself.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::accounts::AccountDb;
use crate::decide::{Place, Remembered};

/// The version of the choices file this reader knows.
pub const CHOICES_VERSION: u32 = 1;

/// The variable that moves the folder (tests always point it at a temporary one).
pub const DIR_VARIABLE: &str = "ISTESSO_ROUTE_DIR";

const FILE_NAME: &str = "choices.json";

/// `ISTESSO_ROUTE_DIR`, or the platform's place for settings + "istesso-route".
pub fn choices_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DIR_VARIABLE).filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if cfg!(target_os = "macos") {
        return home.join("Library/Application Support/istesso-route");
    }
    match std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir.join("istesso-route"),
        _ => home.join(".config/istesso-route"),
    }
}

/// The file in [`choices_dir`].
pub fn choices_path() -> PathBuf {
    choices_dir().join(FILE_NAME)
}

#[derive(Debug)]
pub enum ChoicesError {
    Io {
        path: PathBuf,
        why: String,
    },
    Unreadable {
        path: PathBuf,
        why: String,
    },
    Version {
        path: PathBuf,
        found: String,
        known: u32,
    },
    /// Someone else could have written the file (see the module's text).
    Untrusted {
        path: PathBuf,
        why: String,
    },
}

impl fmt::Display for ChoicesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChoicesError::Io { path, why } => {
                write!(f, "the remembered choices at {} cannot be used: {why}", path.display())
            }
            ChoicesError::Unreadable { path, why } => write!(
                f,
                "the remembered choices at {} cannot be read ({why}); the file was left as it is",
                path.display()
            ),
            ChoicesError::Version { path, found, known } => write!(
                f,
                "the remembered choices at {} are of version {found}; this version of istesso-route reads version {known}; the file was left as it is",
                path.display()
            ),
            ChoicesError::Untrusted { path, why } => write!(
                f,
                "the remembered choices at {} are not used: {why}, and a remembered choice sends a deploy task to its machine without asking; the file was left as it is",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ChoicesError {}

/// How a project is told apart in the choices.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectKey {
    /// The name the task file gives (`project`).
    Name(String),
    /// The project's root, absolute with links followed.
    Location(PathBuf),
}

impl ProjectKey {
    /// The name when the task file gives one, else the location.
    pub fn of(name: Option<&str>, root: &Path) -> ProjectKey {
        match name {
            Some(name) => ProjectKey::Name(name.to_owned()),
            None => ProjectKey::Location(root.to_path_buf()),
        }
    }
}

impl fmt::Display for ProjectKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProjectKey::Name(name) => write!(f, "the project `{name}`"),
            ProjectKey::Location(path) => write!(f, "the project at {}", path.display()),
        }
    }
}

/// One remembered choice, as `istesso-route choices` lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub project: ProjectKey,
    pub task: String,
    pub remembered: Remembered,
}

/// The remembered choices, with the file kept as JSON so that unknown items
/// survive a rewrite.
#[derive(Debug, Clone)]
pub struct Choices {
    path: PathBuf,
    doc: Map<String, Value>,
}

/// The entry's project: by name when it has one, else by location.
fn key_of(entry: &Value) -> Option<ProjectKey> {
    if let Some(name) = entry.get("project_name") {
        return name.as_str().map(|n| ProjectKey::Name(n.to_owned()));
    }
    entry
        .get("project")
        .and_then(Value::as_str)
        .map(|p| ProjectKey::Location(PathBuf::from(p)))
}

fn matches(entry: &Value, project: &ProjectKey, task: &str) -> bool {
    key_of(entry).as_ref() == Some(project)
        && entry.get("task").and_then(Value::as_str) == Some(task)
}

fn new_entry(project: &ProjectKey, task: &str) -> Value {
    match project {
        ProjectKey::Name(name) => json!({ "project_name": name, "task": task }),
        ProjectKey::Location(path) => {
            json!({ "project": path.to_string_lossy(), "task": task })
        }
    }
}

fn remembered_of(entry: &Value) -> Option<Remembered> {
    Some(Remembered {
        host: entry.get("host")?.as_str()?.to_owned(),
        name: entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        chosen_at: entry.get("chosen_at").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// The file's text, read from the handle that was checked; `None` when there
/// is no file. What is checked is in the module's text.
fn read_trusted(path: &Path, accounts: &AccountDb) -> Result<Option<String>, ChoicesError> {
    use std::io::Read;
    let io = |why: String| ChoicesError::Io {
        path: path.to_path_buf(),
        why,
    };
    let untrusted = |why: String| ChoicesError::Untrusted {
        path: path.to_path_buf(),
        why,
    };
    let missing = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;

    // The folder first: in a folder only this user can write to, nobody else
    // can put a file (or a link) in the file's place.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let dir = path
            .parent()
            .filter(|d| !d.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let meta = match std::fs::metadata(dir) {
            Ok(meta) => meta,
            Err(e) if missing(&e) => return Ok(None),
            Err(e) => return Err(io(e.to_string())),
        };
        if !meta.is_dir() {
            return Err(io(format!("{} is not a folder", dir.display())));
        }
        if meta.uid() != crate::find::effective_uid() && meta.uid() != 0 {
            return Err(untrusted(format!(
                "its folder {} belongs to another user",
                dir.display()
            )));
        }
        if let Some(who) = writers(&meta, accounts) {
            return Err(untrusted(format!(
                "{who} can write to its folder {} (`chmod go-w` it if that is not meant)",
                dir.display()
            )));
        }
    }

    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if missing(&e) => return Ok(None),
        Err(e) => return Err(io(e.to_string())),
    };
    let meta = file.metadata().map_err(|e| io(e.to_string()))?;
    if !meta.is_file() {
        return Err(io("it is not a regular file".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != crate::find::effective_uid() {
            return Err(untrusted("it belongs to another user".into()));
        }
        if let Some(who) = writers(&meta, accounts) {
            return Err(untrusted(format!(
                "{who} can write to it (`chmod go-w` it if that is not meant)"
            )));
        }
    }
    #[cfg(not(unix))]
    let _ = accounts;
    let mut text = String::new();
    file.read_to_string(&mut text)
        .map_err(|e| io(e.to_string()))?;
    Ok(Some(text))
}

/// Who besides this user can write to an item: others, or a group that may
/// hold someone else (see `accounts`); `None` when nobody can.
#[cfg(unix)]
fn writers(meta: &std::fs::Metadata, accounts: &AccountDb) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    if meta.mode() & 0o002 != 0 {
        return Some("others".into());
    }
    if meta.mode() & 0o020 != 0 {
        if let Err(why) = accounts.is_private(meta.gid(), crate::find::effective_uid()) {
            return Some(format!(
                "the group `{}`, which may hold other users ({why}),",
                accounts.group_name(meta.gid())
            ));
        }
    }
    None
}

impl Choices {
    /// A missing file is an empty list. A file that cannot be read, one of
    /// an unknown version, or one that someone else could have written, is
    /// an error; it is never overwritten.
    pub fn load(path: &Path) -> Result<Choices, ChoicesError> {
        Choices::load_with(path, &AccountDb::system())
    }

    /// [`Choices::load`] with a given account lookup (tests).
    pub(crate) fn load_with(path: &Path, accounts: &AccountDb) -> Result<Choices, ChoicesError> {
        let Some(text) = read_trusted(path, accounts)? else {
            let mut doc = Map::new();
            doc.insert("version".into(), json!(CHOICES_VERSION));
            doc.insert("choices".into(), json!([]));
            return Ok(Choices {
                path: path.to_path_buf(),
                doc,
            });
        };
        let unreadable = |why: String| ChoicesError::Unreadable {
            path: path.to_path_buf(),
            why,
        };
        let value: Value = serde_json::from_str(&text).map_err(|e| unreadable(e.to_string()))?;
        let Value::Object(doc) = value else {
            return Err(unreadable("the top level is not an object".into()));
        };
        match doc.get("version") {
            Some(v) if v.as_u64() == Some(u64::from(CHOICES_VERSION)) => {}
            Some(v) => {
                return Err(ChoicesError::Version {
                    path: path.to_path_buf(),
                    found: v.to_string(),
                    known: CHOICES_VERSION,
                })
            }
            None => return Err(unreadable("it has no version number".into())),
        }
        match doc.get("choices") {
            Some(Value::Array(_)) => {}
            None => {}
            Some(_) => return Err(unreadable("`choices` is not a list".into())),
        }
        let mut choices = Choices {
            path: path.to_path_buf(),
            doc,
        };
        choices.list_mut();
        Ok(choices)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    fn list(&self) -> &[Value] {
        self.doc
            .get("choices")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    fn list_mut(&mut self) -> &mut Vec<Value> {
        let entry = self
            .doc
            .entry("choices")
            .or_insert_with(|| Value::Array(Vec::new()));
        if !entry.is_array() {
            *entry = Value::Array(Vec::new());
        }
        entry.as_array_mut().expect("just made a list")
    }

    pub fn get(&self, project: &ProjectKey, task: &str) -> Option<Remembered> {
        self.list()
            .iter()
            .filter(|e| matches(e, project, task))
            .find_map(remembered_of)
    }

    /// Remembers `place` for the task, replacing an earlier choice. Unknown
    /// items of an earlier entry are kept.
    pub fn set(&mut self, project: &ProjectKey, task: &str, place: &Place, now: u64) {
        let list = self.list_mut();
        let entry = match list.iter_mut().position(|e| matches(e, project, task)) {
            Some(i) => &mut list[i],
            None => {
                list.push(new_entry(project, task));
                list.last_mut().expect("just pushed")
            }
        };
        if let Value::Object(fields) = entry {
            fields.insert("host".into(), json!(place.id));
            fields.insert("name".into(), json!(place.name));
            fields.insert("chosen_at".into(), json!(now));
        }
    }

    /// Forgets the task's choice. False when there was none.
    pub fn forget(&mut self, project: &ProjectKey, task: &str) -> bool {
        let list = self.list_mut();
        let before = list.len();
        list.retain(|e| !matches(e, project, task));
        list.len() != before
    }

    /// Every remembered choice, in the file's order.
    pub fn all(&self) -> Vec<Choice> {
        self.list()
            .iter()
            .filter_map(|e| {
                Some(Choice {
                    project: key_of(e)?,
                    task: e.get("task")?.as_str()?.to_owned(),
                    remembered: remembered_of(e)?,
                })
            })
            .collect()
    }

    /// Writes a new file next to the old one and renames it; owner-only (0600).
    pub fn save(&self) -> Result<(), ChoicesError> {
        let io = |e: std::io::Error| ChoicesError::Io {
            path: self.path.clone(),
            why: e.to_string(),
        };
        if let Some(dir) = self.path.parent().filter(|d| !d.as_os_str().is_empty()) {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(dir).map_err(io)?;
        }
        let mut text = serde_json::to_string_pretty(&self.doc).map_err(|e| ChoicesError::Io {
            path: self.path.clone(),
            why: e.to_string(),
        })?;
        text.push('\n');

        let mut temporary = self.path.clone().into_os_string();
        temporary.push(format!(".{}.new", std::process::id()));
        let temporary = PathBuf::from(temporary);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let written = (|| {
            use std::io::Write;
            let mut file = options.open(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)
        })();
        if written.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        written.map_err(io)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir = std::env::temp_dir().join(format!(
                "istesso-route-choices-{}-{name}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            TempDir(dir)
        }
        fn file(&self) -> PathBuf {
            self.0.join("settings").join(FILE_NAME)
        }
        /// Writes the file as a person might, with the modes this command
        /// would give it whatever the umask (a group-writable one is judged
        /// by the system's accounts, which the tests do not depend on).
        fn write_by_hand(&self, text: &str) {
            let file = self.file();
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            set_mode(file.parent().unwrap(), 0o700);
            std::fs::write(&file, text).unwrap();
            set_mode(&file, 0o600);
        }
    }

    fn set_mode(path: &Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let _ = (path, mode);
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn place(name: &str, id: &str) -> Place {
        Place {
            id: id.into(),
            name: name.into(),
            local: false,
        }
    }

    const PROJECT: &str = "/example/project";

    fn at(path: &str) -> ProjectKey {
        ProjectKey::Location(PathBuf::from(path))
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        let dir = TempDir::new("missing");
        let choices = Choices::load(&dir.file()).unwrap();
        assert!(choices.all().is_empty());
        assert!(choices.get(&at(PROJECT), "site").is_none());
        assert!(!dir.file().exists());
    }

    #[test]
    fn choose_choose_again_and_forget() {
        let dir = TempDir::new("cycle");
        let project = &at(PROJECT);
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(project, "site", &place("tower", "id-tower"), 100);
        choices.save().unwrap();

        let mut choices = Choices::load(&dir.file()).unwrap();
        let got = choices.get(project, "site").unwrap();
        assert_eq!(
            (got.host.as_str(), got.name.as_str(), got.chosen_at),
            ("id-tower", "tower", 100)
        );

        choices.set(project, "site", &place("mini", "id-mini"), 200);
        choices.save().unwrap();
        let mut choices = Choices::load(&dir.file()).unwrap();
        assert_eq!(choices.all().len(), 1);
        assert_eq!(choices.get(project, "site").unwrap().host, "id-mini");

        assert!(choices.forget(project, "site"));
        assert!(!choices.forget(project, "site"));
        choices.save().unwrap();
        assert!(Choices::load(&dir.file())
            .unwrap()
            .get(project, "site")
            .is_none());
    }

    #[test]
    fn projects_do_not_mix() {
        let dir = TempDir::new("projects");
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(&at("/example/one"), "site", &place("tower", "id-tower"), 1);
        choices.set(&at("/example/two"), "site", &place("mini", "id-mini"), 2);
        assert_eq!(
            choices.get(&at("/example/one"), "site").unwrap().name,
            "tower"
        );
        assert_eq!(
            choices.get(&at("/example/two"), "site").unwrap().name,
            "mini"
        );
        assert!(choices.get(&at("/example/three"), "site").is_none());
        assert!(choices.forget(&at("/example/one"), "site"));
        assert!(choices.get(&at("/example/two"), "site").is_some());
    }

    #[test]
    fn unknown_items_survive_a_rewrite() {
        let dir = TempDir::new("unknown");
        dir.write_by_hand(
            r#"{"version": 1, "note": "kept",
                "choices": [{"project": "/example/project", "task": "site", "host": "id-tower",
                             "name": "tower", "chosen_at": 5, "why": "kept too"},
                            {"strange": true}]}"#,
        );
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(&at(PROJECT), "site", &place("mini", "id-mini"), 9);
        choices.save().unwrap();
        let text = std::fs::read_to_string(dir.file()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["note"], "kept");
        assert_eq!(value["choices"][0]["why"], "kept too");
        assert_eq!(value["choices"][0]["host"], "id-mini");
        assert_eq!(value["choices"][1]["strange"], true);
    }

    #[test]
    fn unknown_versions_and_broken_files_are_errors_and_left_alone() {
        let dir = TempDir::new("broken");
        for text in [
            r#"{"version": 2, "choices": []}"#,
            r#"{"choices": []}"#,
            r#"{"version": 1, "choices": {}}"#,
            "{ not json",
            "[]",
        ] {
            dir.write_by_hand(text);
            let err = Choices::load(&dir.file()).unwrap_err();
            assert!(err.to_string().contains("left as it is"), "{text}: {err}");
            assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), text);
        }
        dir.write_by_hand(r#"{"version": 2}"#);
        let err = Choices::load(&dir.file()).unwrap_err();
        assert!(matches!(err, ChoicesError::Version { .. }));
        assert!(err.to_string().contains("version 2"));
    }

    #[test]
    fn a_file_written_by_hand_is_read() {
        let dir = TempDir::new("hand");
        dir.write_by_hand(
            r#"{"version":1,"choices":[{"project":"/example/project","task":"site","host":"9f8e-7d6c-5b4a-3921"}]}"#,
        );
        let choices = Choices::load(&dir.file()).unwrap();
        let got = choices.get(&at(PROJECT), "site").unwrap();
        assert_eq!(got.host, "9f8e-7d6c-5b4a-3921");
        assert_eq!(got.chosen_at, 0);
        assert_eq!(choices.all().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("mode");
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(&at(PROJECT), "site", &place("tower", "id-tower"), 1);
        choices.save().unwrap();
        let mode = std::fs::metadata(dir.file()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let leftovers = std::fs::read_dir(dir.file().parent().unwrap())
            .unwrap()
            .count();
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn the_folder_follows_the_variable() {
        // Only reads the variable; another test may run at the same time,
        // so this one does not change it.
        if let Some(dir) = std::env::var_os(DIR_VARIABLE).filter(|d| !d.is_empty()) {
            assert_eq!(choices_dir(), PathBuf::from(dir));
        }
        assert!(choices_path().ends_with("choices.json"));
    }

    #[test]
    fn a_named_project_is_shared_by_its_worktrees() {
        let dir = TempDir::new("named");
        let name = ProjectKey::of(Some("example"), Path::new("/example/worktree-a"));
        assert_eq!(name, ProjectKey::Name("example".into()));
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(&name, "site", &place("tower", "id-tower"), 1);
        choices.save().unwrap();
        let choices = Choices::load(&dir.file()).unwrap();
        // Another worktree of the same project finds it by the name.
        let other = ProjectKey::of(Some("example"), Path::new("/example/worktree-b"));
        assert_eq!(choices.get(&other, "site").unwrap().host, "id-tower");
        // A project without a name, at either location, does not.
        assert!(choices.get(&at("/example/worktree-a"), "site").is_none());
        assert!(choices
            .get(&ProjectKey::Name("another".into()), "site")
            .is_none());
        let text = std::fs::read_to_string(dir.file()).unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["choices"][0]["project_name"], "example");
        assert!(value["choices"][0].get("project").is_none());
        assert_eq!(choices.all()[0].project, ProjectKey::Name("example".into()));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_could_have_written_is_not_used_and_left_alone() {
        let dir = TempDir::new("writers");
        let folder = dir.file().parent().unwrap().to_path_buf();
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(&at(PROJECT), "site", &place("tower", "id-tower"), 1);
        choices.save().unwrap();
        let text = std::fs::read_to_string(dir.file()).unwrap();

        set_mode(&dir.file(), 0o602);
        let err = Choices::load(&dir.file()).unwrap_err();
        assert!(matches!(err, ChoicesError::Untrusted { .. }), "{err}");
        let message = err.to_string();
        assert!(
            message.contains("are not used: others can write to it"),
            "{message}"
        );
        assert!(message.contains("left as it is"), "{message}");
        set_mode(&dir.file(), 0o600);

        // The folder is looked at before the file is opened; a missing file
        // in it is no empty list either, since a choice saved there could be
        // replaced.
        set_mode(&folder, 0o703);
        let err = Choices::load(&dir.file()).unwrap_err();
        assert!(matches!(err, ChoicesError::Untrusted { .. }), "{err}");
        assert!(
            err.to_string().contains("others can write to its folder"),
            "{err}"
        );
        let err = Choices::load(&folder.join("other.json")).unwrap_err();
        assert!(matches!(err, ChoicesError::Untrusted { .. }), "{err}");
        set_mode(&folder, 0o700);

        assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), text);
        let choices = Choices::load(&dir.file()).unwrap();
        assert_eq!(choices.get(&at(PROJECT), "site").unwrap().host, "id-tower");
    }

    #[cfg(unix)]
    #[test]
    fn a_group_may_write_only_when_it_holds_nobody_else() {
        use std::os::unix::fs::MetadataExt;
        let dir = TempDir::new("group");
        let mut choices = Choices::load(&dir.file()).unwrap();
        choices.set(&at(PROJECT), "site", &place("tower", "id-tower"), 1);
        choices.save().unwrap();
        set_mode(&dir.file(), 0o620);

        // A lookup that fails: every group counts as one that may hold others.
        let err = Choices::load_with(&dir.file(), &AccountDb::default()).unwrap_err();
        assert!(matches!(err, ChoicesError::Untrusted { .. }), "{err}");
        assert!(err.to_string().contains("may hold other users"), "{err}");

        let gid = std::fs::metadata(dir.file()).unwrap().gid();
        let private = crate::accounts::tests::private_db(crate::find::effective_uid(), gid);
        let choices = Choices::load_with(&dir.file(), &private).unwrap();
        assert!(choices.get(&at(PROJECT), "site").is_some());
    }

    #[test]
    fn a_folder_in_the_files_place_cannot_be_used() {
        let dir = TempDir::new("folder");
        std::fs::create_dir_all(dir.file()).unwrap();
        set_mode(dir.file().parent().unwrap(), 0o700);
        let err = Choices::load(&dir.file()).unwrap_err();
        assert!(matches!(err, ChoicesError::Io { .. }), "{err}");
    }
}
