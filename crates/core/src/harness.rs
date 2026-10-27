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
use crate::role::{HARNESS_ENV, MODEL_ENV, Start};

mod claude_code;
mod codex;
mod dsh;
mod grok;
mod kimi;
mod pi;

pub(crate) use grok::not_authenticated as grok_not_authenticated;

/// A file or a directory of the host user's that a directory the host keeps
/// for a harness links to.
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
    /// The host user's own skills and rules, as this harness finds them in a
    /// session the user starts by hand: linked to as well, in a directory
    /// kept apart, for a worker given them (`Start::host_user`), and in no
    /// other. One the user has none of is left out.
    pub host_user: &'static [Link],
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

/// Where a probe's answer says which model answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnsweredAs {
    /// The answer says it: this id.
    Said(String),
    /// The answer names the session it was kept in: the file under `under`,
    /// in what the host keeps for the harness, whose name ends `ends`. That
    /// session file says it (`Harness::answered_in_session`), and is the
    /// probe's to delete.
    InSession { under: &'static str, ends: String },
    /// It says none.
    Unsaid,
}

/// A harness the fleet can start a worker of a bound repo on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    ClaudeCode,
    Pi,
    Codex,
    Dsh,
    Kimi,
    Grok,
}

/// A harness is written, wherever the fleet answers with one, as its key.
impl serde::Serialize for Harness {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.key())
    }
}

impl Harness {
    /// The harness's fixed capacity-stop words, allowing terminal wrapping.
    /// A rule belongs only to a harness whose screen has been observed.
    pub fn capacity_stop(self, output: &[String]) -> Option<String> {
        match self {
            Harness::Codex => codex::capacity_stop(output),
            _ => None,
        }
    }

    /// The harness's fixed task-refusal words on its screen. Whitespace may
    /// wrap in a terminal; other prose alone establishes no refusal rule.
    pub fn task_refusal(self, output: &[String]) -> Option<String> {
        match self {
            Harness::Codex => codex::task_refusal(output),
            _ => None,
        }
    }

    /// The harness of a worker nothing names one for.
    pub const DEFAULT: Harness = Harness::ClaudeCode;

    /// The harness of a worker started, and of a stage record written,
    /// before either named one: there was one harness then.
    pub const UNNAMED: Harness = Harness::ClaudeCode;

    /// Every harness there is.
    pub const ALL: [Harness; 6] = [
        Harness::ClaudeCode,
        Harness::Pi,
        Harness::Codex,
        Harness::Dsh,
        Harness::Kimi,
        Harness::Grok,
    ];

    /// What a team, a config and a stage record write it as.
    pub const fn key(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::KEY,
            Harness::Pi => pi::KEY,
            Harness::Codex => codex::KEY,
            Harness::Dsh => dsh::KEY,
            Harness::Kimi => kimi::KEY,
            Harness::Grok => grok::KEY,
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
            Harness::Dsh => dsh::NAME,
            Harness::Kimi => kimi::NAME,
            Harness::Grok => grok::NAME,
        }
    }

    /// The word that runs it, where nothing stands for it.
    pub const fn program(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::PROGRAM,
            Harness::Pi => pi::PROGRAM,
            Harness::Codex => codex::PROGRAM,
            Harness::Dsh => dsh::PROGRAM,
            Harness::Kimi => kimi::PROGRAM,
            Harness::Grok => grok::PROGRAM,
        }
    }

    /// The worker launch command its workers are started with where nobody
    /// said another.
    pub const fn stock(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::STOCK,
            Harness::Pi => pi::STOCK,
            Harness::Codex => codex::STOCK,
            Harness::Dsh => dsh::STOCK,
            Harness::Kimi => kimi::STOCK,
            Harness::Grok => grok::STOCK,
        }
    }

    /// The variable that points it at a custom provider, where one does. A
    /// worker's shell inherits none of the fleet's, so the stock command
    /// would start a worker on another provider than the fleet's own. pi
    /// has none: its providers are read from its user directory. Codex has
    /// none either: it keeps a custom provider in its user's configuration,
    /// which a worker is given none of. Kimi reads its generated model
    /// routing from its isolated home.
    pub const fn provider_variable(self) -> Option<&'static str> {
        match self {
            Harness::ClaudeCode => Some(claude_code::PROVIDER),
            Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi | Harness::Grok => None,
        }
    }

    /// Every flag the fleet adds to it: what a worker launch command that
    /// starts it must take.
    pub const fn flags(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::FLAGS,
            Harness::Pi => &pi::FLAGS,
            Harness::Codex => &codex::FLAGS,
            Harness::Dsh => &dsh::FLAGS,
            Harness::Kimi => &kimi::FLAGS,
            Harness::Grok => &grok::FLAGS,
        }
    }

    /// What a host keeps for it before it starts a worker of it
    /// (`Start::kept`). For Claude Code, nothing but the host user's skills
    /// and rules.
    pub const fn kept(self) -> Kept {
        match self {
            Harness::ClaudeCode => claude_code::KEPT,
            Harness::Pi => pi::KEPT,
            Harness::Codex => codex::KEPT,
            Harness::Dsh => dsh::KEPT,
            Harness::Kimi => kimi::KEPT,
            Harness::Grok => grok::KEPT,
        }
    }

    /// Native rule paths under the host user's home which can also be found
    /// by project discovery. These must stay out of a worker whose role
    /// turns host-user rules off, even if its copy is under that home.
    pub fn host_rule_excludes(self, home: &str) -> Vec<String> {
        match self {
            Harness::ClaudeCode => claude_code::host_rule_excludes(home),
            _ => vec![],
        }
    }

    /// The host user's config this harness's kept home is generated from, under
    /// the user's home. None: a worker is given none of that file.
    pub const fn user_config(self) -> Option<&'static str> {
        match self {
            Harness::Kimi => Some(kimi::USER_CONFIG),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Grok => {
                None
            }
        }
    }

    /// Where `kept_system` is written, under the kept home.
    pub const fn kept_system_file(self) -> &'static str {
        match self {
            Harness::Grok => grok::SYSTEM_FILE,
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi => {
                "SYSTEM.md"
            }
        }
    }

    /// The generated configuration this harness's kept home needs, if any.
    /// `host_user` is whether that worker is given the host user's skills and
    /// rules. A harness that reads no user config ignores `user`.
    pub fn kept_configuration(
        self,
        user: Option<&str>,
        role: Option<&str>,
        guard: Option<&str>,
        host_user: bool,
    ) -> Result<Option<String>, String> {
        match self {
            Harness::Kimi => kimi::configuration(user, role, guard).map(Some),
            Harness::Grok => grok::configuration(role, guard, host_user).map(Some),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh => Ok(None),
        }
    }

    /// The generated system prompt this harness's kept home needs, if any.
    pub fn kept_system(self, told: &crate::role::Told) -> Option<String> {
        match self {
            Harness::Kimi => kimi::system(told),
            Harness::Grok => grok::system(told),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh => None,
        }
    }

    /// Where a role's limits for it are written, in the role's directory, in
    /// its own form (docs/adr/0013).
    pub const fn limits(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::LIMITS,
            Harness::Pi => pi::LIMITS,
            Harness::Codex => codex::LIMITS,
            Harness::Dsh => dsh::LIMITS,
            Harness::Kimi => kimi::LIMITS,
            Harness::Grok => grok::LIMITS,
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
            Harness::Dsh => Ok(matches!(held, Held::File(_))),
            Harness::Kimi => kimi::limited(held),
            Harness::Grok => grok::limited(held),
        }
    }

    /// Reads the role's limits file in this harness's own format.
    pub fn limited_file(self, text: &str) -> Result<bool, String> {
        match self {
            Harness::Kimi => kimi::limited_file(text),
            Harness::Grok => grok::limited_file(text),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh => {
                let mut json = serde_json::Deserializer::from_str(text);
                self.limited(Held::File(&mut json)).map_err(|error| error.to_string())
            }
        }
    }

    /// Where this harness reads the role's rules, when it cannot read the
    /// file every harness shares and the fleet's role copy needs a link.
    pub const fn rules_link(self) -> Option<&'static str> {
        match self {
            Harness::Kimi => Some(kimi::RULES_LINK),
            _ => None,
        }
    }

    /// The files it would load, from a role's directory, in place of the
    /// rules every harness works under (`team::RULES`). A role holds none.
    pub const fn before_the_rules(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode | Harness::Dsh | Harness::Grok => &[],
            Harness::Pi => &pi::BEFORE_THE_RULES,
            Harness::Codex => &codex::BEFORE_THE_RULES,
            Harness::Kimi => &kimi::BEFORE_THE_RULES,
        }
    }

    /// The files it is told the repo's own rules from, on the line that
    /// starts it — the first of them the worktree holds — and the team rules
    /// with them (`Start::told`): it loads rules from no directory but the
    /// one it starts in. Empty: it loads both itself, and is told nothing.
    pub const fn told(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh => &[],
            Harness::Codex => &codex::REPO_RULES,
            Harness::Kimi => &kimi::REPO_RULES,
            Harness::Grok => &grok::REPO_RULES,
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
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => &[],
            Harness::Codex => &codex::SKILLS,
        }
    }

    /// The directories of the host user's home it finds skills in beside a
    /// role's, and shows both of two skills of one name from: the fleet
    /// turns off there each skill a role's of the same name stands for
    /// (`Start::shadowed`). Empty: a role's skill wins by itself.
    pub const fn shadowed_in(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => &[],
            Harness::Codex => &codex::SHADOWED_IN,
        }
    }

    /// The shell line whose answer says which models it knows on a host,
    /// asked of the worker launch command `command` with what the host keeps
    /// for it in `kept` (`models` reads it): the models its own login runs,
    /// as it says them. DSH's goes through its headless profile instead
    /// (`dsh::models_question`).
    pub fn models_question(self, command: &str, kept: &str) -> String {
        match self {
            Harness::ClaudeCode => claude_code::models_question(command),
            Harness::Pi => pi::models_question(command, kept),
            Harness::Codex => codex::models_question(command, kept),
            Harness::Dsh => dsh::models_question(kept),
            Harness::Kimi => kimi::models_question(command, kept),
            Harness::Grok => grok::models_question(command, kept),
        }
    }

    /// The shell line that sends `model` the `prompt` through the worker
    /// launch command `command`, with what the host keeps for it in `kept`,
    /// and prints the model's answer: with no model, the one the command
    /// starts on. DSH's goes through its headless profile instead
    /// (`dsh::prompt_question`).
    pub fn prompt_question(
        self,
        command: &str,
        kept: &str,
        model: Option<&str>,
        prompt: &str,
    ) -> String {
        match self {
            Harness::ClaudeCode => claude_code::prompt_question(command, model, prompt),
            Harness::Pi => pi::prompt_question(command, kept, model, prompt),
            Harness::Codex => codex::prompt_question(command, kept, model, prompt),
            Harness::Dsh => dsh::prompt_question(kept, model, prompt),
            Harness::Kimi => kimi::prompt_question(command, kept, model, prompt),
            Harness::Grok => grok::prompt_question(command, kept, model, prompt),
        }
    }

    /// The shell line that probes `model` with the one-line `prompt`, as
    /// `prompt_question` sends it, asked to answer in a form that says which
    /// model answered (`answered_as` reads it).
    pub fn probe_question(
        self,
        command: &str,
        kept: &str,
        model: Option<&str>,
        prompt: &str,
    ) -> String {
        match self {
            Harness::ClaudeCode => claude_code::probe_question(command, model, prompt),
            Harness::Pi => pi::probe_question(command, kept, model, prompt),
            Harness::Codex => codex::probe_question(command, kept, model, prompt),
            Harness::Dsh => dsh::prompt_question(kept, model, prompt),
            Harness::Kimi => kimi::probe_question(command, kept, model, prompt),
            Harness::Grok => grok::probe_question(command, kept, model, prompt),
        }
    }

    /// Which model answered a probe, given what the worker's shell printed
    /// for `probe_question`: its harness's own id for it.
    pub fn answered_as(self, answer: &str) -> AnsweredAs {
        let said = match self {
            Harness::ClaudeCode => claude_code::answered_as(answer),
            Harness::Pi => pi::answered_as(answer),
            Harness::Codex => return codex::answered_as(answer),
            Harness::Dsh => dsh::answered_as(answer),
            Harness::Kimi => kimi::answered_as(answer),
            Harness::Grok => grok::answered_as(answer),
        };
        said.map_or(AnsweredAs::Unsaid, AnsweredAs::Said)
    }

    /// Which model answered, given the session file `AnsweredAs::InSession`
    /// names. `None`: it says none.
    pub fn answered_in_session(self, session: &str) -> Option<String> {
        match self {
            Harness::Codex => codex::answered_in_session(session),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The display name of each model it lists in `answer`, what it printed
    /// for `models_question`, by the name its `models` gives it: a name a
    /// human knows it by too.
    pub fn shown(self, answer: &str) -> Vec<(String, String)> {
        match self {
            Harness::ClaudeCode => claude_code::shown(answer),
            Harness::Dsh => dsh::shown(answer),
            Harness::Kimi => kimi::shown(answer),
            Harness::Grok => grok::shown(answer),
            Harness::Pi | Harness::Codex => vec![],
        }
    }

    /// A refusal the model listing reports, where the harness says why.
    pub fn listing_failure(self, answer: &str) -> Option<String> {
        match self {
            Harness::Dsh => dsh::listing_failure(answer),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The worker launch command `command` of an unbound repo, started on
    /// `model`: its model flag added after it. A harness that takes a model
    /// on no flag of its command (DSH) is started as written.
    pub fn with_model(self, command: &str, model: &str) -> String {
        let flag = match self {
            Harness::ClaudeCode => claude_code::MODEL,
            Harness::Pi => pi::MODEL,
            Harness::Codex => codex::MODEL,
            Harness::Kimi => kimi::MODEL,
            Harness::Grok => grok::MODEL,
            Harness::Dsh => return command.to_string(),
        };
        Line::new().launch(command).fixed(" ").fixed(flag).fixed(" ").word(model).done()
    }

    /// The models it knows, given what the worker's shell printed for
    /// `models_question`. `None`: it said none.
    pub fn models(self, answer: &str) -> Option<Vec<String>> {
        match self {
            Harness::ClaudeCode => claude_code::models(answer),
            Harness::Pi => pi::models(answer),
            Harness::Codex => codex::models(answer),
            Harness::Dsh => dsh::models(answer),
            Harness::Kimi => kimi::models(answer),
            Harness::Grok => grok::models(answer),
        }
    }

    /// Whether `model`, its own name for one as a stage writes it, is among
    /// `listed`, the models it said it knows (`models`).
    pub fn knows(self, listed: &[String], model: &str) -> bool {
        match self {
            Harness::Pi => pi::knows(listed, model),
            // Each of the rest takes any name at all and fails at its first
            // turn on one it has no model for.
            Harness::ClaudeCode | Harness::Codex | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                whole(listed, model)
            }
        }
    }

    /// Whether `model` is the whole name of one of `listed`: nothing another
    /// harness's name for a model would be taken for.
    pub fn names(self, listed: &[String], model: &str) -> bool {
        match self {
            Harness::Pi => pi::names(listed, model),
            // Each of the rest knows a model by its whole name and no other way.
            Harness::ClaudeCode | Harness::Codex | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                whole(listed, model)
            }
        }
    }

    /// The launch configuration that trusts a worker's worktree. Every
    /// harness names one, and the worker-launch help names this same marker.
    pub const fn worktree_trust(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::TRUST,
            Harness::Pi => pi::TRUST,
            Harness::Codex => codex::TRUST,
            Harness::Dsh => dsh::TRUST,
            Harness::Kimi => kimi::TRUST,
            Harness::Grok => grok::TRUST,
        }
    }

    /// Starts a worker in its worktree, outside a role, already trusting it
    /// in this harness's own way. Codex also gets its own user directory.
    pub fn in_worktree(self, command: &str, worktree: &str, kept: &str) -> String {
        match self {
            Harness::ClaudeCode => claude_code::in_worktree(command),
            Harness::Pi => pi::in_worktree(command),
            Harness::Codex => codex::in_worktree(command, worktree, kept),
            Harness::Dsh => dsh::in_worktree(command),
            Harness::Kimi => kimi::in_worktree(command),
            Harness::Grok => grok::in_worktree(command),
        }
    }

    /// Starts an interactive watch with the worker's trust, login and model.
    /// The brief is delivered at its input after startup, as a worker's is.
    pub fn watch(self, command: &str, worktree: &str, kept: &str, model: Option<&str>) -> String {
        match self {
            Harness::ClaudeCode => claude_code::watch(command, model),
            Harness::Pi => pi::watch(command, kept, model),
            Harness::Codex => codex::watch(command, worktree, kept, model),
            Harness::Dsh => dsh::watch(command, worktree, kept, model),
            Harness::Kimi => kimi::watch(command, kept, model),
            Harness::Grok => grok::watch(command, model),
        }
    }

    /// The line a terminal opened on the issue's worktree runs to start a
    /// worker of this harness in its role with the worker launch command
    /// `command`: change to the role's copy, put the stage's facts and this
    /// harness in the environment, and start the command with what this
    /// harness does not load from there: the worktree and its rules, the
    /// user's configuration left out, the guard and the report to the worker
    /// backend, the host user's skills and rules where the worker is given
    /// them, and the stage's model — each in this harness's way.
    pub fn in_role(self, command: &str, start: &Start) -> String {
        let mut line = Line::new().fixed("cd ").word(start.role_dir).fixed(" && export");
        for variable in self.environment() {
            line = line.fixed(" ").fixed(variable).fixed("=1");
        }
        let mut line = match self {
            Harness::ClaudeCode => line,
            Harness::Pi => pi::environment(line, start),
            Harness::Codex => codex::environment(line, start),
            Harness::Dsh => dsh::environment(line, start),
            Harness::Kimi => kimi::environment(line, start),
            Harness::Grok => grok::environment(line, start),
        };
        for (name, value) in start.facts.env() {
            line = line.fixed(" ").fixed(name).fixed("=").word(&value);
        }
        line = line.fixed(" ").fixed(HARNESS_ENV).fixed("=").word(self.key());
        if let Some(model) = start.model {
            line = line.fixed(" ").fixed(MODEL_ENV).fixed("=").word(model);
        }
        let line = line.fixed(" && ").launch(command);
        match self {
            Harness::ClaudeCode => claude_code::added(line, start),
            Harness::Pi => pi::added(line, start),
            Harness::Codex => codex::added(line, start),
            Harness::Dsh => dsh::added(line, start),
            Harness::Kimi => kimi::added(line, start),
            Harness::Grok => grok::added(line, start),
        }
        .done()
    }

    /// The variables this harness is switched by, each set in the
    /// environment of a worker in its role. What it reads a value from is
    /// its module's to add.
    const fn environment(self) -> &'static [&'static str] {
        match self {
            Harness::ClaudeCode => &claude_code::ENVIRONMENT,
            Harness::Kimi => &kimi::ENVIRONMENT,
            Harness::Grok => &grok::ENVIRONMENT,
            Harness::Pi | Harness::Codex | Harness::Dsh => &[],
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
            Harness::Dsh => dsh::asked_its_flags(line),
            Harness::Kimi => kimi::asked_its_flags(line),
            Harness::Grok => grok::asked_its_flags(line),
        }
        .done()
    }

    /// Whether the worker launch command `command` takes what the fleet
    /// adds to it for this harness, given what the worker's shell printed for
    /// `flags_question` (`None`: it failed, or printed nothing). An error
    /// names the reason: used anyway, every worker would start outside its
    /// role, or not at all.
    pub fn takes(self, command: &str, answer: Option<&str>) -> Result<(), String> {
        let refused = |why: String| {
            Err(format!(
                "the worker launch command {command:?} cannot take the fleet's launch flags: \
                 {why}. A worker launch command must end in {} taking flags",
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

    /// Whether `screen`, the end of a stopped worker's terminal, shows work
    /// the worker started itself still running in the background, as this
    /// harness shows it. A harness that shows none is never waited on.
    pub fn shows_background(self, screen: &[String]) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::shows_background(screen),
            Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi | Harness::Grok => false,
        }
    }

    /// Whether any harness shows background work on `screen` (as
    /// `shows_background`): the screen does not say which harness drew it.
    pub fn any_shows_background(screen: &[String]) -> bool {
        Harness::ALL.into_iter().any(|harness| harness.shows_background(screen))
    }

    /// The tool call this harness describes in `described`, as the guard
    /// judges one. An error is a description of no tool call.
    pub fn call<'de, D: Deserializer<'de>>(self, described: D) -> Result<Call, D::Error> {
        match self {
            Harness::ClaudeCode => claude_code::call(described),
            Harness::Pi => pi::call(described),
            Harness::Codex => codex::call(described),
            Harness::Dsh => dsh::call(described),
            Harness::Kimi => kimi::call(described),
            Harness::Grok => grok::call(described),
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
            Harness::Dsh => dsh::REFUSED,
            Harness::Kimi => kimi::REFUSED,
            Harness::Grok => grok::REFUSED,
        }
    }
}

/// Whether `model`, as a stage writes it, is one of `listed`, the models a
/// harness lists on a host: its whole name, as written.
fn whole(listed: &[String], model: &str) -> bool {
    listed.iter().any(|name| name == model.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_is_probed_through_the_command_with_one_line_and_nothing_else() {
        let asked =
            |harness: Harness| harness.prompt_question("agent -x", "/k", Some("m 1"), "Say OK.");
        assert_eq!(asked(Harness::ClaudeCode), "agent -x --model 'm 1' --print 'Say OK.'");
        assert_eq!(
            asked(Harness::Pi),
            "export PI_CODING_AGENT_DIR='/k' && agent -x --model 'm 1' --print 'Say OK.'"
        );
        assert_eq!(
            asked(Harness::Codex),
            "export CODEX_HOME='/k' && agent -x exec --skip-git-repo-check --ephemeral --model \
             'm 1' 'Say OK.'"
        );
        assert_eq!(
            asked(Harness::Dsh),
            "AFKCONVOY_DSH_MODEL='m 1' dsh headless --patch '/k/probe.patch.yml' 'Say OK.'"
        );
        assert_eq!(
            Harness::Dsh.prompt_question("dsh-tui", "/k", None, "Say OK."),
            "dsh headless --patch '/k/probe.patch.yml' 'Say OK.'"
        );
    }

    #[test]
    fn a_probe_asks_for_an_answer_that_says_which_model_answered() {
        let asked =
            |harness: Harness| harness.probe_question("agent -x", "/k", Some("m 1"), "Say OK.");
        assert_eq!(
            asked(Harness::ClaudeCode),
            "agent -x --model 'm 1' --print 'Say OK.' --output-format json"
        );
        assert_eq!(
            asked(Harness::Pi),
            "export PI_CODING_AGENT_DIR='/k' && agent -x --model 'm 1' --mode json --print 'Say OK.'"
        );
        assert_eq!(
            asked(Harness::Codex),
            "export CODEX_HOME='/k' && agent -x exec --skip-git-repo-check --json --model 'm 1' \
             'Say OK.'"
        );
        assert_eq!(
            asked(Harness::Dsh),
            Harness::Dsh.prompt_question("x", "/k", Some("m 1"), "Say OK.")
        );
    }

    #[test]
    fn each_harness_says_which_model_answered_its_probe_in_its_own_way() {
        // As measured on a host, each broken by a terminal where it may be.
        let claude = "{\"result\":\"OK\",\"usage\":{\"input_tokens\":2},\"modelUsage\":{\"claude-op\nus-5-5\":{\"outputTokens\":4}}}";
        assert_eq!(
            Harness::ClaudeCode.answered_as(claude),
            AnsweredAs::Said("claude-opus-5-5".into())
        );
        let pi = "{\"type\":\"session\",\"id\":\"x\"}\n{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",\"api\":\"openai-codex-responses\",\"provider\":\"openai-codex\",\"model\":\"gpt-6-sol\",\"usage\":{}}}";
        assert_eq!(Harness::Pi.answered_as(pi), AnsweredAs::Said("openai-codex/gpt-6-sol".into()));
        let codex = "Reading additional input from stdin...\n{\"type\":\"thread.started\",\"thread_id\":\"01a16c81-722f\"}\n{\"type\":\"turn.started\"}";
        assert_eq!(
            Harness::Codex.answered_as(codex),
            AnsweredAs::InSession { under: "sessions", ends: "01a16c81-722f.jsonl".into() }
        );
        let session = "{\"type\":\"session_meta\",\"payload\":{\"id\":\"x\"}}\n{\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"t\",\"cwd\":\"/\",\"model\":\"gpt-6-sol\"}}";
        assert_eq!(Harness::Codex.answered_in_session(session), Some("gpt-6-sol".into()));
        let dsh = "AFKCONVOY_DSH_ANSWERED deepseek-v4-pro\nOK\n";
        assert_eq!(Harness::Dsh.answered_as(dsh), AnsweredAs::Said("deepseek-v4-pro".into()));
        for harness in Harness::ALL {
            assert_eq!(harness.answered_as("OK\n"), AnsweredAs::Unsaid, "{}", harness.key());
        }
    }

    #[test]
    fn every_harness_adds_its_own_worktree_trust_outside_a_role() {
        for harness in Harness::ALL {
            let line = harness.in_worktree("agent", "/wt", "/kept");
            assert!(line.contains(harness.worktree_trust()), "{}: {line}", harness.key());
            assert!(line.contains("agent"), "{}: {line}", harness.key());
        }
    }

    #[test]
    fn every_watch_is_interactive_trusted_and_given_its_model() {
        for harness in Harness::ALL {
            let line = harness.watch("agent", "/operator dir", "/kept", Some("model one"));
            assert!(line.contains(harness.worktree_trust()), "{}: {line}", harness.key());
            assert!(line.contains("model one"), "{}: {line}", harness.key());

            let headless = ["--print", " exec ", "--prompt", "headless"];
            assert!(!headless.iter().any(|mode| line.contains(mode)), "{}: {line}", harness.key());
        }
    }

    #[test]
    fn an_unbound_command_is_given_its_model_on_its_harness_s_flag() {
        assert_eq!(
            Harness::Codex.with_model("codex --yolo", "gpt-x"),
            "codex --yolo --model 'gpt-x'"
        );
        assert_eq!(Harness::Dsh.with_model("dsh-tui", "ds"), "dsh-tui");
    }
}
