//! A harness (docs/adr/0013): the program a worker is a session of. What the
//! fleet adds to a worker launch command to start a worker in its role
//! (docs/adr/0009), the question that asks a command whether it takes that,
//! the report to the worker backend, and how a tool call reaches the guard
//! and is answered are each one harness's own. What is one harness's is in
//! its module here and nowhere else: nothing outside names a flag, a
//! variable, a hook event or a path of it.

use serde::Deserializer;

use crate::guard::Call;
use crate::line::Line;
use crate::role::Start;

mod claude_code;

/// A harness the fleet can start a worker of a bound repo on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
}

impl Harness {
    /// The harness of a worker nothing names one for.
    pub const DEFAULT: Harness = Harness::ClaudeCode;

    /// What a human calls it.
    pub const fn name(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::NAME,
        }
    }

    /// The word that runs it, where nothing stands for it.
    pub const fn program(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::PROGRAM,
        }
    }

    /// The worker launch command its workers are started with where nobody
    /// said another.
    pub const fn stock(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::STOCK,
        }
    }

    /// The variable that points it at a custom provider. A worker's shell
    /// inherits none of the fleet's, so the stock command would start a
    /// worker on another provider than the fleet's own.
    pub const fn provider_variable(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::PROVIDER,
        }
    }

    /// Every flag the fleet adds to it: what a worker launch command that
    /// starts it must take.
    pub const fn flags(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::FLAGS,
        }
    }

    /// The shell line that tells the worker backend of the state of a worker
    /// of this harness (`Start::report`). On a host where the backend
    /// installed nothing for it, nothing is told.
    pub const fn report(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::REPORT,
        }
    }

    /// The line a terminal opened on the issue's worktree runs to start a
    /// worker of this harness in its role with the worker launch command
    /// `command`: change to the role's copy, put the stage's facts in the
    /// environment, and start the command with the worktree and the team's
    /// copy added, the user's configuration left out, the guard and the
    /// report added, and the stage's model — each in this harness's way.
    pub fn in_role(self, command: &str, start: &Start) -> String {
        let mut line = Line::new().fixed("cd ").word(start.role_dir).fixed(" && export");
        for variable in self.environment() {
            line = line.fixed(" ").fixed(variable).fixed("=1");
        }
        for (name, value) in start.facts.env() {
            line = line.fixed(" ").fixed(name).fixed("=").word(&value);
        }
        let line = line.fixed(" && ").launch(command);
        match self {
            Harness::ClaudeCode => claude_code::added(line, start),
        }
        .done()
    }

    /// The variables this harness is switched by, each set in the
    /// environment of a worker in its role.
    const fn environment(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::ENVIRONMENT,
        }
    }

    /// The shell line whose answer says which flags `command` takes
    /// (`takes` reads it).
    pub fn flags_question(self, command: &str) -> String {
        let line = Line::new().launch(command);
        match self {
            Harness::ClaudeCode => claude_code::asked_its_flags(line),
        }
        .done()
    }

    /// Whether the worker launch command `command` takes what a bound repo
    /// adds to it for this harness, given what the worker's shell printed for
    /// `flags_question` (`None`: it failed, or printed nothing). An error
    /// names the reason: used anyway, every worker would start outside its
    /// role, or not at all.
    pub fn takes(self, command: &str, answer: Option<&str>) -> Result<(), String> {
        let refused = |why: String| {
            Err(format!(
                "this repo is bound to a team, whose workers are started in their roles, and the \
                 worker launch command {command:?} cannot do that: {why}. A bound repo's worker \
                 launch command must end in {} taking flags",
                self.name()
            ))
        };
        let question = self.flags_question(command);
        let Some(answer) = answer.filter(|answer| !answer.trim().is_empty()) else {
            return refused(format!("`{question}` failed in a login shell, or said nothing"));
        };
        let missing: Vec<&str> =
            self.flags().iter().copied().filter(|flag| !answer.contains(flag)).collect();
        if missing.is_empty() {
            Ok(())
        } else {
            refused(format!("`{question}` names no {}", missing.join(", no ")))
        }
    }

    /// The tool call this harness describes in `described`, as the guard
    /// judges one. An error is a description of no tool call.
    pub fn call<'de, D: Deserializer<'de>>(self, described: D) -> Result<Call, D::Error> {
        match self {
            Harness::ClaudeCode => claude_code::call(described),
        }
    }

    /// The exit code that tells this harness the guard refuses a call: the
    /// call is not made, and what the guard wrote on stderr is shown to the
    /// worker.
    pub const fn refused(self) -> u8 {
        match self {
            Harness::ClaudeCode => claude_code::REFUSED,
        }
    }
}
