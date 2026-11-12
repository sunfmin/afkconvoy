//! What one session of a harness is started with (`Harness::in_role`): the
//! directory it starts in, the worktree it works on, the rules and skills it
//! is given, the hook run before each of its tool calls, its model and the
//! directory kept for it. Whoever starts the session says each; nothing here
//! knows why.

/// A directory's skills: the directory under it every harness reads one
/// entry per skill from.
pub const SKILLS: &str = ".claude/skills";
/// The file of a skill's directory that says what the skill is.
pub const SKILL_FILE: &str = "SKILL.md";
/// A directory's rules: the file every harness loads them from, so that they
/// are written once (docs/adr/0013).
pub const RULES: &str = "CLAUDE.md";

/// The profile a DSH session loads, independent of host skills and rules:
/// the one DSH is given beside the host user's, or the host user's alone.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DshProfile {
    #[default]
    Fleet,
    HostUser,
}

impl DshProfile {
    pub fn key(self) -> &'static str {
        match self {
            Self::Fleet => "fleet",
            Self::HostUser => "host-user",
        }
    }
}

/// Where one session starts, and what is added to it.
#[derive(Debug, Clone)]
pub struct Start<'a> {
    /// The directory the session is started in, holding its own rules
    /// ([`RULES`]) and skills ([`SKILLS`]). A harness whose session works
    /// only in its cwd starts in `worktree` instead, and is told them
    /// (`Harness::role_file`).
    pub dir: &'a str,
    /// The checkout the session works on.
    pub worktree: &'a str,
    /// Another directory whose rules and skills are added to the session's
    /// own.
    pub added: &'a str,
    /// The model the session runs in, where one is named.
    pub model: Option<&'a str>,
    /// The shell line run before each tool call: what it prints answers
    /// whether the call runs (`Harness::answer`).
    pub hook: &'a str,
    /// The directory this host keeps for the session's harness, holding what
    /// that harness is given beside the host user's (`Harness::kept`).
    pub kept: &'a str,
    /// Whether the session is given the host user's own skills and rules:
    /// the directory `kept` is then the one kept with them
    /// (`Kept::host_user`).
    pub host_user: bool,
    /// Whether `kept` is the user configuration of a harness the start would
    /// otherwise leave on the host user's own (Claude Code): it is then
    /// pointed there by its variable (`Harness::config_dir`). Every other
    /// harness is pointed at `kept` whatever this says.
    pub kept_config: bool,
    /// Native rule paths of the host user's which this harness must exclude
    /// when `host_user` is false, including ones found as project ancestors
    /// (`Harness::host_rule_excludes`).
    pub host_rules: &'a [String],
    /// The DSH profile the session loads.
    pub dsh_profile: DshProfile,
    /// The files of the host user's skills that a skill of the same name in
    /// `dir` stands for, where its harness would show both
    /// (`Harness::shadowed_in`): each is turned off.
    pub shadowed: &'a [String],
    /// The rules its harness is told on the line that starts it: nothing, of
    /// a harness that loads them itself (`Harness::told`).
    pub told: Told<'a>,
    /// The variables put in the session's environment, in this order, after
    /// the harness's own. Each name is written in its caller's source; each
    /// value goes in as one word.
    pub env: Vec<(&'static str, String)>,
}

/// One set of rules as it was read: the file it is in, and what that holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rules<'a> {
    pub file: &'a str,
    pub text: &'a str,
}

/// The rules a session is given from outside the directory it starts in,
/// read for a harness that loads none from there itself (`Harness::told`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Told<'a> {
    /// The repo's own rules, in the worktree. `None`: the repo holds none.
    pub repo: Option<Rules<'a>>,
    /// The rules of the added directory (`Start::added`). `None`: it holds
    /// none.
    pub added: Option<Rules<'a>>,
    /// The rules of the directory the session would start in
    /// (`Start::dir`), from the file its harness names
    /// (`Harness::role_file`). `None` of a harness that names none: it loads
    /// them from that directory, which is where it starts. A harness that
    /// starts in the worktree names the file (ADR-0043).
    pub own: Option<Rules<'a>>,
}
