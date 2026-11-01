//! The help: the top-level `--help` is the index an agent learns the tool
//! from — the guide, every command's line under who runs it, every topic's
//! line — `help all` the whole manual, `-h` the one-screen listing, and the
//! skill file only points at the index. Each subcommand's `--help` is the next
//! rung: what its one line leaves out.

use crate::common;

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

/// The top-level help `flag` asks for: `-h` the listing, `--help` the index.
fn help(flag: &str) -> String {
    asked(&[flag])
}

/// The whole manual: what `help all` answers.
fn manual() -> String {
    asked(&["help", "all"])
}

/// `land <n>` and `land --issue <n>` are the one usage both the help and a
/// missing issue name.
#[test]
fn land_help_and_a_missing_issue_name_the_same_forms() {
    let forms = "afkconvoy land <n> | afkconvoy land --issue <n>";
    let help = asked(&["land", "--help"]);
    assert!(help.contains(forms), "{help}");
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).arg("land").output().unwrap();
    assert!(!out.status.success());
    let said = String::from_utf8(out.stdout).unwrap();
    assert!(said.contains(forms), "{said}");
}

#[test]
fn cut_help_gives_five_hours_at_the_default_and_three_beside_the_gates() {
    let help = asked(&["help", "cut"]);
    assert!(help.contains("five hours at the default"), "{help}");
    assert!(help.contains("three hours beside the two gate limits"), "{help}");
    assert!(!help.contains("an hour beside"), "{help}");
    assert!(!help.contains("three hours at the default"), "{help}");
    assert!(help.contains("Leaving it out means 18000s"), "{help}");
    assert!(!help.contains("3600"), "{help}");
}

#[test]
fn help_explains_the_idle_wait_and_a_closed_blockers_wake() {
    for text in [asked(&["help"]), asked(&["run", "--help"])] {
        assert!(text.contains("25 minutes"), "{text}");
        assert!(text.contains("3 empty ticks") || text.contains("3 ticks in a row were empty"));
        assert!(text.contains("same tracker"));
        assert!(text.contains("closed issue and the fleet instance that saw it"));
    }
}

/// The most the index may be: what an agent's harness shows whole, with room.
const INDEX_MOST: usize = 21 * 1024;

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

/// The guide the index and the manual open with: everything above the tool's
/// usage, which comes before its commands.
fn guide(long: &str) -> &str {
    long.split("\n## Usage\n").next().unwrap()
}

/// The manual's groups of commands, in order, each with its commands: a
/// `### <group>` and the `#### \`afkconvoy <command>\`` under it.
fn groups(long: &str) -> Vec<(&str, Vec<&str>)> {
    let mut groups: Vec<(&str, Vec<&str>)> = vec![];
    for line in between(long, "## Commands", "## Topics") {
        if let Some(group) = line.strip_prefix("### ") {
            groups.push((group, vec![]));
        } else if let Some(command) = line.strip_prefix("#### `afkconvoy ") {
            let command = command.strip_suffix('`').unwrap();
            groups.last_mut().expect("a command under a group").1.push(command);
        }
    }
    groups
}

/// The commands of the manual's group called `name`.
fn group<'a>(long: &'a str, name: &str) -> Vec<&'a str> {
    let found = groups(long).into_iter().find(|(group, _)| *group == name);
    found.unwrap_or_else(|| panic!("the manual has no group {name:?}")).1
}

/// Every word `text` puts after `before` as a name to run or to ask for: a
/// placeholder, a flag or a path is none.
fn mentioned<'a>(text: &'a str, before: &str) -> BTreeSet<&'a str> {
    let after = text.split(before).skip(1);
    let words = after.filter_map(|rest| rest.split([' ', '`', '\n']).next());
    words.filter(|word| !word.is_empty() && word.bytes().all(|b| b.is_ascii_lowercase())).collect()
}

/// The topics the manual holds, in its order.
fn topics(long: &str) -> Vec<&str> {
    let (_, topics) = long.split_once("\n## Topics\n").expect("the manual's topics");
    let heads = topics.lines().filter_map(|line| line.strip_prefix("### `afkconvoy help "));
    heads.map(|name| name.strip_suffix('`').unwrap()).collect()
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
        name => format!("crates/cli/src/help/topics/{name}.md"),
    }
}

/// Every help text the binary prints, each under where it is written: the
/// guide, each command's, each topic.
fn help_texts() -> Vec<(String, String)> {
    let long = manual();
    let mut texts = vec![("crates/cli/src/lib.rs".to_string(), guide(&long).to_string())];
    for command in listed(&help("-h")) {
        let doc = format!("the doc comment of `{command}`, crates/cli/src/lib.rs");
        texts.push((doc, asked(&[command, "--help"])));
    }
    for topic in topics(&long) {
        texts.push((topic_file(topic), asked(&["help", topic])));
    }
    texts
}

/// What `help <topic>` prints of the file `text`: under its heading, and
/// without the blank lines it ends with.
fn printed(topic: &str, text: &str) -> String {
    format!("# `afkconvoy help {topic}`\n\n{}\n", text.trim_end())
}

/// Help names no decision record. The reader has the binary and its help,
/// and cannot open `docs/adr/`. Each sentence that lost a citation still
/// states the rule the citation sat beside.
#[test]
fn help_names_no_document_the_reader_cannot_open() {
    for args in [["help", "glossary"], ["plan", "--help"], ["run", "--help"]] {
        let said = asked(&args);
        assert!(!said.contains("docs/adr/"), "{args:?} names a document the reader cannot open");
    }
    let glossary = asked(&["help", "glossary"]);
    for rule in [
        "a trimmed Rust port of the `afk-fleet` skill",
        "moves onto it by itself between ticks once no exclusive command runs",
        "a repo config names none",
        "the repo names a tracker's `<owner>/<tracker>#<n>`",
        "keeps confirmed ownership across restart or upgrade",
        "answers every later question in that look from what was read",
        "selects the last harness to take a worker brief, then the host kit's eligible order",
        "names no harness starts on the host kit's first eligible one",
        "no other harness is started in its place",
    ] {
        assert!(glossary.contains(rule), "the glossary lost the rule {rule:?}");
    }
    let plan = asked(&["plan", "--help"]);
    assert!(plan.contains("repo labels of more than one repo"), "{plan}");
    let run = asked(&["run", "--help"]);
    assert!(
        run.contains(
            "while PRs, branches, the gate and their landing turns stay the checkout's own repo's."
        ),
        "run's tracker paragraph lost its rule"
    );
}

/// The glossary is a topic, and the file itself: nothing else is edited for
/// what `help glossary` prints to change.
#[test]
fn help_prints_a_topic_as_the_file_it_is_and_a_commands_help_as_before() {
    let glossary = fs::read_to_string(repository().join("GLOSSARY.md")).unwrap();
    assert_eq!(asked(&["help", "glossary"]), printed("glossary", &glossary));
    assert!(glossary.contains("**Fleet instance**:"));
    assert_eq!(asked(&["help", "wake"]), asked(&["wake", "--help"]));
    assert_eq!(asked(&["help"]), help("--help"));
}

/// `text`, a page whose sections start at `##`, as the manual holds it under a
/// heading of level `under`: each heading outside a fenced block moved down,
/// so that `##` sits one level below `under`.
fn nested(text: &str, under: usize) -> String {
    let mut fenced = false;
    let mut nested = String::new();
    for line in text.lines() {
        let fence = line.trim_start().starts_with("```");
        if !fenced && !fence && line.starts_with('#') {
            nested.push_str(&"#".repeat(under - 1));
        }
        fenced ^= fence;
        nested.push_str(line);
        nested.push('\n');
    }
    nested
}

/// `--help` answers the index, as `help` does: the guide, every command by its
/// one line under its group, in the manual's order, every topic by the sentence
/// it opens with, and last `help all`, which answers the whole manual. It holds
/// no command's text and no topic's, and stays short enough that an agent's
/// harness shows it whole.
#[test]
fn help_answers_the_index_and_help_all_the_whole_manual() {
    let (index, long, short) = (help("--help"), manual(), help("-h"));
    assert_eq!(asked(&["help"]), index);
    assert!(index.len() <= INDEX_MOST, "the index is {} bytes: {INDEX_MOST} at most", index.len());
    assert!(long.len() > 10 * index.len(), "the whole manual is not behind `help all`");
    assert!(index.starts_with("# `afkconvoy`\n") && guide(&index) == guide(&long));
    // Each command by the line `-h` gives it, under its group, as the manual
    // has them.
    let mut groups_listed: Vec<(&str, Vec<&str>)> = vec![];
    for line in between(&index, "## Commands", "## Topics").into_iter().filter(|l| !l.is_empty()) {
        if let Some(group) = line.strip_prefix("### ") {
            groups_listed.push((group, vec![]));
            continue;
        }
        let item = line.strip_prefix("- `afkconvoy ").unwrap_or_else(|| panic!("{line}"));
        let (command, said) = item.split_once("`: ").unwrap_or_else(|| panic!("{line}"));
        let row = short.lines().find(|row| row.trim_start().starts_with(&format!("{command} ")));
        assert!(row.is_some_and(|row| row.ends_with(said)), "{command}: {said}");
        groups_listed.last_mut().expect("a command under a group").1.push(command);
    }
    assert_eq!(groups_listed, groups(&long));
    // Each topic by its first sentence, which its text opens with, then the
    // whole manual.
    let (_, listed) = index.split_once("\n## Topics\n").unwrap();
    let mut names = vec![];
    for line in listed.lines().filter(|line| !line.is_empty()) {
        let item = line.strip_prefix("- `afkconvoy help ").unwrap_or_else(|| panic!("{line}"));
        let (name, said) = item.split_once("`: ").unwrap_or_else(|| panic!("{line}"));
        names.push(name);
        if name == "all" {
            assert!(said.starts_with("The whole manual"), "{said}");
            continue;
        }
        assert!(said.len() > 30 && said.ends_with('.'), "{name} opens with no sentence: {said}");
        let opening = format!("# `afkconvoy help {name}`\n\n{said}");
        assert!(asked(&["help", name]).starts_with(&opening), "{name}: {said}");
    }
    assert_eq!(names, [topics(&long), vec!["all"]].concat());
    // What each command and each topic says past its line is not in it.
    for heading in ["\n#### `afkconvoy ", "\n### `afkconvoy help ", "\n##### When to run it\n"] {
        assert!(!index.contains(heading) && long.contains(heading), "{heading:?}");
    }
    // The guide says what each answers.
    let said = guide(&index);
    assert!(said.contains("`afkconvoy --help` answers the index"), "{said}");
    assert!(said.contains("`afkconvoy help all` answers the whole manual"), "{said}");
}

#[test]
fn the_manual_holds_the_topics_there_are_and_no_other() {
    let long = manual();
    let held: BTreeSet<_> = topics(&long).into_iter().collect();
    let mut there = BTreeSet::from(["glossary".to_string()]);
    if let Ok(files) = fs::read_dir(repository().join("crates/cli/src/help/topics")) {
        let names = files.map(|file| file.unwrap().path());
        there.extend(names.map(|file| file.file_stem().unwrap().to_str().unwrap().to_string()));
    }
    assert_eq!(held, there.iter().map(String::as_str).collect());
    // Each is printed as the file it is, a value from code in place of each
    // `{const:}`, and the manual holds it whole.
    for name in topics(&long) {
        let file = fs::read_to_string(repository().join(topic_file(name))).unwrap();
        let said = asked(&["help", name]);
        if file.contains("{const:") {
            for line in file.lines().filter(|line| !line.contains("{const:")) {
                assert!(said.contains(line), "{name} lost {line:?}");
            }
        } else {
            assert_eq!(said, printed(name, &file), "{name}");
        }
        let (_, body) = said.split_once("\n\n").unwrap();
        let body = nested(body, 3);
        assert!(long.contains(&format!("\n### `afkconvoy help {name}`\n\n{body}")), "{name}");
    }
    let said = guide(&long).split("\n## The topics\n").nth(1).unwrap();
    assert!(said.contains("`afkconvoy help <topic>` when this guide or a command's"), "{said}");
    assert!(said.contains("it prints that topic, Markdown, as"), "{said}");
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
    let long = manual();
    assert_eq!(named.split(", ").collect::<Vec<_>>(), topics(&long));
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
    let (short, long) = (help("-h"), manual());
    let mut commands = listed(&short);
    commands.insert("help");
    // `help all` is no topic, but `help` answers it: the whole manual.
    let topics: BTreeSet<_> = topics(&long).into_iter().chain(["all"]).collect();
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
    let (short, long) = (help("-h"), manual());
    let mut commands = listed(&short);
    commands.insert("help");
    for topic in topics(&long) {
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
    assert!(avoided.contains(&("resync".to_string(), "Rebuild")));
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

/// The topics that are files of `help/topics/`, each with its text: every
/// topic but the glossary, which is the repository's own.
fn topic_files() -> Vec<(String, String)> {
    let long = manual();
    let names = topics(&long).into_iter().filter(|name| *name != "glossary");
    names.map(|name| (name.to_string(), asked(&["help", name]))).collect()
}

/// The heads of a topic's rows: of each item of a list, what it is about,
/// written in backticks or in bold before its `:`.
fn heads(topic: &str) -> Vec<&str> {
    let items = topic.lines().filter_map(|line| line.strip_prefix("- "));
    let heads = items.filter_map(|item| {
        let mark = ["``", "`", "**"].into_iter().find(|mark| item.starts_with(mark))?;
        let (head, _) = item[mark.len()..].split_once(&format!("{mark}:"))?;
        Some(head.trim())
    });
    heads.collect()
}

#[test]
fn troubleshooting_has_a_row_for_every_finding_doctor_can_give() {
    use afkconvoy::doctor::Kind;
    let said = asked(&["help", "troubleshooting"]);
    let heads = heads(&said);
    assert!(Kind::ALL.len() > 20, "{:?}", Kind::ALL);
    for kind in Kind::ALL {
        let symptom = kind.symptom();
        assert!(
            heads.contains(&symptom),
            "`doctor` can give the finding {kind:?}, and `afkconvoy help troubleshooting` has \
             no row headed with its words: {symptom:?}"
        );
    }
    // What it catches: words that are in the topic, but head no row.
    assert!(said.contains("a line to run") && !heads.contains(&"a line to run"));
    // A start's refusals have their rows too, by the words each is made of.
    let no_branch = afkconvoy_core::layout::no_base_branch("main");
    for refusal in [
        "this host cannot work a fleet yet",
        "cannot read the config file",
        "that push would be refused",
        "the remote rejects the fleet's refs under both",
        "is no harness",
        "cannot read the host kit",
        "is public: anyone can comment on its issues",
    ] {
        assert!(heads.contains(&refusal), "no row for the refusal {refusal:?}");
    }
    let public = afkconvoy_core::tracker::public("acme/dev", "/work/dev");
    assert!(public.contains("is public: anyone can comment on its issues"), "{public}");
    assert!(heads.iter().any(|head| no_branch.starts_with(head)), "{no_branch}");
}

/// Every flag `text` gives a command of this tool's, with the command: what
/// stands on a line after `afkconvoy <command>`, up to where the line stops
/// being the command's.
fn flagged(text: &str) -> Vec<(&str, &str)> {
    let mut flagged = vec![];
    for line in text.lines() {
        for said in line.split("afkconvoy ").skip(1) {
            let said = said.split(['`', ';', ':', ',']).next().unwrap();
            let mut words = said.split_whitespace();
            let Some(command) = words.next() else { continue };
            let flags = words.filter(|word| word.starts_with("--"));
            flagged.extend(flags.map(|flag| (command, flag.split('=').next().unwrap())));
        }
    }
    flagged
}

/// Each way the fleet stops for a human, by the heading `humans` gives it.
const STOPS: [&str; 5] = [
    "An escalation",
    "A tripped breaker",
    "A stage `by: human`",
    "A patrol's failed run",
    "A fleet lock",
];

#[test]
fn humans_names_for_each_way_the_fleet_stops_a_command_with_the_flags_its_help_gives() {
    let said = asked(&["help", "humans"]);
    let short = help("-h");
    let commands = listed(&short);
    for stop in STOPS {
        let part = said.split(&format!("\n## {stop}\n")).nth(1);
        let part = part.unwrap_or_else(|| panic!("`humans` has no part {stop:?}"));
        let part = part.split("\n## ").next().unwrap();
        let named = mentioned(part, "afkconvoy ");
        assert!(named.iter().any(|name| commands.contains(name)), "{stop} names no command");
    }
    // What it catches: a flag the command's help does not give.
    let made_up = "Run `afkconvoy rule --issue 7 --because x`, then afkconvoy show --issue 7: it";
    assert_eq!(flagged(made_up), [("rule", "--issue"), ("rule", "--because"), ("show", "--issue")]);
    let mut seen = 0;
    for (topic, text) in topic_files() {
        for (command, flag) in flagged(&text).into_iter().filter(|(_, flag)| *flag != "--help") {
            let given = asked(&[command, "-h"]);
            let gives = given.split_whitespace().any(|word| word.trim_end_matches(',') == flag);
            assert!(gives, "{topic}: `afkconvoy {command} -h` gives no {flag}");
            seen += 1;
        }
    }
    assert!(seen >= 5, "{seen}");
}

#[test]
fn a_topic_is_short_and_refers_to_nothing_outside_the_binary() {
    let files = topic_files();
    assert!(files.len() >= 4);
    for (topic, text) in files {
        let lines = text.lines().count();
        assert!(lines <= 125, "{topic} is {lines} lines: a topic is about 120 at most; split it");
        // No file of this repository, no URL, and not the manual.
        for outside in ["docs/", "crates/", "GLOSSARY", "README", "manual", "://", "github.com"] {
            assert!(!text.contains(outside), "{topic} refers to {outside:?}");
        }
        assert!(text.ends_with('\n') && !text.ends_with("\n\n"), "{topic}");
    }
}

#[test]
fn the_guide_carries_what_an_agent_needs_before_its_first_command() {
    let long = manual();
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
        "each tick starts and one as it ends",
        // When to read a command's own help.
        "afkconvoy <command> --help",
        // The rules an agent would otherwise trip on.
        "One fleet instance per host per repo",
        "`run` returns once its checkout's first tick ended",
        "--worker-command",
        "A watch is the operator for every repo on the host's checkout list",
        "hands the escalated issue back with the line `doctor` names",
        "land it in that repo's turn with `land --watch`",
        "has that repo's fleet cut a version after landing",
        "checks the fault on the new version",
        "Human decisions stay with the human, untouched and listed",
    ] {
        assert!(guide.contains(needed), "the guide lost {needed:?}");
    }
    // Watching is discoverable before the other views, and at the first-run guide.
    let operator = guide.lines().find(|line| line.starts_with("- **Run by the operator**"));
    let operator = operator.expect("the guide says who the operator is");
    assert!(operator.find("doctor --all --since").unwrap() < operator.find("`checkouts`").unwrap());
    // The operator's flow, in order.
    let first = guide.split("\n## What to run first").nth(1).unwrap();
    let at = |command: &str| first.find(&format!("`afkconvoy {command}`")).unwrap();
    assert!(at("config") < at("plan") && at("plan") < at("run"));
    let watching = "afkconvoy doctor --all --since <until of your last look>";
    assert!(first.split("## What a command answers").next().unwrap().contains(watching));
    assert!(first.contains("without `--since`"));
    // Each group the guide describes is one the manual puts commands in.
    let described = guide.lines().filter_map(|line| line.strip_prefix("- **"));
    let described: Vec<_> = described.filter_map(|line| line.split_once("** — ")).collect();
    assert!(described.len() >= 9, "{described:?}");
    for (name, _) in described {
        assert!(!group(&long, name).is_empty(), "the guide describes a group of none: {name}");
    }
}

/// Each group the guide describes names every command of its own in its
/// bullet: the guide reads top to bottom, with no "it" or "them" to match
/// against the commands listed below it.
#[test]
fn each_group_the_guide_describes_names_every_command_of_its_own() {
    let long = manual();
    let described = guide(&long).lines().filter_map(|line| line.strip_prefix("- **"));
    for (name, said) in described.filter_map(|line| line.split_once("** — ")) {
        for command in group(&long, name) {
            assert!(said.contains(&format!("`{command}`")), "{name} names no `{command}`: {said}");
        }
    }
}

/// The index's checkout rule is the form `-h` prints. A flag of the command's
/// own, including `--instance` and `--run`, leaves the checkout where that
/// usage line shows `[CHECKOUT]`. `-C` is only where the usage line shows
/// `-C`. `scaffold`, `watched` and `draft` name no checkout, and stay so.
#[test]
fn the_index_keeps_a_commands_own_flag_from_moving_the_checkout_to_c() {
    let index = help("--help");
    let rule = index
        .lines()
        .find(|line| line.contains("does not move the checkout to `-C`"))
        .expect("the index states the checkout rule");
    for said in [
        "A flag of the command's own, including `--instance` and `--run`, does not move the \
         checkout to `-C`",
        "The checkout is a path after the command whenever that command's usage line shows \
         `[CHECKOUT]`",
        "`-C <checkout>` only when that command's usage line shows `-C`",
        "A fix line and a hand-typed command use that form",
    ] {
        assert!(rule.contains(said), "the checkout rule lost {said:?}: {rule}");
    }

    // The usage line is the form. `[CHECKOUT]` after the command's own flag is
    // the fix line the rule tells you to type.
    let forms = [
        ("heartbeat", "--instance", "afkconvoy heartbeat --instance <instance> <checkout>"),
        ("tick", "--instance", "afkconvoy tick --instance <instance> <checkout>"),
        ("over", "--run", "afkconvoy over --run <run> <checkout>"),
    ];
    for (command, flag, fix) in forms {
        let long = asked(&[command, "--help"]);
        assert!(
            long.contains("Name it after the command, as `[CHECKOUT]`"),
            "{command} no longer names the checkout after the command"
        );
        let usage = usage_of(command);
        let flag_at = usage.find(flag).unwrap_or_else(|| panic!("{command} usage lost {flag}"));
        let checkout_at = usage.find("[CHECKOUT]").unwrap_or_else(|| panic!("{usage}"));
        assert!(flag_at < checkout_at, "{usage}");
        assert!(!usage.contains("-C"), "{command} usage offers -C: {usage}");
        let written = fix_from_usage(&usage);
        assert_eq!(written, fix, "{usage}");
    }

    // `-C` is still the usage parser's refusal, before the command reads anything.
    // A path that is no directory is accepted by that parser and refused at the
    // checkout, so the command writes nothing.
    let path = "/no/such/afkconvoy-checkout";
    for args in [
        ["heartbeat", "-C", path, "--instance", "alpha"],
        ["tick", "-C", path, "--instance", "alpha"],
        ["over", "-C", path, "--run", "rounds"],
    ] {
        let (code, error) = ran(&args);
        assert_eq!(code, 2, "{args:?}: {error}");
        assert!(error.contains("unexpected argument '-C' found"), "{args:?}: {error}");
        assert!(error.contains("[CHECKOUT]"), "{args:?}: {error}");
    }
    for args in [
        vec!["heartbeat", "--instance", "alpha", path],
        vec!["heartbeat", path, "--instance", "alpha"],
        vec!["tick", "--instance", "alpha", path],
        vec!["tick", path, "--instance", "alpha"],
        vec!["over", "--run", "rounds", path],
        vec!["over", path, "--run", "rounds"],
    ] {
        let (code, error) = ran(&args);
        assert_eq!(code, 1, "{args:?}: {error}");
        assert!(error.contains("is not a directory"), "{args:?}: {error}");
        assert!(!error.contains("unexpected argument"), "{args:?}: {error}");
    }

    for command in ["recovery", "rule", "show"] {
        let long = asked(&[command, "--help"]);
        assert!(
            long.contains("Name it with `-C <CHECKOUT>`"),
            "{command} no longer takes the checkout with -C"
        );
        let short = asked(&[command, "-h"]);
        assert!(short.contains("-C <CHECKOUT>"), "{command} -h dropped -C: {short}");
        let usage = usage_of(command);
        assert!(!usage.contains("[CHECKOUT]"), "{usage}");
    }
    for command in ["scaffold", "watched", "draft"] {
        let short = asked(&[command, "-h"]);
        assert!(
            !short.contains("[CHECKOUT]") && !short.contains("-C"),
            "{command} gained a checkout form: {short}"
        );
    }
}

/// The `Usage:` line `-h` prints for `command`.
fn usage_of(command: &str) -> String {
    let short = asked(&[command, "-h"]);
    short
        .lines()
        .find(|line| line.starts_with("Usage:"))
        .unwrap_or_else(|| panic!("{command} -h has no usage line: {short}"))
        .to_string()
}

/// The fix line the index's rule writes from a usage line: drop `[OPTIONS]`,
/// and put `<checkout>` where `[CHECKOUT]` is.
fn fix_from_usage(usage: &str) -> String {
    let mut line = usage.trim_start_matches("Usage:").trim().to_string();
    line = line.replace("[OPTIONS] ", "");
    line = line.replace("<INSTANCE>", "<instance>");
    line = line.replace("<RUN>", "<run>");
    line.replace("[CHECKOUT]", "<checkout>")
}

/// `(exit, error)` of `args`. A usage refusal is exit 2; a checkout the
/// command cannot take is exit 1. Stderr stays empty either way.
fn ran(args: &[&str]) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args(args).output().unwrap();
    assert!(out.stderr.is_empty(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).unwrap();
    let error = serde_json::from_str::<serde_json::Value>(&stdout)
        .ok()
        .and_then(|answer| answer["error"].as_str().map(str::to_string))
        .unwrap_or(stdout);
    (out.status.code().unwrap(), error)
}

/// The index says what a bound repo is, and an unbound one, before it first
/// says either.
#[test]
fn the_index_says_what_a_bound_repo_is_before_it_names_one() {
    let index = help("--help");
    let at = index.find("A repo whose config names a team is a bound repo").expect("defined");
    assert!(!index[..at].to_lowercase().contains("bound"), "{}", &index[..at]);
    let next = index[at..].split(". ").nth(1).unwrap();
    assert!(next.starts_with("One whose config names none is unbound"), "{next}");
}

/// `cast`'s one line says automatic casting is gone, and what a worker runs.
#[test]
fn casts_one_line_says_what_it_does_in_plain_words() {
    let short = asked(&["cast", "-h"]);
    let line = short.lines().next().unwrap();
    for plain in
        ["automatic casting is gone", "model", "harness", "`model:`", "label", "user default"]
    {
        assert!(line.contains(plain), "{line}");
    }
    for term in
        ["harness and model", "`harness:`", "uncast", "a choice to make", "the host kit's first"]
    {
        assert!(!line.contains(term), "{line}");
    }
}

/// Whether `text` shows `key` as a key of a file: followed by its `:`, in
/// backticks or not, and not as the end of a longer key.
fn shows(text: &str, key: &str) -> bool {
    [format!("{key}:"), format!("{key}`:")].iter().any(|shown| {
        text.match_indices(shown.as_str())
            .any(|(at, _)| text[..at].ends_with([' ', '`', '{', '\n']))
    })
}

/// A team's files are described in the topics, each kind of file in its own:
/// the keys are the loader's, so one it learns is one a topic must describe.
#[test]
fn every_key_the_loader_knows_is_described_in_the_topic_of_its_file_and_in_no_other() {
    use afkconvoy_core::team::{
        ESCALATE_KEYS, LAND_KEYS, ON_KEYS, PATROL_KEYS, ROSTER_ENTRY_KEYS, RUN_KEYS, STAGE_KEYS,
        TEAM_KEYS, WORKFLOW_KEYS,
    };
    let files = [
        ("team", [&TEAM_KEYS[..], &ROSTER_ENTRY_KEYS, &RUN_KEYS].concat()),
        (
            "workflows",
            [&WORKFLOW_KEYS[..], &STAGE_KEYS, &ON_KEYS, &ESCALATE_KEYS, &LAND_KEYS].concat(),
        ),
        ("patrols", PATROL_KEYS.to_vec()),
    ];
    // What it catches: a key shown, and one that is only part of another's name.
    assert!(shows("x\nmax_open: 5", "max_open") && shows("{ by: <role>", "by"));
    assert!(shows("- `intake_limit`: the open issues", "intake_limit"));
    assert!(!shows("\nmax_open: 5", "open") && !shows("the stages follow", "stages"));
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

/// The form of a skill's file is said in one topic, which the texts an agent
/// reads on its way to writing a skill name: its path, the keys `check` holds
/// it to, and an example `check` accepts.
#[test]
fn the_skill_file_topic_says_the_form_check_holds_a_skill_s_file_to() {
    use afkconvoy_core::team::{Node, SKILL_FILE, SKILL_KEYS, SKILLS, malformed};
    let topic = asked(&["help", "skill-file"]);
    assert!(topic.contains(&format!("{SKILLS}/review/{SKILL_FILE}")), "{topic}");
    for key in SKILL_KEYS {
        assert!(heads(&topic).contains(&key), "no row for the key {key}");
    }
    // The example it shows is a skill's file: the one of the skill `review`.
    let shown = common::shown("topic `skill-file`", "markdown");
    let front = shown[0].strip_prefix("---\n").and_then(|file| file.split_once("\n---\n"));
    let (front, _) = front.unwrap_or_else(|| panic!("no front matter: {shown:?}"));
    assert!(front.lines().count() >= SKILL_KEYS.len(), "{front}");
    let read: Node = serde_yaml::from_str(front).unwrap();
    assert_eq!(malformed("review", &Ok(read.clone())), None);
    assert!(malformed("rule", &Ok(read)).is_some());
    // Whoever is about to write a skill is sent to it.
    let named = "`afkconvoy help skill-file`";
    for from in
        [&["help", "roles"][..], &["help", "team"], &["scaffold", "--help"], &["check", "--help"]]
    {
        assert!(asked(from).contains(named), "{from:?} does not name {named}");
    }
    assert!(guide(&help("--help")).contains(named));
}

/// `--help` and `check --help` name the topics where they held the
/// description of a team's files, and hold none of it.
#[test]
fn the_guide_and_check_name_the_topics_and_describe_no_file_of_a_team() {
    use afkconvoy_core::team::{PATROL_KEYS, STAGE_KEYS, TEAM_KEYS, WORKFLOW_KEYS};
    let long = help("--help");
    let guide = guide(&long);
    let check = asked(&["check", "--help"]);
    let check = section(&check, "## What it reads and writes");
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
fn the_team_topic_lays_out_the_files_of_a_team_and_a_role() {
    use afkconvoy_core::harness::Harness;
    use afkconvoy_core::team::{PATROLS, ROLE_FORMS, RULES, SKILLS, TEAM_FILE, WORKFLOWS};
    let team = asked(&["help", "team"]);
    // A role's limits are laid out once for each harness.
    let limits = Harness::ALL.map(Harness::limits);
    for file in [&[TEAM_FILE, WORKFLOWS, PATROLS, RULES, SKILLS][..], &limits].concat() {
        assert!(team.contains(&format!("\n- `{file}")), "the topic lays out no {file}");
    }
    // The team rules are a file of the team's; what each of the four kinds of
    // text a worker reads is for is the topic `roles`'s to say.
    assert!(team.contains(&format!("\n- `{RULES}`: the team rules: ")), "{team}");
    assert!(team.contains("where a sentence belongs: [`afkconvoy help roles`](help:roles)"));
    // A role is a directory, named in one of three ways.
    for form in ["owner/repo", "owner/repo/path", "./path"] {
        assert!(heads(&team).contains(&form), "the topic lost the form {form:?}");
    }
    assert!(team.contains(ROLE_FORMS));
    // How a repo is bound, and what `check` holds the team to.
    for said in ["## How a repo is bound to it", "intake_limit", "## What `check` holds it to"] {
        assert!(team.contains(said), "the topic lost {said:?}");
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

/// The README points readers to the binary's help, the one manual: no page
/// of it is kept beside the code (sunfmin/control#243).
#[test]
fn the_readme_points_to_the_help_as_the_whole_manual() {
    for entry in ["afkconvoy --help", "afkconvoy help <topic>", "afkconvoy help all"] {
        assert!(README.contains(entry), "the README lost {entry:?}");
    }
    assert!(!README.contains("docs/manual.md"));
    assert!(!repository().join("docs/manual.md").exists());
}

#[test]
fn the_short_help_is_still_one_screen() {
    let short = help("-h");
    assert!(short.lines().count() <= 57, "{short}");
    assert!(!short.contains("THE COMMANDS"));
}

/// The skill file is one file: `skill` prints it, and `npx skills add` installs
/// it. It says when to reach for afkconvoy and sends its reader to `--help`.
#[test]
fn skill_prints_the_skill_file_which_names_no_command_but_the_help() {
    assert_eq!(asked(&["skill"]), SKILL);
    // The frontmatter a Claude Code skill needs.
    assert!(SKILL.starts_with("---\nname: afkconvoy\ndescription: "));
    let mut parts = SKILL.splitn(3, "---\n").skip(1);
    let (front, body) = (parts.next().unwrap(), parts.next().unwrap());
    assert_eq!(front.lines().count(), 2, "{front}");
    for when in ["GitHub issues", "unattended", "a team of roles", "waits for a human"] {
        assert!(front.contains(when), "the description lost {when:?}");
    }
    // What it catches: a command that is not the help.
    let run = |text: &'static str| text.split("afkconvoy ").skip(1).collect::<Vec<_>>();
    assert_eq!(
        run("`afkconvoy --help`, then `afkconvoy doctor`: x"),
        ["--help`, then `", "doctor`: x"]
    );
    assert!(body.contains("`afkconvoy --help`"));
    for named in SKILL.split("afkconvoy ").skip(1) {
        assert!(named.starts_with("--help`"), "the skill names a command: afkconvoy {named}");
    }
    let long = help("--help");
    for line in guide(&long).lines().map(str::trim).filter(|line| line.len() > 20) {
        assert!(!SKILL.contains(line), "the skill restates the help: {line}");
    }
    assert!(SKILL.lines().count() <= 10, "{SKILL}");
    assert!(README.contains("npx skills add sunfmin/afkconvoy"));
    assert!(README.contains("afkconvoy skill > "));
}

#[test]
fn the_help_says_skill_prints_text_and_its_own_where_to_put_the_file() {
    let long = manual();
    let answers = between(guide(&long), "## What a command answers", "## The commands").join(" ");
    assert!(answers.contains("`skill` the text of a file"), "{answers}");
    assert_eq!(group(&long, "Run by whoever sets an agent up"), ["skill"]);
    let own = asked(&["skill", "--help"]);
    assert!(own.lines().nth(2).unwrap().ends_with("text, not JSON"));
    let when = section(&own, "## When to run it");
    let (user, project) = when.split_once("for a project").expect("both places");
    assert!(user.contains("for a user") && user.contains(" > ~/.claude/skills/afkconvoy/SKILL.md"));
    assert!(project.contains(" > .claude/skills/afkconvoy/SKILL.md"), "{project}");
    assert!(section(&own, "## What it answers").contains("no JSON object"));
}

/// The index names `panel` and a doctor with a critical finding, beside the
/// answers that were already not one JSON object. A checkout `doctor --all`
/// could not ask stays `{"error", "see"}`, which is not a doctor that read it.
#[test]
fn what_a_command_answers_names_panel_and_a_doctor_with_a_critical_finding() {
    let long = manual();
    let answers = between(guide(&long), "## What a command answers", "## The commands").join(" ");
    for said in [
        "`show` a drawing unless asked about an issue",
        "`help` Markdown as `--help` does",
        "`skill` the text of a file",
        "`guard` answers as a hook does",
        "`exclusive` as its command does",
        "`panel` without `--open` one HTML document",
        "`panel --open` is the JSON object `panel --help` describes",
        "exits 0 with no critical finding, and non-zero with one",
        "printing the same object, not {\"error\"}",
        "Under `--all`, a checkout that could not be asked stays {\"error\", \"see\"}",
        "as `doctor --help` says: not a doctor that read the checkout",
        "A fleet instance prints nothing",
    ] {
        assert!(answers.contains(said), "lost {said:?}\n{answers}");
    }
    let see = answers.find("{\"error\", \"see\"}").unwrap();
    let read = answers.find("a doctor that read the checkout").unwrap();
    assert!(see < read, "{answers}");
}

/// What a subcommand's `--help` says under each of these headings, in this
/// order.
const SECTIONS: [&str; 5] = [
    "## When to run it",
    "## Who runs it",
    "## What it reads and writes",
    "## What it answers",
    "## What trips you up",
];

/// The body of one section of a subcommand's `--help`.
fn section(long: &str, title: &str) -> String {
    let next = SECTIONS.iter().position(|section| *section == title).unwrap() + 1;
    between(long, title, SECTIONS.get(next).copied().unwrap_or("\0")).join("\n")
}

#[test]
fn every_visible_subcommand_has_its_text_and_keeps_its_one_line() {
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
        // The one line is the same on both, under the command's heading on
        // `--help`, and the one the listing gives.
        let line = short.lines().next().unwrap();
        let heading = format!("# `afkconvoy {command}`");
        assert_eq!(long.lines().take(3).collect::<Vec<_>>(), [heading.as_str(), "", line]);
        let row = top.lines().find(|row| row.trim_start().starts_with(&format!("{command} ")));
        assert!(row.unwrap().ends_with(line), "{command}: {line}");
    }
}

#[test]
fn the_guide_and_each_subcommand_agree_on_who_runs_it_and_why() {
    let long = manual();
    let who = |command: &str| section(&asked(&[command, "--help"]), "## Who runs it");
    let workers = group(&long, "Run by a worker");
    assert_eq!(
        workers.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "gate",
            "land",
            "verdict",
            "intake",
            "lock",
            "over",
            "exclusive",
            "no-node",
            "cut",
            "watched",
            "asked"
        ])
    );
    for command in &workers {
        let who = who(command);
        if *command == "watched" {
            assert!(who.contains("watch") && who.contains("watch directory"), "{who}");
            continue;
        }
        if *command == "asked" {
            assert!(who.contains("asking session") && who.contains("operator"), "{who}");
            continue;
        }
        assert!(who.contains("worker") && who.contains("in its own worktree"), "{command}: {who}");
    }
    // The guard is nobody's to run: it is a hook of Claude Code's.
    let hooks = group(&long, "Run by a worker's hook");
    assert_eq!(hooks, ["guard"]);
    assert!(who("guard").contains("Claude Code") && who("guard").contains("hook"));
    // Nor is opening issues the operator's: whoever has work for the fleet does.
    // Waking the fleet after opening them is theirs too, and a worker's last.
    let hands = group(&long, "Run by whoever hands the fleet work");
    assert_eq!(hands, ["flows", "wake"]);
    for command in &hands {
        assert!(who(command).contains("hands the fleet work"), "{command}");
    }
    assert!(who("wake").contains("worker") && who("wake").contains("in its own worktree"));
    // Nor is giving an agent of one's own the skill file.
    let sets_up = group(&long, "Run by whoever sets an agent up");
    assert_eq!(sets_up, ["skill"]);
    assert!(who("skill").contains("sets an agent up"));
    let operated = |command: &&str| {
        ![&workers, &hooks, &hands, &sets_up].iter().any(|group| group.contains(command))
    };
    for command in listed(&help("-h")).into_iter().filter(operated) {
        assert!(who(command).contains("operator"), "{command}");
    }
    // Each transition the guide says a tick makes on its own says so itself,
    // and when to make it by hand; `recovery` only shows one.
    let by_hand = group(&long, "One transition by hand");
    for command in by_hand.iter().filter(|command| **command != "recovery") {
        let when = section(&asked(&[command, "--help"]), "## When to run it");
        let when = when.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(when.contains("by hand") && when.contains("A tick does this on its own"), "{when}");
    }
    // …and the plumbing, that it is seldom the command to reach for.
    for command in ["claim", "release", "heartbeat"] {
        let when = section(&asked(&[command, "--help"]), "## When to run it");
        assert!(when.to_lowercase().contains("hardly ever"), "{command}: {when}");
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
    world.list_checkout(&world.clone);
    let mut seen = BTreeSet::new();
    let mut check = |args: &[&str], marks: &[&str]| {
        let command = args[0];
        let answer = world.afk(&[args, &["--now", "1000"]].concat()).ok();
        let answer = answer.as_object().unwrap();
        for mark in marks {
            assert!(answer.contains_key(*mark), "{args:?} is not the case meant: {answer:?}");
        }
        let help = asked(&[command, "--help"]);
        // A command that acts says under "What it logs" what its wake adds,
        // before its usage.
        let logs = help.split_once("\n## What it logs\n").map_or("", |(_, logs)| logs);
        let logs = logs.split("\n## Usage\n").next().unwrap();
        let said = section(&help, "## What it answers") + logs;
        let words: BTreeSet<&str> =
            said.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').collect();
        for key in answer.keys() {
            assert!(words.contains(key.as_str()), "`{command} --help` never names {key:?}");
        }
        seen.insert(command.to_string());
    };
    let agent = [];
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
    check(&["stop"], &["removed"]);
    check(&["stop", "--all"], &["stopped"]);
    check(&["checkouts"], &["host_runner"]);
    check(&["doctor", "--all"], &["checkouts"]);
    assert!(seen.len() >= 15, "{seen:?}");
}

#[test]
fn the_help_of_tick_says_for_each_kind_of_end_what_becomes_of_the_terminal_and_the_worktree() {
    let tick = asked(&["tick", "--help"]);
    let from = tick.find("\n- The step that ends a piece of work").unwrap();
    let said = &tick[from..from + tick[from..].find("\n- An `escalate`").unwrap()];
    for (end, terminal, worktree) in [
        ("- `end_patrol`: ", "terminal is closed", "checkout removed"),
        ("- advance, to a stage or a consult: ", "terminal is closed", "worktree kept"),
        ("- advance, to `done`: ", "terminal is closed", "worktree removed"),
        ("- an escalation: ", "terminal is closed", "worktree"),
    ] {
        let from = said.find(end).unwrap_or_else(|| panic!("{end:?} in {said}"));
        let row = said[from..].lines().next().unwrap();
        assert!(row.contains(terminal) && row.contains(worktree), "{end:?}: {row}");
    }
    assert!(said.contains("A run that failed keeps its checkout"), "{said}");
    assert!(
        said.contains("An issue that lands loses its worktree, and its worker with it"),
        "{said}"
    );
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

/// Whoever opens issues with `gh` learns from `flows` that that woke no fleet,
/// how soon its next tick takes them up, and what starts that tick now; and
/// `-h` does not give `wake` as a worker's alone.
#[test]
fn the_help_of_flows_says_when_an_issue_opened_with_gh_is_taken_up_and_what_wakes_the_fleet() {
    use afkconvoy_core::tick::{BUSY_SECONDS, IDLE_AFTER, IDLE_SECONDS};
    let trips = section(&asked(&["flows", "--help"]), "## What trips you up");
    let said = trips.lines().find(|line| line.contains("wakes no fleet")).expect(&trips);
    for part in [
        "at its next tick".to_string(),
        format!("within {BUSY_SECONDS} seconds while it has work"),
        format!("within {} minutes once it has been idle {IDLE_AFTER} ticks", IDLE_SECONDS / 60),
        "`afkconvoy wake <checkout>`, on the fleet's host, starts that tick now".to_string(),
    ] {
        assert!(said.contains(&part), "{part:?} in {said}");
    }
    let short = help("-h");
    let wake = short.lines().find(|line| line.trim_start().starts_with("wake ")).unwrap();
    assert!(wake.ends_with("(after opening issues with `gh`, and as a worker's last command)"));
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
        "A new commit is not it",
        "without what a worker is not given of it: the directories of its roles",
        "a different commit is all there is to go by",
    ] {
        assert!(moved.contains(part), "{part:?} is not in:\n{moved}");
    }
}

#[test]
fn the_help_of_check_and_of_doctor_describe_the_key_that_names_a_topic() {
    let check = asked(&["check", "--help"]);
    for part in [
        r#"each {"file", "key", "problem", "see"}"#,
        "`see` is the command that prints the help topic describing that file",
        "A fault in a file no topic describes has no `see`.",
    ] {
        assert!(check.contains(part), "{part:?} is not in `check --help`");
    }
    let doctor = asked(&["doctor", "--help"]);
    for part in [
        "evidence, fix, see, watch_id}",
        "- `see`: the command that prints the help topic explaining it",
        "`afkconvoy help humans` where what it reports waits for",
    ] {
        assert!(doctor.contains(part), "{part:?} is not in `doctor --help`");
    }
}

/// The topic `cache` is where the cache directory is named whole — the path,
/// what moves it, what it holds — and every other help that speaks of it sends
/// its reader there instead of naming a path of its own.
#[test]
fn one_topic_says_where_the_cache_directory_is_and_every_other_help_sends_there() {
    let topic = asked(&["help", "cache"]);
    for said in [
        "$XDG_CACHE_HOME/afkconvoy",
        "$HOME/.cache/afkconvoy",
        "Setting XDG_CACHE_HOME, and nothing else",
        "copies/<owner>/<repo>/<commit>",
        "harness/pi, harness/codex",
        "`runs/`",
    ] {
        assert!(topic.contains(said), "{said:?} is not in:\n{topic}");
    }
    let cache = topic_file("cache");
    for (file, text) in help_texts().into_iter().filter(|(file, _)| *file != cache) {
        for path in ["XDG_CACHE_HOME", ".cache"] {
            assert!(!text.contains(path), "{file} names {path:?}: the topic does");
        }
    }
    for command in ["check", "dispatch", "run", "doctor"] {
        let flat = asked(&[command, "--help"]).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains("`afkconvoy help cache` says where"), "{command}");
    }
}

#[test]
fn the_help_of_land_describes_the_exact_commit_and_each_failed_turn() {
    let land = asked(&["land", "--help"]);
    for says in [
        "gate that exact commit",
        "push that commit to the target branch as a fast-forward",
        "recorded green",
        "target_moved",
        "no gate ran and nothing landed",
        "separate checkout",
        "Ordinary turns wait",
    ] {
        assert!(
            land.contains(says) || land.to_lowercase().contains(&says.to_lowercase()),
            "{says}: {land}"
        );
    }
}

/// `checkouts` answers where the host runner serves the panel, and its help
/// says so instead of sending its reader to `panel --open` for it.
#[test]
fn the_help_of_checkouts_says_what_host_runner_panel_holds() {
    let checkouts = asked(&["checkouts", "--help"]);
    assert!(!checkouts.contains("names no panel"), "{checkouts}");
    let row = checkouts.lines().find(|line| line.starts_with("- `host_runner.panel` "));
    let row = row.unwrap_or_else(|| panic!("`host_runner.panel` in {checkouts}"));
    for says in ["where the host runner serves the panel", "Where no host runner runs it is null"] {
        assert!(row.contains(says), "{says:?}: {row}");
    }
}

#[test]
fn kit_and_doctor_print_the_glossarys_research_states_and_kit_gives_both_outcomes() {
    let glossary = fs::read_to_string(repository().join("GLOSSARY.md")).unwrap();
    let states = glossary.lines().find(|line| line.starts_with("Research states: ")).unwrap();
    for command in ["kit", "doctor"] {
        assert!(asked(&[command, "--help"]).contains(states), "{command}");
    }
    let kit = asked(&["kit", "--help"]);
    assert!(kit.contains("\"settled\":\"approval reviewer, unsuited to workers\""));
    assert!(kit.contains("\"good_at\":\"wide refactors; slower on small fixes\""));
    assert!(kit.contains("{path, written, placed, settled, kit}"));
}

#[test]
fn the_retired_train_commands_and_lanes_are_absent_from_public_help() {
    let help = manual();
    for retired in [
        "--train-lanes",
        "AFKCONVOY_TRAIN_LANES",
        "cut_train",
        "tend_train",
        "afkconvoy abandon",
        "landing train given up",
    ] {
        assert!(!help.contains(retired), "{retired} in help");
    }
}

#[test]
fn kit_and_switch_help_explain_operator_removal_and_restoration() {
    let kit = asked(&["kit", "--help"]);
    for says in [
        "afkconvoy kit --remove <harness>",
        "removed by the operator",
        "worker-launch discovery leave it out",
        "Only `afkconvoy switch --harness <harness>` clears the removal",
        "Removal ends no worker",
        "only `kit --remove` records the removal that survives a refresh",
    ] {
        assert!(kit.contains(says), "kit help lost {says:?}");
    }
    let switch = asked(&["switch", "--help"]);
    for says in [
        "the switch clears its removal and puts it first",
        "afkconvoy switch --all --harness <harness>",
        "restored says whether this switch cleared an operator removal",
    ] {
        assert!(switch.contains(says), "switch help lost {says:?}");
    }
}
