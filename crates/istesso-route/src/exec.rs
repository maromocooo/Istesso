//! Carrying out a task at the place that was decided.
//!
//! Only this machine for now. Running on other machines waits for the
//! isolation (M5) and an ADR of its own; until then a task whose place is
//! another machine is not run at all.

use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::decide::Place;
use crate::tasks::Task;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecError {
    /// The place is another machine; nothing was run.
    Elsewhere,
    /// The task has no command; it can be asked about, not run.
    NoCommand,
    /// The program could not be started.
    Start { program: String, why: String },
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::Elsewhere => {
                f.write_str("the task goes to another machine; running there is not built yet")
            }
            ExecError::NoCommand => f.write_str("the task has no `command`; nothing to run"),
            ExecError::Start { program, why } => {
                write!(f, "`{program}` could not be started: {why}")
            }
        }
    }
}

impl std::error::Error for ExecError {}

/// Carries out a task at the place that was decided. Until the isolation
/// (M5) and an ADR for running on other machines exist, there are two
/// implementations: this machine (`Local`), and a fake for tests.
pub trait Executor {
    /// `root` is the project's root; the command runs there. Returns the
    /// command's exit code.
    fn run(&mut self, place: &Place, task: &Task, root: &Path) -> Result<i32, ExecError>;
}

/// Runs a task whose place was decided. Another machine is refused here,
/// before the executor is asked, so that no executor can run it there.
pub fn carry_out(
    executor: &mut dyn Executor,
    place: &Place,
    task: &Task,
    root: &Path,
) -> Result<i32, ExecError> {
    if !place.local {
        return Err(ExecError::Elsewhere);
    }
    if task.command.is_empty() {
        return Err(ExecError::NoCommand);
    }
    executor.run(place, task, root)
}

/// Runs tasks on this machine: no shell, in the project's root, with the
/// standard streams passed through.
///
/// `program`: the path to start in place of the task's first word. A
/// program the task names by a relative path is started by the real path
/// that was checked (`find::check_program`), never by the name again, so
/// that a link on the way cannot be swapped between the check and the
/// start. A script reached through a link therefore sees its real place in
/// `$0`.
#[derive(Debug, Default)]
pub struct Local {
    pub program: Option<PathBuf>,
}

impl Local {
    /// Starts `program` (a checked real path) with the task's arguments.
    pub fn with_program(program: PathBuf) -> Local {
        Local {
            program: Some(program),
        }
    }
}

/// The program to start: a relative path with a `/` in it is read from the
/// project's root; a bare name is looked up on `PATH`.
pub fn program_path(program: &str, root: &Path) -> PathBuf {
    let path = Path::new(program);
    if program.contains('/') && !path.is_absolute() {
        root.join(path)
    } else {
        path.to_path_buf()
    }
}

impl Executor for Local {
    fn run(&mut self, place: &Place, task: &Task, root: &Path) -> Result<i32, ExecError> {
        if !place.local {
            return Err(ExecError::Elsewhere);
        }
        let (program, args) = task.command.split_first().ok_or(ExecError::NoCommand)?;
        let start = self
            .program
            .clone()
            .unwrap_or_else(|| program_path(program, root));
        let status = Command::new(start)
            .args(args)
            .current_dir(root)
            .status()
            .map_err(|e| ExecError::Start {
                program: program.clone(),
                why: e.to_string(),
            })?;
        Ok(exit_code(status))
    }
}

/// The code a finished program ended with. A program ended by a signal
/// gives 128 + the signal's number, as shells report it.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return 128 + signal;
        }
    }
    1
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    /// Records what it was asked to run and returns a set code.
    #[derive(Default)]
    pub struct Fake {
        pub ran: Vec<(String, String, PathBuf)>,
        pub code: i32,
    }

    impl Executor for Fake {
        fn run(&mut self, place: &Place, task: &Task, root: &Path) -> Result<i32, ExecError> {
            self.ran
                .push((place.name.clone(), task.name.clone(), root.to_path_buf()));
            Ok(self.code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fake;
    use super::*;
    use crate::tasks::{Needs, TaskKind};

    fn task(command: &[&str]) -> Task {
        Task {
            name: "t".into(),
            kind: TaskKind::Build,
            needs: Needs::default(),
            prefers: Needs::default(),
            unknown_needs: Vec::new(),
            command: command.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn here() -> Place {
        Place {
            id: "id-here".into(),
            name: "here".into(),
            local: true,
        }
    }

    #[test]
    fn relative_programs_are_read_from_the_root() {
        let root = Path::new("/example/project");
        assert_eq!(program_path("./tools/x", root), root.join("./tools/x"));
        assert_eq!(program_path("tools/x", root), root.join("tools/x"));
        assert_eq!(program_path("cargo", root), PathBuf::from("cargo"));
        assert_eq!(program_path("/bin/true", root), PathBuf::from("/bin/true"));
    }

    #[test]
    fn another_machine_is_never_run_on() {
        let mut elsewhere = here();
        elsewhere.local = false;
        let err = Local::default()
            .run(
                &elsewhere,
                &task(&["/bin/sh", "-c", "exit 0"]),
                Path::new("/"),
            )
            .unwrap_err();
        assert_eq!(err, ExecError::Elsewhere);
    }

    #[test]
    fn a_task_without_a_command_is_not_run() {
        assert_eq!(
            Local::default()
                .run(&here(), &task(&[]), Path::new("/"))
                .unwrap_err(),
            ExecError::NoCommand
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_exit_code_is_passed_through() {
        let code = Local::default()
            .run(&here(), &task(&["/bin/sh", "-c", "exit 7"]), Path::new("/"))
            .unwrap();
        assert_eq!(code, 7);
    }

    #[test]
    fn a_missing_program_is_reported() {
        let err = Local::default()
            .run(&here(), &task(&["./does-not-exist"]), &std::env::temp_dir())
            .unwrap_err();
        assert!(matches!(err, ExecError::Start { .. }));
    }

    #[test]
    fn carry_out_runs_here_and_only_here() {
        let mut fake = Fake {
            code: 4,
            ..Fake::default()
        };
        let code = carry_out(&mut fake, &here(), &task(&["x"]), Path::new("/r")).unwrap();
        assert_eq!(code, 4);
        assert_eq!(
            fake.ran,
            vec![("here".into(), "t".into(), PathBuf::from("/r"))]
        );

        let mut elsewhere = here();
        elsewhere.local = false;
        let mut fake = Fake::default();
        let err = carry_out(&mut fake, &elsewhere, &task(&["x"]), Path::new("/r")).unwrap_err();
        assert_eq!(err, ExecError::Elsewhere);
        assert!(fake.ran.is_empty());

        let err = carry_out(&mut fake, &here(), &task(&[]), Path::new("/r")).unwrap_err();
        assert_eq!(err, ExecError::NoCommand);
        assert!(fake.ran.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_checked_path_is_started_in_place_of_the_name() {
        // The task names something that does not exist; the given path runs.
        let code = Local::with_program(PathBuf::from("/bin/sh"))
            .run(
                &here(),
                &task(&["./not-this", "-c", "exit 5"]),
                Path::new("/"),
            )
            .unwrap();
        assert_eq!(code, 5);
    }
}
