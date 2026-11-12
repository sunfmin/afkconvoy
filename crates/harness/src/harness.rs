//! A harness (docs/adr/0013): the program a worker is a session of. What the
//! fleet adds to a worker launch command to start a worker in its role
//! (docs/adr/0009), the question that asks a command whether it takes that,
//! the report to the worker backend, and how a tool call reaches the guard
//! and is answered are each one harness's own. What is one harness's is in
//! its module here and nowhere else: nothing outside names a flag, a
//! variable, a hook event or a path of it, nor where it keeps its session,
//! nor which of its lines start or end a turn (docs/adr/0044).

use flux_rs::attrs::*;
use serde::{Deserialize, Deserializer};

use crate::line::Line;
use crate::start::Start;
use crate::turn::Turn;

mod claude_code;
mod codex;
mod dsh;
mod grok;
mod kimi;
mod pi;
mod session;

pub use session::{epoch_number, parse_iso};

/// One tool call a session is about to make, as far as a hook judges it. How
/// a harness describes one is the harness's own (`Harness::call`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// It runs this shell line: the only kind a hook judges.
    Shell(String),
    /// It runs none.
    Other,
}

impl Call {
    /// The call that runs the shell line `line`.
    pub fn shell(line: &str) -> Call {
        Call::Shell(line.to_string())
    }
}

/// A program besides a harness itself that a session of it cannot start
/// without (`Harness::needs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Needed {
    /// DeepSeek Harness's own program, which dsh-tui runs.
    Dsh,
}

/// The variable a session's environment names its harness in (`Harness::key`):
/// what a hook reads to know which harness's call it is asked about
/// (`Harness::guarded`).
pub const HARNESS_ENV: &str = "AFKCONVOY_HARNESS";

/// The word of a launch command whose resolving decides whether it can run
/// at all.
pub fn first_word(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

/// A file or a directory of the host user's that a directory the host keeps
/// for a harness links to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// Where it is, under the home of the host's user.
    pub home: &'static str,
    /// Where the link to it is, under the directory.
    pub kept: &'static str,
}

/// Where a harness reads its user's hooks, and how one runs before a tool
/// call there: where the agent setup's guards are wired (sunfmin/control#529).
/// The file holds a JSON object whose `table` key holds, by event, groups of
/// `{"matcher": ..., "hooks": [{"type": "command", "command": ..., "timeout": ...}]}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserHooks {
    /// The settings file, under the home of the host's user.
    pub settings: &'static str,
    /// The key of that file that holds its hooks, by event.
    pub table: &'static str,
    /// The event before each tool call.
    pub event: &'static str,
    /// The matcher that picks the calls that run a shell command.
    pub shell: &'static str,
    /// The matcher that picks the calls that edit a file.
    pub edits: &'static str,
}

/// A harness that keeps each session apart, and reads the user's own model
/// for a start the issue does not name. Codex is the one (docs/adr/0013).
/// The directory, the file and the note are named here; what they mean is
/// that harness's module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Session {
    /// The prefix of one fresh directory under the worktree's temporary
    /// directory. The host keeps the session's directory under it.
    pub fresh: &'static str,
    /// The note in the worktree that names the directory kept for the session.
    pub note: &'static str,
    /// The user's config, under their home, that an automatic start reads.
    pub config: &'static str,
    /// The note in the worktree that records what the start resolved.
    pub record: &'static str,
}

/// A model and an effort a session saved into its own config after it started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub model: Option<String>,
    pub model_source: Option<String>,
    pub effort: Option<String>,
    pub effort_source: Option<String>,
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

/// Where one harness keeps the session of one worktree. The reader opens
/// the files. A missing directory is no turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlace {
    /// The harness directory under a user's home. A directory the fleet
    /// keeps for the harness, and a home [`Harness::noted_home`] names, are
    /// already this directory.
    pub under_home: &'static str,
    /// How the files of this worktree are laid out under that directory.
    pub layout: SessionLayout,
    /// Another layout under the same directory whose sessions are this
    /// worktree's only when they record its turn: a bound Claude Code
    /// worker's words are filed under its role copy's project, not the
    /// worktree's.
    pub also: Option<SessionLayout>,
}

/// How the files of one worktree are laid out under the harness directory.
/// The reader opens them. It does not interpret them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionLayout {
    /// Each file of `extension` directly in `dir` is one session.
    Files { dir: String, extension: &'static str },
    /// Each file of `extension` anywhere under `dir` is one session.
    Tree { dir: String, extension: &'static str },
    /// Each `dir/*/<name>` is one session. `compressed` means the bytes are zstd.
    /// `usage` names the file beside it whose text holds the session's usage,
    /// where the session keeps it apart.
    Named { dir: String, name: &'static str, compressed: bool, usage: Option<&'static str> },
    /// `file` names the session files ([`Harness::indexed_sessions`]).
    Index { file: String },
}

/// One session file an index names ([`Harness::indexed_sessions`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    /// The session file: absolute, or relative to the harness directory.
    pub file: String,
    /// The index line's directory is the worktree read for.
    pub here: bool,
}

/// One file of a session a stand-in writes under the harness directory.
/// The body is the text [`Harness::recorded_turn`] reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFile {
    /// Path under the harness directory.
    pub path: String,
    /// The session text, before `compressed` storage.
    pub body: String,
    /// The body is stored as zstd.
    pub compressed: bool,
    /// Before writing, delete every file of this name under the harness directory.
    pub replaces: Option<String>,
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

/// One keystroke typed at a harness, carrying no line of the operator's.
/// `text` is what `terminal send --text` carries; `None` is the key alone.
/// `enter` is Enter, alone or after that text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keystroke {
    pub text: Option<&'static str>,
    pub enter: bool,
}

/// How a session nobody types at is handed its brief
/// (`Harness::brief_at_start`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BriefAtStart {
    /// The start line that hands the session its brief as its first prompt:
    /// run, the session works on the brief with nobody typing at it.
    Started(String),
    /// This harness is handed its brief only by being typed at: the start
    /// line is run as it is, and this line, [`Harness::brief_line`], is typed
    /// once the session is idle.
    Typed(String),
}

/// Where a harness that keeps a goal for its session keeps it: a SQLite
/// file under the directory kept for the session, and the query whose one
/// answer is the goal's status (`Harness::goal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalStore {
    pub file: &'static str,
    pub query: &'static str,
}

/// A worker's goal, where its harness keeps one (sunfmin/control#544).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Goal {
    /// The session has none: never set, or cleared.
    Unset,
    /// It is pursued: the harness continues it after each turn its model
    /// ends.
    Active,
    Paused,
    /// Its model found the same blocker on three goal turns in a row.
    Blocked,
    /// The account's usage limit stopped it.
    UsageLimited,
    /// Its token budget stopped it.
    BudgetLimited,
    Complete,
}

impl Goal {
    /// A goal that stopped its worker short of its brief: the harness
    /// continues it no more, and the worker is a stopped one.
    pub fn stopped(self) -> bool {
        matches!(self, Goal::Blocked | Goal::BudgetLimited)
    }

    /// The word the store says it by.
    pub fn key(self) -> &'static str {
        match self {
            Goal::Unset => "unset",
            Goal::Active => "active",
            Goal::Paused => "paused",
            Goal::Blocked => "blocked",
            Goal::UsageLimited => "usage_limited",
            Goal::BudgetLimited => "budget_limited",
            Goal::Complete => "complete",
        }
    }
}

/// What a session record says of its goal (sunfmin/control#548): the
/// session's thread, the objective it was set with, if it was, and when each
/// turn its goal began started, the turn the setting began among them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalSession {
    pub thread: Option<String>,
    pub objective: Option<String>,
    pub turns: Vec<u64>,
}

/// One session's row of a goal store: its status and the tokens it used
/// against its budget. A cleared goal has no row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalRow {
    pub status: Goal,
    pub tokens_used: u64,
    pub token_budget: Option<u64>,
}

/// How a harness is asked whether it is logged in, and what logs it in
/// (`Harness::login`): what a start that finds it logged out names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Login {
    /// The shell line whose answer says whether it is logged in
    /// (`Harness::logged_in` reads it).
    pub question: &'static str,
    /// What it is said to be when it is not.
    pub logged_out: &'static str,
    /// The line that logs it in.
    pub line: &'static str,
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

/// A harness is read as that same key. A word that is no harness is refused.
impl<'de> Deserialize<'de> for Harness {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let key = String::deserialize(deserializer)?;
        Harness::read(&key).ok_or_else(|| serde::de::Error::custom(Harness::unknown(&key)))
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

    /// The key that hands `line` to the model now, when `screen` shows that
    /// line waiting in this harness's input queue. `None` when this harness
    /// takes a typed line without another key, or the screen does not show
    /// `line` queued. How a harness recognises its queue, and which key it
    /// asks for, is that harness's.
    pub fn send_queued(self, screen: &[String], line: &str) -> Option<Keystroke> {
        match self {
            Harness::Grok => grok::send_queued(screen, line),
            _ => None,
        }
    }

    /// The harness's account-quota words on its screen, or in a runtime
    /// message it prints. Whitespace may wrap. The same words on another
    /// harness's screen are not this quota: a harness is asked only for its
    /// own. What it returns is the line that showed them.
    pub fn account_quota(self, output: &[String]) -> Option<String> {
        match self {
            Harness::Codex => codex::account_quota(output),
            Harness::Pi => pi::account_quota(output),
            Harness::Kimi => kimi::account_quota(output),
            _ => None,
        }
    }

    /// The usage windows of its account that one record of this harness
    /// says, as the engine reports them: each window's own name, how much of
    /// it is used and when it resets. Which records say them is the
    /// harness's: Claude Code's status line input and its stream's
    /// `rate_limit_event`, a Codex session's `token_count` line, Kimi Code's
    /// answer to its `/usage`. A record that says none, and a harness whose
    /// engine reports none, give none: the caller predicts them itself.
    pub fn windows(self, record: &serde_json::Value) -> Vec<crate::window::Window> {
        match self {
            Harness::ClaudeCode => claude_code::windows(record),
            Harness::Codex => codex::windows(record),
            Harness::Kimi => kimi::windows(record),
            Harness::Pi | Harness::Dsh | Harness::Grok => Vec::new(),
        }
    }

    /// The limit one record of this harness says it met, read from the
    /// record's structured fields, with when it lifts where the record says:
    /// beside the screen words [`Harness::account_quota`] reads. A harness
    /// whose records hold no structured limit gives none.
    pub fn limit_event(self, record: &serde_json::Value) -> Option<crate::window::Limit> {
        match self {
            Harness::ClaudeCode => claude_code::limit_event(record),
            Harness::Codex => codex::limit_event(record),
            Harness::Kimi | Harness::Pi | Harness::Dsh | Harness::Grok => None,
        }
    }

    /// Where a host installs a status line command, which this harness hands
    /// what [`Harness::windows`] reads; `None` for a harness with no status
    /// line command.
    pub const fn status_line(self) -> Option<crate::window::StatusLine> {
        match self {
            Harness::ClaudeCode => Some(claude_code::STATUS_LINE),
            Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi | Harness::Grok => None,
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

    /// Whether `launch`, a worker launch command of it, runs it by its own
    /// name: a harness is looked for only as that command's first word —
    /// what a wrapper runs and logs in to is its own business.
    pub fn runs_itself(self, launch: &str) -> bool {
        first_word(launch) == Some(self.program())
    }

    /// The line that installs it.
    pub const fn install(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::INSTALL,
            Harness::Pi => pi::INSTALL,
            Harness::Codex => codex::INSTALL,
            Harness::Dsh => dsh::INSTALL,
            Harness::Kimi => kimi::INSTALL,
            Harness::Grok => grok::INSTALL,
        }
    }

    /// How it is asked whether it is logged in, and logged in; `None`: it
    /// logs in to nothing a start asks about.
    pub const fn login(self) -> Option<Login> {
        match self {
            Harness::ClaudeCode => claude_code::LOGIN,
            Harness::Pi => pi::LOGIN,
            Harness::Codex => codex::LOGIN,
            Harness::Dsh => dsh::LOGIN,
            Harness::Kimi => kimi::LOGIN,
            Harness::Grok => grok::LOGIN,
        }
    }

    /// Whether the answer to its login question (`Login::question`) says it
    /// is logged in: whether that shell line `succeeded`, and what it
    /// `printed`.
    pub fn logged_in(self, succeeded: bool, printed: &str) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::logged_in(succeeded, printed),
            Harness::Pi => pi::logged_in(succeeded, printed),
            Harness::Codex => codex::logged_in(succeeded, printed),
            Harness::Dsh => dsh::logged_in(succeeded, printed),
            Harness::Kimi => kimi::logged_in(succeeded, printed),
            Harness::Grok => grok::logged_in(succeeded, printed),
        }
    }

    /// The program besides itself a worker of it cannot start without, which
    /// a start looks for once it is found ready: dsh-tui runs dsh's own.
    pub const fn needs(self) -> Option<Needed> {
        match self {
            Harness::Dsh => Some(Needed::Dsh),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Whether a session of it outside a role gets a home of its own: one
    /// whose generated hook files a probe or asking launch must not write
    /// into a home a worker or a watch shares.
    pub const fn own_home_outside_role(self) -> bool {
        match self {
            Harness::Kimi => kimi::OWN_HOME_OUTSIDE_ROLE,
            Harness::Grok => grok::OWN_HOME_OUTSIDE_ROLE,
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh => false,
        }
    }

    /// The hosts its worker's login shell reaches without a proxy unless the
    /// host says otherwise (the host's proxy settings); `None`: none.
    pub const fn no_proxy(self) -> Option<&'static str> {
        match self {
            Harness::Dsh => Some(dsh::NO_PROXY),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Where, under the directory a host keeps for it (`kept`), the package
    /// of its installed program is linked, where one is; `None`: nowhere.
    pub const fn kept_package(self) -> Option<&'static str> {
        match self {
            Harness::Dsh => Some(dsh::KEPT_PACKAGE),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Where it reads its user's global instructions, under the user's home:
    /// where the agent setup's instructions are linked (sunfmin/control#528).
    pub const fn instructions(self) -> &'static str {
        match self {
            Harness::ClaudeCode => claude_code::INSTRUCTIONS,
            Harness::Pi => pi::INSTRUCTIONS,
            Harness::Codex => codex::INSTRUCTIONS,
            Harness::Dsh => dsh::INSTRUCTIONS,
            Harness::Kimi => kimi::INSTRUCTIONS,
            Harness::Grok => grok::INSTRUCTIONS,
        }
    }

    /// The agent `npx skills` installs the agent setup's skills for (`-a`);
    /// `None` where it names none, and the harness reads the shared copies.
    pub const fn skills_agent(self) -> Option<&'static str> {
        match self {
            Harness::ClaudeCode => claude_code::SKILLS_AGENT,
            Harness::Pi => pi::SKILLS_AGENT,
            Harness::Codex => codex::SKILLS_AGENT,
            Harness::Dsh => dsh::SKILLS_AGENT,
            Harness::Kimi => kimi::SKILLS_AGENT,
            Harness::Grok => grok::SKILLS_AGENT,
        }
    }

    /// Where it reads its user's hooks, and how a guard runs before a tool
    /// call there; `None` where the host kit knows no such file of it, and
    /// the agent setup's guards are given it nowhere.
    pub const fn user_hooks(self) -> Option<UserHooks> {
        match self {
            Harness::ClaudeCode => Some(claude_code::USER_HOOKS),
            Harness::Codex => Some(codex::USER_HOOKS),
            Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => None,
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
    /// `role_skills` is the role's `.claude/skills` when this worker has one,
    /// and `role_skill_names` the directories in it. Grok names that directory
    /// and ignores a host skill of the same name (ADR-0043). Every other
    /// harness ignores both.
    pub fn kept_configuration(
        self,
        user: Option<&str>,
        role: Option<&str>,
        guard: Option<&str>,
        host_user: bool,
        role_skills: Option<&str>,
        role_skill_names: &[String],
    ) -> Result<Option<String>, String> {
        match self {
            Harness::Kimi => kimi::configuration(user, role, guard).map(Some),
            Harness::Grok => {
                grok::configuration(role, guard, host_user, role_skills, role_skill_names).map(Some)
            }
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh => Ok(None),
        }
    }

    /// The generated system prompt this harness's kept home needs, if any.
    pub fn kept_system(self, told: &crate::start::Told) -> Option<String> {
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

    /// The role file a worker is told, from the role's directory, beside the
    /// repo's and the team's. A harness that starts in the worktree names the
    /// file, because that file is how the role's rules reach the worker
    /// (ADR-0043). `None`: it starts in the role and loads the file itself,
    /// and is told no role file.
    pub const fn role_file(self) -> Option<&'static str> {
        match self {
            Harness::Grok => Some(grok::ROLE_FILE),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi => {
                None
            }
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
        let Some(flag) = self.model_flag() else { return command.to_string() };
        Line::new().launch(command).fixed(" ").fixed(flag).fixed(" ").word(model).done()
    }

    /// The flag its worker launch command takes a model by. `None`: DSH,
    /// whose profile names the model.
    fn model_flag(self) -> Option<&'static str> {
        match self {
            Harness::ClaudeCode => Some(claude_code::MODEL),
            Harness::Pi => Some(pi::MODEL),
            Harness::Codex => Some(codex::MODEL),
            Harness::Kimi => Some(kimi::MODEL),
            Harness::Grok => Some(grok::MODEL),
            Harness::Dsh => None,
        }
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

    /// Whether `worktree_trust` is flags the worker launch command must take:
    /// outside a role, a launch checks the command takes `flags` only then.
    pub const fn trust_is_flags(self) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::TRUST_IS_FLAGS,
            Harness::Pi => pi::TRUST_IS_FLAGS,
            Harness::Codex => codex::TRUST_IS_FLAGS,
            Harness::Dsh => dsh::TRUST_IS_FLAGS,
            Harness::Kimi => kimi::TRUST_IS_FLAGS,
            Harness::Grok => grok::TRUST_IS_FLAGS,
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

    /// Adds the asking Terminal guard outside a role. Hook spelling and loading
    /// belong to the harness; the decision belongs to the shared guard. Kimi
    /// and Grok's `kept` homes already contain the supplied guard's hooks.
    pub fn guarded(self, command: &str, worktree: &str, kept: &str, guard: &str) -> String {
        let command = match self {
            Harness::ClaudeCode => claude_code::guarded(command, guard),
            Harness::Pi => pi::guarded(command, worktree, kept, guard),
            Harness::Codex => codex::guarded(command, guard),
            Harness::Dsh => dsh::guarded(command, guard),
            Harness::Kimi => kimi::guarded(command, kept),
            Harness::Grok => grok::guarded(command, kept),
        };
        Line::new()
            .fixed("export ")
            .fixed(HARNESS_ENV)
            .fixed("=")
            .word(self.key())
            .fixed(" && ")
            .launch(&command)
            .done()
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

    /// How this harness keeps a session of its own. `None`: the worker launch
    /// command, the fleet's model and the directory the host keeps are the
    /// whole start. Codex names a fresh directory, the user's model unless
    /// the issue names one, and its watch line (docs/adr/0013).
    pub const fn session(self) -> Option<Session> {
        match self {
            Harness::Codex => Some(codex::SESSION),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The model of an automatic start. `explicit` is the issue's model and
    /// wins. Otherwise the user's, read from `text`, the config file `file`
    /// (`Session::config`), and it must be one of `listed`, the host kit's
    /// models for this harness. `None`: this harness has no such preference,
    /// and the fleet's selection stands. An error is a preference the start
    /// cannot use.
    pub fn automatic_model(
        self,
        explicit: Option<&str>,
        text: &str,
        file: &str,
        listed: &[String],
    ) -> Option<Result<(Option<String>, &'static str), String>> {
        match self {
            Harness::Codex => Some(codex::automatic_model(explicit, text, file, listed)),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The effort of a start, from the same config `automatic_model` reads.
    /// `None`: this harness names none.
    pub fn effort(
        self,
        text: &str,
        file: &str,
    ) -> Option<Result<(Option<String>, String), String>> {
        match self {
            Harness::Codex => Some(codex::effort(text, file)),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The service tier of a worker start, from that same config. `None` on
    /// the outer option means this harness names none; on the inner one, that
    /// Codex chooses its own default.
    pub fn service_tier(self, text: &str, file: &str) -> Option<Result<Option<String>, String>> {
        match self {
            Harness::Codex => Some(codex::service_tier(text, file)),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Why `model`, chosen from the user's config, cannot start this issue.
    /// `None`: this harness does not refuse a model that way.
    pub fn outside_issue(self, model: &str) -> Option<String> {
        match self {
            Harness::Codex => Some(codex::outside_issue(model)),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Why `model` is outside the role `role`'s range. `None`: this harness
    /// does not refuse a model that way.
    pub fn outside_role(self, model: &str, role: &str) -> Option<String> {
        match self {
            Harness::Codex => Some(codex::outside_role(model, role)),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// What the session at `file` saved of its model and effort, read from
    /// `text`. `None`: this harness keeps none, or the text is not its config.
    pub fn saved(self, file: &str, text: &str) -> Option<Saved> {
        match self {
            Harness::Codex => codex::saved(file, text),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// `command` naming `model`, unless it already does: the model is said
    /// once, in this harness's way, whether or not it keeps a session
    /// (`selection::Selection::start_line`).
    pub fn ensure_model(self, command: &str, model: Option<&str>) -> String {
        match (self, model, self.model_flag()) {
            (Harness::Codex, _, _) => codex::ensure_model(command, model),
            (_, Some(model), Some(flag)) if !command.contains(flag) => {
                self.with_model(command, model)
            }
            _ => command.to_string(),
        }
    }

    /// `line` with this harness's effort, when `effort` is one it read.
    /// Unchanged when this harness names no effort, or none was read.
    pub fn with_effort(self, line: &str, effort: Option<&str>) -> String {
        match self {
            Harness::Codex => codex::with_effort(line, effort),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                line.to_string()
            }
        }
    }

    /// `line` with this harness's service tier, when one was read. Unchanged
    /// when this harness names none, or none was read.
    pub fn with_service_tier(self, line: &str, service_tier: Option<&str>) -> String {
        match self {
            Harness::Codex => codex::with_service_tier(line, service_tier),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                line.to_string()
            }
        }
    }

    /// The line a watch of this harness starts with, in the directory `kept`.
    /// A harness that keeps a session of its own answers its own line, search
    /// included, and `effort` when it read one. Every other harness answers
    /// `watch`, and `effort` is unused.
    pub fn session_watch(
        self,
        command: &str,
        worktree: &str,
        kept: &str,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> String {
        match self {
            Harness::Codex => codex::session_watch(command, worktree, kept, model, effort),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                self.watch(command, worktree, kept, model)
            }
        }
    }

    /// The line a terminal runs to start a session of this harness with the
    /// launch command `command`, as `start` says: change to the directory
    /// this harness loads a session from, put this harness's own variables
    /// and `start.env` in the environment, and start the command with what
    /// this harness does not load from there: the worktree and its rules, the
    /// user's configuration left out, the hook and the report to the session
    /// backend, the host user's skills and rules where the session is given
    /// them, and the model — each in this harness's way. Every harness but
    /// Grok starts in `start.dir`. Grok's session works only in its cwd, so it
    /// starts in the worktree (ADR-0043).
    pub fn in_role(self, command: &str, start: &Start) -> String {
        self.role_line(command, start).done()
    }

    /// The line `in_role` gives, before it is done: what a resumed session
    /// is started with as well.
    #[spec(fn(Harness, &str, &Start) -> Line[true])]
    fn role_line(self, command: &str, start: &Start) -> Line {
        let directory = match self {
            Harness::Grok => start.worktree,
            _ => start.dir,
        };
        let mut line = Line::new().fixed("cd ").word(directory).fixed(" && export");
        for variable in self.environment() {
            line = line.fixed(" ").fixed(variable).fixed("=1");
        }
        let mut line = match self {
            Harness::ClaudeCode => claude_code::environment(line, start),
            Harness::Pi => pi::environment(line, start),
            Harness::Codex => codex::environment(line, start),
            Harness::Dsh => dsh::environment(line, start),
            Harness::Kimi => kimi::environment(line, start),
            Harness::Grok => grok::environment(line, start),
        };
        for (name, value) in &start.env {
            line = line.fixed(" ").fixed(name).fixed("=").word(value);
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
    }

    /// The line that continues a session a soft stop kept (`stop_answer`),
    /// in its role as `in_role` starts one — the same directory, settings,
    /// hook and model — with `instruction` as its next user message.
    /// `session` is the session's id, which `session_id` reads off its file
    /// (`session_place` says where those are); `None` continues the latest
    /// session of the directory the line changes to. `None` of a harness
    /// that cannot continue a session.
    pub fn resumed(
        self,
        command: &str,
        start: &Start,
        session: Option<&str>,
        instruction: &str,
    ) -> Option<String> {
        let line = self.role_line(command, start);
        let line = match self {
            Harness::ClaudeCode => claude_code::resumed(line, session, instruction),
            Harness::Codex => codex::resumed(line, session, instruction),
            Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => return None,
        };
        Some(line.done())
    }

    /// The id of the session whose file holds `text`: what `resumed`
    /// continues it by. `None` when the text names none, or of a harness
    /// that cannot continue a session.
    pub fn session_id(self, text: &str) -> Option<String> {
        match self {
            Harness::ClaudeCode => claude_code::session_id(text),
            Harness::Codex => codex::session_id(text),
            Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => None,
        }
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
            Harness::Kimi => kimi::shows_background(screen),
            Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Grok => false,
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

    /// Where this harness keeps the session of the worktree `cwd`.
    ///
    /// [`SessionPlace::under_home`] is that directory under a user's home.
    /// A directory the fleet keeps for the harness, and a home [`noted_home`]
    /// names, are already that directory. The reader opens the files
    /// [`SessionPlace::layout`] names. A missing directory is no turn.
    pub fn session_place(self, cwd: &str) -> SessionPlace {
        let (under_home, layout) = match self {
            Harness::ClaudeCode => (claude_code::HOME, claude_code::session_layout(cwd)),
            Harness::Pi => (pi::HOME, pi::session_layout(cwd)),
            Harness::Codex => (codex::HOME, codex::session_layout(cwd)),
            Harness::Dsh => (dsh::HOME, dsh::session_layout(cwd)),
            Harness::Kimi => (kimi::HOME, kimi::session_layout(cwd)),
            Harness::Grok => (grok::HOME, grok::session_layout(cwd)),
        };
        SessionPlace { under_home, layout, also: None }
    }

    /// The turn one session file's `text` records for the worktree `cwd`.
    ///
    /// `None` when the file is another worktree, or it holds no turn of this
    /// send. `not_before` is the start proof's bound: a turn that started
    /// earlier is not this send. A child session does not start or end the
    /// worker's turn.
    /// `brief` is the exact file handed to this worker. A harness that starts
    /// in a role may identify the session by its initial prompt instead of cwd.
    pub fn recorded_turn(
        self,
        cwd: &str,
        text: &str,
        not_before: Option<u64>,
        brief: Option<&str>,
    ) -> Option<Turn> {
        match self {
            Harness::ClaudeCode => claude_code::recorded_turn(cwd, text, not_before, brief),
            Harness::Pi => pi::recorded_turn(cwd, text, not_before),
            Harness::Codex => codex::recorded_turn(cwd, text, not_before, brief),
            Harness::Dsh => dsh::recorded_turn(cwd, text, not_before),
            Harness::Kimi => kimi::recorded_turn(cwd, text, not_before),
            Harness::Grok => grok::recorded_turn(cwd, text, not_before),
        }
    }

    /// The token use one session file's `text` records: the whole session, or
    /// from `window.0` through `window.1` inclusive. `None` is no usage
    /// evidence, not zero (`tokens::fold`). A session that keeps its
    /// usage apart is read from the file its layout names beside it.
    pub fn tokens(self, text: &str, window: Option<(u64, u64)>) -> Option<crate::tokens::Tokens> {
        crate::tokens::fold(text, window, |line, n| self.usage(line, n))
    }

    /// The steps one session file's `text` records, oldest first: each tool
    /// call with what came back of it and whether the harness marked that an
    /// error. A call with no result yet is no step, and a child session's
    /// lines are not the session's own. The text is the session file the
    /// layout names, decompressed where it is stored compressed; Grok's
    /// calls are in the updates its layout names beside its events
    /// (`SessionLayout::Named::usage`).
    pub fn steps(self, text: &str) -> Vec<crate::steps::Step> {
        crate::steps::fold(text, |line| self.said(line))
    }

    /// What one session line says of a tool call or its result.
    fn said(self, line: &serde_json::Value) -> Vec<crate::steps::Said> {
        match self {
            Harness::ClaudeCode => claude_code::said(line),
            Harness::Pi => pi::said(line),
            Harness::Codex => codex::said(line),
            Harness::Kimi => kimi::said(line),
            Harness::Grok => grok::said(line),
            Harness::Dsh => dsh::said(line),
        }
    }

    /// What one session line, the `n`th, says of token use.
    fn usage(self, line: &serde_json::Value, n: usize) -> Option<crate::tokens::Usage> {
        match self {
            Harness::ClaudeCode => claude_code::usage(line, n),
            Harness::Pi => pi::usage(line, n),
            Harness::Codex => codex::usage(line, n),
            Harness::Kimi => kimi::usage(line, n),
            Harness::Grok => grok::usage(line, n),
            Harness::Dsh => None,
        }
    }

    /// The session files an index names, each marked whether its line
    /// names the worktree `cwd`.
    ///
    /// Each path is absolute, or relative to the harness directory. A harness
    /// that keeps no index names none. The reader opens them. A missing file
    /// is no turn, and one it cannot read is an error. A session the index
    /// files under another directory, as a worker started in its role copy
    /// is, belongs to `cwd` only when it names the worktree's brief
    /// ([`Harness::names_brief`]).
    pub fn indexed_sessions(self, cwd: &str, index: &str) -> Vec<Indexed> {
        match self {
            Harness::Kimi => kimi::indexed_sessions(cwd, index),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Grok => {
                Vec::new()
            }
        }
    }

    /// The line typed at a worker to hand it the brief at `brief`. Codex is
    /// given it as its session's goal, which starts its first turn, and
    /// continues it on its own after each turn its model ends short of it.
    /// Every other harness is given the brief pointer.
    pub fn brief_line(self, brief: &str) -> String {
        match self {
            Harness::Codex => codex::brief_line(brief),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                crate::brief::brief_pointer(brief)
            }
        }
    }

    /// How a session of this harness that nobody types at is handed the
    /// brief at `brief`, given the start `line` (`Harness::in_role`'s). Claude
    /// Code takes a prompt after its flags as its first, so it is started
    /// with the brief pointer as that prompt. Codex is handed its brief as
    /// its goal, which only the `/goal` command typed at its prompt sets; a
    /// prompt it starts with is a user message, not that command. No other
    /// harness has been seen to take its first prompt on its start line in
    /// the interactive session the fleet works in: each takes its brief typed.
    pub fn brief_at_start(self, line: &str, brief: &str) -> BriefAtStart {
        match self {
            Harness::ClaudeCode => BriefAtStart::Started(claude_code::brief_at_start(line, brief)),
            Harness::Codex | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                BriefAtStart::Typed(self.brief_line(brief))
            }
        }
    }

    /// A worker's launch `line` with its goal's token budget, where this
    /// harness keeps a goal. Unchanged for every other harness.
    pub fn with_goal_budget(self, line: &str) -> String {
        match self {
            Harness::Codex => codex::with_goal_budget(line),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                line.to_string()
            }
        }
    }

    /// The token budget of a worker's goal; `None` where it keeps none.
    pub fn goal_tokens(self) -> Option<u64> {
        match self {
            Harness::Codex => Some(codex::GOAL_TOKENS),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Where this harness keeps a session's goal; `None` where it keeps none.
    pub fn goal_store(self) -> Option<GoalStore> {
        match self {
            Harness::Codex => Some(codex::GOAL_STORE),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The goal `answer`, what the store answered its query, says. `None`:
    /// it says none this harness knows.
    pub fn goal(self, answer: &str) -> Option<Goal> {
        match self {
            Harness::Codex => codex::goal(answer),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// What the session record `text` says of its goal; `None` where this
    /// harness keeps none.
    pub fn goal_session(self, text: &str) -> Option<GoalSession> {
        match self {
            Harness::Codex => Some(codex::goal_session(text)),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The query of this harness's goal store whose answer is the row of the
    /// session `thread` (`goal_row`); `None` where it keeps no goal, or
    /// `thread` is no thread id it writes.
    pub fn goal_row_query(self, thread: &str) -> Option<String> {
        match self {
            Harness::Codex => codex::goal_row_query(thread),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// The row `answer`, what the store answered `goal_row_query`, says:
    /// `Some(None)` where it has none. `None`: it says nothing this harness
    /// knows.
    pub fn goal_row(self, answer: &str) -> Option<Option<GoalRow>> {
        match self {
            Harness::Codex => codex::goal_row(answer),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// What ends a session's goal when typed, its turn left to finish.
    pub fn goal_end(self) -> Option<Keystroke> {
        match self {
            Harness::Codex => Some(codex::GOAL_END),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
        }
    }

    /// Whether the session `text` shows the harness began a turn of its goal
    /// by itself, after an earlier turn ended.
    pub fn goal_continued(self, text: &str) -> bool {
        match self {
            Harness::Codex => codex::goal_continued(text),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                false
            }
        }
    }

    /// Whether one session file's `text` was prompted with the task brief at
    /// `brief`, the absolute path in the worktree's own git directory. The
    /// harness selects the user messages that associate its session with a
    /// worker, excluding tool output and model replies.
    pub fn names_brief(self, text: &str, brief: &str) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::names_brief(text, brief),
            Harness::Codex => codex::names_brief(text, brief),
            Harness::Kimi => kimi::names_brief(text, brief),
            Harness::Pi | Harness::Dsh | Harness::Grok => false,
        }
    }

    /// The files a stand-in writes under the harness directory so a later
    /// read sees this turn.
    ///
    /// `open` leaves the turn unended. `at` is when it was written, in epoch
    /// seconds. The body is the text [`recorded_turn`] reads.
    pub fn session_files(self, cwd: &str, open: bool, at: u64) -> Vec<SessionFile> {
        match self {
            Harness::ClaudeCode => claude_code::session_files(cwd, open, at),
            Harness::Pi => pi::session_files(cwd, open, at),
            Harness::Codex => codex::session_files(cwd, open, at),
            Harness::Dsh => dsh::session_files(cwd, open, at),
            Harness::Kimi => kimi::session_files(cwd, open, at),
            Harness::Grok => grok::session_files(cwd, open, at),
        }
    }

    /// Where this harness keeps the words of the session of the worktree
    /// `cwd`: Claude Code's project directory, Grok's message file, or where
    /// the harness keeps its turn.
    pub fn note_place(self, cwd: &str) -> SessionPlace {
        match self {
            Harness::ClaudeCode => SessionPlace {
                under_home: claude_code::HOME,
                layout: claude_code::note_layout(cwd),
                also: Some(claude_code::session_layout(cwd)),
            },
            Harness::Grok => {
                SessionPlace { under_home: grok::HOME, layout: grok::note_layout(cwd), also: None }
            }
            Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi => self.session_place(cwd),
        }
    }

    /// Whether one session file's `text` holds `mark` in a message its model
    /// was given as the user's. A line the harness only queued is not one,
    /// nor a tool's output that shows it.
    pub fn holds_note(self, text: &str, mark: &str) -> bool {
        session::holds_note(text, mark)
    }

    /// When the first message of `text` that holds `mark` as the user's
    /// (as `holds_note`) was recorded, in epoch seconds. `None` when no such
    /// message says when.
    pub fn noted_at(self, text: &str, mark: &str) -> Option<u64> {
        session::noted_at(text, mark)
    }

    /// The files a stand-in writes under the harness directory for `line`,
    /// typed at a worker in `cwd` while it worked: given to its model when
    /// `taken`, else only queued. A harness with no stand-in for it writes
    /// none.
    pub fn note_files(self, cwd: &str, line: &str, taken: bool, at: u64) -> Vec<SessionFile> {
        match self {
            Harness::ClaudeCode => claude_code::note_files(cwd, line, taken, at),
            Harness::Grok => grok::note_files(cwd, line, taken, at),
            Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Kimi => Vec::new(),
        }
    }

    /// The index after this worktree's turn is recorded, given the `existing`
    /// index. `""` is an index that was not there. `None` when this harness
    /// keeps no index. Lines for other worktrees stay.
    pub fn session_index(self, cwd: &str, existing: &str) -> Option<String> {
        match self {
            Harness::Kimi => Some(kimi::session_index(cwd, existing)),
            Harness::ClaudeCode | Harness::Pi | Harness::Codex | Harness::Dsh | Harness::Grok => {
                None
            }
        }
    }

    /// The name of a note beside the worktree's git directory.
    ///
    /// Its text is another harness directory whose session is also this
    /// worktree's. `None` when the user's home and the directories the fleet
    /// keeps are the only places.
    pub const fn noted_home(self) -> Option<&'static str> {
        match self {
            Harness::Codex => Some(codex::HOME_NOTE),
            Harness::ClaudeCode | Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => {
                None
            }
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

    /// What a hook run before a tool call answers so that the session stops
    /// before that call, cleanly, its session kept to be continued
    /// (`resumed`): rather than refusing the one call (`refused`), it ends
    /// the session's turn. `reason` is what the session's user is shown.
    /// `None` of a harness with no such answer. Whether the harness honours
    /// it is `stop_reliable`.
    pub fn stop_answer(self, reason: &str) -> Option<Answer> {
        match self {
            Harness::ClaudeCode => Some(claude_code::stop_answer(reason)),
            Harness::Codex => Some(codex::stop_answer(reason)),
            Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => None,
        }
    }

    /// Whether this harness is known to honour its `stop_answer`. A caller
    /// stops a session whose harness does not some other way.
    pub const fn stop_reliable(self) -> bool {
        match self {
            Harness::ClaudeCode => claude_code::STOP_RELIABLE,
            Harness::Codex => codex::STOP_RELIABLE,
            Harness::Pi | Harness::Dsh | Harness::Kimi | Harness::Grok => false,
        }
    }
}

/// What a hook prints on its standard output, and the code it exits with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    pub printed: String,
    pub code: u8,
}

/// Whether `model`, as a stage writes it, is one of `listed`, the models a
/// harness lists on a host: its whole name, as written.
fn whole(listed: &[String], model: &str) -> bool {
    listed.iter().any(|name| name == model.trim())
}

/// The variables a fleet's worker on a stage carries, as the tests of each
/// harness's start line give them: what a caller puts in `Start::env`.
#[cfg(test)]
pub(crate) fn stage_env(harness: Harness, model: Option<&str>) -> Vec<(&'static str, String)> {
    let mut env: Vec<(&'static str, String)> = [
        ("AFKCONVOY_ISSUE", "7"),
        ("AFKCONVOY_FLOW", "feature"),
        ("AFKCONVOY_STAGE", "build"),
        ("AFKCONVOY_ROLE", "dev"),
        ("AFKCONVOY_SKILL", "implement"),
        ("AFKCONVOY_WRITES", "false"),
        ("AFKCONVOY_CONSULT", "false"),
        ("AFKCONVOY_BASE_BRANCH", "main"),
        ("AFKCONVOY_ESCALATE_LABEL", "ready-for-human"),
        ("AFKCONVOY_LOCK_LABEL", "fleet-lock"),
        ("AFKCONVOY_ROLE_COMMIT", "abc"),
        ("AFKCONVOY_TEAM_COMMIT", "def"),
        (HARNESS_ENV, harness.key()),
    ]
    .into_iter()
    .map(|(name, value)| (name, value.to_string()))
    .collect();
    if let Some(model) = model {
        env.push(("AFKCONVOY_MODEL", model.to_string()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_harness_is_looked_for_only_as_the_first_word_of_its_launch_command() {
        let claude = |launch| Harness::ClaudeCode.runs_itself(launch);
        assert!(claude("claude --dangerously-skip-permissions"));
        assert!(claude("  claude"));
        // a wrapper, an alias that ends in claude, another agent, a path
        for other in ["kim --yolo", "wrap claude", "aider", "/usr/local/bin/claude", "claudex", ""]
        {
            assert!(!claude(other), "{other:?}");
        }
        assert!(Harness::Pi.runs_itself("pi --thinking high"));
        for other in ["wrap pi", "pip install", "/opt/homebrew/bin/pi", "claude"] {
            assert!(!Harness::Pi.runs_itself(other), "{other:?}");
        }
        // …and only as its own harness's: pi starts no Claude Code worker
        assert!(!claude("pi"));
        let codex = |launch| Harness::Codex.runs_itself(launch);
        assert!(codex("codex --dangerously-bypass-approvals-and-sandbox"));
        for other in ["wrap codex", "codexx", "claude", "pi"] {
            assert!(!codex(other), "{other:?}");
        }
    }

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
    fn each_harness_says_whether_its_worktree_trust_is_flags() {
        let flags: Vec<Harness> =
            Harness::ALL.into_iter().filter(|harness| harness.trust_is_flags()).collect();
        assert_eq!(flags, [Harness::ClaudeCode, Harness::Pi, Harness::Codex]);
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
    fn the_harness_names_the_role_file_a_worker_is_told() {
        assert_eq!(Harness::Grok.role_file(), Some(crate::start::RULES));
        for harness in Harness::ALL {
            if harness == Harness::Grok {
                continue;
            }
            assert_eq!(harness.role_file(), None, "{}", harness.key());
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

    const AT: u64 = 1_793_106_373;
    const CWD: &str = "/Users/sunfmin/Developments/chessmirror";

    #[test]
    fn each_harness_names_where_it_keeps_a_session() {
        assert!(matches!(
            Harness::ClaudeCode.session_place(CWD).layout,
            SessionLayout::Tree { dir, extension: "jsonl" } if dir == "projects"
        ));
        assert!(matches!(
            Harness::ClaudeCode.note_place(CWD).layout,
            SessionLayout::Files { dir, extension: "jsonl" }
                if dir == "projects/-Users-sunfmin-Developments-chessmirror"
        ));
        assert!(matches!(
            Harness::Pi.session_place(CWD).layout,
            SessionLayout::Files { dir, extension: "jsonl" }
                if dir == "sessions/--Users-sunfmin-Developments-chessmirror--"
        ));
        assert!(matches!(
            Harness::Codex.session_place(CWD).layout,
            SessionLayout::Tree { dir, extension: "jsonl" } if dir == "sessions"
        ));
        assert!(matches!(
            Harness::Dsh.session_place(CWD).layout,
            SessionLayout::Named { dir, name, compressed: true, usage: None }
                if dir == "sessions/--Users-sunfmin-Developments-chessmirror--"
                    && name == "session.v4.jsonl.zstd"
        ));
        assert!(matches!(
            Harness::Kimi.session_place(CWD).layout,
            SessionLayout::Index { file } if file == "session_index.jsonl"
        ));
        assert!(matches!(
            Harness::Grok.session_place(CWD).layout,
            SessionLayout::Named { dir, name, compressed: false, usage: Some("updates.jsonl") }
                if dir == "sessions/%2FUsers%2Fsunfmin%2FDevelopments%2Fchessmirror"
                    && name == "events.jsonl"
        ));
        for harness in Harness::ALL {
            let place = harness.session_place(CWD);
            assert!(!place.under_home.is_empty(), "{}", harness.key());
        }
        assert_eq!(Harness::Codex.noted_home(), Some(codex::HOME_NOTE));
        for harness in Harness::ALL {
            if harness == Harness::Codex {
                continue;
            }
            assert_eq!(harness.noted_home(), None, "{}", harness.key());
        }
    }

    #[test]
    fn a_long_worktree_keeps_a_short_session_name() {
        let cwd = format!("/{}", "segment/".repeat(40));
        assert!(cwd.len() > 255);
        for harness in Harness::ALL {
            let place = harness.session_place(&cwd);
            let dir = match place.layout {
                SessionLayout::Files { dir, .. } | SessionLayout::Named { dir, .. } => dir,
                SessionLayout::Tree { dir, .. } | SessionLayout::Index { file: dir } => dir,
            };
            for component in dir.split('/') {
                assert!(component.len() <= 255, "{}: {component}", harness.key());
            }
            for file in harness.session_files(&cwd, true, AT) {
                for component in file.path.split('/') {
                    assert!(component.len() <= 255, "{}: {component}", harness.key());
                }
            }
        }
        for harness in [Harness::ClaudeCode, Harness::Pi, Harness::Dsh, Harness::Grok] {
            let place = harness.note_place(&cwd);
            let dir = match place.layout {
                SessionLayout::Files { dir, .. } | SessionLayout::Named { dir, .. } => dir,
                SessionLayout::Tree { .. } | SessionLayout::Index { .. } => unreachable!(),
            };
            let component = dir.rsplit('/').next().unwrap();
            assert!(component.starts_with("afkconvoy-"), "{}: {component}", harness.key());
        }
    }

    #[test]
    fn an_open_line_is_an_open_turn_and_an_end_closes_it() {
        for harness in Harness::ALL {
            let open = turn_of(harness, true);
            assert!(matches!(open, Some(Turn::Open { .. })), "{} open: {open:?}", harness.key());
            let closed = turn_of(harness, false);
            assert!(
                matches!(closed, Some(Turn::Closed { .. })),
                "{} closed: {closed:?}",
                harness.key()
            );
            assert_eq!(
                turn_of_bound(harness, true, Some(AT + 1)),
                None,
                "{} must not prove a start before the bound",
                harness.key()
            );
        }
    }

    #[test]
    fn a_child_session_does_not_end_the_workers_turn() {
        use crate::turn::Turn;
        let iso = "2026-10-27T13:06:13Z";
        let claude = format!(
            "{}\n{}\n",
            serde_json::json!({
                "type": "user",
                "timestamp": iso,
                "cwd": CWD,
                "isSidechain": false
            }),
            serde_json::json!({
                "type": "assistant",
                "timestamp": iso,
                "cwd": CWD,
                "isSidechain": true,
                "message": {"stop_reason": "end_turn"}
            })
        );
        let claude = Harness::ClaudeCode.recorded_turn(CWD, &claude, None, None);
        assert_eq!(claude, Some(Turn::Open { since: AT }));

        let grok = format!(
            "{}\n{}\n",
            serde_json::json!({
                "ts": iso,
                "type": "turn_started",
                "session_relationship": "primary"
            }),
            serde_json::json!({
                "ts": iso,
                "type": "turn_ended",
                "session_relationship": "subagent"
            })
        );
        let grok = Harness::Grok.recorded_turn(CWD, &grok, None, None);
        assert_eq!(grok, Some(Turn::Open { since: AT }));

        let primary = format!(
            "{}\n{}\n",
            serde_json::json!({"ts": iso, "type": "turn_started"}),
            serde_json::json!({"ts": iso, "type": "turn_ended"})
        );
        assert_eq!(
            Harness::Grok.recorded_turn(CWD, &primary, None, None),
            Some(Turn::Closed { since: AT, started: Some(AT) })
        );
    }

    /// Claude Code compacts an idle session before its prompt cache expires,
    /// writing the summary as a user entry after the turn ended: the worker is
    /// still at its prompt (sunfmin/control#511).
    #[test]
    fn a_compaction_while_idle_opens_no_turn() {
        let at = |second: u64| crate::harness::session::format_iso(AT + second);
        let lines = [
            serde_json::json!({"type": "user", "timestamp": at(0), "cwd": CWD,
                "isSidechain": false, "message": {"role": "user", "content": "brief"}}),
            serde_json::json!({"type": "assistant", "timestamp": at(10), "cwd": CWD,
                "isSidechain": false, "message": {"stop_reason": "end_turn"}}),
            serde_json::json!({"type": "system", "subtype": "compact_boundary",
                "timestamp": at(600), "cwd": CWD, "isSidechain": false}),
            serde_json::json!({"type": "user", "timestamp": at(600), "cwd": CWD,
                "isSidechain": false, "isCompactSummary": true,
                "isVisibleInTranscriptOnly": true,
                "message": {"role": "user", "content": "This session is being continued"}}),
        ];
        let text: String = lines.iter().map(|line| format!("{line}\n")).collect();
        let turn = Harness::ClaudeCode.recorded_turn(CWD, &text, None, None);
        assert_eq!(turn, Some(Turn::Closed { since: AT + 10, started: Some(AT) }));
    }

    #[test]
    fn the_primary_session_that_started_last_is_the_turn() {
        use crate::turn::prefer;

        let killed = grok_session(AT, None, "primary");
        let finished = grok_session(AT + 40, Some(AT + 60), "primary");
        let killed = Harness::Grok.recorded_turn(CWD, &killed, None, None);
        let finished = Harness::Grok.recorded_turn(CWD, &finished, None, None);
        assert_eq!(killed, Some(Turn::Open { since: AT }));
        let turn = prefer(&[killed.unwrap(), finished.unwrap()]);
        assert_eq!(turn, Some(Turn::Closed { since: AT + 60, started: Some(AT + 40) }));

        let earlier = grok_session(AT, Some(AT + 20), "primary");
        let later = grok_session(AT + 40, None, "primary");
        let turn = prefer(&[
            Harness::Grok.recorded_turn(CWD, &earlier, None, None).unwrap(),
            Harness::Grok.recorded_turn(CWD, &later, None, None).unwrap(),
        ]);
        assert_eq!(turn, Some(Turn::Open { since: AT + 40 }));

        let child = grok_session(AT + 40, Some(AT + 60), "subagent");
        assert_eq!(Harness::Grok.recorded_turn(CWD, &child, None, None), None);
        let lead = Harness::Grok.recorded_turn(CWD, &later, None, None);
        assert_eq!(prefer(&[lead.unwrap()]), Some(Turn::Open { since: AT + 40 }));
    }

    /// One Grok session. `end` closes it. A relationship other than `primary`
    /// is a child. An open primary also records `waiting_for_model`, which is
    /// not an end.
    fn grok_session(start: u64, end: Option<u64>, relationship: &str) -> String {
        let mut body = format!("{}\n", grok_line(start, "turn_started", relationship));
        if end.is_none() && relationship == "primary" {
            body.push_str(&format!(
                "{}\n",
                grok_line(start + 10, "waiting_for_model", relationship)
            ));
        }
        if let Some(end) = end {
            body.push_str(&format!("{}\n", grok_line(end, "turn_ended", relationship)));
        }
        body
    }

    fn grok_line(at: u64, kind: &str, relationship: &str) -> String {
        session::object(vec![
            ("ts", session::format_iso(at).into()),
            ("type", kind.into()),
            ("session_relationship", relationship.into()),
        ])
        .to_string()
    }

    #[test]
    fn another_worktree_s_session_is_not_this_turn() {
        let elsewhere = "/somewhere/else";
        let claude = Harness::ClaudeCode.session_files(elsewhere, true, AT);
        assert_eq!(Harness::ClaudeCode.recorded_turn(CWD, &claude[0].body, None, None), None);
        let pi = Harness::Pi.session_files(elsewhere, true, AT);
        assert_eq!(Harness::Pi.recorded_turn(CWD, &pi[0].body, None, None), None);
        let codex = Harness::Codex.session_files(elsewhere, true, AT);
        assert_eq!(Harness::Codex.recorded_turn(CWD, &codex[0].body, None, None), None);
        let index = Harness::Kimi.session_index(elsewhere, "").unwrap();
        let here = |cwd: &str, index: &str| {
            Harness::Kimi.indexed_sessions(cwd, index).iter().filter(|s| s.here).count()
        };
        assert_eq!(here(CWD, &index), 0);
        let kept = Harness::Kimi.session_index(CWD, &index).unwrap();
        assert_eq!(here(elsewhere, &kept), 1);
        assert_eq!(here(CWD, &kept), 1);
        assert_eq!(Harness::Kimi.indexed_sessions(CWD, &kept).len(), 2);
    }

    /// A Kimi worker starts in its role copy and is prompted with the brief
    /// kept in its worktree's git dir, as on 2026-10-28 (sunfmin/control#381).
    #[test]
    fn a_kimi_wire_prompted_with_this_worktree_s_brief_names_it() {
        let brief = "/repo/.git/worktrees/issue-7/convoy-worker-prompt.md";
        let pointer = crate::brief::brief_pointer(brief);
        let wire = |kind: &str, text: &str| {
            format!("{}\n", serde_json::json!({"type": kind, "time": 1, "prompt": text}))
        };
        assert!(Harness::Kimi.names_brief(&wire("turn.prompt", &pointer), brief));
        let other =
            crate::brief::brief_pointer("/repo/.git/worktrees/issue-70/convoy-worker-prompt.md");
        assert!(!Harness::Kimi.names_brief(&wire("turn.prompt", &other), brief));
        let deeper = crate::brief::brief_pointer(&format!("/elsewhere{brief}"));
        assert!(!Harness::Kimi.names_brief(&wire("turn.prompt", &deeper), brief));
        assert!(!Harness::Kimi.names_brief(&wire("tool.result", &pointer), brief));
        let nested = serde_json::json!({"type": "turn.prompt",
            "input": [{"type": "text", "text": pointer}]});
        assert!(Harness::Kimi.names_brief(&format!("{nested}\n"), brief));
        assert!(!Harness::ClaudeCode.names_brief(&wire("turn.prompt", &pointer), brief));

        let role = "/cache/afkconvoy/copies/acme/team/abc-role+kimi-roles%2Fbuilder";
        let index = serde_json::json!({"workDir": role, "sessionDir": "/k/s"}).to_string();
        let sessions = Harness::Kimi.indexed_sessions(CWD, &index);
        assert_eq!(sessions.len(), 1);
        assert!(!sessions[0].here);
    }

    #[test]
    fn an_index_names_the_session_file_it_records() {
        let index = Harness::Kimi.session_index(CWD, "not json\n").unwrap();
        assert!(index.lines().any(|line| line == "not json"));
        let named: Vec<_> =
            Harness::Kimi.indexed_sessions(CWD, &index).into_iter().map(|s| s.file).collect();
        let written: Vec<_> = Harness::Kimi
            .session_files(CWD, true, AT)
            .iter()
            .map(|file| file.path.clone())
            .collect();
        assert_eq!(named, written);
        let absolute =
            serde_json::json!({"workDir": CWD, "sessionDir": "/tmp/kimi-session"}).to_string();
        let expected = std::path::Path::new("/tmp/kimi-session")
            .join("agents/main/wire.jsonl")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            Harness::Kimi.indexed_sessions(CWD, &absolute),
            vec![Indexed { file: expected, here: true }]
        );
    }

    /// The turn `harness` records in the files it would write, open when `open`.
    fn turn_of(harness: Harness, open: bool) -> Option<crate::turn::Turn> {
        turn_of_bound(harness, open, Some(AT - 2))
    }

    fn turn_of_bound(
        harness: Harness,
        open: bool,
        not_before: Option<u64>,
    ) -> Option<crate::turn::Turn> {
        let mut turns = Vec::new();
        for file in harness.session_files(CWD, open, AT) {
            if let Some(turn) = harness.recorded_turn(CWD, &file.body, not_before, None) {
                turns.push(turn);
            }
        }
        crate::turn::prefer(&turns)
    }
}
