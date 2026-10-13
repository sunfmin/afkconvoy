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
use crate::role::{HARNESS_ENV, Start};

mod claude_code;
mod codex;
mod pi;

/// A file of the host user's that a directory the host keeps for a harness
/// links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// Where it is, under the home of the host's user.
    pub home: &'static str,
    /// Where the link to it is, under the directory.
    pub kept: &'static str,
}

/// What a host keeps for one harness before it starts a worker of it, in a
/// directory of the fleet's own: what that harness is given that is neither
/// its role's nor, but for what is linked to, the host user's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    /// The files of the host user's it links to. One the user has none of is
    /// left out.
    pub links: &'static [Link],
    /// The files the tool carries, each by where it is under the directory
    /// and by what it holds.
    pub files: &'static [(&'static str, &'static str)],
}

/// What a role's directory holds where its limits for one harness go
/// (`Harness::limits`).
pub enum Held<D> {
    Nothing,
    /// A file, with what reads it.
    File(D),
    /// A directory of this many entries.
    Entries(usize),
}

/// A harness the fleet can start a worker of a bound repo on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
    Pi,
    Codex,
}

/// A harness is written, wherever the fleet answers with one, as its key.
impl serde::Serialize for Harness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

impl Harness {
    /// The harness of a worker nothing names one for.
    pub const DEFAULT: Harness = Harness::ClaudeCode;

    /// The harness of a worker started, and of a stage record written,
    /// before either named one: there was one harness then.
    pub const UNNAMED: Harness = Harness::ClaudeCode;

    /// Every harness there is.
    pub const ALL: [Harness; 3] = [Harness::ClaudeCode, Harness::Pi, Harness::Codex];

    /// What a team, a config and a stage record write it as.
    pub const fn key(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::KEY,
            Harness::Pi => pi::KEY,
            Harness::Codex => codex::KEY,
        }
    }

    /// The harness written as `key`; `None` when that is none.
    pub fn read(key: &str) -> Option<Harness> {
        Harness::ALL.into_iter().find(|harness| harness.key() == key)
    }

    /// Why `key` is no harness, with the ones there are.
    pub fn unknown(key: &str) -> String {
        let keys: Vec<&str> = Harness::ALL.iter().map(|harness| harness.key()).collect();
        format!("`{key}` is no harness; the harnesses are: {}", keys.join(", "))
    }

    /// What a human calls it.
    pub const fn name(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::NAME,
            Harness::Pi => pi::NAME,
            Harness::Codex => codex::NAME,
        }
    }

    /// The word that runs it, where nothing stands for it.
    pub const fn program(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::PROGRAM,
            Harness::Pi => pi::PROGRAM,
            Harness::Codex => codex::PROGRAM,
        }
    }

    /// The worker launch command its workers are started with where nobody
    /// said another.
    pub const fn stock(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::STOCK,
            Harness::Pi => pi::STOCK,
            Harness::Codex => codex::STOCK,
        }
    }

    /// The variable that points it at a custom provider, where one does. A
    /// worker's shell inherits none of the fleet's, so the stock command
    /// would start a worker on another provider than the fleet's own. pi
    /// has none: its providers are read from its user directory. Codex has
    /// none either: it keeps a custom provider in its user's configuration,
    /// which a worker is given none of.
    pub const fn provider_variable(self) -> Option<&'static str> {
        match self {
            Harness::ClaudeCode => Some(claude_code::PROVIDER),
            Harness::Pi | Harness::Codex => None,
        }
    }

    /// Every flag the fleet adds to it: what a worker launch command that
    /// starts it must take.
    pub const fn flags(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::FLAGS,
            Harness::Pi => &pi::FLAGS,
            Harness::Codex => &codex::FLAGS,
        }
    }

    /// What a host keeps for it before it starts a worker of it
    /// (`Start::kept`). For Claude Code, nothing.
    pub const fn kept(self) -> Kept {
        match self {
            Harness::ClaudeCode => Kept { links: &[], files: &[] },
            Harness::Pi => pi::KEPT,
            Harness::Codex => codex::KEPT,
        }
    }

    /// Where a role's limits for it are written, in the role's directory, in
    /// its own form (docs/adr/0013).
    pub const fn limits(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::LIMITS,
            Harness::Pi => pi::LIMITS,
            Harness::Codex => codex::LIMITS,
        }
    }

    /// Whether a role holds limits for it, given what the role's directory
    /// `held` at `limits`. What they refuse is not read: that a role's
    /// limits for two harnesses refuse the same things is the role's to see
    /// to. An error is something there that cannot be read as it must be.
    pub fn limited<'de, D: Deserializer<'de>>(self, held: Held<D>) -> Result<bool, D::Error> {
        match self {
            Harness::ClaudeCode => claude_code::limited(held),
            Harness::Pi => Ok(pi::limited(held)),
            Harness::Codex => codex::limited(held),
        }
    }

    /// The files it would load, from a role's directory, in place of the
    /// rules every harness works under (`team::RULES`). A role holds none.
    pub const fn before_the_rules(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &[],
            Harness::Pi => &pi::BEFORE_THE_RULES,
            Harness::Codex => &codex::BEFORE_THE_RULES,
        }
    }

    /// The files it is told the repo's own rules from, on the line that
    /// starts it — the first of them the worktree holds — and the team rules
    /// with them (`Start::told`): it loads rules from no directory but the
    /// one it starts in. Empty: it loads both itself, and is told nothing.
    pub const fn told(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode | Harness::Pi => &[],
            Harness::Codex => &codex::REPO_RULES,
        }
    }

    /// The directories of the one it starts in where it finds skills, when
    /// those are not where a role's are (`team::SKILLS`). The fleet's copy
    /// of a role that holds none of them links the first to the role's
    /// skills; a role that holds one is left as it is, and a worker is shown
    /// the skills there. Empty: it finds a role's where they are, or is
    /// named them.
    pub const fn skills(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode | Harness::Pi => &[],
            Harness::Codex => &codex::SKILLS,
        }
    }

    /// The shell line whose answer says which models it knows on a host,
    /// asked of the worker launch command `command` with what the host keeps
    /// for it in `kept` (`models` reads it). `None`: it has no way to say.
    pub fn models_question(self, command: &str, kept: &str) -> Option<String> {
        match self {
            Harness::ClaudeCode => None,
            Harness::Pi => Some(pi::models_question(command, kept)),
            Harness::Codex => Some(codex::models_question(command, kept)),
        }
    }

    /// The models it knows, given what the worker's shell printed for
    /// `models_question`. `None`: it said none, and a model named for it is
    /// passed as written.
    pub fn models(self, answer: &str) -> Option<Vec<String>> {
        match self {
            Harness::ClaudeCode => None,
            Harness::Pi => pi::models(answer),
            Harness::Codex => codex::models(answer),
        }
    }

    /// Whether `model`, its own name for one as a stage writes it, is among
    /// `listed`, the models it said it knows (`models`). A harness with no
    /// way to say knows whatever it is given.
    pub fn knows(self, listed: &[String], model: &str) -> bool {
        match self {
            Harness::ClaudeCode => true,
            Harness::Pi => pi::knows(listed, model),
            Harness::Codex => codex::knows(listed, model),
        }
    }

    /// Whether `model` is the whole name of one of `listed`: nothing another
    /// harness's name for a model would be taken for.
    pub fn names(self, listed: &[String], model: &str) -> bool {
        match self {
            Harness::ClaudeCode => false,
            Harness::Pi => pi::names(listed, model),
            // Codex knows a model by its whole name and no other way.
            Harness::Codex => codex::knows(listed, model),
        }
    }

    /// The line a terminal opened on the issue's worktree runs to start a
    /// worker of this harness in its role with the worker launch command
    /// `command`: change to the role's copy, put the stage's facts and this
    /// harness in the environment, and start the command with what this
    /// harness does not load from there: the worktree and its rules, the
    /// user's configuration left out, the guard and the report to the worker
    /// backend, and the stage's model — each in this harness's way.
    pub fn in_role(self, command: &str, start: &Start) -> String {
        let mut line = Line::new().fixed("cd ").word(start.role_dir).fixed(" && export");
        for variable in self.environment() {
            line = line.fixed(" ").fixed(variable).fixed("=1");
        }
        let mut line = match self {
            Harness::ClaudeCode => line,
            Harness::Pi => pi::environment(line, start),
            Harness::Codex => codex::environment(line, start),
        };
        for (name, value) in start.facts.env() {
            line = line.fixed(" ").fixed(name).fixed("=").word(&value);
        }
        line = line.fixed(" ").fixed(HARNESS_ENV).fixed("=").word(self.key());
        let line = line.fixed(" && ").launch(command);
        match self {
            Harness::ClaudeCode => claude_code::added(line, start),
            Harness::Pi => pi::added(line, start),
            Harness::Codex => codex::added(line, start),
        }
        .done()
    }

    /// The variables this harness is switched by, each set in the
    /// environment of a worker in its role. What it reads a value from is
    /// its module's to add.
    const fn environment(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::ENVIRONMENT,
            Harness::Pi | Harness::Codex => &[],
        }
    }

    /// The shell line whose answer says which flags `command` takes
    /// (`takes` reads it).
    pub fn flags_question(self, command: &str) -> String {
        let line = Line::new().launch(command);
        match self {
            Harness::ClaudeCode => claude_code::asked_its_flags(line),
            Harness::Pi => pi::asked_its_flags(line),
            Harness::Codex => codex::asked_its_flags(line),
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
            Harness::Pi => pi::call(described),
            Harness::Codex => codex::call(described),
        }
    }

    /// The exit code that tells this harness the guard refuses a call: the
    /// call is not made, and what the guard wrote on stderr is shown to the
    /// worker.
    pub const fn refused(self) -> u8 {
        match self {
            Harness::ClaudeCode => claude_code::REFUSED,
            Harness::Pi => pi::REFUSED,
            Harness::Codex => codex::REFUSED,
        }
    }
}
