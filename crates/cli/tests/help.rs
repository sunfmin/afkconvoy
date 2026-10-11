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
    // The operator's flow, in order.
    let at = |command: &str| guide.find(&format!("afkconvoy {command} ")).unwrap();
    assert!(at("config") < at("plan") && at("plan") < at("run"));
    // clap's own listing still follows it.
    assert_eq!(listed(&long), listed(&help("-h")));
}

#[test]
fn the_short_help_is_still_one_screen() {
    let short = help("-h");
    assert!(short.lines().count() <= 40, "{short}");
    assert!(!short.contains("THE COMMANDS"));
}

#[test]
fn the_skill_points_at_the_help_and_restates_none_of_it() {
    let body = SKILL.splitn(3, "---\n").nth(2).unwrap();
    assert!(SKILL.starts_with("---\nname: afkconvoy\ndescription: "));
    assert!(body.contains("`afkconvoy --help`"));
    let long = help("--help");
    for line in guide(&long).lines().map(str::trim).filter(|line| line.len() > 20) {
        assert!(!body.contains(line), "the skill restates the help: {line}");
    }
    assert!(body.lines().filter(|line| !line.trim().is_empty()).count() <= 3, "{body}");
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
    assert_eq!(workers, ["gate", "land", "wake"]);
    for command in &workers {
        let who = who(command);
        assert!(who.contains("worker") && who.contains("in its own worktree"), "{command}: {who}");
    }
    for command in listed(&help("-h")).into_iter().filter(|command| !workers.contains(command)) {
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
    check(&["plan"], &["frontier"]);
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
    check(&[&["close"][..], &on("9", "alpha")].concat(), &["detail"]);
    check(&[&["park"][..], &on("9", "alpha")].concat(), &["detail"]);
    let reason = [&["escalate"][..], &on("9", "alpha"), &["--reason", "why"]].concat();
    check(&reason, &["detail"]);
    check(&["land", "--train"], &["outcome"]);
    assert!(seen.len() >= 15, "{seen:?}");
}
