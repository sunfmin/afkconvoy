//! The help topics: what belongs to no one command, printed by
//! `afkconvoy help <topic>` as plain text. `TOPICS` is the one place a topic
//! is named: `--help` lists what it holds, and adding a topic is adding its
//! file and its line here.

use afkconvoy_core::team::{Fault, PATROLS, TEAM_FILE, WORKFLOWS};
use serde::{Serialize, Serializer};

use crate::config;

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
    topic!("skill-file", "a skill's file: where it is, its frontmatter, what `check` holds it to"),
    topic!("workflows", "stages, verdicts, rework, a consult, a landing: a workflow's file"),
    topic!("patrols", "a role run on a schedule: a patrol's file, intake, the fleet lock"),
    topic!("example", "the team `scaffold --example hospital` writes, read file by file"),
    topic!("example-run", "that team at work: what a host needs, and one issue's run"),
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

/// A topic that explains something that went wrong: what a fault, a finding
/// or a refusal carries as `see`. Every `see` is one of these, so that none
/// names a topic there is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum See {
    Team,
    Workflows,
    Patrols,
    Humans,
    Troubleshooting,
}

impl See {
    /// Every topic a `see` can name.
    pub const ALL: &[See] =
        &[See::Team, See::Workflows, See::Patrols, See::Humans, See::Troubleshooting];

    /// The name of the topic, as `TOPICS` has it.
    pub fn topic(self) -> &'static str {
        match self {
            See::Team => "team",
            See::Workflows => "workflows",
            See::Patrols => "patrols",
            See::Humans => "humans",
            See::Troubleshooting => "troubleshooting",
        }
    }

    /// The command that prints the topic: what `see` holds.
    pub fn line(self) -> String {
        format!("afkconvoy help {}", self.topic())
    }

    /// The topic that describes the file `fault` is in: a workflow's, a
    /// patrol's, or the team file — which names each role, and whose topic
    /// says what a role's directory holds — and, of a repo's config, the keys
    /// that bind it to a team. `None`: no topic describes that file.
    pub fn of_fault(fault: &Fault) -> Option<See> {
        let file = fault.file.as_str();
        let within = |dir: &str| file.strip_prefix(dir).is_some_and(|rest| rest.starts_with('/'));
        match file {
            TEAM_FILE | config::FILE => Some(See::Team),
            _ if within(WORKFLOWS) => Some(See::Workflows),
            _ if within(PATROLS) => Some(See::Patrols),
            _ => None,
        }
    }

    /// The refusal `why`, as one this topic explains.
    pub fn refusal(self, why: impl std::fmt::Display) -> anyhow::Error {
        Explained { see: self, why: why.to_string() }.into()
    }
}

impl Serialize for See {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.line())
    }
}

/// A refusal a topic explains: what a command that fails with it answers as
/// `error`, and the topic as `see`.
#[derive(Debug)]
pub struct Explained {
    pub see: See,
    why: String,
}

impl std::fmt::Display for Explained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.why)
    }
}

impl std::error::Error for Explained {}

/// The topic that explains `error`, when it was raised as one a topic
/// explains, whatever was said around it since.
pub fn explains(error: &anyhow::Error) -> Option<See> {
    error.chain().find_map(|cause| cause.downcast_ref::<Explained>()).map(|explained| explained.see)
}

/// Says which topic explains what a step is refused with.
pub trait Explain<T> {
    /// The same result, its error one `see` explains. The error is kept as
    /// what it says, not as what it was: it is for a refusal made here, and
    /// not for one something else reads the cause of.
    fn see(self, see: See) -> anyhow::Result<T>;
}

impl<T, E: Into<anyhow::Error>> Explain<T> for Result<T, E> {
    fn see(self, see: See) -> anyhow::Result<T> {
        self.map_err(|error| see.refusal(format!("{:#}", error.into())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_see_names_a_topic_there_is() {
        for see in See::ALL {
            let line = see.line();
            let name = line.strip_prefix("afkconvoy help ").unwrap();
            assert!(text(name).is_some(), "a `see` says `{line}`, and there is no topic {name}");
        }
    }

    #[test]
    fn a_fault_is_explained_by_the_topic_of_its_file() {
        let of = |file: &str| {
            let fault = Fault { file: file.into(), key: String::new(), problem: String::new() };
            See::of_fault(&fault)
        };
        assert_eq!(of("team.md"), Some(See::Team));
        assert_eq!(of("docs/agents/afkconvoy.md"), Some(See::Team));
        assert_eq!(of("workflows/fix.md"), Some(See::Workflows));
        assert_eq!(of("patrols/triage.md"), Some(See::Patrols));
        assert_eq!(of("patrols.md"), None);
        assert_eq!(of("precedents.md"), None);
    }

    #[test]
    fn a_refusal_keeps_its_topic_under_what_is_said_around_it() {
        let refused: anyhow::Result<()> = Err(anyhow::anyhow!("no")).see(See::Humans);
        let error = refused.unwrap_err().context("issue #3");
        assert_eq!(explains(&error), Some(See::Humans));
        assert_eq!(format!("{error:#}"), "issue #3: no");
        assert_eq!(explains(&anyhow::anyhow!("no")), None);
    }
}
