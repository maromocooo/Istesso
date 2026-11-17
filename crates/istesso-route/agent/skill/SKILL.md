---
name: istesso-route
description: Decide which machine a build, run or deploy task goes to before starting it, by asking `istesso-route`. Use before every build, run or deploy in a project that has .istesso/tasks.json.
---

# Where a task goes: istesso-route

The project names its build, run and deploy tasks in `.istesso/tasks.json`,
with what each one needs from the machine it runs on. `istesso-route`
compares those needs with the role table of the paired machines and says
which machine a task goes to, and why. Do not choose a machine yourself, and
do not keep a list of machines in your head: ask the command every time.

## When to use it

Before any build, run or deploy that the project lists as a task. Look in
`.istesso/tasks.json` for the task's name and kind (`build`, `run` or
`deploy`).

## The role table

Every command that decides needs the role table. Pass it on standard input
or with `--hosts <file>`:

```sh
istesso hosts --json | istesso-route where build test
istesso-route where build test --hosts hosts.json
```

## Commands

| Command | What it does |
|---|---|
| `istesso-route where <build\|run\|deploy> <task>` | Says which machine the task goes to, and why. Runs nothing. `--json` prints the answer as JSON. |
| `istesso-route check deploy <task>` | One line: does the deploy task have a single place that answers now? |
| `istesso-route build <task>` | Runs a build task, when its place is this machine. |
| `istesso-route run <task>` | Runs a run task, when its place is this machine. |
| `istesso-route deploy <task>` | Runs a deploy task, when its place is this machine. `--json` ends with one JSON line. |
| `istesso-route choose <task> <machine>` | Remembers the machine a person chose for a deploy task. |
| `istesso-route forget <task>` | Forgets the remembered machine of a deploy task. |
| `istesso-route choices` | Lists the remembered choices and where they are kept. |

## Exit codes

Read the exit code first; the text says the same in words.

| Code | Meaning | What to do |
|---|---|---|
| 0 | A place was decided; for `build`, `run` and `deploy`, the task ended with 0. | Go on. |
| 2 | The command was written wrongly. | Fix the command. |
| 70 | No machine meets the task's needs. | Tell the person what each machine lacks. |
| 71 | Machines meet the needs, but none of them answers. | Tell the person which machine is needed and whether it can be woken. Do not run the task anywhere else. |
| 72 | A deploy task has no single place: none was chosen, or the chosen one cannot be used any more. | Show the person the machines that were listed, and the reason if there is one. Do not choose yourself. When the person has answered, run `istesso-route choose <task> <machine>` with their answer. |
| 73 | A file cannot be read, there is no such task, or the task is of another kind. | Read the message; fix the command or tell the person. |
| 74 | The task states needs this version cannot decide on. | Tell the person; do not run the task anyway. |
| 75 | The place is another machine; nothing was run. | Running on other machines is not built yet. Tell the person which machine the task goes to. |
| 127 | The task's program could not be started. | Tell the person; the task's `command` may be wrong. |

When `build`, `run` or `deploy` ran the task here, the command ends with the
task's own exit code, which may be any number, these included. So these three
commands always end with one fixed line on standard error that says whether
the task ran; read that line, not the exit code alone:

```text
istesso-route: ran build `test` here; it ended with 3
istesso-route: did not run deploy `site`; exit code 72
```

With `--json`, that last line is one JSON object instead, with `ran` (true or
false), `exit_code` and `decision`.

## Rules

- Never pick a machine yourself, and never run a task on another machine by
  other means (ssh, a remote shell) to get around 71, 72 or 75.
- Never write machine names, ids or addresses into project files. Choices
  are kept per device and user, outside the project, by `choose`.
- A deploy goes to the same machine once a person has chosen it; the answer
  then says `remembered`. If the remembered machine no longer fits, the
  command stops with 72 and says why: ask the person again.
- Do not put secrets (keys, tokens, passwords) in task commands or in
  arguments; other users of the same machine can see a program's arguments.
