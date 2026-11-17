//! `istesso-route`: which machine a project's task goes to.
//!
//! Only display and exit codes live here; the rules are in the library.

use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use serde_json::{json, Value};

use istesso_route::choices::{choices_path, Choices, ProjectKey};
use istesso_route::decide::{
    decide, lacks, when_several_match, Decision, Place, Remembered, SeveralMatch, Stale,
};
use istesso_route::exec::{carry_out, ExecError, Local};
use istesso_route::find::{check_program, find_tasks, Found, Program};
use istesso_route::table::{read_table, Candidate};
use istesso_route::tasks::{read_tasks, Task, TaskFile, TaskKind, TASKS_FILE};
use istesso_route::{exit, now_unix, utc_date, utc_time};

#[derive(Parser)]
#[command(
    name = "istesso-route",
    version,
    about = "Decides which machine a project's build, run or deploy task goes to.",
    long_about = "Decides which machine a project's build, run or deploy task goes to, from the \
                  task's needs in .istesso/tasks.json and the role table that \
                  `istesso hosts --json` prints. Pass the role table with --hosts <file> or on \
                  standard input: istesso hosts --json | istesso-route where build test"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Say which machine a task goes to, and why. Runs nothing.
    Where {
        kind: KindArg,
        task: String,
        #[command(flatten)]
        hosts: HostsArg,
        /// Print the answer as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check in one line that a deploy task has a single place that answers now.
    Check {
        #[command(subcommand)]
        what: CheckWhat,
    },
    /// Run a run task, when its place is this machine.
    Run(RunArgs),
    /// Run a build task, when its place is this machine.
    Build(RunArgs),
    /// Run a deploy task, when its place is this machine.
    Deploy(RunArgs),
    /// Remember the machine a person chose for a deploy task.
    Choose {
        task: String,
        /// The machine, by id or by name.
        machine: String,
        #[command(flatten)]
        hosts: HostsArg,
    },
    /// Forget the remembered machine of a deploy task.
    Forget { task: String },
    /// List the remembered choices and where they are kept.
    Choices,
}

#[derive(Subcommand)]
enum CheckWhat {
    /// The deploy task's place is decided and answers.
    Deploy {
        task: String,
        #[command(flatten)]
        hosts: HostsArg,
    },
}

#[derive(clap::Args)]
struct RunArgs {
    task: String,
    #[command(flatten)]
    hosts: HostsArg,
    /// End with one JSON line on standard error instead of the text.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct HostsArg {
    /// The role table (`istesso hosts --json`); standard input when left out.
    #[arg(long, value_name = "FILE")]
    hosts: Option<PathBuf>,
}

#[derive(Clone, Copy, ValueEnum)]
enum KindArg {
    Build,
    Run,
    Deploy,
}

impl From<KindArg> for TaskKind {
    fn from(kind: KindArg) -> TaskKind {
        match kind {
            KindArg::Build => TaskKind::Build,
            KindArg::Run => TaskKind::Run,
            KindArg::Deploy => TaskKind::Deploy,
        }
    }
}

/// A reason to stop before an answer: an exit code and one message.
struct Fail {
    code: i32,
    message: String,
}

fn fail(code: i32, message: impl Into<String>) -> Fail {
    Fail {
        code,
        message: message.into(),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let code = match run(cli.command) {
        Ok(code) => code,
        Err(Fail { code, message }) => {
            eprintln!("istesso-route: {message}");
            code
        }
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

fn run(command: Command) -> Result<i32, Fail> {
    match command {
        Command::Where {
            kind,
            task,
            hosts,
            json,
        } => {
            let (project, task) = project_task(&task, Some(kind.into()))?;
            let decision = decide_task(&project, &task, &hosts)?;
            if json {
                println!("{}", to_json(&task, &decision));
            } else {
                print!("{}", render(&task, &decision));
            }
            Ok(decision.exit_code())
        }
        Command::Check {
            what: CheckWhat::Deploy { task, hosts },
        } => {
            let (project, task) = project_task(&task, Some(TaskKind::Deploy))?;
            let decision = decide_task(&project, &task, &hosts)?;
            if matches!(decision, Decision::Go { .. }) {
                if let Err(Fail { code, message }) = check_program_of(&task, &project.root) {
                    println!("not ready: deploy `{}`: {message}", task.name);
                    return Ok(code);
                }
            }
            println!("{}", one_line(&task, &decision));
            Ok(decision.exit_code())
        }
        Command::Run(args) => Ok(run_task(TaskKind::Run, args)),
        Command::Build(args) => Ok(run_task(TaskKind::Build, args)),
        Command::Deploy(args) => Ok(run_task(TaskKind::Deploy, args)),
        Command::Choose {
            task,
            machine,
            hosts,
        } => choose(&task, &machine, &hosts),
        Command::Forget { task } => forget(&task),
        Command::Choices => list_choices(),
    }
}

/// The project found from the current folder: its root and its task file.
struct Project {
    root: PathBuf,
    file: PathBuf,
    tasks: TaskFile,
}

impl Project {
    /// How remembered choices tell this project apart.
    fn key(&self) -> ProjectKey {
        ProjectKey::of(self.tasks.project.as_deref(), &self.root)
    }
}

fn project() -> Result<Project, Fail> {
    let here = std::env::current_dir().map_err(|e| {
        fail(
            exit::UNREADABLE,
            format!("the current folder cannot be read: {e}"),
        )
    })?;
    // A refused task file ends with the same code as an unreadable one (73):
    // every code of the plan's range is taken.
    let Found { file, root, text } = find_tasks(&here)
        .map_err(|e| fail(exit::UNREADABLE, e.to_string()))?
        .ok_or_else(|| {
            fail(
                exit::UNREADABLE,
                format!(
                    "no {TASKS_FILE} in this folder or its parents up to the project's root \
                     (the first folder with .git; without one, this folder only)"
                ),
            )
        })?;
    let tasks = read_tasks(&text)
        .map_err(|e| fail(exit::UNREADABLE, format!("{}: {e}", file.display())))?;
    Ok(Project { root, file, tasks })
}

fn project_task(name: &str, kind: Option<TaskKind>) -> Result<(Project, Task), Fail> {
    let project = project()?;
    let Some(task) = project.tasks.tasks.get(name).cloned() else {
        if let Some(skipped) = project.tasks.skipped_task(name) {
            return Err(fail(
                exit::UNREADABLE,
                format!(
                    "the task `{name}` in {} was skipped: {}",
                    project.file.display(),
                    skipped.why
                ),
            ));
        }
        let known: Vec<&str> = project.tasks.tasks.keys().map(String::as_str).collect();
        return Err(fail(
            exit::UNREADABLE,
            format!(
                "no task `{name}` in {} (tasks: {})",
                project.file.display(),
                if known.is_empty() {
                    "none".to_owned()
                } else {
                    known.join(", ")
                }
            ),
        ));
    };
    if let Some(kind) = kind {
        if task.kind != kind {
            return Err(fail(
                exit::UNREADABLE,
                format!(
                    "`{name}` is a {} task, not a {kind} task; use `istesso-route {} {name}`",
                    task.kind, task.kind
                ),
            ));
        }
    }
    Ok((project, task))
}

fn read_hosts(hosts: &HostsArg) -> Result<Vec<Candidate>, Fail> {
    let text = match &hosts.hosts {
        Some(path) => std::fs::read_to_string(path).map_err(|e| {
            fail(
                exit::UNREADABLE,
                format!("the role table {} cannot be read: {e}", path.display()),
            )
        })?,
        None => {
            let mut stdin = std::io::stdin();
            if stdin.is_terminal() {
                return Err(fail(
                    exit::UNREADABLE,
                    "no role table: pass one with --hosts <file>, or on standard input: \
                     istesso hosts --json | istesso-route ...",
                ));
            }
            let mut text = String::new();
            stdin.read_to_string(&mut text).map_err(|e| {
                fail(
                    exit::UNREADABLE,
                    format!("the role table cannot be read from standard input: {e}"),
                )
            })?;
            text
        }
    };
    read_table(&text).map_err(|e| fail(exit::UNREADABLE, e.to_string()))
}

/// The remembered choice for the task, for kinds that stop and list.
fn remembered(project: &Project, task: &Task) -> Result<Option<Remembered>, Fail> {
    if when_several_match(task.kind) != SeveralMatch::StopAndList {
        return Ok(None);
    }
    let choices =
        Choices::load(&choices_path()).map_err(|e| fail(exit::UNREADABLE, e.to_string()))?;
    Ok(choices.get(&project.key(), &task.name))
}

fn decide_task(project: &Project, task: &Task, hosts: &HostsArg) -> Result<Decision, Fail> {
    let table = read_hosts(hosts)?;
    let remembered = remembered(project, task)?;
    Ok(decide(task, &table, remembered.as_ref()))
}

/// `run`, `build` and `deploy`. Standard output belongs to the task; what
/// this command says goes to standard error, and its last line always says,
/// in a fixed form, whether the task ran: the exit code alone cannot tell,
/// because a task may end with any number, 70-75 included.
fn run_task(kind: TaskKind, args: RunArgs) -> i32 {
    let outcome = carry_out_task(kind, &args);
    let (code, ran, decision) = match outcome {
        Ok(Outcome {
            code,
            ran,
            task,
            decision,
        }) => (code, ran, Some((task, decision))),
        Err(Fail { code, message }) => {
            eprintln!("istesso-route: {message}");
            (code, false, None)
        }
    };
    if args.json {
        let mut line = json!({
            "task": args.task,
            "kind": kind.as_str(),
            "ran": ran,
            "exit_code": code,
            "decision": Value::Null,
        });
        if let Some((task, decision)) = &decision {
            line["decision"] = to_json(task, decision);
        }
        eprintln!("{line}");
    } else {
        eprintln!("{}", ran_line(kind, &args.task, ran, code));
    }
    code
}

/// The fixed last line of `run`, `build` and `deploy`.
fn ran_line(kind: TaskKind, task: &str, ran: bool, code: i32) -> String {
    if ran {
        format!("istesso-route: ran {kind} `{task}` here; it ended with {code}")
    } else {
        format!("istesso-route: did not run {kind} `{task}`; exit code {code}")
    }
}

struct Outcome {
    code: i32,
    ran: bool,
    task: Task,
    decision: Decision,
}

fn carry_out_task(kind: TaskKind, args: &RunArgs) -> Result<Outcome, Fail> {
    let (project, task) = project_task(&args.task, Some(kind))?;
    let decision = decide_task(&project, &task, &args.hosts)?;
    let not_run = |code: i32, task: Task, decision: Decision| Outcome {
        code,
        ran: false,
        task,
        decision,
    };
    let Decision::Go { place, .. } = &decision else {
        if !args.json {
            eprint!("{}", render(&task, &decision));
        }
        return Ok(not_run(decision.exit_code(), task, decision));
    };
    let mut local = Local::default();
    if place.local {
        local.program = check_program_of(&task, &project.root)?;
    }
    match carry_out(&mut local, place, &task, &project.root) {
        Ok(code) => Ok(Outcome {
            code,
            ran: true,
            task,
            decision,
        }),
        Err(ExecError::Elsewhere) => {
            if !args.json {
                eprint!("{}", render(&task, &decision));
                eprintln!(
                    "  not run: running on another machine is not built yet (it waits for the isolation, M5)"
                );
            }
            Ok(not_run(exit::ELSEWHERE, task, decision))
        }
        Err(e @ ExecError::NoCommand) => {
            Err(fail(exit::UNREADABLE, format!("`{}`: {e}", task.name)))
        }
        Err(e @ ExecError::Start { .. }) => {
            Err(fail(exit::CANNOT_START, format!("`{}`: {e}", task.name)))
        }
    }
}

/// Checks the program the task names. A relative program that cannot be
/// trusted ends like an untrusted task file (73); one that is not there,
/// where the folders down to it pass, cannot be started (127). Either way
/// nothing runs. `Ok` holds the path to start (`None`: as named).
fn check_program_of(task: &Task, root: &std::path::Path) -> Result<Option<PathBuf>, Fail> {
    let Some(program) = task.command.first() else {
        return Ok(None);
    };
    match check_program(program, root) {
        Ok(Program::Unchecked) => Ok(None),
        Ok(Program::Checked(real)) => Ok(Some(real)),
        Ok(Program::Missing(path)) => Err(fail(
            exit::CANNOT_START,
            format!(
                "the program `{program}` of `{}` cannot be started: {} is not there",
                task.name,
                path.display()
            ),
        )),
        Err(e) => Err(fail(
            exit::UNREADABLE,
            format!("the program `{program}` of `{}` is not run: {e}", task.name),
        )),
    }
}

fn choose(name: &str, machine: &str, hosts: &HostsArg) -> Result<i32, Fail> {
    let (project, task) = project_task(name, None)?;
    if when_several_match(task.kind) != SeveralMatch::StopAndList {
        return Err(fail(
            exit::UNREADABLE,
            format!(
                "`{name}` is a {} task; only deploy tasks are remembered (build and run tasks pick a machine in order)",
                task.kind
            ),
        ));
    }
    if !task.unknown_needs.is_empty() {
        return Err(fail(
            exit::NOT_UNDERSTOOD,
            format!(
                "`{name}` states needs this version does not know ({}); no machine can be chosen for it",
                task.unknown_needs.join(", ")
            ),
        ));
    }
    let table = read_hosts(hosts)?;
    let candidate = pick_machine(&table, machine)?;
    let missing = lacks(&task.needs, candidate);
    if !missing.is_empty() {
        return Err(fail(
            exit::NOBODY,
            format!(
                "{} does not meet the needs of {} `{name}`: it {}; nothing was remembered",
                candidate.name,
                task.kind,
                missing.join("; it ")
            ),
        ));
    }
    let path = choices_path();
    let mut choices = Choices::load(&path).map_err(|e| fail(exit::UNREADABLE, e.to_string()))?;
    let place = Place::of(candidate);
    let key = project.key();
    choices.set(&key, name, &place, now_unix());
    choices
        .save()
        .map_err(|e| fail(exit::UNREADABLE, e.to_string()))?;
    println!(
        "{} `{name}` goes to {} ({}) from now on",
        task.kind, place.name, place.id
    );
    if !candidate.answers() {
        println!(
            "  {} does not answer now; the task waits for it",
            place.name
        );
    }
    println!("  remembered for {key} in {}", path.display());
    println!("  `istesso-route forget {name}` drops it");
    Ok(exit::OK)
}

/// The one machine `machine` names: by id first, then by name.
fn pick_machine<'a>(table: &'a [Candidate], machine: &str) -> Result<&'a Candidate, Fail> {
    if let Some(found) = table.iter().find(|c| c.id == machine) {
        return Ok(found);
    }
    let named: Vec<&Candidate> = table.iter().filter(|c| c.name == machine).collect();
    match named.as_slice() {
        [one] => Ok(one),
        [] => Err(fail(
            exit::USAGE,
            format!(
                "no machine `{machine}` in the role table (machines: {})",
                table
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
        several => Err(fail(
            exit::USAGE,
            format!(
                "{} machines are called `{machine}`; name one by id: {}",
                several.len(),
                several
                    .iter()
                    .map(|c| c.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn forget(name: &str) -> Result<i32, Fail> {
    let project = project()?;
    let path = choices_path();
    let mut choices = Choices::load(&path).map_err(|e| fail(exit::UNREADABLE, e.to_string()))?;
    if choices.forget(&project.key(), name) {
        choices
            .save()
            .map_err(|e| fail(exit::UNREADABLE, e.to_string()))?;
        println!("forgot the machine chosen for `{name}`");
    } else {
        println!("no machine was remembered for `{name}` in this project");
    }
    Ok(exit::OK)
}

fn list_choices() -> Result<i32, Fail> {
    let path = choices_path();
    let choices = Choices::load(&path).map_err(|e| fail(exit::UNREADABLE, e.to_string()))?;
    let all = choices.all();
    if all.is_empty() {
        println!(
            "no remembered choices (they are kept in {})",
            path.display()
        );
        return Ok(exit::OK);
    }
    println!("remembered choices in {}:", path.display());
    for choice in all {
        let r = &choice.remembered;
        println!(
            "  {}: `{}` goes to {} ({}), chosen on {}",
            choice.project,
            choice.task,
            r.name,
            r.host,
            utc_date(r.chosen_at)
        );
    }
    Ok(exit::OK)
}

// ---- display -----------------------------------------------------------------

fn place_label(place: &Place) -> String {
    if place.local {
        format!("{} (this machine)", place.name)
    } else {
        place.name.clone()
    }
}

fn machines(n: usize) -> String {
    if n == 1 {
        "1 machine meets".to_owned()
    } else {
        format!("{n} machines meet")
    }
}

fn stale_line(chosen: &Remembered, why: &Stale) -> String {
    match why {
        Stale::Gone => format!(
            "the remembered choice, {}, is not among the paired machines any more",
            chosen.name
        ),
        Stale::NoLongerMeets(lacks) => format!(
            "the remembered choice, {}, no longer meets its needs: it {}",
            chosen.name,
            lacks.join("; it ")
        ),
    }
}

/// The answer of `where` (and of `run` when nothing runs): the conclusion on
/// the first line, the reasons indented below.
fn render(task: &Task, decision: &Decision) -> String {
    let head = format!("{} `{}`", task.kind, task.name);
    let mut out = Vec::new();
    match decision {
        Decision::Go {
            place,
            because,
            remembered,
        } => {
            out.push(format!("{head} goes to: {}", place_label(place)));
            for reason in because {
                out.push(format!("  it {reason}"));
            }
            if let Some(at) = remembered {
                out.push(format!(
                    "  chosen for this task on {} (remembered; `istesso-route choose` changes it, `istesso-route forget` drops it)",
                    utc_date(*at)
                ));
            }
        }
        Decision::Stopped { places, remembered } => {
            if *remembered {
                out.push(format!(
                    "{head} stops: the remembered choice, {}, does not answer",
                    places.first().map(|p| p.place.name.as_str()).unwrap_or("?")
                ));
            } else {
                out.push(format!(
                    "{head} cannot go anywhere now: the machines that meet its needs do not answer"
                ));
            }
            for asleep in places {
                let seen = match asleep.seen_at {
                    Some(at) => format!("last seen {}", utc_time(at)),
                    None => "never seen".to_owned(),
                };
                let wake = if asleep.wake_possible {
                    "it can be woken over the network"
                } else {
                    "it cannot be woken over the network"
                };
                out.push(format!(
                    "  {} ({}): {seen}; {wake}",
                    asleep.place.name, asleep.place.id
                ));
            }
            out.push("  its role table is the one last seen; no other machine is tried".to_owned());
        }
        Decision::Ambiguous { places, stale } => {
            out.push(format!(
                "{head} has no single place: {} its needs",
                machines(places.len())
            ));
            for place in places {
                out.push(format!("  {} ({})", place_label(place), place.id));
            }
            if let Some((chosen, why)) = stale {
                out.push(format!("  {}", stale_line(chosen, why)));
            }
            out.push(format!(
                "  choose one: istesso-route choose {} <machine>",
                task.name
            ));
            if stale.is_some() {
                out.push(format!(
                    "  or forget the choice: istesso-route forget {}",
                    task.name
                ));
            }
        }
        Decision::Nobody { lacking } => {
            out.push(format!("{head} has no place: no machine meets its needs"));
            if lacking.is_empty() {
                out.push("  the role table lists no machines".to_owned());
            }
            for lack in lacking {
                out.push(format!(
                    "  {}: it {}",
                    place_label(&lack.place),
                    lack.lacks.join("; it ")
                ));
            }
        }
        Decision::NotUnderstood { needs } => {
            out.push(format!(
                "{head} cannot be decided by this version: it states needs this version does not know"
            ));
            for need in needs {
                out.push(format!("  {need}"));
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// The answer of `check deploy`: one line.
fn one_line(task: &Task, decision: &Decision) -> String {
    let head = format!("{} `{}`", task.kind, task.name);
    match decision {
        Decision::Go {
            place, remembered, ..
        } => match remembered {
            Some(at) => format!(
                "ok: {head} goes to {} ({}), remembered choice from {}",
                place.name,
                place.id,
                utc_date(*at)
            ),
            None => format!("ok: {head} goes to {} ({})", place.name, place.id),
        },
        Decision::Stopped { places, .. } => format!(
            "not ready: {head}: {} does not answer",
            places
                .iter()
                .map(|p| p.place.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Decision::Ambiguous {
            places,
            stale: Some((chosen, why)),
        } => format!(
            "not ready: {head}: {}; {} its needs; choose one: istesso-route choose {} <machine>",
            stale_line(chosen, why),
            machines(places.len()),
            task.name
        ),
        Decision::Ambiguous { places, stale: None } => format!(
            "not ready: {head} has no single place: {} its needs; choose one: istesso-route choose {} <machine>",
            machines(places.len()),
            task.name
        ),
        Decision::Nobody { .. } => format!("not ready: {head}: no machine meets its needs"),
        Decision::NotUnderstood { needs } => format!(
            "not ready: {head}: needs this version does not know ({})",
            needs.join(", ")
        ),
    }
}

fn place_json(place: &Place) -> Value {
    json!({ "id": place.id, "name": place.name, "local": place.local })
}

fn to_json(task: &Task, decision: &Decision) -> Value {
    let mut out = json!({ "task": task.name, "kind": task.kind.as_str() });
    let extra = match decision {
        Decision::Go {
            place,
            because,
            remembered,
        } => json!({
            "decision": "go",
            "place": place_json(place),
            "because": because,
            "remembered": remembered.map(|at| json!({ "chosen_at": at })),
        }),
        Decision::Stopped { places, remembered } => json!({
            "decision": "stopped",
            "places": places.iter().map(|a| json!({
                "place": place_json(&a.place),
                "wake_possible": a.wake_possible,
                "seen_at": a.seen_at,
            })).collect::<Vec<_>>(),
            "remembered": remembered,
        }),
        Decision::Ambiguous { places, stale } => json!({
            "decision": "ambiguous",
            "places": places.iter().map(place_json).collect::<Vec<_>>(),
            "stale": stale.as_ref().map(|(chosen, why)| json!({
                "host": chosen.host,
                "name": chosen.name,
                "chosen_at": chosen.chosen_at,
                "why": match why { Stale::Gone => "gone", Stale::NoLongerMeets(_) => "no_longer_meets" },
                "lacks": match why { Stale::Gone => Vec::new(), Stale::NoLongerMeets(l) => l.clone() },
            })),
        }),
        Decision::Nobody { lacking } => json!({
            "decision": "nobody",
            "lacking": lacking.iter().map(|l| json!({
                "place": place_json(&l.place),
                "lacks": l.lacks,
            })).collect::<Vec<_>>(),
        }),
        Decision::NotUnderstood { needs } => json!({
            "decision": "not_understood",
            "needs": needs,
        }),
    };
    if let (Value::Object(out), Value::Object(extra)) = (&mut out, extra) {
        out.extend(extra);
    }
    out
}
