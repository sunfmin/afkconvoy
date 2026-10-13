//! The help: the top-level `--help` is the text an agent learns the tool from,
//! `-h` the one-screen listing, and the skill file only points at the former.
//! Each subcommand's `--help` is the next rung: what its one line leaves out.

mod common;

use std::collections::BTreeSet;
use std::process::Command;

use common::World;

const SKILL: &str = include_str!("../../../skills/afkconvoy/SKILL.md");
const README: &str = include_str!("../../../README.md");

/// What the binary prints for `args`, which must exit 0 and leave stderr empty.
fn asked(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(args).output().unwrap();
    assert!(out.status.success(), "{args:?} exits 0");
    assert!(out.stderr.is_empty(), "{args:?} prints to stdout only");
    String::from_utf8(out.stdout).unwrap()
}

/// The top-level help `flag` asks for.
fn help(flag: &str) -> String {
    asked(&[flag])
}

/// The lines of `text` from the one starting with `from` up to the one
/// starting with `to`, both left out.
fn between<'a>(text: &'a str, from: &str, to: &str) -> Vec<&'a str> {
    let after = text.lines().skip_while(|line| !line.starts_with(from)).skip(1);
    after.take_while(|line| !line.starts_with(to)).collect()
}

/// The subcommands clap lists: every visible one, but its own `help`.
fn listed(short: &str) -> BTreeSet<&str> {
    let names = between(short, "Commands:", "Options:").into_iter();
    names.filter_map(|line| line.split_whitespace().next()).filter(|name| *name != "help").collect()
}

/// The guide `--help` opens with: everything above clap's own listing.
fn guide(long: &str) -> &str {
    long.split("\nUsage:").next().unwrap()
}

/// The subcommands the guide sorts into groups: its lines of bare names.
fn grouped(guide: &str) -> Vec<&str> {
    let lines = between(guide, "THE COMMANDS", "RULES").into_iter();
    lines.filter(|line| line.starts_with("      ")).flat_map(str::split_whitespace).collect()
}

/// Every word the guide puts after `afkconvoy ` as a command to run.
fn mentioned(guide: &str) -> BTreeSet<&str> {
    let after = guide.split("afkconvoy ").skip(1);
    let words = after.filter_map(|rest| rest.split([' ', '`', '\n']).next());
    words.filter(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase())).collect()
}

#[test]
fn the_guide_groups_every_visible_subcommand_once_and_names_no_other() {
    let (short, long) = (help("-h"), help("--help"));
    let (listed, grouped) = (listed(&short), grouped(guide(&long)));
    assert!(listed.len() > 10, "{short}");
    assert_eq!(grouped.iter().copied().collect::<BTreeSet<_>>(), listed);
    assert_eq!(grouped.len(), listed.len(), "a subcommand is in two groups: {grouped:?}");
    // Nor does its prose tell anyone to run a command that does not exist.
    let prose: BTreeSet<_> =
        mentioned(guide(&long)).into_iter().filter(|w| *w != "works").collect();
    assert!(!prose.is_empty() && prose.is_subset(&listed), "{prose:?}");
}

#[test]
fn the_guide_carries_what_an_agent_needs_before_its_first_command() {
    let long = help("--help");
    let guide = guide(&long);
    for needed in [
        // What it needs.
        "docs/agents/afkconvoy.md",
        "`gh` logged in",
        "`orca` on the PATH",
        // The answer contract.
        "exactly one JSON object on stdout",
        "{\"error\": …}",
        "Warnings go to stderr",
        "one line per tick",
        // When to read a command's own help.
        "afkconvoy <command> --help",
        // The rules an agent would otherwise trip on.
        "One fleet instance per host per repo",
        "`run` never returns",
        "--worker-command",
    ] {
        assert!(guide.contains(needed), "the guide lost {needed:?}");
    }
    // What to run when something looks wrong is named first among the operator's.
    assert_eq!(group(guide, "  The operator")[0], "doctor");
    let lead = between(guide, "  The operator", "      doctor").join(" ");
    assert!(lead.contains("what to run when something looks wrong"), "{lead}");
    // The operator's flow, in order.
    let at = |command: &str| guide.find(&format!("afkconvoy {command} ")).unwrap();
    assert!(at("config") < at("plan") && at("plan") < at("run"));
    // clap's own listing still follows it.
    assert_eq!(listed(&long), listed(&help("-h")));
}

/// The guide is where an agent learns to design a team from any directory:
/// the files of a team and of a role, every key, and the loop.
#[test]
fn the_guide_teaches_the_files_of_a_team_and_a_role_and_the_loop() {
    use afkconvoy::scaffold::{HOOKS, RULES};
    use afkconvoy_core::team::{
        ESCALATE_KEYS, LAND_KEYS, ON_KEYS, PATROL_KEYS, PATROLS, SKILLS, STAGE_KEYS, TEAM_FILE,
        TEAM_KEYS, WORKFLOW_KEYS, WORKFLOWS,
    };
    let long = help("--help");
    let guide = guide(&long);
    for file in [TEAM_FILE, WORKFLOWS, PATROLS, RULES, SKILLS, HOOKS] {
        assert!(guide.contains(&format!("      {file}")), "the guide lays out no {file}");
    }
    let keys = [
        &TEAM_KEYS[..],
        &WORKFLOW_KEYS,
        &STAGE_KEYS,
        &ON_KEYS,
        &ESCALATE_KEYS,
        &LAND_KEYS,
        &PATROL_KEYS,
    ];
    for key in keys.concat() {
        assert!(guide.contains(&format!(" {key}:")), "the guide never shows the key {key}");
    }
    // The loop, in order: a team with its roles in one repo, one more role
    // there, then `check` after each edit.
    let at = |line: &str| guide.find(line).unwrap_or_else(|| panic!("the guide lost {line:?}"));
    assert!(
        at("afkconvoy scaffold team <dir> --role <name> --repo") < at("afkconvoy scaffold role")
    );
    assert!(at("afkconvoy scaffold role") < at("afkconvoy check --dir <dir> --team"));
    assert!(guide.contains("until it exits 0"));
    assert!(guide.contains("skills installer") && guide.contains("wraps no installer"));
    // `check` lays the same files out, from the same text.
    let files = guide
        .split("\n  A team is a directory — a whole repo, or a subdirectory of one — that holds:")
        .nth(1)
        .unwrap();
    let files = files.split("\n\n  The loop").next().unwrap();
    assert!(asked(&["check", "--help"]).contains(files));
    assert!(!long.contains("{files}"));
}

/// One repo for a team and its roles is the layout a new team starts from
/// (docs/adr/0014): `scaffold` shows it before a role on its own.
#[test]
fn scaffold_shows_a_team_and_its_roles_in_one_repo_first() {
    let said = asked(&["scaffold", "--help"]);
    let at = |line: &str| said.find(line).unwrap_or_else(|| panic!("scaffold lost {line:?}"));
    let one_repo = "afkconvoy scaffold team hospital --role fellow --role attending --repo";
    assert!(at(one_repo) < at("afkconvoy scaffold role"));
    assert!(at(one_repo) < at("<name>=<owner/repo>"));
    assert!(said.contains("\"team_line\": \"reviewer: ./roles/reviewer\""));
}

#[test]
fn the_readme_says_how_a_repo_is_bound_and_how_a_role_gets_its_skills() {
    let bind = README.split("\n## Bind a repo to a team\n").nth(1).expect("the section");
    let bind = bind.split("\n## ").next().unwrap();
    for said in [
        "team: owner/repo",
        "`patrols`",
        "`intake_limit`",
        "flow label",
        "`flow:<name>`",
        "afkconvoy scaffold role",
        "afkconvoy scaffold team",
        "afkconvoy check",
        "### What a human is asked, and when",
        "afkconvoy rule",
        "`by: human`",
        "npx skills add",
        "wraps no installer",
    ] {
        assert!(bind.contains(said), "the README's section on binding lost {said:?}");
    }
}

#[test]
fn the_short_help_is_still_one_screen() {
    let short = help("-h");
    assert!(short.lines().count() <= 44, "{short}");
    assert!(!short.contains("THE COMMANDS"));
}

#[test]
fn the_skill_points_at_the_help_and_restates_none_of_it() {
    let body = SKILL.splitn(3, "---\n").nth(2).unwrap();
    assert!(SKILL.starts_with("---\nname: afkconvoy\ndescription: "));
    assert!(body.contains("`afkconvoy --help`"));
    assert!(body.contains("`afkconvoy flows`"), "it sends whoever opens issues to `flows`");
    assert!(body.contains("`afkconvoy doctor`"), "it sends whoever asks what is wrong to `doctor`");
    let long = help("--help");
    for line in guide(&long).lines().map(str::trim).filter(|line| line.len() > 20) {
        assert!(!body.contains(line), "the skill restates the help: {line}");
    }
    assert!(body.lines().filter(|line| !line.trim().is_empty()).count() <= 4, "{body}");
    assert!(README.contains("npx skills add sunfmin/afkconvoy"));
}

/// What a subcommand's `--help` says under each of these, in this order.
const SECTIONS: [&str; 5] = [
    "WHEN TO RUN IT",
    "WHO RUNS IT",
    "WHAT IT READS AND WRITES",
    "WHAT IT ANSWERS",
    "WHAT TRIPS YOU UP",
];

/// The body of one section of a subcommand's long help.
fn section(long: &str, title: &str) -> String {
    let next = SECTIONS.iter().position(|section| *section == title).unwrap() + 1;
    between(long, title, SECTIONS.get(next).copied().unwrap_or("\0")).join("\n")
}

/// The subcommands the guide puts in the group whose lead starts with `lead`.
fn group<'a>(guide: &'a str, lead: &str) -> Vec<&'a str> {
    let after = guide.lines().skip_while(|line| !line.starts_with(lead));
    let names = after.skip(1).find(|line| line.starts_with("      ")).unwrap();
    names.split_whitespace().collect()
}

#[test]
fn every_visible_subcommand_has_a_long_help_and_keeps_its_one_line() {
    let top = help("-h");
    for command in listed(&top) {
        let (short, long) = (asked(&[command, "-h"]), asked(&[command, "--help"]));
        let mut from = 0;
        for title in SECTIONS {
            let at = long[from..].find(&format!("\n{title}\n"));
            from += at.unwrap_or_else(|| panic!("`{command} --help` has no {title}, in order"));
            assert!(section(&long, title).trim().len() > 20, "{command}: {title} says nothing");
            assert!(!short.contains(title), "`{command} -h` carries the long help");
        }
        // The one line is the same on both, and the one the listing gives.
        let line = short.lines().next().unwrap();
        assert_eq!(long.lines().next(), Some(line));
        let row = top.lines().find(|row| row.trim_start().starts_with(&format!("{command} ")));
        assert!(row.unwrap().ends_with(line), "{command}: {line}");
    }
}

#[test]
fn the_guide_and_each_subcommand_agree_on_who_runs_it_and_why() {
    let long = help("--help");
    let guide = guide(&long);
    let who = |command: &str| section(&asked(&[command, "--help"]), "WHO RUNS IT");
    let workers = group(guide, "  A worker");
    assert_eq!(workers, ["gate", "land", "verdict", "intake", "lock", "over", "wake"]);
    for command in &workers {
        let who = who(command);
        assert!(who.contains("worker") && who.contains("in its own worktree"), "{command}: {who}");
    }
    // The guard is nobody's to run: it is a hook of Claude Code's.
    let hooks = group(guide, "  A worker's hook");
    assert_eq!(hooks, ["guard"]);
    assert!(who("guard").trim_start().starts_with("Claude Code, as a hook"));
    // Nor is opening issues the operator's: whoever has work for the fleet does.
    let hands = group(guide, "  Whoever hands the fleet work");
    assert_eq!(hands, ["flows"]);
    assert!(who("flows").trim_start().starts_with("Whoever hands the fleet work: "));
    let operated = |command: &&str| {
        !workers.contains(command) && !hooks.contains(command) && !hands.contains(command)
    };
    for command in listed(&help("-h")).into_iter().filter(operated) {
        assert!(who(command).trim_start().starts_with("The operator, "), "{command}");
    }
    // Each transition the guide says a tick makes on its own says so itself,
    // and when to make it by hand; `recovery` only shows one.
    let by_hand = group(guide, "  One transition by hand");
    for command in by_hand.iter().filter(|command| **command != "recovery") {
        let when = section(&asked(&[command, "--help"]), "WHEN TO RUN IT");
        let when = when.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(when.contains("by hand") && when.contains("A tick does this on its own"), "{when}");
        assert!(when.contains("Do it yourself when"), "{command}: {when}");
    }
    // …and the plumbing, that it is seldom the command to reach for.
    for command in ["claim", "release", "heartbeat"] {
        let when = section(&asked(&[command, "--help"]), "WHEN TO RUN IT");
        assert!(when.trim_start().starts_with("Hardly ever:"), "{command}: {when}");
    }
}

/// Runs each command in a world and holds its answer against its own help:
/// every key the answer has is one the help names.
#[test]
fn a_help_names_every_key_its_command_answers_with() {
    let world = World::new();
    world.set_gate("true");
    world.issue(7, &["ready-for-agent"]);
    world.issue(8, &["ready-for-agent"]);
    let mut seen = BTreeSet::new();
    let mut check = |args: &[&str], marks: &[&str]| {
        let command = args[0];
        let answer = world.afk(&[args, &["--now", "1000"]].concat()).ok();
        let answer = answer.as_object().unwrap();
        for mark in marks {
            assert!(answer.contains_key(*mark), "{args:?} is not the case meant: {answer:?}");
        }
        let help = asked(&[command, "--help"]);
        let said = section(&help, "WHAT IT ANSWERS");
        let words: BTreeSet<&str> =
            said.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').collect();
        for key in answer.keys() {
            assert!(words.contains(key.as_str()), "`{command} --help` never names {key:?}");
        }
        seen.insert(command.to_string());
    };
    let agent = ["--worker-command", "agent"];
    let on =
        |issue: &'static str, instance: &'static str| ["--issue", issue, "--instance", instance];

    check(&["config"], &["base_branch"]);
    let note = "```yaml\nstages:\n  - { name: draft, by: human, on: { pass: done } }\n```\n";
    world.repo("acme/team", &[("team.md", "```yaml\n{}\n```\n"), ("workflows/note.md", note)]);
    check(&["check", "--team", "acme/team"], &["workflows"]);
    check(&["rehearse", "--team", "acme/team", "note", "pass"], &["steps"]);
    check(&["scaffold", "role", world.root.join("role").to_str().unwrap()], &["scaffolded"]);
    check(&["flows"], &["ready_label"]);
    check(&["plan"], &["frontier"]);
    check(&["doctor"], &["findings"]);
    check(&["scan"], &["claims"]);
    check(&["takeover", "--list"], &["instances"]);
    check(&[&["claim"][..], &on("8", "beta")].concat(), &["sha"]);
    check(&[&["claim"][..], &on("8", "gamma")].concat(), &["owner"]);
    check(&["heartbeat", "--instance", "beta"], &["reason"]);
    check(&["heartbeat", "--instance", "delta"], &["refreshed"]);
    check(&["takeover", "--from", "ghost", "--instance", "alpha"], &["detail"]);
    check(&["takeover", "--from", "beta", "--instance", "alpha"], &["heartbeat_age"]);
    check(&["takeover", "--from", "beta", "--instance", "alpha", "--yes"], &["as"]);
    check(&[&["release"][..], &on("8", "alpha")].concat(), &["released"]);
    check(&["recovery", "--issue", "7"], &["tier"]);
    check(&[&["dispatch"][..], &on("7", "alpha"), &agent].concat(), &["terminal"]);
    check(&[&["dispatch"][..], &on("7", "beta"), &agent].concat(), &["owner"]);
    let fresh = [&["dispatch"][..], &on("7", "alpha"), &agent, &["--start", "fresh"]].concat();
    check(&fresh, &["discarded"]);
    check(&[&["tick", "--instance", "alpha"][..], &agent].concat(), &["did"]);
    check(&["gate"], &["recorded"]);
    check(&["wake"], &["woke"]);
    check(&["lock", "--issue", "8"], &["raised"]);
    world.close_issue(8);
    check(&[&["close"][..], &on("9", "alpha")].concat(), &["detail"]);
    check(&[&["park"][..], &on("9", "alpha")].concat(), &["detail"]);
    let reason = [&["escalate"][..], &on("9", "alpha"), &["--reason", "why"]].concat();
    check(&reason, &["detail"]);
    check(&["land", "--train"], &["outcome"]);
    assert!(seen.len() >= 15, "{seen:?}");
}

#[test]
fn the_help_of_tick_says_for_each_kind_of_end_what_becomes_of_the_terminal_and_the_worktree() {
    let tick = asked(&["tick", "--help"]);
    let ends = between(&tick, "  - The step that ends a piece of work", "  - An `escalate`");
    let said = ends.join("\n");
    for (end, terminal, worktree) in [
        ("end_patrol  ", "terminal is closed", "checkout removed"),
        ("end_proposal  ", "terminal is closed", "checkout removed"),
        ("advance, to a  ", "terminal is closed", "worktree kept"),
        ("advance, to `done`  ", "terminal is closed", "worktree removed"),
        ("an escalation  ", "terminal is closed", "worktree"),
    ] {
        let from = said.find(end).unwrap_or_else(|| panic!("{end:?} in {said}"));
        let row: String = said[from..].lines().take(2).collect();
        assert!(row.contains(terminal) && row.contains(worktree), "{end:?}: {row}");
    }
    assert!(said.contains("A run that failed keeps its checkout"), "{said}");
    assert!(said.contains("only worker alive is the train's"), "{said}");
}

#[test]
fn the_help_of_tick_and_of_run_say_what_run_does_about_the_hop_between_two_stages() {
    let most = afkconvoy_core::tick::BACK_TO_BACK;
    let tick = asked(&["tick", "--help"]);
    assert!(tick.contains("`run` does not sleep across that hop"), "{tick}");
    assert!(tick.contains(&format!("at most {most} ticks back to back")), "{tick}");
    let run = asked(&["run", "--help"]);
    assert!(run.contains("· next now\n"), "{run}");
    assert!(run.contains(&format!("At most {most} ticks run back to back")), "{run}");
}

/// A role is a directory, named in one of three ways: the guide and `check`
/// lay them out, and neither says any more that a role is a repo.
#[test]
fn the_help_lays_out_the_three_ways_to_name_a_role_and_calls_no_role_a_repo() {
    for text in [help("--help"), asked(&["check", "--help"])] {
        for form in ["      owner/repo  ", "      owner/repo/path  ", "      ./path  "] {
            assert!(text.contains(form), "the help lost the form {form:?}");
        }
        assert!(text.contains(afkconvoy_core::team::ROLE_FORMS));
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        for gone in ["a role is a repo", "a team is a repo", "each a repo"] {
            assert!(!flat.contains(gone), "the help still says {gone:?}");
        }
    }
    let config = asked(&["config", "--help"]);
    assert!(config.contains("owner/repo/path"));
}
