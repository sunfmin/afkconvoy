//! Reads a repo's config out of its markdown file.

use std::path::{Path, PathBuf};

use afkconvoy_core::config::{Config, GateNeed, Settings};
use afkconvoy_core::cut;
use afkconvoy_gate::declared::{self, Fault, fenced_all};
use anyhow::{Context, Result, anyhow};

use crate::git::Repo;

/// Where a repo keeps its config, relative to the checkout's top.
pub const FILE: &str = "docs/agents/afkconvoy.md";

/// A warning budget can follow the main block so older afkconvoy versions
/// can still gate and land the change that introduces it.
pub const BUDGET_BLOCK: &str = "```yaml gate-budget";

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GateBudget {
    gate_budget_seconds: u64,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct GateNeeds {
    checks: Vec<GateNeed>,
}

/// The config file of the checkout `repo`.
pub fn file_of(repo: &Repo) -> PathBuf {
    repo.dir.join(FILE)
}

pub fn load(file: &Path) -> Result<Config> {
    let text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read the config file {}", file.display()))?;
    parse(&text).with_context(|| format!("config file {}", file.display()))
}

/// The config in a markdown text: its first ```` ```yaml ```` block, and the
/// optional gate-budget blocks, the fast and slow gate of its ```` ```yaml gates ```` block
/// (`GATES_BLOCK`) and the cut settings across its ```` ```yaml cut ```` blocks (`cut::BLOCK`). New
/// settings can follow the first cut block: older versions read only that
/// first block and skip additions, while duplicate keys still fail here.
pub fn parse(markdown: &str) -> Result<Config> {
    let yaml = yaml_block(markdown).ok_or_else(|| anyhow!("has no ```yaml block"))?;
    let mut settings: Settings = serde_yaml::from_str(&yaml)?;
    let budgets = fenced_all(markdown, BUDGET_BLOCK);
    if !budgets.is_empty() {
        let budget: GateBudget =
            serde_yaml::from_str(&budgets.join("\n")).context("its ```yaml gate-budget blocks")?;
        if settings.gate_budget_seconds.replace(budget.gate_budget_seconds).is_some() {
            return Err(anyhow!(
                "duplicate field `gate_budget_seconds`: declare it in one block only"
            ));
        }
    }
    let blocks = fenced_all(markdown, cut::BLOCK);
    let steps = match blocks.is_empty() {
        false => Some(cut_steps(&blocks.join("\n")).context("its ```yaml cut blocks")?),
        true => None,
    };
    let config = Config::new(settings)?.cutting(steps);
    let gates = declared::gates(markdown, config.gate_seconds(), config.gate_budget_seconds)
        .map_err(said)
        .context("its ```yaml gates block")?;
    let mut config = config.split(gates);
    for (block, target) in
        [("gate-needs", &mut config.gate_needs), ("cut-needs", &mut config.cut_needs)]
    {
        let blocks = fenced_all(markdown, &format!("```yaml {block}"));
        if blocks.is_empty() {
            continue;
        }
        let needs: GateNeeds = serde_yaml::from_str(&blocks.join("\n"))
            .with_context(|| format!("its ```yaml {block} block"))?;
        let mut names = std::collections::HashSet::new();
        for check in &needs.checks {
            anyhow::ensure!(
                !check.name.trim().is_empty()
                    && !check.command.trim().is_empty()
                    && check.seconds > 0,
                "{block}: each check needs a name, command and positive seconds"
            );
            anyhow::ensure!(
                match (&check.install, check.install_seconds) {
                    (None, None) => true,
                    (Some(install), Some(seconds)) => !install.trim().is_empty() && seconds > 0,
                    _ => false,
                },
                "{block}: check {} gives `install` and positive `install_seconds` together, or neither",
                check.name
            );
            anyhow::ensure!(
                names.insert(&check.name),
                "{block}: duplicate check name {}",
                check.name
            );
        }
        *target = needs.checks;
    }
    config.heavy_commands = heavy_commands(markdown)?;
    // An outer gate/land would ask for the lane a second time before it
    // could even reach the inherited gate wrapper.
    if config.gate_command.split_whitespace().collect::<Vec<_>>().windows(2).any(|pair| {
        pair[0].trim_matches(['\'', '"']).ends_with("afkconvoy")
            && matches!(pair[1].trim_matches(['\'', '"']), "gate" | "land")
    }) {
        return Err(anyhow!(
            "gate_command must name the build-and-test command, never `afkconvoy gate` or `land`; set `gate_command: afkconvoy exclusive -- ./gate.sh` for a heavy gate, or `gate_command: ./gate.sh` for a light gate"
        ));
    }
    Ok(config)
}

/// The heavy commands a markdown text declares in its ```` ```yaml
/// heavy-commands ```` blocks (`HEAVY_BLOCK`): none where it has none. Each
/// is a pattern of words, nonempty and declared once.
pub fn heavy_commands(markdown: &str) -> Result<Vec<String>> {
    declared::heavy_commands(markdown).map_err(|fault| match fault {
        Fault::Yaml(error) => {
            anyhow::Error::from(error).context("its ```yaml heavy-commands block")
        }
        Fault::Said(why) => anyhow!(why),
    })
}

/// A fault of a block, as the error it is.
fn said(fault: Fault) -> anyhow::Error {
    match fault {
        Fault::Yaml(error) => anyhow::Error::from(error),
        Fault::Said(why) => anyhow!(why),
    }
}

/// The heavy commands the config file `file` declares, read off their block
/// alone: what the guard asks at each call, which a fault elsewhere in the
/// file does not change. None where the file cannot be read.
pub fn heavy_commands_of(file: &Path) -> Result<Vec<String>> {
    match std::fs::read_to_string(file) {
        Ok(text) => {
            heavy_commands(&text).with_context(|| format!("config file {}", file.display()))
        }
        Err(_) => Ok(vec![]),
    }
}

pub use afkconvoy_gate::declared::heavy_gate;

/// The cut steps a ```` ```yaml cut ```` block says: `set` and `publish`,
/// each something to run, and optionally `initial`, three version numbers.
fn cut_steps(yaml: &str) -> Result<cut::Steps> {
    let steps: cut::Steps = serde_yaml::from_str(yaml)?;
    steps.checked().map_err(|key| match key {
        "initial" => anyhow!("`initial` must be X.Y.Z, three whole numbers without leading zeroes or a v prefix; set `initial: 0.1.0` or remove it"),
        _ => anyhow!("`{key}` names nothing to run"),
    })
}

/// The text of the first ```` ```yaml ```` block of a markdown text: the one
/// main config block; gate-budget and cut settings may follow separately.
pub fn yaml_block(markdown: &str) -> Option<String> {
    fenced(markdown, "```yaml")
}

/// The text of the first fenced block of a markdown text whose opening line
/// is `opening`, and nothing else but trailing space.
fn fenced(markdown: &str, opening: &str) -> Option<String> {
    fenced_all(markdown, opening).into_iter().next()
}

/// What a markdown text says above its first ```` ```yaml ```` block, with no
/// space around it; nothing where it has no such block.
pub fn prose(markdown: &str) -> String {
    let above: Vec<&str> =
        markdown.lines().take_while(|line| line.trim_end() != "```yaml").collect();
    if above.len() == markdown.lines().count() {
        return String::new();
    }
    above.join("\n").trim().to_string()
}

/// What `afkconvoy config --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

First, before `plan` and `run`: it shows what the fleet goes by in this repo, or why the config cannot be read. Run it again after editing the config.

## Who runs it

The operator, in a checkout of the repo.

## What it reads and writes

It reads one file — {const:CONFIG_FILE} of the checkout it is about, or `--file` — and of that file the first ```yaml block and any ```yaml gate-budget blocks. It writes nothing, and reads neither GitHub nor the remote.

## The config file

A markdown file. The keys of its ```yaml block are the keys of the answer below, named and written alike: a file holding every key, each at its default, is

~~~markdown
# afkconvoy config

```yaml
base_branch: "main"                # required: it has no default
gate_command: ./gate.sh            # required: it has no default
gate_seconds: {const:GATE_SECONDS}
ready_label: ready-for-agent
epic_labels: [epic, prd]
escalate_label: ready-for-human
lock_label: fleet-lock
team: acme/team                    # naming a team binds the repo
patrols: []
intake_limit: {const:INTAKE_LIMIT}
```
~~~

The optional `gate_budget_seconds` is omitted from this example: it has no default of its own. A key left out is at the value shown, so the shortest config is the first two lines, and a bound repo's those and `team`. The last two are read only beside `team`: an unbound repo leaves all three out, and may say one key more, never beside `team`:

```
escalate_to: human                 # legacy consult is accepted; both escalate for the watch
```

An optional `gate_budget_seconds` declares a fixed warning budget: a positive whole number of seconds below `gate_seconds`. It never ends a run. Put it in the first yaml block, or in a separate block opened by ```yaml gate-budget; that block accepts only `gate_budget_seconds`. Declare it once: duplicate keys fail, including across blocks. Older versions skip the separate block and keep their termination limit, so they can land and cut the version that first supports the budget. They keep warning at {const:SLOW_GATE_PERCENT}% of that limit until upgraded. The budget is fixed in the repo, with its machine class, baseline version, cache conditions and rationale; it never follows recent runs upward. Queue waiting is outside both durations.

A repo may split its gate in two, in a block of its own opened by the line ```yaml gates: the fast gate, what a worker passes before it opens or updates its PR, and the slow gate, everything else the gate checks. Each says its `command`, its limit `seconds` and its warning budget `budget_seconds`:

~~~text
```yaml gates
fast:
  command: afkconvoy exclusive -- ./gate.sh fast
  seconds: 600
  budget_seconds: 300
slow:
  command: afkconvoy exclusive -- ./gate.sh slow
  seconds: 3000
  budget_seconds: 300
```
~~~

The two are the gate split, not a gate more: together they check what `gate_command` checks: a landing turn runs the fast gate per candidate and the slow gate once per round (`afkconvoy land --help`), and a base tip with no record is gated with `gate_command` whole. So each budget is below its limit, their limits add up to no more than `gate_seconds`, and their budgets to no more than `gate_budget_seconds` where it is declared. `afkconvoy gate --fast` and `--slow` run each alone (`afkconvoy gate --help`). Older versions skip the block.

What is outside these blocks is yours to explain it in — but for a second block opened by the line ```yaml cut, which names the repo's cut steps and turns cutting on (`afkconvoy cut --help`). It is read beside this one; older versions read only this one, and refuse nothing for it.

No command writes this file: write it, commit it, and run `afkconvoy config`.

## What it answers

{"base_branch", "gate_command", "gate_seconds", "ready_label", "epic_labels", "escalate_label", "lock_label"} — every key, the defaults filled in. `gate_seconds` is the gate limit: how many seconds a run of `gate_command` is given, by `afkconvoy gate` and by a landing turn alike, before it is ended and is red (default {const:GATE_SECONDS}). It is written nowhere else: neither command has a flag for it. A config that leaves it out is a finding of `afkconvoy doctor`. `lock_label` is the label of a fleet lock (default fleet-lock): while an open issue carries it, the fleet works lock issues only (`afkconvoy lock --help`). How many claims a fleet instance holds at work at once is no key: it is its checkout's, said on the host that works it (`afkconvoy run --concurrency`), and {const:CONCURRENCY} until that host says otherwise. A config that still names `concurrency` is read all the same, but for that key, which changes nothing: `afkconvoy doctor` reports it as `info`, since nothing is held up, with the file and key to delete. Where the checkout says none and the config names a different number than the one in force, its fix first gives the `afkconvoy run` that keeps that number on the host. `afkconvoy plan` says the concurrency in force, and who says it.

A repo may declare host requirements in a separate block, which older releases read past:

```yaml gate-needs
checks:
  - name: compiler
    command: test "$(compiler --version)" = "1.0"
    seconds: 10
```
A separate `yaml cut-needs` block uses the same `checks` form for cutting. Before claiming a due cut, the host runs these checks; a failure leaves the cut for another host and is named in the tick, plan and doctor. Output follows the same privacy rules.

Each name is unique and nonempty, each command nonempty, and seconds positive. Each runs with `sh -e` in the checkout, with the host's environment — a fresh login shell's, as the host runner has — never that of the command that set it off: a worker's sandbox proxy, `TMPDIR` and tool homes do not reach it. The startup probe refuses a failed or timed-out check; a passing check stands for an hour. A check's stdout is discarded. On failure its last 20 stderr lines are stored in a user-only file in the host cache, overwritten by its next run; the refusal names the file, never its contents.

A check may declare how to install what it checks, with `install` and `install_seconds` together (both or neither):

```yaml gate-needs
checks:
  - name: flux-checker
    command: ./verify.sh pins
    seconds: 10
    install: ./verify.sh install
    install_seconds: 5400
```
Where such a check fails, the host runs its install with `sh -e` in the checkout, in the host's exclusive queue (`afkconvoy exclusive --help`) as a heavy command, within `install_seconds` — past it, it is ended and named as timed out — then the check again. A pass admits; a fail refuses naming the check, the install's status and the user-only host-cache file of its last 20 stderr lines; its stdout is discarded, as a check's is. A failed install is not run again for an hour unless the check's declaration changes, which is tried at once; it stands only while its check fails: a later check that passes on this host drops it. A failed install a worker's command set off (one with AFKCONVOY_ISSUE or AFKCONVOY_PATROL) is not noted: its harness's sandbox may have failed it, so it is not said as failed on this host and does not hold the host's own install. A running fleet instance's hourly recheck installs the same way before it ends broken, and a due cut installs before it is claimed. The startup probe warns when an install ran and passed; `plan` answers each install noted on the host as `need_installs` [{kind, check, at, seconds, status, passed, stderr}]; and `doctor` says each as `was installed on this host` or `install failed on this host`, never its output.

The contract of an install: it writes only under the user's home, never to system directories and never with sudo, and installs the pinned versions the repo already names, reading them from where the repo keeps them, not from a copy. The owner gives standing consent for exactly this — what a repo declares as a gate or cut need, at the version it pins, installed under the user's home (sunfmin/control#499); anything else still asks first.

A repo may declare its heavy commands in a separate block, which older releases read past: what fills the host by itself, which a worker runs only through the host's exclusive queue (`afkconvoy exclusive --help`). Each is a pattern of words:

```yaml heavy-commands
commands:
  - cargo build
  - cargo nextest
  - ./gate.sh
```
A simple command of a worker's shell call runs a pattern when its words, from its program on, begin with the pattern's; a pattern's program with no `/` matches a program by its file name, so `cargo build` matches `~/.cargo/bin/cargo build --release` but not `cargo fmt`. The guard refuses such a call outside `afkconvoy exclusive`, naming the pattern and the line to run instead (`afkconvoy guard --help`): `cd server && cargo nextest run -p x` is refused for `afkconvoy exclusive -- cargo nextest run -p x`, and allowed so wrapped. What `afkconvoy gate`, `land` and `cut` run is never refused: it already queues. Each pattern is nonempty and declared once. The answer has them as {"heavy_commands"}, a list, where the file declares any; a repo that declares none sees no change.

Where a budget is declared, the answer also has `gate_budget_seconds`. The last {const:SLOW_GATE_LANDINGS} recorded landings on one host must each exceed it to warn through the existing slow-gate issue and doctor finding. Without it the warning threshold stays {const:SLOW_GATE_PERCENT}% of `gate_seconds`. A warning neither ends nor fails a gate.

A repo whose config names a team is a bound repo, and answers three keys more: {"team", "patrols", "intake_limit"} — the team's directory as owner/repo or owner/repo/path (a whole repo, or a subdirectory of one), which of its patrols run here, by name (default none; `afkconvoy check` holds them against the team's, and `afkconvoy tick --help` says when one runs), how many issues taken in by workers may be open (default {const:INTAKE_LIMIT}). A config names no harness: what each worker runs is its team's `roster`'s to say (`afkconvoy help roster`), within what the host kit holds (`afkconvoy kit --help`). A repo that names no team is unbound and answers the seven above and one legacy key: {"escalate_to"}, always `human`. Existing `consult` values still load, but every hand-off escalates for the host's watch to answer. Only a consult explicitly named by a bound repo's stage is asked before escalation (`afkconvoy help humans`).

Where the file names cut steps, the answer has one key more: {"cut": {"set", "publish", "initial"?}} — each as the ```yaml cut block says it. Optional `initial: X.Y.Z` declares the first version to cut when the remote has no version tag; after the first tag it is ignored. Where it splits its gate, one more: {"gates": {"fast", "slow"}}, each {"command", "seconds", "budget_seconds"}.

Errors: the file cannot be read; it has no ```yaml block; a ```yaml cut block has a key but `set`, `publish` and `initial`, lacks a required step, a step names nothing to run, or `initial` is not X.Y.Z; `base_branch` or `gate_command` is missing; a key is unknown or of the wrong type; `gate_seconds` is not a whole number from 1; `gate_budget_seconds` is not a positive whole number below `gate_seconds`, is declared twice, or its separate block has another key; the ```yaml heavy-commands block has a key but `commands`, an empty pattern or one declared twice; the ```yaml gates block lacks `fast` or `slow` or one of their keys, has another key, a gate names nothing to run, has no seconds or a budget not below them, or the two gates' limits or budgets add up past the gate's; `team` is neither owner/repo nor owner/repo/path; a `harness`, which a config names no more — the error says the line to delete; `patrols` or `intake_limit` is set where no `team` is; `escalate_to` is neither consult nor human, or is set beside `team` — the error names the key. Without `--file`, outside a git checkout: "is not in a git checkout", or git's own words for a checkout it refuses.

## What trips you up

- It reads the config only. Whether the team it names loads is `afkconvoy check`.
- It reads the file as it is in this checkout, committed or not. A worker reads the one in its own worktree, cut from its target branch. The destination remains pinned even if that file names a different configured base."#;

#[cfg(test)]
mod tests {
    use super::*;

    fn error(yaml: &str) -> String {
        format!("{:#}", parse(&format!("intro\n```yaml\n{yaml}\n```\n")).unwrap_err())
    }

    #[test]
    fn only_the_yaml_block_is_read() {
        let text = "# title\nbase_branch: nope\n```yaml\nbase_branch: main\ngate_command: make\n```\nmore\n";
        let config = parse(text).unwrap();
        assert_eq!(config.base_branch, "main");
        assert_eq!(config.gate_command, "make");
    }

    #[test]
    fn existing_exclusive_gate_wrappers_use_the_worker_lanes_and_recursion_is_refused() {
        for command in [
            "afkconvoy exclusive -- ./gate.sh",
            "/opt/homebrew/bin/afkconvoy exclusive -- ./gate.sh",
            "'/opt/homebrew/bin/afkconvoy' exclusive -C . -- make test",
            "afkconvoy exclusive -- make gate",
        ] {
            let config = parse(&format!(
                "```yaml\nbase_branch: main\ngate_command: {}\n```\n",
                serde_json::to_string(command).unwrap()
            ))
            .unwrap();
            assert!(heavy_gate(&config.gate_command), "{command}");
        }
        assert!(!heavy_gate("./gate.sh"));
        for command in
            ["afkconvoy gate", "afkconvoy exclusive -- afkconvoy gate", "afkconvoy land --train"]
        {
            let why = error(&format!("base_branch: main\ngate_command: {command}"));
            assert!(why.contains("gate_command: afkconvoy exclusive -- ./gate.sh"), "{why}");
        }
        for key in ["gate_lanes", "train_lanes"] {
            assert!(
                error(&format!("base_branch: main\ngate_command: make\n{key}: 2")).contains(key)
            );
        }
    }

    #[test]
    fn the_prose_is_what_stands_above_the_yaml_block() {
        let text = "\n# Fix\n\nFor a defect.\n\n```yaml\nstages: []\n```\nnot this\n";
        assert_eq!(prose(text), "# Fix\n\nFor a defect.");
        assert_eq!(prose("```yaml\nstages: []\n```\nafter\n"), "");
        assert_eq!(prose("  \n```yaml\n```\n"), "");
        assert_eq!(prose("no block here\n"), "", "a file with no block says nothing");
    }

    #[test]
    fn a_file_with_no_block_is_no_config() {
        assert!(parse("base_branch: main\n").is_err());
        assert!(parse("```yaml\nbase_branch: main\n").is_err(), "an unclosed block");
    }

    #[test]
    fn an_unknown_key_is_named_not_ignored() {
        let e = error("base_branch: main\ngate_command: make\nretry: 2");
        assert!(e.contains("unknown field `retry`"), "{e}");
    }

    #[test]
    fn a_wrong_type_names_its_key() {
        for value in ["many", "0"] {
            let e = error(&format!("base_branch: main\ngate_command: make\nconcurrency: {value}"));
            assert!(e.contains("concurrency"), "{e}");
        }
        let e = error("base_branch: main\ngate_command: make\nepic_labels: epic");
        assert!(e.contains("epic_labels"), "{e}");
    }

    #[test]
    fn a_gate_limit_is_a_whole_number_of_seconds_from_1() {
        let limit =
            |value: &str| format!("base_branch: main\ngate_command: make\ngate_seconds: {value}");
        for wrong in ["0", "-5", "1.5", "soon", "[600]", "\"600\""] {
            let e = error(&limit(wrong));
            assert!(e.contains("gate_seconds"), "{wrong}: {e}");
        }
        let config = parse(&format!("```yaml\n{}\n```\n", limit("1"))).unwrap();
        assert_eq!(config.gate_seconds(), 1);
        // Budget rule enumeration belongs beside gate-limit parsing; the
        // CLI keeps one representative budget rejection to check its wiring.
        for wrong in ["0", "-1", "1.5", "soon", "[600]", "\"600\"", "3600"] {
            let e = error(&format!(
                "base_branch: main\ngate_command: make\ngate_budget_seconds: {wrong}"
            ));
            assert!(e.contains("gate_budget_seconds"), "{wrong}: {e}");
        }
        let e = error(
            "base_branch: main\ngate_command: make\ngate_seconds: 600\ngate_budget_seconds: 601",
        );
        assert!(e.contains("gate_budget_seconds"), "{e}");
        let valid = parse(
            "```yaml\nbase_branch: main\ngate_command: make\ngate_budget_seconds: 600\n```\n",
        )
        .unwrap();
        assert_eq!(valid.gate_budget_seconds, Some(600));
    }

    #[test]
    fn the_gate_split_in_two_is_a_block_of_its_own() {
        let main = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\ngate_seconds: 3600\ngate_budget_seconds: 600\n```\n";
        let gates = |fast: &str, slow: &str| {
            format!(
                "{main}```yaml gates\nfast:\n  command: ./gate.sh fast\n  {fast}\nslow:\n  command: ./gate.sh slow\n  {slow}\n```\n"
            )
        };
        let split = |fast: &str, slow: &str| parse(&gates(fast, slow));
        assert_eq!(parse(main).unwrap().gates, None);
        let config =
            split("seconds: 600\n  budget_seconds: 120", "seconds: 3000\n  budget_seconds: 480")
                .unwrap();
        let fast = config.gate(afkconvoy_core::config::Which::Fast).unwrap();
        assert_eq!(
            (fast.command.as_str(), fast.seconds, fast.budget_seconds),
            ("./gate.sh fast", 600, 120)
        );
        assert_eq!(config.gate(afkconvoy_core::config::Which::Slow).unwrap().seconds, 3000);
        // The whole gate is unchanged: a landing turn still runs it.
        assert_eq!((config.gate_command.as_str(), config.gate_seconds()), ("./gate.sh", 3600));
        for (fast, slow, key) in [
            (
                "seconds: 0\n  budget_seconds: 0",
                "seconds: 3000\n  budget_seconds: 480",
                "fast.seconds",
            ),
            (
                "seconds: 600\n  budget_seconds: 600",
                "seconds: 3000\n  budget_seconds: 480",
                "fast.budget_seconds",
            ),
            (
                "seconds: 600\n  budget_seconds: 120",
                "seconds: 3001\n  budget_seconds: 480",
                "past gate_seconds",
            ),
            (
                "seconds: 600\n  budget_seconds: 121",
                "seconds: 3000\n  budget_seconds: 480",
                "past gate_budget_seconds",
            ),
            ("seconds: 600", "seconds: 3000\n  budget_seconds: 480", "budget_seconds"),
            (
                "seconds: 600\n  budget_seconds: 120\n  retry: 2",
                "seconds: 3000\n  budget_seconds: 480",
                "retry",
            ),
        ] {
            let why = format!("{:#}", split(fast, slow).unwrap_err());
            assert!(why.contains(key) && why.contains("```yaml gates"), "{key}: {why}");
        }
    }

    #[test]
    fn the_cut_steps_are_a_block_of_their_own_which_turns_cutting_on() {
        let config = "```yaml\nbase_branch: main\ngate_command: make\n```\n";
        assert_eq!(parse(config).unwrap().cut, None, "no block: cutting is off");
        let steps =
            "```yaml cut\nset: ./bump \"$VERSION\"\npublish: |\n  ./ship\n  brew upgrade x\n```\n";
        let cut = parse(&format!("{config}\nTo cut:\n\n{steps}")).unwrap().cut.unwrap();
        assert_eq!(cut.set, "./bump \"$VERSION\"");
        assert_eq!(cut.publish, "./ship\nbrew upgrade x\n");
        assert_eq!(cut.busy_after_seconds, cut::BUSY_AFTER_SECONDS);
        assert_eq!(
            parse(&format!("{config}{steps}\n```yaml cut\nbusy_after_seconds: 0\n```\n"))
                .unwrap()
                .cut
                .unwrap()
                .busy_after_seconds,
            0,
            "zero is kept"
        );
        let steps = format!("{steps}\n```yaml cut\nbusy_after_seconds: 120\n```\n");
        assert_eq!(
            parse(&format!("{config}{steps}")).unwrap().cut.unwrap().busy_after_seconds,
            120
        );
        // An afkconvoy that reads the first block alone reads none of it.
        let both = format!("{config}{steps}");
        assert_eq!(yaml_block(&both).unwrap(), "base_branch: main\ngate_command: make\n");
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct OlderCut {
            set: String,
            publish: String,
        }
        let older: OlderCut = serde_yaml::from_str(&fenced(&both, cut::BLOCK).unwrap()).unwrap();
        assert_eq!(older.set, cut.set);
        assert_eq!(older.publish, cut.publish);
        let duplicate = format!("{both}\n```yaml cut\nbusy_after_seconds: 240\n```\n");
        assert!(format!("{:#}", parse(&duplicate).unwrap_err()).contains("duplicate field"));
        assert!(prose(&format!("{steps}{config}")).starts_with("```yaml cut"));
        let wrong = |steps: &str| {
            format!("{:#}", parse(&format!("{config}```yaml cut\n{steps}\n```\n")).unwrap_err())
        };
        let e = wrong("set: ./bump\npublish: ./ship\ndeploy: now");
        assert!(e.contains("```yaml cut") && e.contains("unknown field `deploy`"), "{e}");
        let e = wrong("set: ./bump");
        assert!(e.contains("publish"), "{e}");
        let e = wrong("set: ' '\npublish: ./ship");
        assert!(e.contains("`set` names nothing to run"), "{e}");
    }

    #[test]
    fn a_missing_required_key_is_named() {
        assert!(error("gate_command: make").contains("base_branch"));
        assert!(error("base_branch: main").contains("gate_command"));
    }

    #[test]
    fn an_initial_cut_declares_three_version_numbers() {
        let config = "```yaml\nbase_branch: main\ngate_command: make\n```\n";
        let declared = |initial: &str| {
            parse(&format!(
                "{config}```yaml cut\ninitial: {initial}\nset: bump\npublish: ship\n```\n"
            ))
        };
        for initial in ["0.0.0", "0.1.0", "12.34.56"] {
            let cut = declared(initial).unwrap().cut.unwrap();
            assert_eq!(cut.initial.as_deref(), Some(initial));
            assert_eq!(cut.initial_version().unwrap().to_string(), initial);
        }
        for wrong in [
            "v0.1.0",
            "0.1",
            "0.1.0-rc1",
            "0.01.0",
            "-1.2.3",
            "''",
            "[0, 1, 0]",
            "18446744073709551616.0.0",
        ] {
            let error = format!("{:#}", declared(wrong).unwrap_err());
            assert!(error.contains("initial"), "{wrong}: {error}");
        }
    }
}
