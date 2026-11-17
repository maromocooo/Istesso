//! The file a project states its tasks and their needs in: `.istesso/tasks.json`.
//!
//! The file carries a version number. A reader refuses a version it does not
//! know and shows both numbers. Unknown items at the top and inside a task
//! are skipped and remembered; a task of an unknown kind, or one whose shape
//! cannot be read, is skipped alone. Inside `needs` nothing is skipped
//! silently: a need this version does not know keeps the task from being
//! decided, because dropping it would send the task to a machine that may
//! not meet it.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use istesso_proto::{OsFamily, Role};
use serde_json::{Map, Value};

/// The version of the task file this reader knows.
pub const TASKS_VERSION: u32 = 1;

/// Where the task file lives, relative to the project's root.
pub const TASKS_FILE: &str = ".istesso/tasks.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskKind {
    Build,
    Run,
    Deploy,
}

impl TaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Build => "build",
            TaskKind::Run => "run",
            TaskKind::Deploy => "deploy",
        }
    }

    pub fn parse(text: &str) -> Option<TaskKind> {
        match text {
            "build" => Some(TaskKind::Build),
            "run" => Some(TaskKind::Run),
            "deploy" => Some(TaskKind::Deploy),
            _ => None,
        }
    }
}

impl fmt::Display for TaskKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Developer tools the role table reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Cargo,
    Docker,
    Gstreamer,
    Xcode,
}

impl Tool {
    pub fn as_str(self) -> &'static str {
        match self {
            Tool::Cargo => "cargo",
            Tool::Docker => "docker",
            Tool::Gstreamer => "gstreamer",
            Tool::Xcode => "xcode",
        }
    }

    pub fn parse(text: &str) -> Option<Tool> {
        match text {
            "cargo" => Some(Tool::Cargo),
            "docker" => Some(Tool::Docker),
            "gstreamer" => Some(Tool::Gstreamer),
            "xcode" => Some(Tool::Xcode),
            _ => None,
        }
    }
}

/// What a task needs from the machine it runs on. Every field narrows the
/// choice; an empty `Needs` is met by any machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    /// All of them.
    pub roles: Vec<Role>,
    pub os: Option<OsFamily>,
    /// As the role table writes it, e.g. "aarch64".
    pub arch: Option<String>,
    pub tools: Vec<Tool>,
    /// A machine the file names, by id or by name. Nothing else is considered.
    pub host: Option<String>,
}

impl Needs {
    /// True when nothing but (at most) the named machine is asked for, so a
    /// machine whose role table is not known can still meet the needs.
    pub fn asks_nothing_of_the_table(&self) -> bool {
        self.roles.is_empty() && self.os.is_none() && self.arch.is_none() && self.tools.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub name: String,
    pub kind: TaskKind,
    pub needs: Needs,
    /// Needs this version does not know, as they were written ("roles: quantum",
    /// "gpu_memory"). A task that has any is never decided.
    pub unknown_needs: Vec<String>,
    /// Program and arguments; no shell. Empty: the task can be asked about,
    /// not run.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskFile {
    pub tasks: BTreeMap<String, Task>,
    /// What the reader skipped and why: unknown keys, tasks of an unknown
    /// kind, tasks whose shape it cannot read.
    pub skipped: Vec<Skipped>,
}

impl TaskFile {
    /// Why the task of this name was skipped, if it was.
    pub fn skipped_task(&self, name: &str) -> Option<&Skipped> {
        self.skipped
            .iter()
            .find(|s| s.task.as_deref() == Some(name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// What was skipped, e.g. "the key `comment`" or "the task `x`".
    pub what: String,
    pub why: String,
    /// For a skipped task, its name. `None` for a skipped key.
    pub task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TasksError {
    NotJson(String),
    /// No `version`, or one that is not a whole number.
    NoVersion,
    /// The file is of a version this reader does not know.
    Version {
        found: u64,
        known: u32,
    },
    NoTasks,
}

impl fmt::Display for TasksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TasksError::NotJson(why) => write!(f, "the task file is not a JSON object: {why}"),
            TasksError::NoVersion => {
                f.write_str("the task file has no version number (`version` must be a whole number)")
            }
            TasksError::Version { found, known } => write!(
                f,
                "the task file is of version {found}; this version of istesso-route reads version {known}"
            ),
            TasksError::NoTasks => f.write_str("the task file has no `tasks` object"),
        }
    }
}

impl std::error::Error for TasksError {}

/// Reads a task file.
pub fn read_tasks(text: &str) -> Result<TaskFile, TasksError> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| TasksError::NotJson(e.to_string()))?;
    let Value::Object(top) = value else {
        return Err(TasksError::NotJson("the top level is not an object".into()));
    };
    let version = match top.get("version") {
        None => return Err(TasksError::NoVersion),
        Some(v) => v.as_u64().ok_or(TasksError::NoVersion)?,
    };
    if version != u64::from(TASKS_VERSION) {
        return Err(TasksError::Version {
            found: version,
            known: TASKS_VERSION,
        });
    }
    let Some(Value::Object(entries)) = top.get("tasks") else {
        return Err(TasksError::NoTasks);
    };

    let mut skipped = Vec::new();
    for key in top.keys() {
        if key != "version" && key != "tasks" {
            skipped.push(Skipped {
                what: format!("the key `{key}`"),
                why: "this version does not know it".into(),
                task: None,
            });
        }
    }

    let mut tasks = BTreeMap::new();
    for (name, entry) in entries {
        match read_task(name, entry, &mut skipped) {
            Ok(task) => {
                tasks.insert(name.clone(), task);
            }
            Err(why) => skipped.push(Skipped {
                what: format!("the task `{name}`"),
                why,
                task: Some(name.clone()),
            }),
        }
    }
    Ok(TaskFile { tasks, skipped })
}

const TASK_KEYS: [&str; 3] = ["kind", "needs", "command"];

fn read_task(name: &str, entry: &Value, skipped: &mut Vec<Skipped>) -> Result<Task, String> {
    let Value::Object(fields) = entry else {
        return Err("it is not an object".into());
    };
    let kind = match fields.get("kind") {
        None => return Err("it has no `kind`".into()),
        Some(Value::String(k)) => TaskKind::parse(k)
            .ok_or_else(|| format!("its kind `{k}` is not known to this version"))?,
        Some(_) => return Err("its `kind` is not a string".into()),
    };
    let command = match fields.get("command") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| i.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()
            .ok_or("its `command` is not a list of strings")?,
        Some(_) => return Err("its `command` is not a list of strings".into()),
    };
    let (needs, unknown_needs) = match fields.get("needs") {
        None => (Needs::default(), Vec::new()),
        Some(Value::Object(needs)) => read_needs(needs),
        Some(_) => return Err("its `needs` is not an object".into()),
    };
    for key in fields.keys() {
        if !TASK_KEYS.contains(&key.as_str()) {
            skipped.push(Skipped {
                what: format!("the key `{key}` of the task `{name}`"),
                why: "this version does not know it".into(),
                task: None,
            });
        }
    }
    Ok(Task {
        name: name.to_owned(),
        kind,
        needs,
        unknown_needs,
        command,
    })
}

/// Reads `needs`. Anything it cannot use goes into the second list, in words.
fn read_needs(fields: &Map<String, Value>) -> (Needs, Vec<String>) {
    let mut needs = Needs::default();
    let mut unknown = Vec::new();
    for (key, value) in fields {
        match key.as_str() {
            "roles" => match string_list(value) {
                Some(items) => {
                    for item in items {
                        match parse_role(item) {
                            Some(role) => needs.roles.push(role),
                            None => unknown.push(format!("roles: {item}")),
                        }
                    }
                }
                None => unknown.push("roles: not a list of strings".into()),
            },
            "tools" => match string_list(value) {
                Some(items) => {
                    for item in items {
                        match Tool::parse(item) {
                            Some(tool) => needs.tools.push(tool),
                            None => unknown.push(format!("tools: {item}")),
                        }
                    }
                }
                None => unknown.push("tools: not a list of strings".into()),
            },
            "os" => match value.as_str() {
                Some(text) => match parse_os(text) {
                    Some(os) => needs.os = Some(os),
                    None => unknown.push(format!("os: {text}")),
                },
                None => unknown.push("os: not a string".into()),
            },
            "arch" => match value.as_str() {
                Some(text) => needs.arch = Some(text.to_owned()),
                None => unknown.push("arch: not a string".into()),
            },
            "host" => match value.as_str() {
                Some(text) => needs.host = Some(text.to_owned()),
                None => unknown.push("host: not a string".into()),
            },
            other => unknown.push(other.to_owned()),
        }
    }
    (needs, unknown)
}

fn string_list(value: &Value) -> Option<Vec<&str>> {
    value.as_array()?.iter().map(Value::as_str).collect()
}

/// A role as the role table writes it; `None` for one this version does not know.
pub fn parse_role(text: &str) -> Option<Role> {
    serde_json::from_value(Value::String(text.to_owned())).ok()
}

/// The name the role table uses for a role.
pub fn role_name(role: Role) -> String {
    match serde_json::to_value(role) {
        Ok(Value::String(s)) => s,
        _ => format!("{role:?}"),
    }
}

/// An OS family as the role table writes it. The catch-all `unknown` is not
/// a family a task can ask for.
fn parse_os(text: &str) -> Option<OsFamily> {
    match serde_json::from_value(Value::String(text.to_owned())) {
        Ok(OsFamily::Unknown) | Err(_) => None,
        Ok(os) => Some(os),
    }
}

/// The name the role table uses for an OS family.
pub fn os_name(os: OsFamily) -> String {
    match serde_json::to_value(os) {
        Ok(Value::String(s)) => s,
        _ => format!("{os:?}"),
    }
}

/// Why a task file that was found is not used. The file names commands that
/// `run`, `build` and `deploy` start with this user's rights, so a file that
/// someone else could have written is refused, never skipped silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FindError {
    /// The file or its `.istesso` folder belongs to another user.
    NotOwned { path: PathBuf },
    /// The file or its `.istesso` folder can be written by its group or by others.
    Writable { path: PathBuf },
    /// The file or its `.istesso` folder is a link that leads out of the project.
    OutsideProject { path: PathBuf, project: PathBuf },
    /// The file or its folder cannot be looked at (e.g. a broken link).
    Unreadable { path: PathBuf, why: String },
}

impl fmt::Display for FindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FindError::NotOwned { path } => write!(
                f,
                "{} is not used: it belongs to another user, and its commands would run as you",
                path.display()
            ),
            FindError::Writable { path } => write!(
                f,
                "{} is not used: others can write to it, and its commands would run as you (`chmod go-w` it if that is not meant)",
                path.display()
            ),
            FindError::OutsideProject { path, project } => write!(
                f,
                "{} is not used: it is a link that leads out of the project {}",
                path.display(),
                project.display()
            ),
            FindError::Unreadable { path, why } => {
                write!(f, "{} cannot be looked at: {why}", path.display())
            }
        }
    }
}

impl std::error::Error for FindError {}

/// Looks for `.istesso/tasks.json` from `start` up to the project's
/// boundary, with the user's home folder from `HOME` as the upper limit.
/// See [`find_tasks_within`].
pub fn find_tasks(start: &Path) -> Result<Option<(PathBuf, PathBuf)>, FindError> {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from);
    find_tasks_within(start, home.as_deref())
}

/// Looks for `.istesso/tasks.json` in `start` and its parents, but not past
/// the project's boundary. Returns the file and the project's root (the
/// folder that holds `.istesso`), both canonical.
///
/// - The boundary is the first folder from `start` upwards that holds `.git`
///   (a file or a folder). Without one, only `start` itself is looked at.
/// - The search for `.git` never goes above `home`, and never reaches the
///   root of the file system.
/// - `start` is canonicalized once; the file and its `.istesso` folder may
///   be links only when they lead to a place inside the boundary.
/// - On Unix, the file and its `.istesso` folder must belong to the user
///   this runs as, and must not be writable by their group or by others.
///   Elsewhere ownership is not checked; the boundary and links still are.
pub fn find_tasks_within(
    start: &Path,
    home: Option<&Path>,
) -> Result<Option<(PathBuf, PathBuf)>, FindError> {
    let start = start.canonicalize().map_err(|e| FindError::Unreadable {
        path: start.to_path_buf(),
        why: e.to_string(),
    })?;
    let home = home.and_then(|h| h.canonicalize().ok());
    let Some(boundary) = project_boundary(&start, home.as_deref()) else {
        return Ok(None);
    };
    for dir in start.ancestors() {
        if !dir.starts_with(&boundary) {
            break;
        }
        let folder = dir.join(".istesso");
        let file = dir.join(TASKS_FILE);
        if std::fs::symlink_metadata(&file).is_err() {
            continue;
        }
        let folder = inside(&folder, &boundary)?;
        let file = inside(&file, &boundary)?;
        trusted(&folder)?;
        trusted(&file)?;
        return Ok(Some((file, dir.to_path_buf())));
    }
    Ok(None)
}

/// The highest folder the search may look in: the first folder from `start`
/// upwards that holds `.git`, else `start` itself. `None` when `start` is
/// the root of the file system.
fn project_boundary(start: &Path, home: Option<&Path>) -> Option<PathBuf> {
    start.parent()?;
    for dir in start.ancestors() {
        if dir.parent().is_none() {
            break;
        }
        if std::fs::symlink_metadata(dir.join(".git")).is_ok() {
            return Some(dir.to_path_buf());
        }
        if Some(dir) == home {
            break;
        }
    }
    Some(start.to_path_buf())
}

/// The canonical place of `path`, which must lie inside `boundary`.
fn inside(path: &Path, boundary: &Path) -> Result<PathBuf, FindError> {
    let real = path.canonicalize().map_err(|e| FindError::Unreadable {
        path: path.to_path_buf(),
        why: e.to_string(),
    })?;
    if !real.starts_with(boundary) {
        return Err(FindError::OutsideProject {
            path: path.to_path_buf(),
            project: boundary.to_path_buf(),
        });
    }
    Ok(real)
}

/// Refuses a file or folder that another user owns or that others can write.
#[cfg(unix)]
fn trusted(path: &Path) -> Result<(), FindError> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).map_err(|e| FindError::Unreadable {
        path: path.to_path_buf(),
        why: e.to_string(),
    })?;
    if meta.uid() != effective_uid() {
        return Err(FindError::NotOwned {
            path: path.to_path_buf(),
        });
    }
    if meta.mode() & 0o022 != 0 {
        return Err(FindError::Writable {
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn trusted(_path: &Path) -> Result<(), FindError> {
    Ok(())
}

/// The user this process runs as.
#[cfg(unix)]
fn effective_uid() -> u32 {
    extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: geteuid(2) takes no arguments, always succeeds and touches no
    // memory of the caller. `uid_t` is a 32-bit unsigned integer on the
    // Unix systems this builds for (Linux, macOS).
    unsafe { geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAN_EXAMPLE: &str = r#"{
      "version": 1,
      "tasks": {
        "test":    { "kind": "build",  "needs": { "tools": ["cargo"] },
                     "command": ["cargo", "test", "--workspace"] },
        "mac-app": { "kind": "build",  "needs": { "roles": ["xcode_build"] },
                     "command": ["xcodebuild", "-scheme", "App"] },
        "encode":  { "kind": "run",    "needs": { "os": "linux", "roles": ["gpu", "hardware_encode"] },
                     "command": ["./tools/encode-check"] },
        "site":    { "kind": "deploy", "needs": { "roles": ["always_on_candidate"] },
                     "command": ["./tools/publish"] }
      }
    }"#;

    #[test]
    fn a_version_1_file_is_read() {
        let file = read_tasks(PLAN_EXAMPLE).unwrap();
        assert_eq!(file.tasks.len(), 4);
        assert!(file.skipped.is_empty());
        let encode = &file.tasks["encode"];
        assert_eq!(encode.kind, TaskKind::Run);
        assert_eq!(encode.needs.os, Some(OsFamily::Linux));
        assert_eq!(encode.needs.roles, vec![Role::Gpu, Role::HardwareEncode]);
        assert_eq!(encode.command, vec!["./tools/encode-check"]);
        assert_eq!(file.tasks["test"].needs.tools, vec![Tool::Cargo]);
        assert_eq!(file.tasks["site"].kind, TaskKind::Deploy);
        assert!(file.tasks.values().all(|t| t.unknown_needs.is_empty()));
    }

    #[test]
    fn files_without_a_known_version_are_refused() {
        assert_eq!(read_tasks(r#"{"tasks": {}}"#), Err(TasksError::NoVersion));
        assert_eq!(
            read_tasks(r#"{"version": "1", "tasks": {}}"#),
            Err(TasksError::NoVersion)
        );
        assert_eq!(
            read_tasks(r#"{"version": 1.5, "tasks": {}}"#),
            Err(TasksError::NoVersion)
        );
        for found in [0, 2] {
            let text = format!(r#"{{"version": {found}, "tasks": {{}}}}"#);
            let err = read_tasks(&text).unwrap_err();
            assert_eq!(err, TasksError::Version { found, known: 1 });
            let shown = err.to_string();
            assert!(shown.contains(&found.to_string()) && shown.contains("version 1"));
        }
    }

    #[test]
    fn other_broken_files_are_refused() {
        assert!(matches!(
            read_tasks("not json"),
            Err(TasksError::NotJson(_))
        ));
        assert!(matches!(read_tasks("[1, 2]"), Err(TasksError::NotJson(_))));
        assert_eq!(read_tasks(r#"{"version": 1}"#), Err(TasksError::NoTasks));
        assert_eq!(
            read_tasks(r#"{"version": 1, "tasks": []}"#),
            Err(TasksError::NoTasks)
        );
    }

    #[test]
    fn unknown_keys_are_skipped_and_remembered() {
        let file = read_tasks(
            r#"{"version": 1, "comment": "hi",
                "tasks": {"a": {"kind": "build", "timeout": 30, "command": ["true"]}}}"#,
        )
        .unwrap();
        assert_eq!(file.tasks["a"].command, vec!["true"]);
        assert_eq!(file.skipped.len(), 2);
        assert!(file.skipped.iter().any(|s| s.what.contains("`comment`")));
        assert!(file
            .skipped
            .iter()
            .any(|s| s.what == "the key `timeout` of the task `a`"));
        // An unknown key inside a task does not make the task skipped.
        assert!(file.skipped_task("a").is_none());
    }

    #[test]
    fn only_the_unreadable_tasks_are_skipped() {
        let file = read_tasks(
            r#"{"version": 1, "tasks": {
                "good":    {"kind": "run"},
                "teleport":{"kind": "teleport"},
                "nokind":  {"needs": {}},
                "badcmd":  {"kind": "build", "command": "make all"},
                "badcmd2": {"kind": "build", "command": ["make", 1]},
                "badneeds":{"kind": "build", "needs": ["cargo"]},
                "notobj":  3
            }}"#,
        )
        .unwrap();
        assert_eq!(file.tasks.keys().collect::<Vec<_>>(), vec!["good"]);
        assert!(file.tasks["good"].command.is_empty());
        for name in [
            "teleport", "nokind", "badcmd", "badcmd2", "badneeds", "notobj",
        ] {
            let skipped = file.skipped_task(name).unwrap_or_else(|| panic!("{name}"));
            assert!(skipped.what.contains(name));
            assert!(!skipped.why.is_empty());
        }
        assert!(file
            .skipped_task("teleport")
            .unwrap()
            .why
            .contains("teleport"));
    }

    #[test]
    fn unknown_needs_are_kept_not_dropped() {
        let file = read_tasks(
            r#"{"version": 1, "tasks": {"x": {"kind": "build", "needs": {
                "roles": ["gpu", "quantum"], "tools": ["cargo", "bazel"],
                "os": "beos", "gpu_memory": 8
            }}}}"#,
        )
        .unwrap();
        let task = &file.tasks["x"];
        assert_eq!(task.needs.roles, vec![Role::Gpu]);
        assert_eq!(task.needs.tools, vec![Tool::Cargo]);
        let mut unknown = task.unknown_needs.clone();
        unknown.sort();
        assert_eq!(
            unknown,
            vec!["gpu_memory", "os: beos", "roles: quantum", "tools: bazel"]
        );
        // The catch-all family is not something a task can ask for either.
        let file = read_tasks(
            r#"{"version": 1, "tasks": {"x": {"kind": "build", "needs": {"os": "unknown", "roles": "gpu"}}}}"#,
        )
        .unwrap();
        let mut unknown = file.tasks["x"].unknown_needs.clone();
        unknown.sort();
        assert_eq!(unknown, vec!["os: unknown", "roles: not a list of strings"]);
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(
            parse_role("always_on_candidate"),
            Some(Role::AlwaysOnCandidate)
        );
        assert_eq!(role_name(Role::XcodeBuild), "xcode_build");
        assert_eq!(os_name(OsFamily::Macos), "macos");
        assert_eq!(parse_os("linux"), Some(OsFamily::Linux));
    }

    /// A folder of its own under the temporary folder, removed afterwards.
    /// It also serves as `home`, so that nothing above it is ever looked at.
    struct Base(PathBuf);

    impl Base {
        fn new(name: &str) -> Base {
            let dir = std::env::temp_dir()
                .join(format!("istesso-route-find-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Base(dir.canonicalize().unwrap())
        }

        fn find(&self, start: &str) -> Result<Option<(PathBuf, PathBuf)>, FindError> {
            find_tasks_within(&self.0.join(start), Some(&self.0))
        }

        fn dir(&self, rel: impl AsRef<Path>) -> PathBuf {
            let rel = rel.as_ref();
            assert!(rel.is_relative());
            let dir = self.0.join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            set_mode(&dir, 0o755);
            dir
        }

        /// A task file in `rel`, owned by this user and writable by it alone.
        fn tasks(&self, rel: &str) -> PathBuf {
            let folder = self.dir(Path::new(rel).join(".istesso"));
            let file = folder.join("tasks.json");
            std::fs::write(&file, PLAN_EXAMPLE).unwrap();
            set_mode(&file, 0o644);
            file
        }
    }

    impl Drop for Base {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
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

    #[test]
    fn the_search_from_a_subfolder_finds_the_project_s_file() {
        let base = Base::new("sub");
        base.dir("proj/.git");
        let file = base.tasks("proj");
        base.dir("proj/a/b/c");
        let (found, root) = base.find("proj/a/b/c").unwrap().unwrap();
        assert_eq!(found, file);
        assert_eq!(root, base.0.join("proj"));
        assert!(root.is_absolute());
        // `.git` may be a file too (a worktree).
        let other = Base::new("sub-file");
        other.dir("proj");
        std::fs::write(other.0.join("proj/.git"), "gitdir: elsewhere\n").unwrap();
        other.tasks("proj");
        other.dir("proj/x");
        assert!(other.find("proj/x").unwrap().is_some());
    }

    #[test]
    fn a_file_above_the_project_root_is_not_taken() {
        let base = Base::new("above");
        base.tasks("outer");
        base.dir("outer/proj/.git");
        base.dir("outer/proj/sub");
        assert_eq!(base.find("outer/proj/sub").unwrap(), None);
        assert_eq!(base.find("outer/proj").unwrap(), None);
    }

    #[test]
    fn without_a_project_root_only_the_start_folder_is_looked_at() {
        let base = Base::new("norepo");
        base.tasks("work");
        base.dir("work/sub");
        assert_eq!(base.find("work/sub").unwrap(), None);
        assert!(base.find("work").unwrap().is_some());
    }

    #[test]
    fn the_search_stops_at_home() {
        // `.git` and a task file above home are not used.
        let base = Base::new("home");
        base.dir(".git");
        base.tasks("");
        base.dir("me/work");
        let home = base.0.join("me");
        assert_eq!(
            find_tasks_within(&home.join("work"), Some(&home)).unwrap(),
            None
        );
        // Home itself may be the project.
        base.dir("me/.git");
        base.tasks("me");
        assert!(find_tasks_within(&home.join("work"), Some(&home))
            .unwrap()
            .is_some());
    }

    #[test]
    fn the_root_of_the_file_system_is_never_a_project() {
        assert_eq!(project_boundary(Path::new("/"), None), None);
        assert_eq!(find_tasks_within(Path::new("/"), None).unwrap(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_write_is_refused() {
        let base = Base::new("writable");
        base.dir("proj/.git");
        let file = base.tasks("proj");
        assert!(base.find("proj").unwrap().is_some());

        set_mode(&file, 0o666);
        let err = base.find("proj").unwrap_err();
        assert_eq!(err, FindError::Writable { path: file.clone() });
        assert!(err.to_string().contains("others can write"));
        set_mode(&file, 0o664);
        assert!(matches!(base.find("proj"), Err(FindError::Writable { .. })));

        set_mode(&file, 0o644);
        set_mode(file.parent().unwrap(), 0o777);
        let err = base.find("proj").unwrap_err();
        assert_eq!(
            err,
            FindError::Writable {
                path: file.parent().unwrap().to_path_buf()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_file_owned_as_expected_is_taken() {
        use std::os::unix::fs::MetadataExt;
        let base = Base::new("owned");
        base.dir("proj/.git");
        let file = base.tasks("proj");
        assert_eq!(std::fs::metadata(&file).unwrap().uid(), effective_uid());
        assert!(trusted(&file).is_ok());
        assert_eq!(base.find("proj").unwrap().unwrap().0, file);
        // A file of another user (root owns `/`) is refused. Creating one
        // needs root, so only the check is tried here.
        if effective_uid() != 0 {
            assert_eq!(
                trusted(Path::new("/")),
                Err(FindError::NotOwned { path: "/".into() })
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_project_is_refused() {
        use std::os::unix::fs::symlink;
        let base = Base::new("link");
        let outside = base.tasks("elsewhere");
        base.dir("proj/.git");
        base.dir("proj/.istesso");
        symlink(&outside, base.0.join("proj/.istesso/tasks.json")).unwrap();
        assert!(matches!(
            base.find("proj"),
            Err(FindError::OutsideProject { .. })
        ));
        // The folder as a link out of the project.
        base.dir("proj2/.git");
        symlink(outside.parent().unwrap(), base.0.join("proj2/.istesso")).unwrap();
        assert!(matches!(
            base.find("proj2"),
            Err(FindError::OutsideProject { .. })
        ));
        // A link that stays inside the project is fine.
        base.dir("proj3/.git");
        let real = base.tasks("proj3/config");
        base.dir("proj3/.istesso");
        symlink(&real, base.0.join("proj3/.istesso/tasks.json")).unwrap();
        assert_eq!(base.find("proj3").unwrap().unwrap().0, real);
        // A broken link cannot be looked at.
        base.dir("proj4/.git");
        base.dir("proj4/.istesso");
        symlink(
            base.0.join("nothing"),
            base.0.join("proj4/.istesso/tasks.json"),
        )
        .unwrap();
        assert!(matches!(
            base.find("proj4"),
            Err(FindError::Unreadable { .. })
        ));
    }
}
