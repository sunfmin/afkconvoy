//! The guard (docs/adr/0012): whether one thing a worker is about to do is
//! refused, and what the fleet checks after a stage that a guard may have
//! missed. The limits that hold for every worker of a bound repo have this
//! one home.
//!
//! A guard recognises commands. It reads the words of each simple command of
//! a shell line; what a script, a variable or a quoted substitution hides
//! gets past it. It stops a worker that errs, not one that means to get by.

use crate::inspection::INSPECTION_ENV;
use crate::intake::INTAKE_RECORD;
use crate::patrol::PATROL_ENV;
use crate::precedent::PROPOSAL_ENV;
use crate::role::{
    BASE_BRANCH_ENV, CONSULT_ENV, ESCALATE_LABEL_ENV, LOCK_LABEL_ENV, STAGE_ENV, WRITES_ENV,
};
use crate::stage_record::{STAGE_LABEL, STAGE_RECORD};
use crate::team::FLOW_LABEL;

/// What ends a simple command of a shell line.
const COMMAND_ENDS: [char; 7] = ['\n', ';', '|', '&', '(', ')', '`'];
/// What ends a word without ending its command.
const WORD_ENDS: [char; 2] = ['<', '>'];
/// The words a command may stand behind and still be the command run.
const WRAPPERS: [&str; 12] =
    ["sudo", "env", "command", "exec", "nohup", "time", "!", "{", "then", "do", "else", "if"];
/// The options of `git` itself that take the next word as their value.
const GIT_VALUES: [&str; 5] = ["-C", "-c", "--git-dir", "--work-tree", "--namespace"];
/// The options of `git push` that take the next word as their value.
const PUSH_VALUES: [&str; 4] = ["-o", "--push-option", "--repo", "--receive-pack"];
/// The options of `git push` that push every branch, the base branch among them.
const PUSH_EVERY: [&str; 3] = ["--all", "--mirror", "--branches"];
/// The `git` subcommands that leave a commit on the branch.
const COMMITS: [&str; 5] = ["commit", "merge", "cherry-pick", "revert", "am"];
/// The programs that change the host — delete what is on it, or stop what
/// runs on it — which a consult does not run (docs/adr/0018).
const HOST_CHANGES: [&str; 7] = ["rm", "rmdir", "unlink", "shred", "kill", "pkill", "killall"];
/// The `git` subcommands, with the word after them where it matters, that
/// delete from the host.
const GIT_DELETES: [(&str, Option<&str>); 3] =
    [("clean", None), ("worktree", Some("remove")), ("worktree", Some("prune"))];

/// The options of `gh` that take the next word as their value and may stand
/// before the command's own words.
const GH_VALUES: [&str; 2] = ["-R", "--repo"];
/// The options of `gh` that add a label to an issue or take one off it.
const LABEL_MOVES: [&str; 2] = ["--add-label", "--remove-label"];
/// The options that label an issue as it is created. On any other command
/// they filter a listing, which writes nothing.
const LABEL_AT_CREATE: [&str; 2] = ["--label", "-l"];
/// What an option of `gh api` that sends a field starts with.
const API_FIELDS: [&str; 5] = ["-f", "-F", "--field", "--raw-field", "--input"];

/// One tool call a worker is about to make, as far as a guard judges it. How
/// a harness describes one is the harness's own (`Harness::call`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    /// It runs this shell line: the only kind a guard judges.
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

/// What a worker is on, as far as its limits say it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum At {
    Stage(String),
    Patrol(String),
    /// The proposal of a precedent: its worker commits and pushes, in its
    /// clone of the team's repo, and is held to what holds every worker.
    Proposal,
    /// The inspection of the hot worker so named (docs/adr/0019): held as a
    /// consult, whatever its facts say.
    Inspection(String),
}

/// What the worker of one stage, or of one patrol, may not do, read from the
/// facts of its start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    at: At,
    /// Whether the worker may commit.
    writes: bool,
    consult: bool,
    base_branch: String,
    escalate_label: String,
    lock_label: String,
}

impl Limits {
    /// The limits of the stage whose facts `var` answers for, by the names a
    /// worker's environment carries them under (`role::Facts::env`). An error
    /// names the fact that is missing: a guard that cannot read its stage
    /// allows nothing.
    pub fn read(var: impl Fn(&str) -> Option<String>) -> Result<Limits, String> {
        let fact = |name: &str| {
            var(name).ok_or_else(|| format!("the stage's facts are incomplete: {name} is not set"))
        };
        let flag = |name: &str| match fact(name)?.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            other => Err(format!("{name} is {other:?}, neither true nor false")),
        };
        // A patrol's worker is at no stage, and neither is a proposal's:
        // its facts name the patrol, or the issue whose ruling it proposes.
        // An inspection's is at none either: it names the hot worker.
        let at = match (var(PATROL_ENV), var(PROPOSAL_ENV), var(INSPECTION_ENV)) {
            (_, _, Some(worker)) => At::Inspection(worker),
            (Some(patrol), _, None) => At::Patrol(patrol),
            (None, Some(_), None) => At::Proposal,
            (None, None, None) => At::Stage(fact(STAGE_ENV)?),
        };
        let inspection = matches!(at, At::Inspection(_));
        Ok(Limits {
            at,
            writes: flag(WRITES_ENV)? && !inspection,
            consult: flag(CONSULT_ENV)? || inspection,
            base_branch: fact(BASE_BRANCH_ENV)?,
            escalate_label: fact(ESCALATE_LABEL_ENV)?,
            lock_label: fact(LOCK_LABEL_ENV)?,
        })
    }

    /// Why the simple command `words` is refused, when it is.
    fn refuses(&self, words: &[String]) -> Option<String> {
        let mut rest = words.iter().map(String::as_str);
        let mut wrapped = false;
        let program = loop {
            let word = rest.next()?;
            if WRAPPERS.contains(&word) {
                wrapped = true;
            } else if !(is_assignment(word) || wrapped && word.starts_with('-')) {
                break word;
            }
        };
        let args: Vec<&str> = rest.collect();
        let name = program.rsplit('/').next();
        if self.consult
            && let Some(name) = name.filter(|name| HOST_CHANGES.contains(name))
        {
            return Some(host_change(&format!("`{name}`")));
        }
        match name {
            Some("sh" | "bash" | "zsh" | "dash" | "ksh") => {
                let mut rest = args.iter();
                while let Some(arg) = rest.next() {
                    if arg.starts_with('-') && arg.contains('c') {
                        return rest.next().and_then(|line| {
                            commands(line).iter().find_map(|words| self.refuses(words))
                        });
                    }
                }
                None
            }
            Some("git") => self.refuses_git(&args),
            Some("gh") => self.refuses_gh(&args),
            Some("orca") if self.consult => {
                let words = positionals(&args, &[]);
                let removes = words.first() == Some(&"worktree")
                    && words.get(1).is_some_and(|verb| ["rm", "remove"].contains(verb));
                removes.then(|| host_change("`orca worktree rm`"))
            }
            _ => None,
        }
    }

    fn refuses_git(&self, args: &[&str]) -> Option<String> {
        let words = positionals(args, &GIT_VALUES);
        let subcommand = *words.first()?;
        if subcommand == "push" {
            let every = args.iter().find(|arg| PUSH_EVERY.contains(arg));
            // The first word after `push` is the remote; the rest are refspecs.
            let named = positionals(args, &PUSH_VALUES);
            let to_base = named.iter().skip(2).find(|refspec| self.lands_on_base(refspec));
            if let Some(pushed) = every.or(to_base) {
                return Some(format!(
                    "`git push … {pushed}` pushes the base branch `{}`: nothing reaches it \
                     except through the landing train. Push your own branch only",
                    self.base_branch
                ));
            }
        }
        let next = words.get(1).copied();
        let deletes = GIT_DELETES
            .iter()
            .any(|(sub, verb)| *sub == subcommand && verb.is_none_or(|verb| next == Some(verb)));
        if self.consult && deletes {
            return Some(host_change(&format!("`git {subcommand}`")));
        }
        if COMMITS.contains(&subcommand) || subcommand == "push" {
            if let At::Inspection(worker) = &self.at {
                return Some(format!(
                    "an inspection writes no code: `git {subcommand}` is refused. Say what you \
                     found of {worker} with `afkconvoy inspected`, and the fleet tells it"
                ));
            }
            if self.consult {
                return Some(format!(
                    "a consult writes no code: `git {subcommand}` is refused. Rule on what \
                     you were asked, or let it through"
                ));
            }
            match &self.at {
                At::Patrol(patrol) => {
                    return Some(format!(
                        "the patrol `{patrol}` writes no code: `git {subcommand}` is refused. \
                         Open an issue for what you found and take it in: every change goes \
                         through a workflow"
                    ));
                }
                At::Stage(stage) if !self.writes => {
                    return Some(format!(
                        "the stage `{stage}` is marked `writes: false`: `git {subcommand}` is \
                         refused. A stage that writes nothing leaves no commit and pushes \
                         nothing; say what you found in the text of your verdict"
                    ));
                }
                At::Stage(_) | At::Proposal | At::Inspection(_) => {}
            }
        }
        None
    }

    /// Whether what `refspec` pushes to is the base branch.
    fn lands_on_base(&self, refspec: &str) -> bool {
        let refspec = refspec.strip_prefix('+').unwrap_or(refspec);
        let to = refspec.rsplit(':').next().unwrap_or(refspec);
        to.strip_prefix("refs/heads/").unwrap_or(to) == self.base_branch
    }

    fn refuses_gh(&self, args: &[&str]) -> Option<String> {
        let words = positionals(args, &GH_VALUES);
        let (group, verb) = (words.first().copied()?, words.get(1).copied());
        let writes = if group == "api" {
            api_writes(args)
        } else {
            matches!(verb, Some("comment" | "create" | "edit"))
        };
        if writes && args.iter().any(|arg| arg.contains(STAGE_RECORD)) {
            return Some(
                "a stage record is written by the fleet's own command and nothing else: declare \
                 the verdict with `afkconvoy verdict`, as your brief says"
                    .to_string(),
            );
        }
        if writes && args.iter().any(|arg| arg.contains(INTAKE_RECORD)) {
            return Some(
                "an intake record is written by the fleet's own command and nothing else: take \
                 the issue in with `afkconvoy intake`"
                    .to_string(),
            );
        }
        if group == "api" {
            return self.refuses_api(args, &words);
        }
        if (group, verb) == ("pr", Some("merge")) {
            return Some(merging());
        }
        let mut rest = args.iter().copied();
        while let Some(arg) = rest.next() {
            let (option, joined) = match arg.split_once('=') {
                Some((option, value)) => (option, Some(value)),
                None => (arg, None),
            };
            let moves = LABEL_MOVES.contains(&option)
                || verb == Some("create") && LABEL_AT_CREATE.contains(&option);
            if !moves {
                continue;
            }
            let Some(labels) = joined.or_else(|| rest.next()) else { continue };
            if let Some(why) = labels.split(',').find_map(|label| self.fleets(label.trim())) {
                return Some(why);
            }
        }
        None
    }

    fn refuses_api(&self, args: &[&str], words: &[&str]) -> Option<String> {
        let path = |holds: &str| words.iter().any(|word| word.contains(holds));
        if words
            .iter()
            .any(|word| word.contains("/pulls/") && word.trim_end_matches('/').ends_with("/merge"))
        {
            return Some(merging());
        }
        if path("/issues/") && path("/labels") && api_writes(args) {
            return Some(
                "writing an issue's labels through `gh api` is refused: the guard cannot read \
                 which label it moves, and the flow, stage, escalate and lock labels are the \
                 fleet's. Use `gh issue edit --add-label` or `--remove-label` for a label of \
                 your own"
                    .to_string(),
            );
        }
        None
    }

    /// Why `label` is not a worker's to add or remove, when it is the fleet's.
    fn fleets(&self, label: &str) -> Option<String> {
        if label.eq_ignore_ascii_case(&self.lock_label) {
            return Some(format!(
                "`{label}` is the lock label, and a worker neither adds nor removes it by hand: \
                 a fleet lock is raised with `afkconvoy lock --issue <n>`, and ends when its \
                 issue closes"
            ));
        }
        let lower = label.to_ascii_lowercase();
        if lower.starts_with(FLOW_LABEL) {
            return Some(format!(
                "`{label}` is a flow label, and a worker neither adds nor removes one by hand: \
                 take an issue in to a workflow with `afkconvoy intake`, which puts it on on the \
                 record; where an issue goes from there is read off the verdict its stage declares"
            ));
        }
        let kind = if lower.starts_with(STAGE_LABEL) {
            "a stage"
        } else if label.eq_ignore_ascii_case(&self.escalate_label) {
            "the escalate"
        } else {
            return None;
        };
        Some(format!(
            "`{label}` is {kind} label, and a worker neither adds nor removes one: where an \
             issue goes is read off the verdict its stage declares"
        ))
    }
}

fn host_change(what: &str) -> String {
    format!(
        "a consult does not change the host: {what} is refused. What needs it, rule to the stage \
         that writes, or open as an issue"
    )
}

fn merging() -> String {
    "merging a PR is refused: a finished PR reaches the base branch through the landing train, \
     and a worker never merges one"
        .to_string()
}

/// Whether `word` sets a variable for the command after it.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// The words of `args` that are no option and no option's value, given the
/// options that take the next word as theirs.
fn positionals<'a>(args: &[&'a str], values: &[&str]) -> Vec<&'a str> {
    let mut found = vec![];
    let mut rest = args.iter().copied();
    while let Some(arg) = rest.next() {
        if values.contains(&arg) {
            rest.next();
        } else if !arg.starts_with('-') {
            found.push(arg);
        }
    }
    found
}

/// Whether a `gh api` call with `args` writes: it sends a field, or names a
/// method that is not GET.
fn api_writes(args: &[&str]) -> bool {
    let mut rest = args.iter().copied();
    while let Some(arg) = rest.next() {
        let method = match arg {
            "-X" | "--method" => rest.next(),
            _ => arg.strip_prefix("--method=").or_else(|| arg.strip_prefix("-X")),
        };
        let writes = match method {
            Some(method) => !method.eq_ignore_ascii_case("GET"),
            None => API_FIELDS.iter().any(|field| arg.starts_with(field)),
        };
        if writes {
            return true;
        }
    }
    false
}

/// A shell line as it is read: the simple commands found so far, and the one
/// being read.
#[derive(Default)]
struct Reading {
    commands: Vec<Vec<String>>,
    words: Vec<String>,
    word: String,
    /// Whether a word has begun: a quoted one may be empty.
    begun: bool,
}

impl Reading {
    fn letter(&mut self, c: char) {
        self.word.push(c);
        self.begun = true;
    }

    /// Begins a word at a quote → where what follows it stands.
    fn quoted(&mut self, within: Within) -> Within {
        self.begun = true;
        within
    }

    fn end_word(&mut self) {
        if self.begun {
            self.words.push(std::mem::take(&mut self.word));
            self.begun = false;
        }
    }

    fn end_command(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            self.commands.push(std::mem::take(&mut self.words));
        }
    }
}

/// The simple commands of the shell line `line`, each as its words with the
/// quoting taken off. Nothing is expanded: a variable is the text that names
/// it.
fn commands(line: &str) -> Vec<Vec<String>> {
    let mut reading = Reading::default();
    let mut within = Within::Plain;
    let mut escaped = false;
    let mut chars = line.chars().peekable();
    let mut heredocs: Vec<(usize, String, bool)> = vec![];
    while let Some(c) = chars.next() {
        if escaped {
            escaped = false;
            // A line carried on outside quotes ends no word and adds nothing.
            if c != '\n' || within != Within::Plain {
                reading.letter(c);
            }
            continue;
        }
        if c == '\n' && matches!(within, Within::Plain | Within::Comment) {
            within = Within::Plain;
            reading.end_command();
            for (command, delimiter, tabs) in heredocs.drain(..) {
                let body = heredoc_body(&mut chars, &delimiter, tabs);
                // The body belongs to the receiving command as data. In
                // particular, a handwritten record sent to gh still counts.
                if let Some(words) = reading.commands.get_mut(command) {
                    words.push(body);
                }
            }
            continue;
        }
        match within {
            Within::Comment if c == '\n' => within = Within::Plain,
            Within::Comment => {}
            Within::Single if c == '\'' => within = Within::Plain,
            Within::Double if c == '"' => within = Within::Plain,
            Within::Double if c == '\\' => escaped = true,
            Within::Single | Within::Double => reading.letter(c),
            Within::Plain if c == '\'' => within = reading.quoted(Within::Single),
            Within::Plain if c == '"' => within = reading.quoted(Within::Double),
            Within::Plain if c == '\\' => escaped = true,
            Within::Plain if c == '<' && chars.peek() == Some(&'<') => {
                chars.next();
                reading.end_word();
                if chars.peek() == Some(&'<') {
                    // A here-string's next word is already ordinary data.
                    chars.next();
                } else {
                    let tabs = chars.peek() == Some(&'-');
                    if tabs {
                        chars.next();
                    }
                    let delimiter = heredoc_delimiter(&mut chars);
                    heredocs.push((reading.commands.len(), delimiter, tabs));
                }
            }
            Within::Plain if c == '#' && !reading.begun => {
                reading.end_command();
                within = Within::Comment;
            }
            Within::Plain if COMMAND_ENDS.contains(&c) => reading.end_command(),
            Within::Plain if c.is_whitespace() || WORD_ENDS.contains(&c) => reading.end_word(),
            Within::Plain => reading.letter(c),
        }
    }
    reading.end_command();
    reading.commands
}

/// Quote removal for the one word after << (or <<-). Unlike a command
/// argument, its value names where the following lines of data end.
fn heredoc_delimiter(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    while chars.peek().is_some_and(|c| *c == ' ' || *c == '\t') {
        chars.next();
    }
    let mut delimiter = String::new();
    let mut within = Within::Plain;
    while let Some(&c) = chars.peek() {
        if within == Within::Plain
            && (c.is_whitespace() || COMMAND_ENDS.contains(&c) || WORD_ENDS.contains(&c))
        {
            break;
        }
        chars.next();
        match (within, c) {
            (Within::Plain, '\'') => within = Within::Single,
            (Within::Plain, '"') => within = Within::Double,
            (Within::Single, '\'') | (Within::Double, '"') => within = Within::Plain,
            (Within::Plain | Within::Double, '\\') => {
                if let Some(next) = chars.next() {
                    delimiter.push(next);
                }
            }
            _ => delimiter.push(c),
        }
    }
    delimiter
}

/// Consume one pending heredoc, leaving the next command (or heredoc) unread.
fn heredoc_body(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    delimiter: &str,
    tabs: bool,
) -> String {
    let mut body = String::new();
    while chars.peek().is_some() {
        let line: String = chars.by_ref().take_while(|c| *c != '\n').collect();
        let line = if tabs { line.trim_start_matches('\t') } else { &line };
        if line == delimiter {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    body
}

/// Where in a shell line a character stands.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Within {
    Plain,
    /// Between two single quotes, where every character is itself.
    Single,
    /// Between two double quotes, where a backslash still escapes.
    Double,
    /// After a `#`, to the end of its line.
    Comment,
}

/// Why `call` is refused under `limits`; `None` when the worker may make it.
/// Refused for every worker: adding or removing a flow, stage, escalate or
/// lock label; writing a stage record by hand; merging a PR; pushing the base
/// branch. Refused besides in a stage that writes nothing, and for a consult:
/// committing and pushing. The reason is written for the worker to read.
pub fn refused(call: &Call, limits: &Limits) -> Option<String> {
    let Call::Shell(line) = call else { return None };
    commands(line).iter().find_map(|words| limits.refuses(words))
}

/// Where a stage that writes nothing started: what the fleet keeps of it to
/// check, once the stage has declared, what a guard may have missed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WritesNothing {
    /// The claim the stage was dispatched under.
    pub claim: String,
    /// The commit its branch was at when its worker started.
    pub tip: String,
    pub stage: String,
    /// Whether the worker is the stage's consult, which writes no code
    /// whatever the stage may.
    pub consult: bool,
}

/// What the second line of a kept start says of a consult's.
const CONSULT_NOTE: &str = "consult";

impl WritesNothing {
    /// The text it is kept as: one line, and a second of a consult's.
    pub fn note(&self) -> String {
        let consult = if self.consult { format!("\n{CONSULT_NOTE}") } else { String::new() };
        format!("{} {} {}{consult}", self.claim, self.tip, self.stage)
    }

    /// What `note` kept; `None` when the text is no such note.
    pub fn read(note: &str) -> Option<WritesNothing> {
        let mut lines = note.trim().lines();
        let mut parts = lines.next()?.splitn(3, ' ');
        let (claim, tip, stage) = (parts.next()?, parts.next()?, parts.next()?);
        if claim.is_empty() || tip.is_empty() || stage.is_empty() {
            return None;
        }
        Some(WritesNothing {
            claim: claim.to_string(),
            tip: tip.to_string(),
            stage: stage.to_string(),
            consult: lines.next() == Some(CONSULT_NOTE),
        })
    }

    /// Why the stage failed, when the branch of the claim `claim` is now at
    /// `tip` and that is not where this stage started under that claim: it
    /// left a commit. `None` under another claim, whose stage this is not.
    pub fn broken(&self, claim: &str, tip: &str) -> Option<String> {
        if self.claim != claim || self.tip == tip {
            return None;
        }
        let who = if self.stage == crate::stock::ROLE {
            "a consult writes no code, and the stock consult".to_string()
        } else if self.consult {
            format!("a consult writes no code, and the consult of the stage `{}`", self.stage)
        } else {
            format!("the stage `{}` is marked `writes: false`, and it", self.stage)
        };
        Some(format!(
            "{who} left a commit on the branch: it started at {} and the branch is now at {tip}",
            self.tip
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::{Facts, On};

    fn facts(writes: bool, consult: bool) -> Facts<'static> {
        Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "build" },
            role: "dev",
            skill: "implement",
            writes,
            consult,
            base_branch: "main",
            escalate_label: "ready-for-human",
            lock_label: "fleet-lock",
            role_commit: "abc",
            team_commit: "def",
        }
    }

    fn limits(writes: bool, consult: bool) -> Limits {
        let env = facts(writes, consult).env();
        Limits::read(|name| env.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone())).unwrap()
    }

    /// Why a writing stage's worker is refused `line`.
    fn why(line: &str) -> Option<String> {
        refused(&Call::shell(line), &limits(true, false))
    }

    fn allowed(lines: &[&str]) {
        for line in lines {
            assert_eq!(why(line), None, "{line}");
        }
    }

    fn refused_for(lines: &[&str], reason: &str) {
        for line in lines {
            let why = why(line).unwrap_or_else(|| panic!("allowed: {line}"));
            assert!(why.contains(reason), "{line}: {why}");
        }
    }

    #[test]
    fn the_limits_are_read_from_what_a_start_puts_in_the_environment() {
        let read = limits(false, true);
        assert_eq!(
            (&read.at, read.writes, read.consult, read.base_branch.as_str()),
            (&At::Stage("build".to_string()), false, true, "main")
        );
        assert_eq!(
            (read.escalate_label.as_str(), read.lock_label.as_str()),
            ("ready-for-human", "fleet-lock")
        );
        let missing = Limits::read(|_| None).unwrap_err();
        assert!(missing.contains("AFKCONVOY_STAGE is not set"), "{missing}");
        let odd = Limits::read(|name| Some(if name == WRITES_ENV { "yes" } else { "x" }.into()));
        assert!(odd.unwrap_err().contains("AFKCONVOY_WRITES is \"yes\""));
    }

    #[test]
    fn a_proposal_s_worker_is_at_no_stage_and_merges_nothing() {
        let facts = Facts { on: On::Proposal { issue: 7 }, ..facts(true, false) };
        let env = facts.env();
        assert!(env.iter().all(|(name, _)| *name != STAGE_ENV), "a proposal has no stage");
        let limits = Limits::read(|name| {
            env.iter().find(|(known, _)| *known == name).map(|(_, value)| value.clone())
        })
        .unwrap();
        assert_eq!(limits.at, At::Proposal);
        // It commits in a clone of the team's repo and pushes its own branch from it.
        for line in ["git commit -m entry", "git push origin convoy-precedent-7-c2001"] {
            assert_eq!(refused(&Call::shell(line), &limits), None, "{line}");
        }
        // The PR is a human's to merge.
        let merging = Call::shell("gh pr merge 31 --repo acme/team --squash");
        assert!(refused(&merging, &limits).is_some());
    }

    #[test]
    fn an_inspection_is_held_as_a_consult_whatever_its_facts_say() {
        // Started with a writing role's facts, it is held all the same.
        let facts = Facts { on: On::Inspection("#7"), ..facts(true, false) };
        let env = facts.env();
        assert!(env.iter().all(|(name, _)| *name != STAGE_ENV), "an inspection has no stage");
        let limits = Limits::read(|name| {
            env.iter().find(|(known, _)| *known == name).map(|(_, value)| value.clone())
        })
        .unwrap();
        assert_eq!(
            (&limits.at, limits.writes, limits.consult),
            (&At::Inspection("#7".into()), false, true)
        );
        let commit = refused(&Call::shell("git commit -m x"), &limits).expect("a refusal");
        assert!(
            commit.contains("an inspection writes no code") && commit.contains("#7"),
            "{commit}"
        );
        assert!(refused(&Call::shell("git push origin HEAD"), &limits).is_some());
        for host in ["kill -9 4242", "rm -rf target", "git clean -fdx"] {
            let why = refused(&Call::shell(host), &limits).unwrap_or_else(|| panic!("{host}"));
            assert!(why.contains("does not change the host"), "{host}: {why}");
        }
        for looking in ["ps -o pid,command -p 4242", "sample 4242 5", "du -sh target"] {
            assert_eq!(refused(&Call::shell(looking), &limits), None, "{looking}");
        }
    }

    #[test]
    fn a_patrol_s_worker_is_at_no_stage_and_writes_no_code() {
        let facts = Facts { on: On::Patrol("triage"), ..facts(false, false) };
        let env = facts.env();
        assert!(env.iter().all(|(name, _)| *name != STAGE_ENV), "a patrol has no stage");
        let limits = Limits::read(|name| {
            env.iter().find(|(known, _)| *known == name).map(|(_, value)| value.clone())
        })
        .unwrap();
        assert_eq!(limits.at, At::Patrol("triage".to_string()));
        for line in ["git commit -m x", "git push origin HEAD"] {
            let why = refused(&Call::shell(line), &limits).unwrap_or_else(|| panic!("{line}"));
            assert!(why.contains("the patrol `triage` writes no code"), "{why}");
        }
        assert_eq!(refused(&Call::shell("gh issue create --title x --body y"), &limits), None);
        let labelled = refused(&Call::shell("gh issue create --title x --label flow:fix"), &limits);
        assert!(labelled.is_some(), "a flow label is put on by `intake` only");
    }

    #[test]
    fn the_train_worker_takes_a_defect_s_issue_in_and_puts_no_flow_label_on_by_hand() {
        // The line its brief gives, from the train's worktree.
        allowed(&[
            "/opt/bin/afkconvoy intake --train --issue 12 --workflow feature",
            "cd /wt/convoy-train && afkconvoy intake --train --issue 12 --workflow 'small fix'",
            "gh issue create --title 'develop is red' --body-file /tmp/defect.md",
        ]);
        refused_for(
            &[
                "gh issue create --title 'develop is red' --body x --label flow:feature",
                "gh issue edit 12 --add-label flow:feature",
                "gh issue comment 12 --body '<!--convoy:intake workflow=feature by=train-->'",
            ],
            "afkconvoy intake",
        );
    }

    #[test]
    fn a_shell_line_is_read_as_its_simple_commands_and_their_words() {
        let read = |line: &str| -> Vec<String> {
            commands(line).iter().map(|words| words.join("·")).collect()
        };
        assert_eq!(read("cd /wt && git  push"), ["cd·/wt", "git·push"]);
        assert_eq!(read("a 'b c' \"d \\\" e\" f\\ g ''"), ["a·b c·d \" e·f g·"]);
        assert_eq!(read("a; b | c\nd $(e) `f`"), ["a", "b", "c", "d·$", "e", "f"]);
        assert_eq!(read("a # not; this\nb#c >out 2>&1"), ["a", "b#c·out·2", "1"]);
        assert_eq!(read("a \\\n b"), ["a·b"]);
        assert_eq!(read("  \n ;; "), [] as [String; 0]);
    }

    #[test]
    fn the_seven_inspection_and_patrol_calls_are_data_not_commands() {
        let inspection = limits(false, true);
        for line in [
            "afkconvoy inspected --worker 7 --text-file /dev/stdin <<'REPORT'\nrm target would free space\nREPORT\n",
            "afkconvoy inspected --worker 7 --text-file - <<\"REPORT\"\ngit worktree remove /w/old\nREPORT\n",
            "cat <<REPORT > report.md\nrm -rf cache\ngit worktree prune\nREPORT\nafkconvoy inspected --worker 7 --text-file report.md",
            "afkconvoy inspected --worker 7 --text-file - <<- 'REPORT'\n\trm target\n\tgit worktree remove /w/old\n\tREPORT\n",
        ] {
            assert_eq!(refused(&Call::shell(line), &inspection), None, "{line}");
        }
        allowed(&[
            r#"gh issue view 7 --json comments --jq '.comments[] | select(.body | contains("convoy:intake"))'"#,
            r#"gh issue view 8 --comments --template '{{range .comments}}{{if contains .body "convoy:intake"}}{{.body}}{{end}}{{end}}'"#,
            r#"gh api repos/o/r/issues/9/comments --jq '.[] | select(.body | contains("convoy:intake"))'"#,
        ]);
    }

    #[test]
    fn heredocs_end_at_their_delimiter_and_keep_the_receiving_command() {
        let consult = limits(false, true);
        for line in [
            "cat <<'EOF'\nrm target\nEOF\nrm target",
            "cat <<EOF # a report\ngit worktree prune\nEOF\ngit worktree prune",
            "cat <<A <<B\nrm target\nA\ngit commit -m x\nB\ngit commit -m x",
            "cat <<'EOF' | cat\nrm target\nEOF\ngit push origin HEAD",
            "cat <<E'OF'\nrm target\nEOF\nkill 42",
            "cat <<\\EOF\nrm target\nEOF\nrm target",
        ] {
            assert!(refused(&Call::shell(line), &consult).is_some(), "{line}");
        }
        allowed(&[
            "cat <<A <<B\nrm target\nA\ngh pr merge 7\nB\ngit status",
            "cat <<'EOF' | cat\ngh pr merge 7\nEOF\n",
            "cat <<< 'gh pr merge 7'",
            "jq -f - <<'QUERY'\nselect(.body | contains(\"convoy:intake\"))\nQUERY\n",
        ]);
        refused_for(
            &[
                "gh issue comment 7 --body-file - <<'EOF'\n<!--convoy:stage stage=build verdict=pass-->\nEOF\n",
                "gh api repos/o/r/issues/7/comments --input - <<'EOF'\n{\"body\":\"<!--convoy:stage stage=build verdict=pass-->\"}\nEOF\n",
            ],
            "`afkconvoy verdict`",
        );
    }

    #[test]
    fn shell_c_arguments_are_commands_even_though_quoted() {
        let consult = limits(false, true);
        for shell in ["sh", "bash", "/bin/sh", "/bin/bash"] {
            for command in [
                "gh pr merge 7",
                "git push origin main",
                "git commit -m x",
                "git merge origin/main",
                "git push origin HEAD",
                "rm target",
                "rmdir target",
                "unlink target",
                "shred target",
                "kill 42",
                "pkill cargo",
                "killall cargo",
                "git clean -fdx",
                "git worktree remove /w/old",
                "git worktree prune",
                "orca worktree rm --worktree path:/w/old",
                "gh issue edit 7 --add-label flow:feature",
                "gh issue edit 7 --remove-label stage:build",
                "gh issue edit 7 --add-label fleet-lock",
                "gh issue edit 7 --add-label ready-for-human",
                "gh api repos/o/r/issues/7/labels -f labels[]=flow:feature",
                "gh issue comment 7 --body convoy:stage",
                "gh issue comment 7 --body convoy:intake",
            ] {
                let direct = refused(&Call::shell(command), &consult).expect(command);
                let wrapped = format!("{shell} -c '{command}'");
                assert_eq!(refused(&Call::shell(&wrapped), &consult), Some(direct), "{wrapped}");
            }
        }
        refused_for(&["env FOO=x bash -lc 'sh -c \"gh pr merge 7\"'"], "merging a PR");
        allowed(&["sh -c 'echo gh pr merge 7'", "bash -c 'cat <<EOF\ngh pr merge 7\nEOF\n'"]);
    }

    #[test]
    fn what_a_worker_does_all_day_is_allowed() {
        allowed(&[
            "cargo test --workspace",
            "git status && git add -A && git commit -m 'a step' && git push origin HEAD",
            "git push -u origin sunfmin/issue-7-main",
            "git push origin main:issue-7",
            "git push",
            "git log main..HEAD",
            "git fetch origin main && git merge origin/main",
            "gh issue view 7 --comments",
            "gh issue list --label flow:feature",
            "gh issue edit 7 --add-label bug,needs-design",
            "gh issue comment 7 --body 'the stage is half done'",
            "gh pr create --base main --title t --body 'Closes #7'",
            "gh pr view 70 --json mergeable",
            "gh api repos/o/r/issues/7/labels",
            "gh api repos/o/r/pulls/70",
            "afkconvoy verdict pass --issue 7 --stage build --text 'convoy:stage in a text'",
            "afkconvoy intake --issue 12 --workflow feature",
            "echo git push origin main",
            "grep -rn 'gh pr merge' docs",
        ]);
        let reads = Call::Other;
        assert_eq!(refused(&reads, &limits(false, true)), None);
    }

    #[test]
    fn no_worker_adds_or_removes_a_label_of_the_fleet() {
        refused_for(
            &[
                "gh issue edit 7 --add-label flow:feature",
                "gh issue edit 7 --remove-label=flow:feature",
                "gh issue edit 7 --add-label bug,Flow:chore",
                "gh -R acme/app issue edit 7 --remove-label 'flow:feature'",
                "gh issue create --title t --body b -l flow:feature",
            ],
            "is a flow label",
        );
        refused_for(&["gh issue edit 7 --add-label stage:review"], "is a stage label");
        refused_for(&["gh issue edit 7 --add-label ready-for-human"], "is the escalate label");
        refused_for(
            &[
                "gh issue edit 7 --remove-label fleet-lock",
                "gh issue create -t t --label fleet-lock",
            ],
            "is the lock label",
        );
        refused_for(
            &[
                "gh api repos/o/r/issues/7/labels -f 'labels[]=flow:feature'",
                "gh api -X DELETE repos/o/r/issues/7/labels/flow:feature",
                "gh api --method=PUT repos/o/r/issues/7/labels --input labels.json",
            ],
            "through `gh api`",
        );
    }

    #[test]
    fn no_worker_writes_a_stage_record_by_hand() {
        refused_for(
            &[
                "gh issue comment 7 --body '<!--convoy:stage stage=build verdict=pass-->\ndone'",
                "gh api repos/o/r/issues/7/comments -f body='<!-- convoy:stage stage=x -->'",
            ],
            "`afkconvoy verdict`",
        );
    }

    #[test]
    fn no_worker_writes_an_intake_record_by_hand() {
        refused_for(
            &[
                "gh issue comment 12 --body '<!--convoy:intake workflow=feature role=dev from=7-->'",
                "gh api repos/o/r/issues/12/comments -f body='<!-- convoy:intake workflow=x -->'",
            ],
            "`afkconvoy intake`",
        );
    }

    #[test]
    fn no_worker_merges_a_pr() {
        refused_for(
            &[
                "gh pr merge 70 --squash",
                "cd /wt && gh pr merge --auto",
                "GH_TOKEN=x /opt/homebrew/bin/gh pr merge 70",
                "gh api -X PUT repos/o/r/pulls/70/merge",
            ],
            "merging a PR is refused",
        );
    }

    #[test]
    fn no_worker_pushes_the_base_branch() {
        refused_for(
            &[
                "git push origin main",
                "git push origin HEAD:main",
                "git push -f origin +issue-7:refs/heads/main",
                "git -C /wt push origin issue-7 :main",
                "git commit -m x; git push --all origin",
                "sudo -n git push --mirror",
                "git push -o ci.skip origin HEAD:main",
                "if true; then git push origin main; fi",
                "echo \"$(date)\" && (git push origin main)",
            ],
            "pushes the base branch `main`",
        );
    }

    #[test]
    fn a_stage_that_writes_nothing_and_a_consult_neither_commit_nor_push() {
        let lines = [
            "git commit -am 'a fix while reviewing'",
            "git -c user.name=x commit --amend",
            "git push origin HEAD",
            "git push",
            "git merge origin/main",
            "git cherry-pick abc",
        ];
        for line in lines {
            let reviewing = refused(&Call::shell(line), &limits(false, false)).expect(line);
            assert!(
                reviewing.contains("the stage `build` is marked `writes: false`"),
                "{reviewing}"
            );
            // A consult is held to it whatever its stage is marked.
            let consulting = refused(&Call::shell(line), &limits(true, true)).expect(line);
            assert!(consulting.contains("a consult writes no code"), "{consulting}");
        }
        for line in ["git status", "git diff main...HEAD", "git log -3", "cargo test"] {
            assert_eq!(refused(&Call::shell(line), &limits(false, true)), None, "{line}");
        }
        // What every worker is refused, they are refused too.
        assert!(refused(&Call::shell("gh pr merge 70"), &limits(false, false)).is_some());
    }

    #[test]
    fn a_consult_runs_what_a_human_answering_runs_and_changes_nothing_of_the_host() {
        let consult = limits(true, true);
        for line in crate::waiting::ANSWERING {
            let line = line.replace("<n>", "7").replace("<ready label>", "ready-for-agent");
            assert_eq!(refused(&Call::shell(&line), &consult), None, "{line}");
        }
        for line in ["df -h /", "uptime", "orca terminal list --worktree path:/w", "git status"] {
            assert_eq!(refused(&Call::shell(line), &consult), None, "{line}");
        }
        let changes = [
            "rm -rf ~/Library/Caches/x",
            "/bin/rm target",
            "sudo kill -9 4242",
            "pkill cargo",
            "git clean -fdx",
            "git worktree remove /w/old",
            "git worktree prune",
            "orca worktree rm --worktree path:/w/old",
        ];
        for line in changes {
            let why = refused(&Call::shell(line), &consult).expect(line);
            assert!(why.contains("a consult does not change the host"), "{why}");
        }
        // A stage's worker is held to none of it.
        assert_eq!(refused(&Call::shell("rm -rf target"), &limits(true, false)), None);
        assert_eq!(refused(&Call::shell("git worktree list"), &consult), None);
    }

    #[test]
    fn a_commit_left_by_a_stage_that_writes_nothing_is_a_failure() {
        let started = WritesNothing {
            claim: "c1".into(),
            tip: "aaa".into(),
            stage: "code review".into(),
            consult: false,
        };
        assert_eq!(started.note(), "c1 aaa code review");
        assert_eq!(WritesNothing::read("c1 aaa code review\n"), Some(started.clone()));
        for none in ["", "c1", "c1 aaa", "c1 aaa "] {
            assert_eq!(WritesNothing::read(none), None, "{none:?}");
        }
        assert_eq!(started.broken("c1", "aaa"), None);
        assert_eq!(started.broken("c2", "bbb"), None, "another claim's stage");
        let why = started.broken("c1", "bbb").unwrap();
        assert!(why.contains("the stage `code review` is marked `writes: false`"), "{why}");
        assert!(why.contains("started at aaa") && why.contains("now at bbb"), "{why}");
        // A consult's start says whose it is, and so does its failure.
        let consult = WritesNothing { consult: true, ..started.clone() };
        assert_eq!(consult.note(), "c1 aaa code review\nconsult");
        assert_eq!(WritesNothing::read(&consult.note()), Some(consult.clone()));
        let why = consult.broken("c1", "bbb").unwrap();
        assert!(
            why.starts_with(
                "a consult writes no code, and the consult of the stage `code review` \
                             left a commit"
            ),
            "{why}"
        );
    }
}
