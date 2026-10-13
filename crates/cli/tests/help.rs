//! The help: the top-level `--help` is the text an agent learns the tool from,
//! `-h` the one-screen listing, and the skill file only points at the former.
//! Each subcommand's `--help` is the next rung: what its one line leaves out.

mod common;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
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
    let lines = between(guide, "THE COMMANDS", "THE TOPICS").into_iter();
    lines.filter(|line| line.starts_with("      ")).flat_map(str::split_whitespace).collect()
}

/// Every word `text` puts after `before` as a name to run or to ask for: a
/// placeholder, a flag or a path is none.
fn mentioned<'a>(text: &'a str, before: &str) -> BTreeSet<&'a str> {
    let after = text.split(before).skip(1);
    let words = after.filter_map(|rest| rest.split([' ', '`', '\n']).next());
    words.filter(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase())).collect()
}

/// The topics `--help` lists, each with its line.
fn topics(long: &str) -> Vec<(&str, &str)> {
    let rows = between(long, "THE TOPICS", "RULES").into_iter();
    let rows = rows.filter(|row| row.starts_with("      "));
    let rows = rows.map(|row| row.trim().split_once("  ").expect("a name and its line"));
    rows.map(|(name, line)| (name, line.trim_start())).collect()
}

/// Where this repository is checked out.
fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The file a topic's text is, from the repository's root: the glossary is the
/// repository's own, and any other topic a file of `help/topics/`.
fn topic_file(name: &str) -> String {
    match name {
        "glossary" => "GLOSSARY.md".to_string(),
        name => format!("crates/cli/src/help/topics/{name}.txt"),
    }
}

/// Every help text the binary prints, each under the file it is written in:
/// `--help`, each command's, each topic.
fn help_texts() -> Vec<(String, String)> {
    let long = help("--help");
    let mut texts = vec![("crates/cli/src/help.txt".to_string(), long.clone())];
    for command in listed(&help("-h")) {
        let file = format!("crates/cli/src/help/{command}.txt");
        texts.push((file, asked(&[command, "--help"])));
    }
    for (topic, _) in topics(&long) {
        texts.push((topic_file(topic), asked(&["help", topic])));
    }
    texts
}

/// The glossary is a topic, and the file itself: nothing else is edited for
/// what `help glossary` prints to change.
#[test]
fn help_prints_a_topic_as_the_file_it_is_and_a_commands_help_as_before() {
    let glossary = fs::read_to_string(repository().join("GLOSSARY.md")).unwrap();
    assert_eq!(asked(&["help", "glossary"]), glossary);
    assert!(glossary.contains("**Fleet instance**:"));
    assert_eq!(asked(&["help", "wake"]), asked(&["wake", "--help"]));
    assert_eq!(asked(&["help"]), help("--help"));
}

#[test]
fn the_guide_lists_the_topics_there_are_and_no_other() {
    let long = help("--help");
    let listed: BTreeSet<_> = topics(&long).into_iter().map(|(name, _)| name).collect();
    let mut there = BTreeSet::from(["glossary".to_string()]);
    if let Ok(files) = fs::read_dir(repository().join("crates/cli/src/help/topics")) {
        let names = files.map(|file| file.unwrap().path());
        there.extend(names.map(|file| file.file_stem().unwrap().to_str().unwrap().to_string()));
    }
    assert_eq!(listed, there.iter().map(String::as_str).collect());
    // Each is printed as the file it is, and has a line that says something.
    for (name, line) in topics(&long) {
        let file = fs::read_to_string(repository().join(topic_file(name))).unwrap();
        assert_eq!(asked(&["help", name]), file, "{name}");
        assert!(line.len() > 20, "{name}: {line}");
    }
    let said = guide(&long).split("\nTHE TOPICS\n").nth(1).unwrap();
    assert!(said.contains("`afkconvoy help <topic>` prints one: plain text, as"), "{said}");
}

#[test]
fn help_refuses_a_name_that_is_neither_a_command_nor_a_topic() {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(["help", "nosuch"]).output();
    let out = out.unwrap();
    assert_eq!(out.status.code(), Some(2));
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let error = answer["error"].as_str().unwrap();
    assert!(error.contains("`nosuch` is neither a command nor a help topic"), "{error}");
    let named = error.split("The topics: ").nth(1).unwrap().split('.').next().unwrap();
    let long = help("--help");
    let topics: Vec<_> = topics(&long).into_iter().map(|(name, _)| name).collect();
    assert_eq!(named.split(", ").collect::<Vec<_>>(), topics);
    assert!(error.contains("The commands: `afkconvoy -h` lists them."), "{error}");
}

/// Where the help says `afkconvoy` and then a word that is no command: the
/// tool is the subject of a sentence there, and nothing is to be run.
const SUBJECT_OF: [&str; 2] = ["works", "that"];

/// What `text` tells its reader to run or to ask for that does not exist.
fn missing(text: &str, commands: &BTreeSet<&str>, topics: &BTreeSet<&str>) -> Vec<String> {
    let run = mentioned(text, "afkconvoy ").into_iter();
    let run = run.filter(|word| !commands.contains(word) && !SUBJECT_OF.contains(word));
    let asked = mentioned(text, "afkconvoy help ").into_iter();
    let asked = asked.filter(|word| !commands.contains(word) && !topics.contains(word));
    let run = run.map(|word| format!("afkconvoy {word}"));
    run.chain(asked.map(|word| format!("afkconvoy help {word}"))).collect()
}

#[test]
fn no_help_text_names_a_command_or_a_topic_that_does_not_exist() {
    let (short, long) = (help("-h"), help("--help"));
    let mut commands = listed(&short);
    commands.insert("help");
    let topics: BTreeSet<_> = topics(&long).into_iter().map(|(name, _)| name).collect();
    // What it catches: a command and a topic there is none of, and nothing else.
    let made_up = "Run `afkconvoy plan`, then afkconvoy frobnicate <dir> <dir>; `afkconvoy help \
                   glossary`, afkconvoy <command> --help and afkconvoy --help say more.";
    assert_eq!(missing(made_up, &commands, &topics), ["afkconvoy frobnicate"]);
    let caught = missing("see `afkconvoy help nosuch`", &commands, &topics);
    assert_eq!(caught, ["afkconvoy help nosuch"]);
    let texts = help_texts();
    assert!(texts.len() > 30);
    for (file, text) in texts {
        let missing = missing(&text, &commands, &topics);
        assert!(missing.is_empty(), "{file} names what does not exist: {missing:?}");
    }
}

/// How many single-letter edits — one put in, taken out or changed — turn `a`
/// into `b`.
fn edits(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, from) in a.iter().enumerate() {
        let mut last = row[0];
        row[0] = i + 1;
        for (j, to) in b.iter().enumerate() {
            let changed = last + usize::from(from != to);
            last = row[j + 1];
            row[j + 1] = changed.min(row[j] + 1).min(last + 1);
        }
    }
    row[b.len()]
}

/// `help <name>` is one namespace: a topic a typo away from a command would
/// be printed for it.
#[test]
fn no_topic_is_named_as_a_command_is_or_within_one_edit_of_one() {
    assert_eq!([edits("rule", "rule"), edits("rules", "rule"), edits("role", "rule")], [0, 1, 1]);
    assert_eq!([edits("ruel", "rule"), edits("roles", "rule"), edits("", "help")], [2, 2, 4]);
    let (short, long) = (help("-h"), help("--help"));
    let mut commands = listed(&short);
    commands.insert("help");
    for (topic, _) in topics(&long) {
        for command in &commands {
            assert!(
                edits(topic, command) > 1,
                "the topic {topic} is read as the command {command}"
            );
        }
    }
}

/// The words the glossary's entries list under _Avoid_, each with its entry.
/// One the entry gives a note in brackets is rightly used for something else,
/// which no test can tell from the use avoided: it is left out.
fn avoided(glossary: &str) -> Vec<(String, &str)> {
    let mut avoided = Vec::new();
    let mut entry = "";
    for line in glossary.lines() {
        if let Some(term) = line.strip_prefix("**").and_then(|rest| rest.strip_suffix("**:")) {
            entry = term;
        }
        // A note may hold a comma of its own: a word ends where the next one starts.
        let words = line.strip_prefix("_Avoid_: ").unwrap_or_default().split(", ");
        let mut noted = false;
        for word in words {
            let whole = !noted && !word.contains('(');
            noted = (noted || word.contains('(')) && !word.ends_with(')');
            if whole && !word.is_empty() {
                avoided.push((word.to_lowercase(), entry));
            }
        }
    }
    avoided
}

/// Each avoided word `text` holds — as a whole word, one or several — with
/// the file, the word, the entry that avoids it and the line it is on. A word
/// that is quoted, or is part of a key, a flag or a path, names something of
/// the tool's and is no use of the word.
fn offences(file: &str, text: &str, avoided: &[(String, &str)]) -> Vec<String> {
    let mut offences = Vec::new();
    // The glossary's own lists of them are no use of them.
    for line in text.lines().filter(|line| !line.starts_with("_Avoid_: ")) {
        let lower = line.to_lowercase();
        for (word, entry) in avoided {
            let held = lower.match_indices(word.as_str()).any(|(at, _)| {
                let after = &lower[at + word.len()..];
                let after = after.strip_prefix('s').unwrap_or(after);
                let named = |c: char| c.is_alphanumeric() || "_-/`\"".contains(c);
                let suffix = after.strip_prefix('.').is_some_and(|rest| rest.starts_with(named));
                !lower[..at].ends_with(named) && !after.starts_with(named) && !suffix
            });
            if held {
                let line = line.trim();
                offences.push(format!("{file}: `{word}`, which **{entry}** avoids, in: {line}"));
            }
        }
    }
    offences
}

#[test]
fn no_help_text_holds_a_word_the_glossary_says_to_avoid() {
    let glossary = fs::read_to_string(repository().join("GLOSSARY.md")).unwrap();
    let avoided = avoided(&glossary);
    assert!(avoided.len() > 80, "{avoided:?}");
    assert!(avoided.contains(&("merge queue".to_string(), "Landing train")));
    assert!(!avoided.iter().any(|(word, _)| word.contains('(')), "{avoided:?}");
    // What it catches, and how it names it.
    let made_up = "  The Dashboards are slow.\n_Avoid_: launcher\n  A relauncher is none.";
    assert_eq!(
        offences("help/x.txt", made_up, &avoided),
        ["help/x.txt: `dashboard`, which **Panel** avoids, in: The Dashboards are slow."]
    );
    let offences: Vec<_> =
        help_texts().iter().flat_map(|(file, text)| offences(file, text, &avoided)).collect();
    assert!(
        offences.is_empty(),
        "a help text holds a word the glossary says to avoid. Say the entry's own term — or, \
         where the word is rightly used for something else, say for what in brackets after it \
         in the entry's _Avoid_:\n{}",
        offences.join("\n")
    );
}

#[test]
fn the_guide_groups_every_visible_subcommand_once_and_names_no_other() {
    let (short, long) = (help("-h"), help("--help"));
    let (listed, grouped) = (listed(&short), grouped(guide(&long)));
    assert!(listed.len() > 10, "{short}");
    assert_eq!(grouped.iter().copied().collect::<BTreeSet<_>>(), listed);
    assert_eq!(grouped.len(), listed.len(), "a subcommand is in two groups: {grouped:?}");
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

/// The topics that are files of `help/topics/`: each name with its text.
fn topic_files() -> Vec<(String, String)> {
    let files = fs::read_dir(repository().join("crates/cli/src/help/topics")).unwrap();
    let mut topics: Vec<_> = files
        .map(|file| file.unwrap().path())
        .map(|file| {
            let name = file.file_stem().unwrap().to_str().unwrap().to_string();
            (name, fs::read_to_string(file).unwrap())
        })
        .collect();
    topics.sort();
    topics
}

/// Whether `text` shows `key` as a key of a file: followed by its `:`, and
/// not as the end of a longer key.
fn shows(text: &str, key: &str) -> bool {
    let shown = format!("{key}:");
    text.match_indices(&shown).any(|(at, _)| text[..at].ends_with([' ', '`', '{']))
}

/// A team's files are described in the topics, each kind of file in its own:
/// the keys are the loader's, so one it learns is one a topic must describe.
#[test]
fn every_key_the_loader_knows_is_described_in_the_topic_of_its_file_and_in_no_other() {
    use afkconvoy_core::team::{
        ESCALATE_KEYS, LAND_KEYS, ON_KEYS, PATROL_KEYS, PRECEDENTS_KEYS, STAGE_KEYS, TEAM_KEYS,
        WORKFLOW_KEYS,
    };
    let files = [
        ("team", [&TEAM_KEYS[..], &PRECEDENTS_KEYS].concat()),
        (
            "workflows",
            [&WORKFLOW_KEYS[..], &STAGE_KEYS, &ON_KEYS, &ESCALATE_KEYS, &LAND_KEYS].concat(),
        ),
        ("patrols", PATROL_KEYS.to_vec()),
    ];
    // What it catches: a key shown, and one that is only part of another's name.
    assert!(shows("      max_open: 5", "max_open") && shows("{ by: <role>", "by"));
    assert!(!shows("      max_open: 5", "open") && !shows("the stages follow", "stages"));
    for (topic, keys) in &files {
        let text = asked(&["help", topic]);
        for key in keys {
            assert!(shows(&text, key), "`afkconvoy help {topic}` never shows the key {key}");
            // A key two kinds of file have is described once for each.
            for (other, theirs) in files.iter().filter(|(other, _)| other != topic) {
                let said = theirs.contains(key) || !shows(&asked(&["help", other]), key);
                assert!(said, "{key} is a key of {topic}'s file, and {other} shows it too");
            }
        }
    }
}

/// `--help` and `check --help` name the topics where they held the
/// description of a team's files, and hold none of it.
#[test]
fn the_guide_and_check_name_the_topics_and_describe_no_file_of_a_team() {
    use afkconvoy_core::team::{PATROL_KEYS, STAGE_KEYS, TEAM_KEYS, WORKFLOW_KEYS};
    let long = help("--help");
    let guide = guide(&long);
    let check = asked(&["check", "--help"]);
    let check = section(&check, "WHAT IT READS AND WRITES");
    for (text, from) in [(guide, "--help"), (check.as_str(), "check --help")] {
        for topic in ["team", "workflows", "patrols"] {
            let named = format!("`afkconvoy help {topic}`");
            assert!(text.contains(&named), "`{from}` does not name {named}");
        }
        let keys = [&TEAM_KEYS[..], &WORKFLOW_KEYS, &STAGE_KEYS, &PATROL_KEYS].concat();
        for key in keys.into_iter().filter(|key| *key != "harness") {
            let laid_out =
                text.lines().any(|line| line.trim_start().starts_with(&format!("{key}:")));
            assert!(!laid_out, "`{from}` still lays out the key {key}");
        }
        assert!(!text.contains("{files}") && !text.contains("      team.md  "), "{from}");
    }
    // It is shorter for it: what it held was longer than all the rest of it is.
    assert!(guide.lines().count() < 150, "the guide is {} lines", guide.lines().count());
}

/// The team topic is where an agent learns the files of a team and of a role.
#[test]
fn the_team_topic_lays_out_the_files_of_a_team_and_a_role_and_the_four_kinds_of_text() {
    use afkconvoy_core::harness::Harness;
    use afkconvoy_core::team::{
        PATROLS, PRECEDENTS, ROLE_FORMS, RULES, SKILLS, TEAM_FILE, WORKFLOWS,
    };
    let team = asked(&["help", "team"]);
    // A role's limits are laid out once for each harness.
    let limits = Harness::ALL.map(Harness::limits);
    for file in [&[TEAM_FILE, WORKFLOWS, PATROLS, PRECEDENTS, RULES, SKILLS][..], &limits].concat()
    {
        assert!(team.contains(&format!("      {file}")), "the topic lays out no {file}");
    }
    // The team rules are a file of the team's, and the four kinds of text a
    // worker reads are each said with what it is for.
    assert!(team.contains(&format!("      {RULES}               the team rules: ")), "{team}");
    for (kind, is_for) in [
        ("the repo's own rules", "what that one codebase asks"),
        ("the team rules", "how the team"),
        ("a role's rules", "what one kind of"),
        ("the precedents", "what a human"),
    ] {
        let said = team.split(&format!("      {kind}  ")).nth(1);
        let said = said.unwrap_or_else(|| panic!("the topic names no {kind}"));
        let said = said.split("\n\n").next().unwrap();
        assert!(said.contains(is_for), "the topic does not say what {kind} is for: {said}");
    }
    // A role is a directory, named in one of three ways.
    for form in ["      owner/repo  ", "      owner/repo/path  ", "      ./path  "] {
        assert!(team.contains(form), "the topic lost the form {form:?}");
    }
    assert!(team.contains(ROLE_FORMS));
    // How a repo is bound, and what `check` holds the team to.
    for said in ["HOW A REPO IS BOUND TO IT", "intake_limit", "WHAT CHECK HOLDS IT TO"] {
        assert!(team.contains(said), "the topic lost {said:?}");
    }
}

/// A topic that is a file of `help/topics/` says first what its thing is and
/// when to read it, stays short, and ends with the topics to read next.
#[test]
fn a_topic_opens_with_what_it_is_and_when_to_read_it_and_ends_with_what_to_read_next() {
    let topics = topic_files();
    assert!(topics.len() >= 3, "{}", topics.len());
    for (name, text) in &topics {
        let lines = text.lines().count();
        assert!(lines <= 120, "the topic {name} is {lines} lines: split it");
        let opening = text.split("\n\n").next().unwrap();
        assert!(opening.starts_with("A ") && opening.contains(" is "), "{name}: {opening}");
        assert!(opening.contains(". Read this to "), "{name} does not say when to read it");
        let (_, next) = text.rsplit_once("\nREAD NEXT\n").expect("its last section");
        let rows: Vec<_> = next.lines().filter(|row| !row.is_empty()).collect();
        assert!(rows.len() >= 2, "{name}: {next}");
        for row in rows {
            let other = row.trim_start().strip_prefix("afkconvoy help ");
            let other = other.and_then(|rest| rest.split_whitespace().next());
            let other = other.unwrap_or_else(|| panic!("{name} ends with more than topics: {row}"));
            assert!(other != name && asked(&["help", other]).len() > 100, "{name}: {row}");
        }
    }
}

/// The loop that designs a team is the guide's: the commands, in order.
#[test]
fn the_guide_teaches_the_loop_that_designs_a_team() {
    let long = help("--help");
    let guide = guide(&long);
    // A team with its roles in one repo, one more role there, then `check`
    // after each edit.
    let at = |line: &str| guide.find(line).unwrap_or_else(|| panic!("the guide lost {line:?}"));
    assert!(
        at("afkconvoy scaffold team <dir> --role <name> --repo") < at("afkconvoy scaffold role")
    );
    assert!(at("afkconvoy scaffold role") < at("afkconvoy check --dir <dir> --team"));
    assert!(guide.contains("until it exits 0"));
    assert!(guide.contains("skills installer") && guide.contains("wraps no installer"));
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

/// A role is a directory: no help says any more that a role is a repo.
#[test]
fn no_help_calls_a_role_or_a_team_a_repo() {
    for (file, text) in help_texts() {
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
        for gone in ["a role is a repo", "a team is a repo", "each a repo"] {
            assert!(!flat.contains(gone), "{file} still says {gone:?}");
        }
    }
    let config = asked(&["config", "--help"]);
    assert!(config.contains("owner/repo/path"));
}

#[test]
fn the_help_of_doctor_says_what_moved_means() {
    let said = asked(&["doctor", "--help"]);
    let moved = said.split("\n\n").find(|part| part.trim_start().starts_with("Moved: ")).unwrap();
    for part in [
        "the role's directory holds something else at the head of its repo than at the",
        "A new commit is\n  not it",
        "without what a worker is not given of it: the directories of its roles",
        "a different commit is all there is to go by",
    ] {
        assert!(moved.contains(part), "{part:?} is not in:\n{moved}");
    }
}
