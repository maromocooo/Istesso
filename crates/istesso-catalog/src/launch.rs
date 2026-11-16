//! How an installed application is started.
//!
//! The catalogue says what is installed ([`crate::scan_local`]). This module
//! says how one entry is started as a program: which executable, with which
//! arguments, in which directory. A host that is asked for an application by
//! its id decides all of this from its own files, at the moment it starts
//! the application; nothing here is taken from another device.
//!
//! The rules for `Exec` are those of the Desktop Entry Specification. No
//! shell is involved.

use std::fmt;
use std::path::{Path, PathBuf};

use istesso_proto::{AppEntry, Launch};

/// A program to run for an application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// A path, or a name to look up in `PATH`.
    pub program: String,
    pub args: Vec<String>,
    pub working_dir: Option<PathBuf>,
}

/// Why an application cannot be started as a window of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotStartable {
    /// It runs inside a terminal (`Terminal=true`).
    Terminal,
    /// Its entry has no command that can be run.
    NoCommand(String),
    /// The program its entry names is not there (`TryExec`).
    Missing(String),
    /// Its entry is gone, or hidden, since the list was made.
    Gone,
    /// Starting this kind of application is not built yet.
    NotYet(&'static str),
}

impl fmt::Display for NotStartable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotStartable::Terminal => {
                f.write_str("the application runs in a terminal and has no window of its own")
            }
            NotStartable::NoCommand(why) => {
                write!(f, "the application's entry has no usable command ({why})")
            }
            NotStartable::Missing(program) => {
                write!(f, "the application's program is not installed ({program})")
            }
            NotStartable::Gone => {
                f.write_str("the application is no longer installed on this machine")
            }
            NotStartable::NotYet(what) => f.write_str(what),
        }
    }
}

/// How `app`, an entry of this machine's catalogue, is started here.
pub fn plan(app: &AppEntry) -> Result<LaunchPlan, NotStartable> {
    match &app.launch {
        Launch::DesktopEntry { desktop_id, .. } => {
            plan_desktop_id(&crate::linux::default_dirs(), desktop_id)
        }
        Launch::AppBundle { .. } => Err(NotStartable::NotYet(
            "showing a Mac application to another device is not built yet",
        )),
    }
}

/// Reads the desktop entry `desktop_id` again, from the first of `dirs`
/// that has it, and says how it is started.
pub fn plan_desktop_id(dirs: &[PathBuf], desktop_id: &str) -> Result<LaunchPlan, NotStartable> {
    // An id is a file name. Anything else could point out of the
    // directories.
    let is_file_name = !desktop_id.is_empty()
        && !desktop_id.starts_with('.')
        && !desktop_id.contains(['/', '\\', '\0']);
    if !is_file_name {
        return Err(NotStartable::Gone);
    }
    for dir in dirs {
        let file = dir.join(desktop_id);
        if let Ok(text) = std::fs::read_to_string(&file) {
            return plan_desktop_entry(&text, &file);
        }
    }
    Err(NotStartable::Gone)
}

/// Says how the application described by the text of a `.desktop` file is
/// started. `file` is where the text came from (the `%k` field code).
pub fn plan_desktop_entry(text: &str, file: &Path) -> Result<LaunchPlan, NotStartable> {
    let mut in_main_section = false;
    let mut entry_type = None;
    let mut exec = None;
    let mut try_exec = None;
    let mut working_dir = None;
    let mut name = String::new();
    let mut icon = None;
    let mut terminal = false;
    let mut hidden = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_main_section = line == "[Desktop Entry]";
            continue;
        }
        if !in_main_section {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let is_true = value.eq_ignore_ascii_case("true");
        match key.trim() {
            "Type" => entry_type = Some(value),
            "Exec" => exec = Some(value),
            "TryExec" => try_exec = Some(unescape(value)),
            "Path" if !value.is_empty() => working_dir = Some(PathBuf::from(unescape(value))),
            "Name" => name = unescape(value),
            "Icon" if !value.is_empty() => icon = Some(unescape(value)),
            "Terminal" => terminal = is_true,
            "Hidden" | "NoDisplay" if is_true => hidden = true,
            _ => {}
        }
    }

    if hidden {
        return Err(NotStartable::Gone);
    }
    if entry_type != Some("Application") {
        return Err(NotStartable::NoCommand("it is not an application".into()));
    }
    if terminal {
        return Err(NotStartable::Terminal);
    }
    if let Some(program) = try_exec.filter(|program| !program.is_empty()) {
        if !program_exists(&program) {
            return Err(NotStartable::Missing(program));
        }
    }
    let exec = exec.ok_or_else(|| NotStartable::NoCommand("no Exec line".into()))?;
    let mut words = parse_exec(exec, &name, icon.as_deref(), file)
        .map_err(NotStartable::NoCommand)?
        .into_iter();
    let program = words
        .next()
        .ok_or_else(|| NotStartable::NoCommand("the Exec line is empty".into()))?;
    Ok(LaunchPlan {
        program,
        args: words.collect(),
        working_dir,
    })
}

/// Undoes the escapes every string value of a desktop entry may carry:
/// `\s`, `\n`, `\t`, `\r` and `\\`.
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            // Not an escape the specification knows: left as written.
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Splits the value of an `Exec` key into the program and its arguments.
///
/// Arguments are separated by spaces. An argument in double quotes is taken
/// as it is, spaces included; inside the quotes a backslash protects `"`,
/// `` ` ``, `$` and `\`. Field codes stand for what the launcher fills in:
/// files and addresses to open (`%f`, `%F`, `%u`, `%U`; none here, so they
/// vanish), the icon (`%i`), the name (`%c`) and the entry's own file
/// (`%k`). `%%` is a percent sign.
pub fn parse_exec(
    value: &str,
    name: &str,
    icon: Option<&str>,
    file: &Path,
) -> Result<Vec<String>, String> {
    // The escapes of string values come off first, the quotes second.
    let value = unescape(value);

    // (text, whether any of it stood in quotes)
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut word: Option<(String, bool)> = None;
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => words.extend(word.take()),
            '"' => {
                let (text, quoted) = word.get_or_insert_with(Default::default);
                *quoted = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '`' | '$' | '\\')) => text.push(escaped),
                            Some(other) => {
                                text.push('\\');
                                text.push(other);
                            }
                            None => return Err("a quotation is not closed".into()),
                        },
                        Some(other) => text.push(other),
                        None => return Err("a quotation is not closed".into()),
                    }
                }
            }
            other => word.get_or_insert_with(Default::default).0.push(other),
        }
    }
    words.extend(word.take());

    let mut out = Vec::new();
    for (text, quoted) in words {
        // Field codes are not looked for inside quotes.
        if quoted {
            out.push(text);
            continue;
        }
        match text.as_str() {
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            "%i" => {
                if let Some(icon) = icon {
                    out.push("--icon".to_string());
                    out.push(icon.to_string());
                }
            }
            _ => {
                let mut filled = String::with_capacity(text.len());
                let mut chars = text.chars();
                while let Some(c) = chars.next() {
                    if c != '%' {
                        filled.push(c);
                        continue;
                    }
                    match chars.next() {
                        Some('%') => filled.push('%'),
                        Some('c') => filled.push_str(name),
                        Some('k') => filled.push_str(&file.to_string_lossy()),
                        // Files to open, and codes no longer in use: nothing.
                        _ => {}
                    }
                }
                out.push(filled);
            }
        }
    }
    if out.is_empty() {
        return Err("the Exec line is empty".into());
    }
    Ok(out)
}

/// True when `program` (a path, or a name in `PATH`) can be run.
fn program_exists(program: &str) -> bool {
    fn runnable(path: &Path) -> bool {
        let Ok(metadata) = std::fs::metadata(path) else {
            return false;
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            metadata.is_file()
        }
    }
    if program.contains('/') {
        return runnable(Path::new(program));
    }
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).any(|dir| runnable(&dir.join(program))))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(value: &str) -> Vec<String> {
        parse_exec(
            value,
            "Example",
            Some("example-icon"),
            Path::new("/apps/example.desktop"),
        )
        .unwrap()
    }

    fn entry(body: &str) -> Result<LaunchPlan, NotStartable> {
        plan_desktop_entry(
            &format!("[Desktop Entry]\nType=Application\nName=Example\n{body}\n"),
            Path::new("/apps/example.desktop"),
        )
    }

    #[test]
    fn a_plain_command_is_split_at_spaces() {
        assert_eq!(exec("firefox %u"), ["firefox"]);
        assert_eq!(exec("code   --new-window\t%F"), ["code", "--new-window"]);
        assert_eq!(
            exec("env FOO=bar app --flag"),
            ["env", "FOO=bar", "app", "--flag"]
        );
    }

    #[test]
    fn quoted_arguments_keep_their_spaces() {
        // Two spaces inside the quotes stay two spaces.
        assert_eq!(
            exec(r#""/opt/My  App/bin/app" --title "two  words" %U"#),
            ["/opt/My  App/bin/app", "--title", "two  words"]
        );
        // Quoted and unquoted parts that touch are one argument.
        assert_eq!(exec(r#"app --name="A B""#), ["app", "--name=A B"]);
        // An empty argument is an argument.
        assert_eq!(exec(r#"app "" last"#), ["app", "", "last"]);
    }

    #[test]
    fn backslashes_protect_inside_quotes() {
        // As written in a file: the string escape (\\ for \) comes off
        // first, then the quoting rule (\" for ").
        assert_eq!(
            exec(r#"app "say \\"hi\\" for \\$5""#),
            ["app", r#"say "hi" for $5"#]
        );
        assert_eq!(exec(r#"app "a\\\\b""#), ["app", r"a\b"]);
        // No shell: these are ordinary characters.
        assert_eq!(
            exec("app $HOME ; rm -rf *"),
            ["app", "$HOME", ";", "rm", "-rf", "*"]
        );
        assert!(parse_exec(r#"app "never closed"#, "", None, Path::new("")).is_err());
    }

    #[test]
    fn field_codes_are_filled_in_or_dropped() {
        assert_eq!(exec("echo 100%%"), ["echo", "100%"]);
        assert_eq!(exec("app %i"), ["app", "--icon", "example-icon"]);
        assert_eq!(exec("app --caption %c"), ["app", "--caption", "Example"]);
        assert_eq!(exec("app %k"), ["app", "/apps/example.desktop"]);
        // Nothing to open: the code leaves nothing behind.
        assert_eq!(exec("app --file=%f %d"), ["app", "--file="]);
        // Inside quotes a percent sign is a percent sign.
        assert_eq!(exec(r#"app "%f""#), ["app", "%f"]);
        // Without an icon there is nothing to pass.
        assert_eq!(
            parse_exec("app %i", "", None, Path::new("")).unwrap(),
            ["app"]
        );
        assert!(parse_exec("%f", "", None, Path::new("")).is_err());
    }

    #[test]
    fn an_entry_becomes_a_program_with_arguments() {
        assert_eq!(
            entry("Exec=blender --window-geometry 0 0 800 600 %f\nPath=/srv/projects"),
            Ok(LaunchPlan {
                program: "blender".into(),
                args: vec![
                    "--window-geometry".into(),
                    "0".into(),
                    "0".into(),
                    "800".into(),
                    "600".into()
                ],
                working_dir: Some(PathBuf::from("/srv/projects")),
            })
        );
        // Action groups after the main one do not count.
        let plan = entry("Exec=app\n\n[Desktop Action other]\nExec=other --thing").unwrap();
        assert_eq!(plan.program, "app");
        assert!(plan.args.is_empty());
        assert_eq!(plan.working_dir, None);
    }

    #[test]
    fn entries_that_cannot_be_windows_say_why() {
        assert_eq!(
            entry("Exec=htop\nTerminal=true"),
            Err(NotStartable::Terminal)
        );
        assert_eq!(entry("Exec=app\nHidden=true"), Err(NotStartable::Gone));
        assert!(matches!(entry(""), Err(NotStartable::NoCommand(_))));
        assert!(matches!(entry("Exec="), Err(NotStartable::NoCommand(_))));
        assert_eq!(
            entry("Exec=app\nTryExec=/nonexistent/istesso-test-program"),
            Err(NotStartable::Missing(
                "/nonexistent/istesso-test-program".into()
            ))
        );
        assert!(matches!(
            plan_desktop_entry(
                "[Desktop Entry]\nType=Link\nURL=https://example.com\n",
                Path::new("x")
            ),
            Err(NotStartable::NoCommand(_))
        ));
        // Every reason reads as a sentence for the person who asked.
        assert!(NotStartable::Terminal.to_string().contains("terminal"));
        assert!(NotStartable::Missing("x".into())
            .to_string()
            .contains("(x)"));
    }

    #[cfg(unix)]
    #[test]
    fn a_program_that_is_there_passes_the_check() {
        // `sh` is on every machine these tests run on.
        assert!(entry("Exec=sh -c true\nTryExec=sh").is_ok());
        assert!(entry("Exec=sh -c true\nTryExec=/bin/sh").is_ok());
    }

    #[test]
    fn an_entry_is_read_again_from_the_first_directory_that_has_it() {
        let root =
            std::env::temp_dir().join(format!("istesso-catalog-launch-{}", std::process::id()));
        let user = root.join("user");
        let system = root.join("system");
        std::fs::create_dir_all(&user).unwrap();
        std::fs::create_dir_all(&system).unwrap();
        let write = |dir: &Path, exec: &str| {
            std::fs::write(
                dir.join("tool.desktop"),
                format!("[Desktop Entry]\nType=Application\nName=Tool\nExec={exec}\n"),
            )
            .unwrap();
        };
        write(&system, "tool --system");
        let dirs = [user.clone(), system.clone(), root.join("missing")];
        assert_eq!(
            plan_desktop_id(&dirs, "tool.desktop").unwrap().args,
            ["--system"]
        );
        write(&user, "tool --user");
        assert_eq!(
            plan_desktop_id(&dirs, "tool.desktop").unwrap().args,
            ["--user"]
        );

        // Ids are file names; nothing else is looked up.
        assert_eq!(
            plan_desktop_id(&dirs, "other.desktop"),
            Err(NotStartable::Gone)
        );
        assert_eq!(
            plan_desktop_id(&dirs, "../user/tool.desktop"),
            Err(NotStartable::Gone)
        );
        assert_eq!(plan_desktop_id(&dirs, ""), Err(NotStartable::Gone));
        // A Mac application is not started for another device yet.
        let bundle = AppEntry {
            id: "com.example.app".into(),
            name: "Example".into(),
            platform: istesso_proto::OsFamily::Macos,
            version: None,
            launch: Launch::AppBundle {
                bundle_id: "com.example.app".into(),
                path: "/Applications/Example.app".into(),
            },
            icon: None,
            categories: Vec::new(),
        };
        assert!(matches!(plan(&bundle), Err(NotStartable::NotYet(_))));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
