//! The guard (docs/adr/0012): whether one thing a worker is about to do is
//! refused, and what the fleet checks after a stage that a guard may have
//! missed. The limits that hold for every worker of a bound repo have this
//! one home.
//!
//! A guard recognises commands. It reads the words of each simple command of
//! a shell line; what a script, a variable or a quoted substitution hides
//! gets past it. It stops a worker that errs, not one that means to get by.

use serde::Deserialize;

use crate::role::{
    BASE_BRANCH_ENV, CONSULT_ENV, ESCALATE_LABEL_ENV, LOCK_LABEL_ENV, STAGE_ENV, WRITES_ENV,
};
use crate::stage_record::{STAGE_LABEL, STAGE_RECORD};
use crate::team::FLOW_LABEL;

/// The tool a worker runs shell lines with: the only one a guard judges.
const SHELL_TOOL: &str = "Bash";
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

/// One tool call, as Claude Code describes it to a hook before making it.
#[derive(Debug, Clone, Deserialize)]
pub struct Call {
    tool_name: String,
    #[serde(default)]
    tool_input: Input,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Input {
    /// The shell line, when the tool runs one.
    #[serde(default)]
    command: Option<String>,
}

impl Call {
    /// The call that runs the shell line `line`.
    pub fn shell(line: &str) -> Call {
        Call {
            tool_name: SHELL_TOOL.to_string(),
            tool_input: Input { command: Some(line.to_string()) },
        }
    }
}

/// What the worker of one stage may not do, read from the stage's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    stage: String,
    /// Whether the stage may commit.
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
        Ok(Limits {
            stage: fact(STAGE_ENV)?,
            writes: flag(WRITES_ENV)?,
            consult: flag(CONSULT_ENV)?,
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
        match program.rsplit('/').next() {
            Some("git") => self.refuses_git(&args),
            Some("gh") => self.refuses_gh(&args),
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
        if COMMITS.contains(&subcommand) || subcommand == "push" {
            if self.consult {
                return Some(format!(
                    "a consult writes no code: `git {subcommand}` is refused. Rule on the \
                     escalation, or let it through"
                ));
            }
            if !self.writes {
                return Some(format!(
                    "the stage `{}` is marked `writes: false`: `git {subcommand}` is refused. A \
                     stage that writes nothing leaves no commit and pushes nothing; say what you \
                     found in the text of your verdict",
                    self.stage
                ));
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
        if args.iter().any(|arg| arg.contains(STAGE_RECORD)) {
            return Some(
                "a stage record is written by the fleet's own command and nothing else: declare \
                 the verdict with `afkconvoy verdict`, as your brief says"
                    .to_string(),
            );
        }
        let words = positionals(args, &GH_VALUES);
        let (group, verb) = (words.first().copied()?, words.get(1).copied());
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
        let lower = label.to_ascii_lowercase();
        let kind = if lower.starts_with(FLOW_LABEL) {
            "a flow"
        } else if lower.starts_with(STAGE_LABEL) {
            "a stage"
        } else if label.eq_ignore_ascii_case(&self.escalate_label) {
            "the escalate"
        } else if label.eq_ignore_ascii_case(&self.lock_label) {
            "the lock"
        } else {
            return None;
        };
        Some(format!(
            "`{label}` is {kind} label, and a worker neither adds nor removes one: where an \
             issue goes is read off the verdict its stage declares"
        ))
    }
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
    for c in line.chars() {
        if escaped {
            escaped = false;
            // A line carried on outside quotes ends no word and adds nothing.
            if c != '\n' || within != Within::Plain {
                reading.letter(c);
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
    if call.tool_name != SHELL_TOOL {
        return None;
    }
    let line = call.tool_input.command.as_deref()?;
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
}

impl WritesNothing {
    /// The one line it is kept as.
    pub fn note(&self) -> String {
        format!("{} {} {}", self.claim, self.tip, self.stage)
    }

    /// What `note` kept; `None` when the text is no such line.
    pub fn read(note: &str) -> Option<WritesNothing> {
        let mut parts = note.trim().splitn(3, ' ');
        let (claim, tip, stage) = (parts.next()?, parts.next()?, parts.next()?);
        if claim.is_empty() || tip.is_empty() || stage.is_empty() {
            return None;
        }
        Some(WritesNothing {
            claim: claim.to_string(),
            tip: tip.to_string(),
            stage: stage.to_string(),
        })
    }

    /// Why the stage failed, when the branch of the claim `claim` is now at
    /// `tip` and that is not where this stage started under that claim: it
    /// left a commit. `None` under another claim, whose stage this is not.
    pub fn broken(&self, claim: &str, tip: &str) -> Option<String> {
        if self.claim != claim || self.tip == tip {
            return None;
        }
        Some(format!(
            "the stage `{}` is marked `writes: false`, and it left a commit on the branch: it \
             started at {} and the branch is now at {tip}",
            self.stage, self.tip
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::role::Facts;

    fn facts(writes: bool, consult: bool) -> Facts<'static> {
        Facts {
            issue: 7,
            flow: "feature",
            stage: "build",
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
            (read.stage.as_str(), read.writes, read.consult, read.base_branch.as_str()),
            ("build", false, true, "main")
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
            "echo git push origin main",
            "grep -rn 'gh pr merge' docs",
        ]);
        let reads = Call { tool_name: "Read".into(), tool_input: Input::default() };
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
    fn a_commit_left_by_a_stage_that_writes_nothing_is_a_failure() {
        let started =
            WritesNothing { claim: "c1".into(), tip: "aaa".into(), stage: "code review".into() };
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
    }
}
