//! The top-level help: `--help` is the text an agent learns the tool from,
//! `-h` the one-screen listing, and the skill file only points at the former.

use std::collections::BTreeSet;
use std::process::Command;

const SKILL: &str = include_str!("../../../skills/afkconvoy/SKILL.md");
const README: &str = include_str!("../../../README.md");

/// What the binary prints for `flag`, which must exit 0 and leave stderr empty.
fn help(flag: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).arg(flag).output().unwrap();
    assert!(out.status.success(), "{flag} exits 0");
    assert!(out.stderr.is_empty(), "{flag} prints to stdout only");
    String::from_utf8(out.stdout).unwrap()
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
