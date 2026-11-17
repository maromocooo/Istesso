//! The skill and the entry line say what the command does: every command
//! they name exists, and the exit codes they list are the code's.

use std::collections::BTreeSet;
use std::process::Command;

use istesso_route::exit;

const BIN: &str = env!("CARGO_BIN_EXE_istesso-route");
const SKILL: &str = include_str!("../agent/skill/SKILL.md");
const ENTRY: &str = include_str!("../agent/entry.md");

fn help(args: &[&str]) -> String {
    let output = Command::new(BIN).args(args).arg("--help").output().unwrap();
    assert!(output.status.success(), "{args:?} --help failed");
    String::from_utf8(output.stdout).unwrap()
}

/// The words that follow `istesso-route` in a text, e.g. `where`, `check`.
fn subcommands(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for (i, _) in text.match_indices("istesso-route ") {
        let rest = &text[i + "istesso-route ".len()..];
        let word: String = rest
            .chars()
            .take_while(|c| c.is_ascii_lowercase() || *c == '-')
            .collect();
        if !word.is_empty() {
            found.insert(word);
        }
    }
    found
}

/// Whether `help` lists `name` as a subcommand (an indented line that starts with it).
fn lists(help: &str, name: &str) -> bool {
    help.lines().any(|line| {
        let line = line.trim_start();
        line.split_whitespace().next() == Some(name)
    })
}

#[test]
fn every_command_the_skill_names_exists() {
    let top = help(&[]);
    let named = subcommands(SKILL);
    for command in [
        "where", "check", "build", "run", "deploy", "choose", "forget", "choices",
    ] {
        assert!(
            named.contains(command),
            "the skill does not name `{command}`"
        );
    }
    for command in &named {
        assert!(
            lists(&top, command),
            "`istesso-route {command}` is not in --help:\n{top}"
        );
    }
    // `check deploy`.
    assert!(SKILL.contains("istesso-route check deploy"));
    assert!(lists(&help(&["check"]), "deploy"));
    // `--json` and `--hosts`.
    let where_help = help(&["where"]);
    assert!(SKILL.contains("--json") && where_help.contains("--json"));
    assert!(SKILL.contains("--hosts") && where_help.contains("--hosts"));
}

#[test]
fn the_exit_codes_in_the_skill_are_the_code_s() {
    let section = SKILL
        .split("## Exit codes")
        .nth(1)
        .and_then(|rest| rest.split("\n## ").next())
        .expect("the skill has an exit code section");
    let listed: BTreeSet<i32> = section
        .lines()
        .filter_map(|line| line.strip_prefix("| "))
        .filter_map(|line| line.split(' ').next())
        .filter_map(|code| code.parse().ok())
        .collect();
    let known: BTreeSet<i32> = exit::ALL.iter().map(|(code, _)| *code).collect();
    assert_eq!(listed, known);
}

#[test]
fn the_entry_is_one_line_naming_the_command_and_the_file() {
    let lines: Vec<&str> = ENTRY.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 1);
    let line = lines[0];
    assert!(line.contains("istesso-route <build|run|deploy> <task>"));
    assert!(line.contains(".istesso/tasks.json"));
    let top = help(&[]);
    for command in ["build", "run", "deploy"] {
        assert!(lists(&top, command));
    }
}
