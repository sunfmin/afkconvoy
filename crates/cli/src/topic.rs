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

/// A topic that is not the repository's own glossary is a file of
/// `help/topics/`, named `<name>.txt`.
pub const TOPICS: &[Topic] = &[Topic {
    name: "glossary",
    line: "every term the help uses, each with its one definition",
    text: include_str!("../../../GLOSSARY.md"),
}];

/// The text of the topic called `name`.
pub fn text(name: &str) -> Option<&'static str> {
    TOPICS.iter().find(|topic| topic.name == name).map(|topic| topic.text)
}

/// The topics as `--help` lists them: a name and its line, one topic a row.
pub fn listing() -> String {
    let rows = TOPICS.iter().map(|topic| format!("      {:<10}{}", topic.name, topic.line));
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
