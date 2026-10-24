//! The help topics: what belongs to no one command, each a type whose doc
//! comment is its Markdown, printed by `afkconvoy help <topic>` and placed in
//! the manual in the order `manual` registers them. A topic is named nowhere
//! else: a message points at one by its `REFERENCE`.

use afkconvoy_core::team::{Fault, PATROLS, TEAM_FILE, WORKFLOWS};
use agent_help::{Topic, TopicRef};
use serde::{Serialize, Serializer};

use crate::config;

#[derive(Topic)]
#[topic(name = "concepts")]
#[doc = include_str!("help/topics/concepts.md")]
pub struct Concepts;

#[derive(Topic)]
#[topic(name = "team")]
#[doc = include_str!("help/topics/team.md")]
pub struct Team;

#[derive(Topic)]
#[topic(name = "roles")]
#[doc = include_str!("help/topics/roles.md")]
pub struct Roles;

#[derive(Topic)]
#[topic(name = "roster")]
#[doc = include_str!("help/topics/roster.md")]
pub struct Roster;

#[derive(Topic)]
#[topic(name = "skill-file")]
#[doc = include_str!("help/topics/skill-file.md")]
pub struct SkillFile;

#[derive(Topic)]
#[topic(name = "workflows")]
#[doc = include_str!("help/topics/workflows.md")]
pub struct Workflows;

#[derive(Topic)]
#[topic(name = "patrols")]
#[doc = include_str!("help/topics/patrols.md")]
pub struct Patrols;

#[derive(Topic)]
#[topic(name = "example")]
#[doc = include_str!("help/topics/example.md")]
pub struct Example;

#[derive(Topic)]
#[topic(name = "example-run")]
#[doc = include_str!("help/topics/example-run.md")]
pub struct ExampleRun;

#[derive(Topic)]
#[topic(name = "cache")]
#[doc = include_str!("help/topics/cache.md")]
pub struct Cache;

#[derive(Topic)]
#[topic(name = "tick-log")]
#[doc = include_str!("help/topics/tick-log.md")]
pub struct TickLog;

#[derive(Topic)]
#[topic(name = "worker-launch")]
#[doc = include_str!("help/topics/worker-launch.md")]
pub struct WorkerLaunch;

#[derive(Topic)]
#[topic(name = "worker-proxy")]
#[doc = include_str!("help/topics/worker-proxy.md")]
pub struct WorkerProxy;

#[derive(Topic)]
#[topic(name = "humans")]
#[doc = include_str!("help/topics/humans.md")]
pub struct Humans;

#[derive(Topic)]
#[topic(name = "troubleshooting")]
#[doc = include_str!("help/topics/troubleshooting.md")]
pub struct Troubleshooting;

// The repository's own glossary: every term the help uses, each with its one
// definition. Nothing else is edited for what `help glossary` prints.
#[derive(Topic)]
#[topic(name = "glossary")]
#[doc = include_str!("../../../GLOSSARY.md")]
pub struct Glossary;

/// `help` refuses `name`, neither a command nor one of `topics`.
pub fn unknown(name: &str, topics: &[&str]) -> String {
    format!(
        "`{name}` is neither a command nor a help topic. The topics: {}. The commands: \
         `afkconvoy -h` lists them.",
        topics.join(", ")
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

    /// The command that prints the topic.
    pub fn reference(self) -> TopicRef {
        match self {
            See::Team => Team::REFERENCE,
            See::Workflows => Workflows::REFERENCE,
            See::Patrols => Patrols::REFERENCE,
            See::Humans => Humans::REFERENCE,
            See::Troubleshooting => Troubleshooting::REFERENCE,
        }
    }

    /// The command that prints the topic: what `see` holds.
    pub fn line(self) -> String {
        self.reference().to_string()
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
    fn every_see_names_a_topic_the_manual_answers() {
        let manual = crate::manual();
        for see in See::ALL {
            let line = see.line();
            let name = see.reference().topic();
            assert_eq!(line, format!("afkconvoy help {name}"));
            assert!(
                manual.topic_help(name).is_ok(),
                "a `see` says `{line}`, and there is no topic"
            );
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
