//! The guard (docs/adr/0012): whether one thing a worker is about to do is
//! refused, and what the fleet checks after a stage that a guard may have
//! missed. The limits that hold for every worker of a bound repo have this
//! one home.
//!
//! A guard recognises commands. It reads the words of each simple command of
//! a shell line; what a script, a variable or a quoted substitution hides
//! gets past it. It stops a worker that errs, not one that means to get by.

use crate::account::{ACCOUNT, WORKER};
use crate::host_watch::{WATCH_ENV, kept};
use crate::intake::INTAKE_RECORD;
use crate::patrol::PATROL_ENV;
use crate::role::{
    BASE_BRANCH_ENV, CONSULT_ENV, ESCALATE_LABEL_ENV, LOCK_LABEL_ENV, STAGE_ENV, TARGET_BRANCH_ENV,
    WRITES_ENV,
};
use crate::stage_record::{STAGE_LABEL, STAGE_RECORD};
use crate::team::FLOW_LABEL;

use afkconvoy_gate::shell::{Command, WRAPPERS, commands, is_assignment, positionals};

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
/// The programs that delete the paths they are given.
const DELETES: [&str; 4] = ["rm", "rmdir", "unlink", "shred"];
/// The programs that end every process a name matches, whoever started it.
const ENDS_BY_NAME: [&str; 2] = ["pkill", "killall"];
/// The options of `kill` that take the next word as their value.
const KILL_VALUES: [&str; 2] = ["-s", "-n"];
/// The options of `git push` that force it: what others built on is lost.
const PUSH_FORCES: [&str; 3] = ["-f", "--force", "--force-with-lease"];
/// Where what is deleted outside the home folder is the system's temporary
/// files, no one's own.
const TEMPORARY: [&str; 4] = ["/tmp", "/private/tmp", "/var/folders", "/private/var/folders"];
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

/// One tool call a worker is about to make, as far as a guard judges it: the
/// harness crate's, as each harness describes one (`Harness::call`).
pub use afkconvoy_harness::Call;

/// What a worker is on, as far as its limits say it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum At {
    Stage(String),
    Patrol(String),
    /// The watch of the hot worker so named, or of the host
    /// (docs/adr/0026): the operator on its host, whatever its facts say,
    /// within the bounds of any unattended worker.
    Watch(String),
}

/// What the guard knows of one process of this host that a worker would end:
/// whom it is, as the effect layer reads the host's processes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Process {
    /// The fleets of this host started it — a worker, the landing train, an
    /// watch — or it runs under one they started, or was left by one.
    Fleets,
    /// The host runner.
    Runner,
    /// Anything else: the user's own, another app, orca — or no process
    /// runs under that pid.
    Other,
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
    target_branch: Option<String>,
    escalate_label: String,
    lock_label: String,
    /// The home folder, which a watch reads paths under; `None` where
    /// the environment names none.
    home: Option<String>,
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
        let at = match (var(PATROL_ENV), var(WATCH_ENV)) {
            (_, Some(worker)) => At::Watch(worker),
            (Some(patrol), None) => At::Patrol(patrol),
            (None, None) => At::Stage(fact(STAGE_ENV)?),
        };
        // A watch is the operator on its host: it commits, and it is
        // no consult, whatever its facts say.
        let watch = matches!(at, At::Watch(_));
        Ok(Limits {
            at,
            writes: flag(WRITES_ENV)? || watch,
            consult: flag(CONSULT_ENV)? && !watch,
            base_branch: fact(BASE_BRANCH_ENV)?,
            target_branch: var(TARGET_BRANCH_ENV),
            escalate_label: fact(ESCALATE_LABEL_ENV)?,
            lock_label: fact(LOCK_LABEL_ENV)?,
            home: var("HOME").filter(|home| home.starts_with('/')),
        })
    }

    /// Why the simple command `words` is refused, when it is, `processes`
    /// saying whom each process of the host is.
    fn refuses(&self, command: &Command, processes: &impl Fn(u32) -> Process) -> Option<String> {
        let mut rest = command.iter().map(String::as_str);
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
        if let (At::Watch(_), Some(name)) = (&self.at, name)
            && let Some(why) = self.refuses_operator(name, &args, processes)
        {
            return Some(why);
        }
        match name {
            Some("sh" | "bash" | "zsh" | "dash" | "ksh") => {
                let mut rest = args.iter();
                while let Some(arg) = rest.next() {
                    if arg.starts_with('-') && !arg.starts_with("--") && arg.contains('c') {
                        return rest.next().and_then(|line| {
                            commands(line).iter().find_map(|words| self.refuses(words, processes))
                        });
                    }
                }
                None
            }
            Some("git") => self.refuses_git(&args),
            Some("gh") => self.refuses_gh(&args, &command.input),
            Some("orca") if self.consult => {
                orca_removes(&args).then(|| host_change("`orca worktree rm`"))
            }
            _ => None,
        }
    }

    /// Why the watch, the operator on its host, is refused the program
    /// `name` run with `args`: what holds it to the bounds of any unattended
    /// worker beside what holds every worker — only what the fleets of this
    /// host started or own, and never what cannot be rebuilt.
    fn refuses_operator(
        &self,
        name: &str,
        args: &[&str],
        processes: &impl Fn(u32) -> Process,
    ) -> Option<String> {
        if ENDS_BY_NAME.contains(&name) {
            return Some(format!(
                "a watch ends only what the fleets of this host started: `{name}` ends every \
                 process its name matches, the user's own among them. Find the pid (`ps`), and \
                 `kill <pid>`"
            ));
        }
        if name == "kill" {
            return ended(args).into_iter().find_map(|target| ends(target, processes));
        }
        if DELETES.contains(&name) {
            return deleted(args).into_iter().find_map(|path| self.keeps(path));
        }
        if name == "orca" && orca_removes(args) && has_any(args, &["--force"]) {
            return Some(loses_changes("`orca worktree rm --force`"));
        }
        let words = positionals(args, &[]);
        if name == "afkconvoy" && words.first() == Some(&"stop") {
            return Some(
                "a watch never stops the host runner, nor takes a checkout off its list: \
                 `afkconvoy stop` is the user's. Change what holds the work back instead"
                    .to_string(),
            );
        }
        None
    }

    /// Why a watch may not delete `path`, a word of a deleting
    /// command: it is, or holds, what cannot be rebuilt under the home folder
    /// (`watch::KEPT`), or it is outside the home folder and the
    /// system's temporary files — the user's programs and files. A path the
    /// guard cannot place — relative, or named by another variable — is the
    /// worker's to place.
    fn keeps(&self, path: &str) -> Option<String> {
        let home = self.home.as_deref().unwrap_or("");
        let under = |root: &str, path: &str| {
            !root.is_empty()
                && path
                    .strip_prefix(root)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
        };
        let path = ["~", "$HOME", "${HOME}"]
            .iter()
            .find_map(|named| {
                let rest = path.strip_prefix(named)?;
                (rest.is_empty() || rest.starts_with('/')).then(|| format!("{home}{rest}"))
            })
            .unwrap_or_else(|| path.to_string());
        if !path.starts_with('/') {
            return None;
        }
        if under(home, &path) {
            let relative = path.strip_prefix(home).unwrap_or_default();
            return kept(relative).then(|| {
                format!(
                    "a watch never deletes what cannot be rebuilt: `{path}` is, or holds, \
                     model weights, downloads, media, Docker's images or the user's own files"
                )
            });
        }
        if TEMPORARY.iter().any(|root| under(root, &path)) {
            return None;
        }
        Some(format!(
            "a watch never touches what is outside the home folder: `{path}` is the \
             user's, or the system's"
        ))
    }

    fn refuses_git(&self, args: &[&str]) -> Option<String> {
        let words = positionals(args, &GIT_VALUES);
        let subcommand = *words.first()?;
        if subcommand == "push" {
            let every = args.iter().find(|arg| PUSH_EVERY.contains(arg));
            // The first word after `push` is the remote; the rest are refspecs.
            let named = positionals(args, &PUSH_VALUES);
            let candidate = named.iter().skip(2).find(|refspec| {
                let to = refspec.rsplit(':').next().unwrap_or(refspec);
                to.starts_with("refs/convoy/candidates/")
                    || to.starts_with("refs/heads/convoy-candidates/")
                    || to.starts_with("convoy-candidates/")
            });
            if let Some(pushed) = candidate {
                return Some(format!(
                    "`git push … {pushed}` changes candidate ownership: only afkconvoy land may assign, invalidate or publish candidates; push your source branch only"
                ));
            }
            let target = named.iter().skip(2).find(|refspec| {
                let to = refspec.rsplit(':').next().unwrap_or(refspec);
                to.starts_with("refs/convoy/target/")
                    || to.starts_with("refs/heads/convoy-target/")
                    || to.starts_with("convoy-target/")
                    || to.starts_with("refs/convoy/delivery/")
                    || to.starts_with("refs/heads/convoy-delivery/")
                    || to.starts_with("convoy-delivery/")
                    || to.starts_with("refs/convoy/delivery-pr/")
                    || to.starts_with("refs/heads/convoy-delivery-pr/")
                    || to.starts_with("convoy-delivery-pr/")
                    || to.starts_with("refs/convoy/delivery-evidence/")
                    || to.starts_with("refs/heads/convoy-delivery-evidence/")
                    || to.starts_with("convoy-delivery-evidence/")
                    || to.starts_with("refs/convoy/delivery-settled/")
                    || to.starts_with("refs/heads/convoy-delivery-settled/")
                    || to.starts_with("convoy-delivery-settled/")
            });
            if let Some(pushed) = target {
                return Some(format!(
                    "`git push … {pushed}` changes a pinned target or delivery: use afkconvoy target or afkconvoy delivery; push your source branch only"
                ));
            }
            let to_base = named.iter().skip(2).find(|refspec| self.lands_on_base(refspec));
            if let Some(pushed) = every.or(to_base) {
                let target = self.target_branch.as_deref().unwrap_or(&self.base_branch);
                return Some(format!(
                    "`git push … {pushed}` pushes the base branch `{}`: nothing reaches it \
                     except through a landing turn. The target branch `{target}` is protected too. Push your own branch only",
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
        if let At::Watch(_) = &self.at
            && let Some(why) = git_loses(subcommand, next, args)
        {
            return Some(why);
        }
        if COMMITS.contains(&subcommand) || subcommand == "push" {
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
                At::Stage(_) | At::Watch(_) => {}
            }
        }
        None
    }

    /// Whether what `refspec` pushes to is the base branch.
    fn lands_on_base(&self, refspec: &str) -> bool {
        let refspec = refspec.strip_prefix('+').unwrap_or(refspec);
        let to = refspec.rsplit(':').next().unwrap_or(refspec);
        let branch = to.strip_prefix("refs/heads/").unwrap_or(to);
        branch == self.base_branch
            || self.target_branch.as_deref() == Some(branch)
            || branch.starts_with("delivery/jira-")
    }

    fn refuses_gh(&self, args: &[&str], input: &str) -> Option<String> {
        let words = positionals(args, &GH_VALUES);
        let (group, verb) = (words.first().copied()?, words.get(1).copied());
        let writes = if group == "api" {
            api_writes(args)
        } else {
            matches!(verb, Some("comment" | "create" | "edit"))
        };
        let contains =
            |marker: &str| input.contains(marker) || args.iter().any(|arg| arg.contains(marker));
        if writes && contains(STAGE_RECORD) {
            return Some(
                "a stage record is written by the fleet's own command and nothing else: declare \
                 the verdict with `afkconvoy verdict`, as your brief says"
                    .to_string(),
            );
        }
        if writes && contains(INTAKE_RECORD) {
            return Some(
                "an intake record is written by the fleet's own command and nothing else: take \
                 the issue in with `afkconvoy intake`"
                    .to_string(),
            );
        }
        if writes && contains(ACCOUNT) {
            return Some(
                "a work account is written by the fleet when work on an issue ends, and nothing \
                 else writes one"
                    .to_string(),
            );
        }
        if writes && contains(WORKER) {
            return Some(
                "a worker start is written by the fleet when it starts the worker, and nothing \
                 else writes one"
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
        if label.eq_ignore_ascii_case(crate::plan::REPAIR_LABEL) {
            return Some(format!(
                "`{label}` is the fleet repair label; only the fleet assigns repair workers"
            ));
        }
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

/// Why a watch may not run `git subcommand`, `next` the word after it,
/// with `args`: a force push, or what deletes a change no commit holds.
fn git_loses(subcommand: &str, next: Option<&str>, args: &[&str]) -> Option<String> {
    match subcommand {
        "push" => {
            let forced = args.iter().any(|arg| {
                PUSH_FORCES.iter().any(|force| {
                    arg == force
                        || arg.strip_prefix(*force).is_some_and(|rest| rest.starts_with('='))
                })
            });
            let plus =
                positionals(args, &PUSH_VALUES).iter().skip(2).any(|spec| spec.starts_with('+'));
            (forced || plus).then(|| {
                "a watch never force-pushes: what others built on would be lost. Push a \
                 branch of your own, as any worker's change goes"
                    .to_string()
            })
        }
        "clean" => {
            // Its short options, each group of them read as one: `-fdX`.
            let (mut dry, mut ignored, mut untracked) =
                (has_any(args, &["--dry-run"]), false, false);
            for arg in args {
                if arg.starts_with('-') && !arg.starts_with("--") {
                    dry |= arg.contains('n');
                    ignored |= arg.contains('X');
                    untracked |= arg.contains('x');
                }
            }
            let ignored_only = ignored && !untracked;
            (!dry && !ignored_only).then(|| {
                loses_changes(
                    "`git clean` without `-X` (deleting what git does not track is deleting a \
                     change no commit holds; `git clean -fdX` deletes only what git ignores)",
                )
            })
        }
        "worktree" if next == Some("remove") && has_any(args, &["-f", "--force"]) => {
            Some(loses_changes("`git worktree remove --force`"))
        }
        "reset" if has_any(args, &["--hard"]) => Some(loses_changes("`git reset --hard`")),
        _ => None,
    }
}

fn loses_changes(what: &str) -> String {
    format!(
        "a watch never deletes a change no commit holds: {what} is refused. Commit it, or \
         leave it"
    )
}

/// The processes `kill` with `args` ends: the words after its signal, or
/// none where it only lists signals.
fn ended<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut targets = vec![];
    let (mut options, mut signalled) = (true, false);
    let mut rest = args.iter().copied();
    while let Some(arg) = rest.next() {
        if options && arg == "--" {
            options = false;
        } else if options && ["-l", "-L"].contains(&arg) {
            return vec![];
        } else if options && KILL_VALUES.contains(&arg) {
            rest.next();
            signalled = true;
        } else if options && !signalled && arg.starts_with('-') {
            signalled = true;
        } else {
            options = false;
            targets.push(arg);
        }
    }
    targets
}

/// Why a watch may not end `target`, a word `kill` was given: a pid
/// of a process the fleets of this host did not start, the host runner, or
/// what names none — a variable, a process group. A job of its own shell
/// (`%1`) is its own.
fn ends(target: &str, processes: &impl Fn(u32) -> Process) -> Option<String> {
    if target.starts_with('%') {
        return None;
    }
    let Ok(pid) = target.parse::<u32>() else {
        return Some(format!(
            "a watch ends a process by its pid, and `{target}` names none the guard can \
             read: find the pid (`ps`), and `kill <pid>`"
        ));
    };
    match processes(pid) {
        Process::Fleets => None,
        Process::Runner => {
            Some(format!("{pid} is the host runner, which a watch never ends: it is the user's"))
        }
        Process::Other => Some(format!(
            "the fleets of this host did not start {pid}, and a watch ends only what they \
             started — a worker's process, the landing train's, a watch's, or one they left \
             behind. Never the user's own, another app or orca"
        )),
    }
}

/// The paths a deleting command with `args` deletes: its words but its
/// options, every word after `--`.
fn deleted<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut paths = vec![];
    let mut options = true;
    for arg in args {
        if options && *arg == "--" {
            options = false;
        } else if !(options && arg.starts_with('-') && arg.len() > 1) {
            paths.push(*arg);
        }
    }
    paths
}

/// Whether `orca` with `args` removes a worktree.
fn orca_removes(args: &[&str]) -> bool {
    let words = positionals(args, &[]);
    words.first() == Some(&"worktree")
        && words.get(1).is_some_and(|verb| ["rm", "remove"].contains(verb))
}

fn has_any(args: &[&str], options: &[&str]) -> bool {
    args.iter().any(|arg| options.contains(arg))
}

fn host_change(what: &str) -> String {
    format!(
        "a consult does not change the host: {what} is refused. What needs it, rule to the stage \
         that writes, or open as an issue"
    )
}

fn merging() -> String {
    "merging a PR is refused: a finished PR reaches the base branch through a landing turn, \
     and a worker never merges one"
        .to_string()
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

/// Why `call` is refused under `limits`, `processes` saying whom each process
/// of the host is; `None` when the worker may make it.
/// Refused for every worker: adding or removing a flow, stage, escalate or
/// lock label; writing a stage record by hand; merging a PR; pushing the base
/// branch. Refused besides in a stage that writes nothing, and for a consult:
/// committing and pushing, and changing the host. Refused besides for an
/// watch, the operator on its host: ending what the fleets of this host
/// did not start, deleting what cannot be rebuilt or a change no commit holds,
/// a force push, and stopping the host runner. The reason is written for the
/// worker to read.
pub fn refused(
    call: &Call,
    limits: &Limits,
    processes: &impl Fn(u32) -> Process,
) -> Option<String> {
    let Call::Shell(line) = call else { return None };
    commands(line).iter().find_map(|words| limits.refuses(words, processes))
}

/// Why `call` is refused for running one of a repo's heavy commands,
/// `patterns`, outside the host's exclusive queue (sunfmin/control#577):
/// the gate crate's rule (`afkconvoy_gate::shell::heavy`); `None` when it
/// runs none bare, or is no shell call.
pub fn heavy(call: &Call, patterns: &[String]) -> Option<String> {
    let Call::Shell(line) = call else { return None };
    afkconvoy_gate::shell::heavy(line, patterns)
}

/// Refuses Orca input to the asking Terminal. The caller resolves an omitted
/// Terminal through Orca; an explicit handle needs no lookup. This uses the
/// same shell reader as the stage guards, including wrappers and `sh -c`.
pub fn asking_input(
    call: &Call,
    protected: &str,
    current: &impl Fn(&[String]) -> Result<Option<String>, String>,
) -> Result<Option<String>, String> {
    let Call::Shell(line) = call else { return Ok(None) };
    for command in commands(line) {
        let mut rest = command.iter().map(String::as_str);
        let mut wrapped = false;
        let program = loop {
            let Some(word) = rest.next() else { break "" };
            if WRAPPERS.contains(&word) {
                wrapped = true;
            } else if !(is_assignment(word) || wrapped && word.starts_with('-')) {
                break word.rsplit('/').next().unwrap_or(word);
            }
        };
        let args: Vec<_> = rest.collect();
        if matches!(program, "sh" | "bash" | "zsh" | "dash" | "ksh") {
            for (at, arg) in args.iter().enumerate() {
                if arg.starts_with('-')
                    && !arg.starts_with("--")
                    && arg.contains('c')
                    && let Some(line) = args.get(at + 1)
                    && let Some(why) = asking_input(&Call::shell(line), protected, current)?
                {
                    return Ok(Some(why));
                }
            }
        }
        if program != "orca" {
            continue;
        }
        // Consume option values, so text which happens to say --terminal or
        // terminal send is data, never another target or command.
        let mut words = vec![];
        let mut terminal = None;
        let mut routing = vec![];
        let mut help = false;
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            if arg == "--terminal" {
                terminal = args.next();
            } else if let Some(value) = arg.strip_prefix("--terminal=") {
                terminal = Some(value);
            } else if ["--environment", "--pairing-code", "--worktree"].contains(&arg) {
                routing.push(arg.to_string());
                if let Some(value) = args.next() {
                    routing.push(value.to_string());
                }
            } else if ["--environment=", "--pairing-code=", "--worktree="]
                .iter()
                .any(|flag| arg.starts_with(flag))
            {
                routing.push(arg.to_string());
            } else if arg == "--help" {
                help = true;
            } else if ["--text", "--text-file", "--wait-submit", "--retry-request"].contains(&arg) {
                args.next();
            } else if !arg.starts_with('-') {
                words.push(arg);
            }
        }
        if help || words.get(..2) != Some(&["terminal", "send"]) {
            continue;
        }
        let target = match terminal {
            Some(terminal) => Some(terminal.to_string()),
            None => current(&routing)?,
        };
        if target.as_deref() == Some(protected) {
            return Ok(Some("input to the asking session's Terminal is refused: only the host runner's marked deliveries and the operator may supply its input".into()));
        }
    }
    Ok(None)
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
        let who = if self.consult {
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

    #[test]
    fn asking_input_is_refused_through_wrappers_and_shells_but_text_is_data() {
        let current = |_: &[String]| Ok(Some("asks".to_string()));
        for line in [
            "orca terminal send --terminal asks --text 'yes' --enter",
            "env X=1 /bin/orca terminal send --terminal=asks --interrupt",
            "cd /w && orca --json terminal send --terminal asks --enter",
            "bash -lc 'orca terminal send --terminal asks --text yes'",
            "orca terminal send --text yes",
            "orca terminal send --terminal asks --retry-request key --text-file /tmp/input",
        ] {
            assert!(
                asking_input(&Call::shell(line), "asks", &current).unwrap().is_some(),
                "{line}"
            );
        }
        for line in [
            "orca terminal send --terminal worker --text 'yes' --enter",
            "orca terminal read --terminal asks",
            "orca terminal show --terminal asks",
            "orca terminal send --terminal asks --help",
            "echo 'orca terminal send --terminal asks'",
            "orca terminal send --terminal worker --text '--terminal asks'",
            "orca terminal send --terminal worker --text 'terminal send'",
        ] {
            assert_eq!(asking_input(&Call::shell(line), "asks", &current), Ok(None), "{line}");
        }
        assert_eq!(asking_input(&Call::Other, "asks", &current), Ok(None));
        assert!(
            asking_input(&Call::shell("orca terminal send --enter"), "asks", &|_| Err(
                "unread".into()
            ))
            .is_err()
        );
    }
    use crate::role::{Facts, On};

    /// The fleets of this test's host started 4242 and 4243, and its host
    /// runner is 77; every other pid is someone else's.
    fn whom(pid: u32) -> Process {
        match pid {
            4242 | 4243 => Process::Fleets,
            77 => Process::Runner,
            _ => Process::Other,
        }
    }

    /// Why `call` is refused under `limits`, on this test's host (`whom`).
    fn refused(call: &Call, limits: &Limits) -> Option<String> {
        super::refused(call, limits, &whom)
    }

    fn facts(writes: bool, consult: bool) -> Facts<'static> {
        Facts {
            on: On::Stage { issue: 7, flow: "feature", stage: "build" },
            role: "dev",
            skill: "implement",
            writes,
            consult,
            base_branch: "main",
            target_branch: None,
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

    /// The limits of a watch started with a consult's facts, whose
    /// home folder is /Users/kim.
    fn watch() -> Limits {
        let facts = Facts { on: On::Watch("#7"), ..facts(false, true) };
        let mut env = facts.env();
        assert!(env.iter().all(|(name, _)| *name != STAGE_ENV), "a watch has no stage");
        env.push(("HOME", "/Users/kim".to_string()));
        Limits::read(|name| {
            env.iter().find(|(known, _)| *known == name).map(|(_, value)| value.clone())
        })
        .unwrap()
    }

    #[test]
    fn a_watch_is_the_operator_on_its_host_whatever_its_facts_say() {
        // Started with a consult's facts, it is no consult all the same.
        let limits = watch();
        assert_eq!(
            (&limits.at, limits.writes, limits.consult),
            (&At::Watch("#7".into()), true, false)
        );
        for line in [
            // It changes the repo on a branch of its own, as any worker's change goes.
            "git switch -c afkconvoy-gate-queued && git commit -am 'queue the gate' && git push -u origin HEAD",
            "gh pr create --base main --title 'Queue the gate' --body x",
            // It changes the host, as the operator would.
            "kill 4242",
            "kill -9 4242 4243",
            "kill -s TERM -- 4242",
            "kill %1",
            "kill -l",
            "rm -rf /Users/kim/orca/workspaces/widgets/issue-7/target",
            "rm -rf ~/Library/Caches/Homebrew \"$HOME/.cargo/registry/cache\"",
            "rm -rf target node_modules",
            "rm -f /tmp/afkconvoy-leak.jsonl",
            "git clean -fdX",
            "git clean -n",
            "git worktree remove /Users/kim/orca/workspaces/widgets/issue-3",
            "orca worktree rm --worktree path:/Users/kim/orca/workspaces/widgets/issue-3",
            "afkconvoy run /src/widgets --concurrency 2",
            "afkconvoy restart 7 --instance alpha",
            "afkconvoy switch 7 --instance alpha --harness codex",
            "orca terminal send --terminal term-1 --text 'run the suite through afkconvoy exclusive' --enter",
            "ps -o pid,command -p 4242",
            "sample 4242 5",
            "du -sh target",
        ] {
            assert_eq!(refused(&Call::shell(line), &limits), None, "{line}");
        }
    }

    #[test]
    fn a_watch_ends_only_what_the_fleets_of_its_host_started() {
        let limits = watch();
        let why = |line: &str| {
            refused(&Call::shell(line), &limits).unwrap_or_else(|| panic!("allowed: {line}"))
        };
        // A process the fleet did not start is refused, wherever it stands.
        for line in ["kill 501", "kill -9 4242 501", "sh -c 'kill -TERM 501'", "kill -- 501"] {
            let said = why(line);
            assert!(said.contains("the fleets of this host did not start 501"), "{line}: {said}");
        }
        assert!(why("kill 77").contains("77 is the host runner"));
        for line in ["kill $PID", "kill -9 -4242", "kill $(pgrep cargo)", "kill -- -4242"] {
            assert!(why(line).contains("by its pid"), "{line}");
        }
        for line in ["pkill cargo", "killall -9 cargo", "/usr/bin/pkill -f nextest"] {
            assert!(why(line).contains("ends every process its name matches"), "{line}");
        }
        assert!(why("afkconvoy stop --all").contains("never stops the host runner"));
        assert!(why("/opt/homebrew/bin/afkconvoy stop /src/widgets").contains("never stops"));
    }

    #[test]
    fn a_watch_never_deletes_what_cannot_be_rebuilt() {
        let limits = watch();
        let why = |line: &str| {
            refused(&Call::shell(line), &limits).unwrap_or_else(|| panic!("allowed: {line}"))
        };
        for line in [
            "rm -rf ~/.cache/huggingface/hub",
            "rm -rf /Users/kim/Downloads/old.dmg",
            "rm -rf \"$HOME/Movies\"",
            "rm -rf ${HOME}/Library/Containers/com.docker.docker",
            "rm -rf ~/.cache",
            "rm -rf ~",
            "rm -rf ~/*",
            "rmdir -- ~/Pictures/x",
        ] {
            assert!(why(line).contains("never deletes what cannot be rebuilt"), "{line}");
        }
        for line in ["rm -rf /Applications/Orca.app", "unlink /opt/homebrew/bin/gh", "rm -rf /"] {
            assert!(why(line).contains("outside the home folder"), "{line}");
        }
        for line in [
            "git clean -fdx",
            "git clean -fd",
            "git -C /w/a clean -f -d -x",
            "git worktree remove --force /w/a",
            "git reset --hard origin/main",
            "orca worktree rm --worktree path:/w/a --force",
        ] {
            assert!(why(line).contains("a change no commit holds"), "{line}");
        }
    }

    #[test]
    fn a_watch_pushes_no_base_branch_and_forces_nothing() {
        let limits = watch();
        let why = |line: &str| {
            refused(&Call::shell(line), &limits).unwrap_or_else(|| panic!("allowed: {line}"))
        };
        // A push to the base branch is refused, as for every worker.
        for line in ["git push origin main", "git push origin HEAD:main", "git push --all"] {
            assert!(why(line).contains("pushes the base branch `main`"), "{line}");
        }
        for line in [
            "git push -f origin HEAD",
            "git push --force origin afkconvoy-gate-queued",
            "git push --force-with-lease=x:y origin HEAD",
            "git push origin +HEAD:afkconvoy-gate-queued",
        ] {
            assert!(why(line).contains("never force-pushes"), "{line}");
        }
        assert!(why("gh pr merge 70 --squash").contains("merging a PR is refused"));
        assert!(why("gh issue edit 7 --add-label ready-for-human").contains("escalate label"));
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
    fn the_seven_watch_and_patrol_calls_are_data_not_commands() {
        let watch = limits(false, true);
        for line in [
            "cat /dev/stdin <<'REPORT'\nrm target would free space\nREPORT\n",
            "cat - <<\"REPORT\"\ngit worktree remove /w/old\nREPORT\n",
            "cat <<REPORT > report.md\nrm -rf cache\ngit worktree prune\nREPORT\ncat report.md",
            "cat - <<- 'REPORT'\n\trm target\n\tgit worktree remove /w/old\n\tREPORT\n",
        ] {
            assert_eq!(refused(&Call::shell(line), &watch), None, "{line}");
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
            "gh issue edit 7 --add-label <<'EOF'\nflow:feature\nEOF\n",
            "sh -c <<'EOF'\ngh pr merge 7\nEOF\n",
            "bash --norc 'echo gh pr merge 7'",
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
                "gh issue comment 7 --body convoy:account",
                "gh issue comment 7 --body convoy:worker",
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
    fn no_worker_writes_a_work_account_or_a_worker_start() {
        refused_for(
            &[
                "gh issue comment 7 --body '<!--convoy:account end=2 start=1-->\nwall'",
                "gh issue edit 7 --body '<!--convoy:account-red tree=a seconds=1-->'",
                "gh api repos/o/r/issues/7/comments -f body='<!--convoy:worker at=1 claim=c harness=pi-->'",
            ],
            "written by the fleet",
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
        let tracker = crate::tracker::Tracker::Other {
            tracker: crate::config::RepoName::read("acme/tracker").unwrap(),
            repo: crate::config::RepoName::read("acme/dev").unwrap(),
        };
        for tracker in [crate::tracker::Tracker::Own, tracker] {
            for line in crate::waiting::answering(&tracker) {
                let line = line.replace("<n>", "7").replace("<ready label>", "ready-for-agent");
                assert_eq!(refused(&Call::shell(&line), &consult), None, "{line}");
            }
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

    fn bizkernel() -> Vec<String> {
        ["cargo build", "cargo test", "cargo nextest", "cargo clippy", "./gate.sh"]
            .map(String::from)
            .to_vec()
    }

    #[test]
    fn a_heavy_command_run_bare_is_refused_with_the_line_to_run_instead() {
        let why = heavy(&Call::shell("cd cli && cargo nextest run -p x"), &bizkernel())
            .expect("a refusal");
        assert!(why.contains("`cargo nextest` is a heavy command"), "{why}");
        assert!(
            why.contains("Run `afkconvoy exclusive -- cargo nextest run -p x` instead"),
            "{why}"
        );
        let quoted = "RUST_LOG=debug cargo nextest run -E 'test(a_b)'";
        let why = heavy(&Call::shell(quoted), &bizkernel()).expect("a refusal");
        assert!(
            why.contains(
                "`RUST_LOG=debug afkconvoy exclusive -- cargo nextest run -E 'test(a_b)'`"
            ),
            "{why}"
        );
        for line in [
            "./gate.sh fast",
            "/Users/me/.cargo/bin/cargo build --release",
            "sh -c 'cargo clippy --workspace'",
            "git status; cargo test -p core",
        ] {
            assert!(heavy(&Call::shell(line), &bizkernel()).is_some(), "allowed: {line}");
        }
    }

    #[test]
    fn a_heavy_command_in_the_exclusive_queue_or_none_declared_is_allowed() {
        for line in [
            "cd cli && afkconvoy exclusive -- cargo nextest run -p x",
            "afkconvoy -C . exclusive -- ./gate.sh",
            "afkconvoy exclusive -C . -- sh -c 'cargo test'",
            // afkconvoy's own heavy steps already queue through exclusive.
            "afkconvoy gate --fast",
            "afkconvoy land --issue 577",
            "afkconvoy cut",
            "cargo fmt --all",
            "cargo buildx",
            "echo 'cargo test'",
            "gate.sh",
        ] {
            assert_eq!(heavy(&Call::shell(line), &bizkernel()), None, "{line}");
        }
        assert_eq!(heavy(&Call::shell("cd cli && cargo nextest run -p x"), &[]), None);
        assert_eq!(heavy(&Call::Other, &bizkernel()), None);
    }
}
