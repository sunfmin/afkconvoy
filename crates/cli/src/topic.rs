//! The help topics: what belongs to no one command, printed by
//! `afkconvoy help <topic>` as plain text. `TOPICS` is the one place a topic
//! is named: `--help` lists what it holds, and adding a topic is adding its
//! file and its line here.

/// One help topic: the name it is asked for by, the line `--help` gives it,
/// and its whole text.
pub struct Topic {
    pub name: &'static str,
    pub line: &'static str,
    pub text: &'static str,
}

/// A topic that is not the repository's own glossary: the file
/// `help/topics/<name>.txt`.
macro_rules! topic {
    ($name:literal, $line:literal) => {
        Topic {
            name: $name,
            line: $line,
            text: include_str!(concat!("help/topics/", $name, ".txt")),
        }
    };
}

/// The topics, in the order `--help` lists them: the way in first, the
/// reference last.
pub const TOPICS: &[Topic] = &[
    topic!("concepts", "one issue from intake to landing, and the four things that carry it"),
    topic!("team", "a team's directory, its file, how it names a role, binding a repo"),
    topic!("roles", "what a role is, what goes where in one, and the four kinds of text"),
    topic!("workflows", "stages, verdicts, rework, a consult, a landing: a workflow's file"),
    topic!("patrols", "a role run on a schedule: a patrol's file, intake, the fleet lock"),
    topic!("example", "the team `scaffold --example hospital` writes, read file by file"),
    topic!("humans", "every way the fleet stops for a human, and the command that answers"),
    topic!("troubleshooting", "a symptom, its cause and its fix: each finding, each refusal"),
    Topic {
        name: "glossary",
        line: "every term the help uses, each with its one definition",
        text: include_str!("../../../GLOSSARY.md"),
    },
];

/// The text of the topic called `name`.
pub fn text(name: &str) -> Option<&'static str> {
    TOPICS.iter().find(|topic| topic.name == name).map(|topic| topic.text)
}

/// The topics as `--help` lists them: a name and its line, one topic a row.
pub fn listing() -> String {
    let width = TOPICS.iter().map(|topic| topic.name.len()).max().unwrap_or(0) + 2;
    let rows = TOPICS.iter().map(|topic| format!("      {:<width$}{}", topic.name, topic.line));
    rows.collect::<Vec<_>>().join("\n")
}

/// What `help` refuses a name with that is neither a command's nor a topic's.
pub fn unknown(name: &str) -> String {
    let names: Vec<_> = TOPICS.iter().map(|topic| topic.name).collect();
    format!(
        "`{name}` is neither a command nor a help topic. The topics: {}. The commands: \
         `afkconvoy -h` lists them.",
        names.join(", ")
    )
}
