//! Deciding where a task goes, and why.
//!
//! 0. A task with needs this version does not know is not decided.
//! 1. Machines that do not meet the needs are set aside, each with what it
//!    lacks. A machine whose role table is not known meets only needs that
//!    ask nothing of the table. If nothing is left: `Nobody`.
//! 2. Build and run pick from the rest in order: this machine, then machines
//!    that answer, then lower load; names break ties.
//! 3. If none of the machines that meet the needs answers, stop (`Stopped`).
//!    Nothing that does not meet the needs is tried instead.
//! 4. Deploy does not pick in order: when several machines meet the needs,
//!    it stops and lists them, and a person chooses. The choice is
//!    remembered and used from then on, as long as it still fits.
//! 5. The role table of a machine that does not answer is the one last seen;
//!    it guides the answer but is not trusted beyond that.

use std::cmp::Ordering;

use istesso_proto::HostState;

use crate::table::Candidate;
use crate::tasks::{os_name, role_name, Needs, Task, TaskKind};

/// A machine, as far as an answer needs to name it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub id: String,
    pub name: String,
    pub local: bool,
}

impl Place {
    pub fn of(candidate: &Candidate) -> Place {
        Place {
            id: candidate.id.clone(),
            name: candidate.name.clone(),
            local: candidate.local,
        }
    }
}

/// A machine that meets the needs but does not answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asleep {
    pub place: Place,
    pub wake_possible: bool,
    pub seen_at: Option<u64>,
}

/// A machine that does not meet the needs, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lack {
    pub place: Place,
    pub lacks: Vec<String>,
}

/// A choice a person made for one task of one project (see `choices`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remembered {
    /// The machine's id in the role table.
    pub host: String,
    /// For display only.
    pub name: String,
    pub chosen_at: u64,
}

/// Why a remembered choice cannot be used any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stale {
    /// The machine is not in the role table.
    Gone,
    /// The machine no longer meets the needs; what it lacks.
    NoLongerMeets(Vec<String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Run it there. `because` names the needs it meets and why it came first.
    /// `remembered`: when the person chose it, if the place is a remembered choice.
    Go {
        place: Place,
        because: Vec<String>,
        remembered: Option<u64>,
    },
    /// Machines meet the needs, but none of them answers. Nothing else is tried.
    /// `remembered`: the one machine listed is a remembered choice.
    Stopped {
        places: Vec<Asleep>,
        remembered: bool,
    },
    /// A deploy task and no single place: more than one machine meets the
    /// needs and none was chosen, or the chosen one cannot be used (`stale`).
    Ambiguous {
        places: Vec<Place>,
        stale: Option<(Remembered, Stale)>,
    },
    /// No machine meets the needs; says what each one lacks.
    Nobody { lacking: Vec<Lack> },
    /// The task states needs this version does not know.
    NotUnderstood { needs: Vec<String> },
}

impl Decision {
    /// The exit code `istesso-route` ends with for this decision.
    pub fn exit_code(&self) -> i32 {
        use crate::exit;
        match self {
            Decision::Go { .. } => exit::OK,
            Decision::Stopped { .. } => exit::STOPPED,
            Decision::Ambiguous { .. } => exit::AMBIGUOUS,
            Decision::Nobody { .. } => exit::NOBODY,
            Decision::NotUnderstood { .. } => exit::NOT_UNDERSTOOD,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeveralMatch {
    StopAndList,
    PickInOrder,
}

/// The one place that says what happens when several machines meet the needs
/// of a task: build and run pick in order, deploy stops and lists.
pub fn when_several_match(kind: TaskKind) -> SeveralMatch {
    match kind {
        TaskKind::Build | TaskKind::Run => SeveralMatch::PickInOrder,
        TaskKind::Deploy => SeveralMatch::StopAndList,
    }
}

/// Whether a machine that meets the needs counts when deciding if several
/// machines match, for kinds that stop and list.
///
/// The planner's proposal, not the owner's decision: machines that do not
/// answer count too. Choosing the one that answers would be picking in order
/// again. Returning `candidate.answers()` here instead would let a deploy go
/// to the only machine that answers.
pub fn counts_when_several_match(candidate: &Candidate) -> bool {
    let _ = candidate;
    true
}

/// What `candidate` lacks for `needs`, in words; empty when it meets them.
pub fn lacks(needs: &Needs, candidate: &Candidate) -> Vec<String> {
    let mut lacks = Vec::new();
    if let Some(host) = &needs.host {
        if &candidate.id != host && &candidate.name != host {
            lacks.push(format!("is not the machine the task names (`{host}`)"));
        }
    }
    if needs.asks_nothing_of_the_table() {
        return lacks;
    }
    let Some(manifest) = &candidate.manifest else {
        lacks.push("has no role table that this version can read".into());
        return lacks;
    };
    if let Some(os) = needs.os {
        if manifest.os.family != os {
            lacks.push(format!(
                "runs {}, not {}",
                manifest.os.family.label(),
                os.label()
            ));
        }
    }
    if let Some(arch) = &needs.arch {
        if &manifest.os.arch != arch {
            lacks.push(format!("is {}, not {arch}", manifest.os.arch));
        }
    }
    for role in &needs.roles {
        if !manifest.roles.contains(role) {
            lacks.push(format!("does not have the role {}", role_name(*role)));
        }
    }
    for tool in &needs.tools {
        let tools = &manifest.tools;
        let has = match tool {
            crate::tasks::Tool::Cargo => tools.cargo,
            crate::tasks::Tool::Docker => tools.docker,
            crate::tasks::Tool::Gstreamer => tools.gstreamer,
            crate::tasks::Tool::Xcode => tools.xcode.is_some(),
        };
        if !has {
            lacks.push(format!("does not have {}", tool.as_str()));
        }
    }
    lacks
}

/// The needs a machine meets, in words, for the reasons of an answer.
fn meets(needs: &Needs) -> Vec<String> {
    let mut met = Vec::new();
    if needs.host.is_some() {
        met.push("is the machine the task names".to_owned());
    }
    if let Some(os) = needs.os {
        met.push(format!("runs {} ({})", os.label(), os_name(os)));
    }
    if let Some(arch) = &needs.arch {
        met.push(format!("is {arch}"));
    }
    for role in &needs.roles {
        met.push(format!("has the role {}", role_name(*role)));
    }
    for tool in &needs.tools {
        met.push(format!("has {}", tool.as_str()));
    }
    if met.is_empty() {
        met.push("meets the task's needs (it states none)".to_owned());
    }
    met
}

fn asleep(candidate: &Candidate) -> Asleep {
    Asleep {
        place: Place::of(candidate),
        wake_possible: candidate.wake_possible(),
        seen_at: candidate.seen_at,
    }
}

/// Build and run order: this machine, machines that answer, lower load (an
/// unknown load after a known one), then the name.
fn in_order(a: &Candidate, b: &Candidate) -> Ordering {
    let key = |c: &Candidate| (!c.local, c.state != HostState::Awake, c.load().is_none());
    key(a)
        .cmp(&key(b))
        .then_with(|| {
            a.load()
                .unwrap_or(0.0)
                .partial_cmp(&b.load().unwrap_or(0.0))
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| a.name.cmp(&b.name))
}

pub fn decide(task: &Task, table: &[Candidate], remembered: Option<&Remembered>) -> Decision {
    if !task.unknown_needs.is_empty() {
        return Decision::NotUnderstood {
            needs: task.unknown_needs.clone(),
        };
    }

    let mut meeting: Vec<&Candidate> = Vec::new();
    let mut lacking = Vec::new();
    for candidate in table {
        let lacks = lacks(&task.needs, candidate);
        if lacks.is_empty() {
            meeting.push(candidate);
        } else {
            lacking.push(Lack {
                place: Place::of(candidate),
                lacks,
            });
        }
    }
    if meeting.is_empty() {
        return Decision::Nobody { lacking };
    }

    let met = meets(&task.needs);
    match when_several_match(task.kind) {
        SeveralMatch::StopAndList => {
            if let Some(chosen) = remembered {
                return decide_remembered(task, table, &meeting, chosen, met);
            }
            let counted: Vec<&Candidate> = meeting
                .iter()
                .copied()
                .filter(|c| counts_when_several_match(c))
                .collect();
            if counted.len() >= 2 {
                return Decision::Ambiguous {
                    places: counted.iter().map(|c| Place::of(c)).collect(),
                    stale: None,
                };
            }
            pick_in_order(&meeting, met, None)
        }
        SeveralMatch::PickInOrder => pick_in_order(&meeting, met, Some(&task.prefers)),
    }
}

fn decide_remembered(
    task: &Task,
    table: &[Candidate],
    meeting: &[&Candidate],
    chosen: &Remembered,
    mut because: Vec<String>,
) -> Decision {
    let listed = || meeting.iter().map(|c| Place::of(c)).collect();
    let Some(candidate) = table.iter().find(|c| c.id == chosen.host) else {
        return Decision::Ambiguous {
            places: listed(),
            stale: Some((chosen.clone(), Stale::Gone)),
        };
    };
    let lacks = lacks(&task.needs, candidate);
    if !lacks.is_empty() {
        return Decision::Ambiguous {
            places: listed(),
            stale: Some((chosen.clone(), Stale::NoLongerMeets(lacks))),
        };
    }
    if !candidate.answers() {
        return Decision::Stopped {
            places: vec![asleep(candidate)],
            remembered: true,
        };
    }
    because.push("was chosen by a person for this task".to_owned());
    Decision::Go {
        place: Place::of(candidate),
        because,
        remembered: Some(chosen.chosen_at),
    }
}

/// Picks the first machine that answers. `prefers`: what a build or run task
/// would rather have; `None` for a deploy, where at most one machine counts.
///
/// Order: machines that meet the preferences, then this machine, machines
/// that answer, lower load, the name (see `in_order`).
fn pick_in_order(
    meeting: &[&Candidate],
    mut because: Vec<String>,
    prefers: Option<&Needs>,
) -> Decision {
    let mut answering: Vec<&Candidate> = meeting.iter().copied().filter(|c| c.answers()).collect();
    if answering.is_empty() {
        return Decision::Stopped {
            places: meeting.iter().map(|c| asleep(c)).collect(),
            remembered: false,
        };
    }
    let Some(prefers) = prefers else {
        because.push("is the only machine that meets the needs".to_owned());
        return Decision::Go {
            place: Place::of(answering[0]),
            because,
            remembered: None,
        };
    };
    let stated = *prefers != Needs::default();
    let preferred = |c: &Candidate| stated && lacks(prefers, c).is_empty();
    answering.sort_by(|a, b| preferred(b).cmp(&preferred(a)).then_with(|| in_order(a, b)));
    let first = answering[0];
    if stated {
        let wished = meets(prefers).join(", ");
        if preferred(first) {
            because.push(format!(
                "also has what the task prefers ({wished}), so it comes before this machine"
            ));
        } else {
            because.push(format!(
                "no machine that answers has what the task prefers ({wished})"
            ));
        }
    }
    // Why it comes first among the machines that are equally preferred.
    let tier: Vec<&Candidate> = answering
        .iter()
        .copied()
        .filter(|c| preferred(c) == preferred(first))
        .collect();
    let why_first = if first.local {
        "is this machine (this machine comes first)".to_owned()
    } else if tier.len() == 1 {
        if stated && preferred(first) {
            "is the only machine that answers and has it".to_owned()
        } else {
            "is the only machine that meets the needs and answers".to_owned()
        }
    } else {
        match first.load() {
            Some(load) if tier[1].load() != Some(load) => {
                format!("answers and has the lowest load ({load:.2})")
            }
            _ => "answers, and comes first by name among the machines that answer".to_owned(),
        }
    };
    because.push(why_first);
    Decision::Go {
        place: Place::of(first),
        because,
        remembered: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::fixtures::*;
    use crate::table::read_table;
    use crate::tasks::read_tasks;

    fn task(kind: &str, needs: &str) -> Task {
        let text = format!(
            r#"{{"version": 1, "tasks": {{"t": {{"kind": "{kind}", "needs": {needs}}}}}}}"#
        );
        read_tasks(&text).unwrap().tasks.remove("t").unwrap()
    }

    fn go_to(decision: &Decision) -> &str {
        match decision {
            Decision::Go { place, .. } => &place.name,
            other => panic!("not a place: {other:?}"),
        }
    }

    fn two() -> Vec<Candidate> {
        read_table(TWO).unwrap()
    }

    fn three() -> Vec<Candidate> {
        read_table(THREE).unwrap()
    }

    fn asleep_table() -> Vec<Candidate> {
        read_table(TOWER_ASLEEP).unwrap()
    }

    // The rows of the table "where to run what" in AGENTS.md, as needs, with
    // the two machines of `hosts-two.json`.
    #[test]
    fn the_agents_md_table_gives_the_same_places() {
        let table = two();
        // Common core (any PC): this machine comes first.
        assert_eq!(
            go_to(&decide(
                &task("build", r#"{"tools": ["cargo"]}"#),
                &table,
                None
            )),
            "air"
        );
        // Screen measurement (a Mac).
        assert_eq!(
            go_to(&decide(&task("run", r#"{"os": "macos"}"#), &table, None)),
            "air"
        );
        // Linux host (Ubuntu with a GPU).
        assert_eq!(
            go_to(&decide(
                &task(
                    "run",
                    r#"{"os": "linux", "roles": ["gpu", "hardware_encode"]}"#
                ),
                &table,
                None
            )),
            "tower"
        );
        // Apple builds and signing (a Mac with Xcode).
        assert_eq!(
            go_to(&decide(
                &task("build", r#"{"roles": ["xcode_build"]}"#),
                &table,
                None
            )),
            "air"
        );
        // Mac host ("prefer the always-on Mac"): with only one Mac, the Mac
        // at hand.
        assert_eq!(go_to(&decide(&mac_host(), &table, None)), "air");
        // Rows that cannot be written as needs: the test application's window
        // (the role table has no role for "can open a window"), and latency
        // measurement over a cable (two machines at once; a task goes to one).
    }

    /// The row "Mac host (prefer the always-on Mac)".
    fn mac_host() -> Task {
        task_with(
            "run",
            r#"{"os": "macos"}"#,
            r#"{"roles": ["always_on_candidate"]}"#,
        )
    }

    fn task_with(kind: &str, needs: &str, prefers: &str) -> Task {
        let text = format!(
            r#"{{"version": 1, "tasks": {{"t": {{"kind": "{kind}", "needs": {needs}, "prefers": {prefers}}}}}}}"#
        );
        read_tasks(&text).unwrap().tasks.remove("t").unwrap()
    }

    // The same rows with the three machines of `hosts-three.json`: the
    // always-on Mac takes the Mac host row; every other row stays.
    #[test]
    fn the_agents_md_table_gives_the_same_places_with_three_machines() {
        let table = three();
        assert_eq!(
            go_to(&decide(
                &task("build", r#"{"tools": ["cargo"]}"#),
                &table,
                None
            )),
            "air"
        );
        assert_eq!(
            go_to(&decide(&task("run", r#"{"os": "macos"}"#), &table, None)),
            "air"
        );
        assert_eq!(
            go_to(&decide(
                &task(
                    "run",
                    r#"{"os": "linux", "roles": ["gpu", "hardware_encode"]}"#
                ),
                &table,
                None
            )),
            "tower"
        );
        assert_eq!(
            go_to(&decide(
                &task("build", r#"{"roles": ["xcode_build"]}"#),
                &table,
                None
            )),
            "air"
        );
        let decision = decide(&mac_host(), &table, None);
        assert_eq!(go_to(&decision), "mini");
        let Decision::Go { because, .. } = decision else {
            panic!()
        };
        assert!(because.iter().any(|b| b.contains("what the task prefers")));
    }

    #[test]
    fn preferences_order_but_never_widen() {
        // No machine that answers has the preference: the usual order, said so.
        let decision = decide(&mac_host(), &two(), None);
        let Decision::Go { place, because, .. } = decision else {
            panic!()
        };
        assert_eq!(place.name, "air");
        assert!(because
            .iter()
            .any(|b| b.starts_with("no machine that answers has")));
        // A preferred machine that does not answer is passed over.
        let mut table = three();
        table[2].state = HostState::Unknown;
        assert_eq!(go_to(&decide(&mac_host(), &table, None)), "air");
        // A preference never lets in a machine that lacks the needs.
        let linux_pref = task_with("build", r#"{"os": "macos"}"#, r#"{"os": "linux"}"#);
        assert_eq!(go_to(&decide(&linux_pref, &three(), None)), "air");
        // Deploy does not pick in order, so it does not look at preferences.
        let deploy = task_with(
            "deploy",
            r#"{"roles": ["always_on_candidate"]}"#,
            r#"{"os": "macos"}"#,
        );
        assert!(matches!(
            decide(&deploy, &three(), None),
            Decision::Ambiguous { .. }
        ));
        // An unknown preference is not decided either.
        let unknown = task_with("run", "{}", r#"{"roles": ["quiet"]}"#);
        assert!(matches!(
            decide(&unknown, &three(), None),
            Decision::NotUnderstood { .. }
        ));
    }

    #[test]
    fn this_machine_comes_first() {
        let decision = decide(&task("build", r#"{"tools": ["cargo"]}"#), &three(), None);
        let Decision::Go {
            place,
            because,
            remembered,
        } = decision
        else {
            panic!()
        };
        assert!(place.local);
        assert_eq!(remembered, None);
        assert!(because.iter().any(|b| b == "has cargo"));
        assert!(because.iter().any(|b| b.contains("this machine")));
    }

    #[test]
    fn a_machine_that_answers_comes_before_one_that_does_not() {
        let mut table = three();
        table.retain(|c| !c.local);
        // tower: load 0.1; mini: load 0.3. Put tower to sleep.
        table[0].state = HostState::Unknown;
        table[0].status = None;
        let decision = decide(&task("build", "{}"), &table, None);
        assert_eq!(go_to(&decision), "mini");
    }

    #[test]
    fn lower_load_comes_first() {
        let mut table = three();
        table.retain(|c| !c.local);
        let decision = decide(&task("build", r#"{"tools": ["cargo"]}"#), &table, None);
        assert_eq!(go_to(&decision), "tower");
        let Decision::Go { because, .. } = decision else {
            panic!()
        };
        assert!(because.iter().any(|b| b.contains("lowest load (0.10)")));

        table[0].status.as_mut().unwrap().load = Some(0.9);
        assert_eq!(go_to(&decide(&task("build", "{}"), &table, None)), "mini");
        // An unknown load comes after a known one.
        table[1].status = None;
        assert_eq!(go_to(&decide(&task("build", "{}"), &table, None)), "tower");
    }

    #[test]
    fn machines_that_meet_the_needs_but_do_not_answer_stop_it() {
        let decision = decide(
            &task("run", r#"{"os": "linux", "roles": ["gpu"]}"#),
            &asleep_table(),
            None,
        );
        let Decision::Stopped { places, remembered } = &decision else {
            panic!("{decision:?}")
        };
        assert!(!remembered);
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].place.name, "tower");
        assert!(places[0].wake_possible);
        assert_eq!(places[0].seen_at, Some(1_794_870_000));
        assert_eq!(decision.exit_code(), crate::exit::STOPPED);
    }

    #[test]
    fn nobody_meets_the_needs_and_each_machine_says_why() {
        let decision = decide(
            &task(
                "build",
                r#"{"os": "windows", "arch": "riscv64", "tools": ["docker"]}"#,
            ),
            &two(),
            None,
        );
        let Decision::Nobody { lacking } = &decision else {
            panic!("{decision:?}")
        };
        assert_eq!(lacking.len(), 2);
        let air = &lacking[0];
        assert_eq!(air.place.name, "air");
        assert!(air.lacks.contains(&"runs macOS, not Windows".to_owned()));
        assert!(air.lacks.contains(&"is aarch64, not riscv64".to_owned()));
        assert!(air.lacks.contains(&"does not have docker".to_owned()));
        let tower = &lacking[1];
        assert!(!tower.lacks.iter().any(|l| l.contains("docker")));
        assert_eq!(decision.exit_code(), crate::exit::NOBODY);
    }

    #[test]
    fn a_machine_without_a_role_table_meets_only_empty_needs() {
        let table = read_table(r#"[{"id": "x1", "name": "blank", "state": "awake"}]"#).unwrap();
        assert_eq!(go_to(&decide(&task("build", "{}"), &table, None)), "blank");
        assert_eq!(
            go_to(&decide(
                &task("build", r#"{"host": "blank"}"#),
                &table,
                None
            )),
            "blank"
        );
        let decision = decide(&task("build", r#"{"tools": ["cargo"]}"#), &table, None);
        let Decision::Nobody { lacking } = decision else {
            panic!()
        };
        assert!(lacking[0].lacks[0].contains("no role table"));
    }

    #[test]
    fn a_named_host_is_the_only_one_considered() {
        let table = two();
        let by_id = decide(
            &task("build", r#"{"host": "9f8e-7d6c-5b4a-3921"}"#),
            &table,
            None,
        );
        assert_eq!(go_to(&by_id), "tower");
        let by_name = decide(&task("build", r#"{"host": "tower"}"#), &table, None);
        assert_eq!(go_to(&by_name), "tower");
        let Decision::Go { because, .. } = by_name else {
            panic!()
        };
        assert!(because.contains(&"is the machine the task names".to_owned()));

        let none = decide(&task("build", r#"{"host": "nowhere"}"#), &table, None);
        let Decision::Nobody { lacking } = none else {
            panic!()
        };
        assert!(lacking.iter().all(|l| l.lacks[0].contains("`nowhere`")));
        // A named host must still meet the other needs.
        let lacking_role = decide(
            &task("build", r#"{"host": "tower", "roles": ["xcode_build"]}"#),
            &table,
            None,
        );
        assert!(matches!(lacking_role, Decision::Nobody { .. }));
    }

    #[test]
    fn unknown_needs_are_not_decided() {
        let decision = decide(&task("build", r#"{"roles": ["quantum"]}"#), &two(), None);
        assert_eq!(
            decision,
            Decision::NotUnderstood {
                needs: vec!["roles: quantum".into()]
            }
        );
        assert_eq!(decision.exit_code(), crate::exit::NOT_UNDERSTOOD);
    }

    #[test]
    fn several_match_is_decided_in_one_place() {
        assert_eq!(
            when_several_match(TaskKind::Build),
            SeveralMatch::PickInOrder
        );
        assert_eq!(when_several_match(TaskKind::Run), SeveralMatch::PickInOrder);
        assert_eq!(
            when_several_match(TaskKind::Deploy),
            SeveralMatch::StopAndList
        );
    }

    const SITE: &str = r#"{"roles": ["always_on_candidate"]}"#;

    #[test]
    fn a_deploy_with_one_place_goes_there() {
        let decision = decide(&task("deploy", SITE), &two(), None);
        assert_eq!(go_to(&decision), "tower");
        let Decision::Go {
            because,
            remembered,
            ..
        } = decision
        else {
            panic!()
        };
        assert_eq!(remembered, None);
        assert!(because.iter().any(|b| b.contains("only machine")));
    }

    #[test]
    fn a_deploy_with_one_place_that_does_not_answer_stops() {
        let decision = decide(&task("deploy", SITE), &asleep_table(), None);
        assert!(matches!(
            decision,
            Decision::Stopped {
                remembered: false,
                ..
            }
        ));
    }

    #[test]
    fn a_deploy_with_two_places_stops_and_lists_them() {
        let decision = decide(&task("deploy", SITE), &three(), None);
        let Decision::Ambiguous { places, stale } = &decision else {
            panic!("{decision:?}")
        };
        assert!(stale.is_none());
        assert_eq!(
            places.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["tower", "mini"]
        );
        assert_eq!(decision.exit_code(), crate::exit::AMBIGUOUS);

        // Also when one of the two does not answer.
        let mut table = three();
        table[1].state = HostState::Unknown;
        table[1].status = None;
        assert!(matches!(
            decide(&task("deploy", SITE), &table, None),
            Decision::Ambiguous { .. }
        ));
    }

    fn chose(name: &str, id: &str) -> Remembered {
        Remembered {
            host: id.into(),
            name: name.into(),
            chosen_at: 1_794_879_000,
        }
    }

    const TOWER_ID: &str = "9f8e-7d6c-5b4a-3921";

    #[test]
    fn a_remembered_place_is_used_and_marked() {
        let decision = decide(
            &task("deploy", SITE),
            &three(),
            Some(&chose("tower", TOWER_ID)),
        );
        assert_eq!(go_to(&decision), "tower");
        let Decision::Go {
            remembered,
            because,
            ..
        } = decision
        else {
            panic!()
        };
        assert_eq!(remembered, Some(1_794_879_000));
        assert!(because.iter().any(|b| b.contains("chosen by a person")));
    }

    #[test]
    fn a_remembered_place_that_does_not_answer_stops_without_going_elsewhere() {
        let mut table = three();
        table[1].state = HostState::Unknown;
        table[1].status = None;
        let decision = decide(
            &task("deploy", SITE),
            &table,
            Some(&chose("tower", TOWER_ID)),
        );
        let Decision::Stopped { places, remembered } = decision else {
            panic!()
        };
        assert!(remembered);
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].place.name, "tower");
    }

    #[test]
    fn a_remembered_place_that_lost_the_role_stops_with_the_reason() {
        let mut table = three();
        table[1]
            .manifest
            .as_mut()
            .unwrap()
            .roles
            .retain(|r| *r != istesso_proto::Role::AlwaysOnCandidate);
        let decision = decide(
            &task("deploy", SITE),
            &table,
            Some(&chose("tower", TOWER_ID)),
        );
        let Decision::Ambiguous { places, stale } = decision else {
            panic!()
        };
        // Only mini meets the needs now, and it still stops.
        assert_eq!(places.len(), 1);
        assert_eq!(places[0].name, "mini");
        let (chosen, why) = stale.unwrap();
        assert_eq!(chosen.name, "tower");
        assert_eq!(
            why,
            Stale::NoLongerMeets(vec!["does not have the role always_on_candidate".into()])
        );
    }

    #[test]
    fn a_remembered_place_that_is_gone_stops_with_the_reason() {
        let mut table = three();
        table.retain(|c| c.name != "tower");
        let decision = decide(
            &task("deploy", SITE),
            &table,
            Some(&chose("tower", TOWER_ID)),
        );
        let Decision::Ambiguous { places, stale } = decision else {
            panic!()
        };
        assert_eq!(places.len(), 1);
        assert_eq!(stale.unwrap().1, Stale::Gone);
    }

    #[test]
    fn build_and_run_do_not_look_at_remembered_places() {
        let remembered = chose("tower", TOWER_ID);
        for kind in ["build", "run"] {
            let decision = decide(&task(kind, "{}"), &three(), Some(&remembered));
            assert_eq!(go_to(&decision), "air", "{kind}");
            let Decision::Go { remembered, .. } = decision else {
                panic!()
            };
            assert_eq!(remembered, None);
        }
    }
}
