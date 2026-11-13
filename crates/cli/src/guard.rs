//! `afkconvoy guard`: what every worker and the watch ask before each tool
//! call. All protect the asking Terminal; bound workers also carry their
//! stage's facts (docs/adr/0012). How the call is described and answered is
//! the harness's
//! (`afkconvoy_core::harness`); whether the call is refused is the core's to
//! say, in one place for every harness (`afkconvoy_core::guard`). Whom a
//! process a watch would end is, this reads off the host's processes,
//! once a call ends one.

use std::cell::OnceCell;
use std::collections::HashSet;
use std::io::Read;
use std::process::ExitCode;

use afkconvoy_core::guard::{Limits, Process, refused};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::role::HARNESS_ENV;

use crate::orca::Worktree;

/// Reads one tool call on stdin and the stage's facts from the environment →
/// exit 0, saying nothing, when the worker may make the call; else the
/// reason on stderr and the exit code that refuses it. A call or a stage it
/// cannot read is refused: nothing is allowed unread.
pub fn guard(terminal_input_only: bool) -> ExitCode {
    let harness = match asking(std::env::var(HARNESS_ENV).ok().as_deref()) {
        Ok(harness) => harness,
        Err(why) => {
            eprintln!("afkconvoy guard: {why}");
            return ExitCode::from(Harness::UNNAMED.refused());
        }
    };
    let mut described = String::new();
    let read = OnceCell::new();
    let whom = |pid: u32| whom(read.get_or_init(on_host), pid);
    let decided = match std::io::stdin().read_to_string(&mut described) {
        Ok(_) => decide_on_host(harness, &described, terminal_input_only, &whom),
        Err(error) => Err(format!("the tool call could not be read: {error}")),
    };
    match decided {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(why)) | Err(why) => {
            eprintln!("afkconvoy guard: {why}");
            ExitCode::from(harness.refused())
        }
    }
}

/// The asking journal, rather than its title or directory, names the protected
/// Terminal. The runner does not invoke a harness hook. The asking session's
/// own hooks are exempt by their runtime Terminal identity, including a
/// launch acknowledged before its creation reply reached the runner.
fn decide_on_host(
    harness: Harness,
    described: &str,
    terminal_input_only: bool,
    processes: &impl Fn(u32) -> Process,
) -> Result<Option<String>, String> {
    let mut input = serde_json::Deserializer::from_str(described);
    let unread = |error| format!("what it was handed describes no tool call: {error}");
    let call = harness.call(&mut input).map_err(unread)?;
    input.end().map_err(unread)?;
    if let Some(terminal) =
        crate::asking_terminal::protected().map_err(|error| error.to_string())?
        && std::env::var("ORCA_TERMINAL_HANDLE").ok().as_deref() != Some(terminal.as_str())
    {
        let current = |routing: &[String]| {
            let mut args = vec!["terminal", "show"];
            args.extend(routing.iter().map(String::as_str));
            crate::orca::orca(&args)
                .and_then(|shown| {
                    shown["terminal"]["handle"]
                        .as_str()
                        .map(|handle| Some(handle.to_string()))
                        .ok_or_else(|| anyhow::anyhow!("Orca returned no Terminal handle"))
                })
                .map_err(|error| format!("cannot identify the input Terminal: {error}"))
        };
        if let Some(why) = afkconvoy_core::guard::asking_input(&call, &terminal, &current)? {
            return Ok(Some(why));
        }
    }
    let heavy = heavy_commands(std::env::var_os("TMPDIR").as_deref().map(std::path::Path::new))?;
    if let Some(why) = afkconvoy_core::guard::heavy(&call, &heavy) {
        return Ok(Some(why));
    }
    if terminal_input_only {
        return Ok(None);
    }
    decide(harness, described, |name| std::env::var(name).ok(), processes)
}

/// The heavy commands of the repo whose worktree the worker's `tmp`, its
/// `TMPDIR`, is: the link the fleet made to the worktree's own
/// (`worker_tmp`), read off that worktree's config as it stands. None where
/// `tmp` is no worker's — a watch, or a session the fleet did not start —
/// or its worktree has no config. An error is a declaration that cannot be
/// read: a guard allows nothing unread.
fn heavy_commands(tmp: Option<&std::path::Path>) -> Result<Vec<String>, String> {
    let Some(own) = tmp.and_then(|tmp| std::fs::read_link(tmp).ok()) else { return Ok(vec![]) };
    if own.file_name() != Some(crate::worker_tmp::TMP_DIR.as_ref()) {
        return Ok(vec![]);
    }
    let Some(worktree) = own.parent() else { return Ok(vec![]) };
    crate::config::heavy_commands_of(&worktree.join(crate::config::FILE))
        .map_err(|error| format!("{error:#}"))
}

/// The harness that asks, by what the worker's environment `names` for it: a
/// worker started before a start named one is a session of the one there was
/// then. An error is a name that is no harness: its call cannot be read.
fn asking(names: Option<&str>) -> Result<Harness, String> {
    match names {
        None => Ok(Harness::UNNAMED),
        Some(key) => Harness::read(key).ok_or_else(|| Harness::unknown(key)),
    }
}

/// Why the tool call `harness` `described` is refused under the facts `var`
/// answers for, `processes` saying whom each process of the host is, when it
/// is. An error is a call or a stage that cannot be read.
fn decide(
    harness: Harness,
    described: &str,
    var: impl Fn(&str) -> Option<String>,
    processes: &impl Fn(u32) -> Process,
) -> Result<Option<String>, String> {
    let unread = |error| format!("what it was handed describes no tool call: {error}");
    let mut described = serde_json::Deserializer::from_str(described);
    let call = harness.call(&mut described).map_err(unread)?;
    described.end().map_err(unread)?;
    Ok(refused(&call, &Limits::read(var)?, processes))
}

/// The processes of this host the fleets of this host started
/// (`hot::fleets`), and its host runner's pid and its keeper's where one
/// is known.
struct OnHost {
    fleets: HashSet<u32>,
    runner: Option<u32>,
    keeper: Option<u32>,
}

/// What `ps` and the host runner's lock say now. A worktree the fleet put
/// a worker in is one whose git dir keeps the note of one.
fn on_host() -> OnHost {
    let procs = crate::hot::processes();
    let ours = |worktree: &std::path::Path| {
        Worktree::at(worktree.to_path_buf(), String::new()).holds_a_worker()
    };
    OnHost {
        fleets: crate::hot::fleets(&procs, ours),
        runner: crate::host_runner::pid(),
        keeper: crate::host_runner::keeper(),
    }
}

/// Whom `pid` is on the host `read`: the host runner before all, as it
/// may itself have been started by a worker.
fn whom(read: &OnHost, pid: u32) -> Process {
    if read.runner == Some(pid) || read.keeper == Some(pid) {
        Process::Runner
    } else if read.fleets.contains(&pid) {
        Process::Fleets
    } else {
        Process::Other
    }
}

/// What `afkconvoy guard --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Never by hand. Every worker and the watch ask it before each tool call. Outside a bound stage, --terminal-input-only checks input to the asking Terminal without requiring stage facts. Bound workers also check what their stage may not do. Read this to learn what a worker is refused, or why one was.

## Who runs it

Claude Code, as a hook: the fleet adds it to every worker of a bound repo as it starts it, whatever the role's repo holds, and Claude Code runs it before each tool call the worker makes. A call it refuses is not made.

pi, through an extension of afkconvoy's own, named on the line that starts a pi worker: pi hands it each tool call before making it, it asks this command, and a call this command does not allow — one it refuses, and one it could not be asked of — is not made. A pi worker whose extension is not there does not start.

Codex, as a hook too, said on the line that starts a Codex worker and kept in no file: Codex runs it before each tool call. Codex makes a call whose hook fails in any way but one, so the hook turns every answer but an allowed call into that one: a call this command refuses, and one it could not be asked of, is not made.

DSH, through a plugin of afkconvoy's own, which the overlay on the line that starts a DSH worker loads last: it asks this command before every tool call. A DSH worker whose plugin is missing or broken does not start.

What each harness's worker is started with, the guard among it: [`afkconvoy help worker-launch`](help:worker-launch).

A role's own limits are hooks in the role's repo — for pi, extensions there. They load because the worker starts there, run beside this one, and refuse in the same way.

## What it reads and writes

It writes nothing. It reads two things, and a third for a watch that would end a process:

Standard input: one tool call, as Claude Code describes it to a `PreToolUse` hook — a JSON object with `tool_name` and `tool_input`. Only a `Bash` call is judged, by the words of each simple command of its `tool_input.command`. pi's is described as pi describes it to an extension — `toolName` and `input` — and only a `bash` call is judged, the same way. Codex describes its own as Claude Code does, and a `Bash` call is judged; its `apply_patch` hands the text of a patch as `tool_input.command`, which is no shell line and is not judged as one. DSH describes a call with `name` and `arguments`: `bash` and `bash_persistent` are judged by `arguments.command`. What is refused is decided once, for all.

The environment: the facts of the worker's stage, which the fleet put there when it started the worker — AFKCONVOY_STAGE, AFKCONVOY_WRITES, AFKCONVOY_CONSULT, AFKCONVOY_BASE_BRANCH, AFKCONVOY_ESCALATE_LABEL, AFKCONVOY_LOCK_LABEL — and, for a selected task destination, AFKCONVOY_TARGET_BRANCH, protected alongside the configured base — and AFKCONVOY_HARNESS, the harness that asks: how the call is described is its own. A watch names its host in AFKCONVOY_WATCH, in place of a stage, and HOME, the home folder it reads paths under.

The host's processes, as `ps` says them, and the host runner's pid, as its lock says it: whom each process a `kill` names is. The fleets of this host started a process carrying a worker's `TMPDIR` — the link a fleet made for it (`afkconvoy help worker-launch`), or the own one (.afkconvoy-tmp) of a worktree they put a worker in — or one under either, as a gate sets, and every process under one of those, or left behind by one.

Refused for every worker:

- input to the asking session's Terminal (`orca terminal send`, including raw text, Enter, interrupts and retried prompts). The current handle comes from the asking launch record, never its title or directory. The host runner's own deliveries run outside worker hooks; the asking session itself is exempt by its runtime Terminal identity. Input to another Terminal is unchanged.
- adding or removing a flow label, a `stage:` label, the escalate label or the lock label (`gh issue edit --add-label`, `--remove-label`, `gh issue create --label`, and any write of an issue's labels through `gh api`)
- writing a stage record by hand: only `afkconvoy verdict` writes one
- writing an intake record by hand: only `afkconvoy intake` writes one, and it is what puts a flow label on for a worker
- writing a work account or a worker start by hand: the fleet writes each, and a worker writes neither
- merging a PR (`gh pr merge`, or the merge endpoint through `gh api`)
- pushing the base branch (`git push` with a refspec that lands on it, `--all`, `--mirror`)
- running one of the repo's heavy commands outside the host's exclusive queue: a simple command whose words begin with a pattern its config's `yaml heavy-commands` block declares (`afkconvoy config --help`), unless it is `afkconvoy exclusive …`. The reason names the pattern and the line to run instead, `afkconvoy exclusive -- <the command>`. The declaration is read from the worker's worktree, the one its `TMPDIR` links to, bound worker or unbound alike, even with --terminal-input-only; a session with no such `TMPDIR`, and the watch, have none. A block that cannot be read refuses every shell call, naming the fault. What `afkconvoy gate`, `land` and `cut` run is no tool call and already queues: it is never refused.

Refused besides in a stage its workflow marks `writes: false`, for every consult, and for a patrol's worker (AFKCONVOY_PATROL, in place of a stage's facts):

- committing and pushing (`git commit`, `merge`, `cherry-pick`, `revert`, `am`, `push`)

A watch is no consult: it is the operator on its host, started so that the fleet's work goes on (`afkconvoy run --help`). It commits, pushes a branch of its own and changes the host, whatever the facts of its role say. It is held instead to the bounds of any unattended worker — refused, beside what every worker is:

- ending what the fleets of this host did not start: `kill` of a pid that is no worker's, watch's process, nor one they left behind; of the host runner or its keeper; of what names no pid the guard can read — a variable, a process group; and `pkill` and `killall`, which end whatever a name matches, the user's own among them
- deleting what cannot be rebuilt: `rm`, `rmdir`, `unlink` or `shred` of a path that is, or holds, model weights, downloads, media, Docker's images or the user's own files under the home folder, or of a path outside it but for the system's temporary files; a path it cannot place — relative, or named by another variable — is its own to place
- deleting a change no commit holds: `git clean` without `-X`, `git worktree remove --force`, `git reset --hard`, `orca worktree rm --force`
- a force push: `--force`, `-f`, `--force-with-lease`, or a refspec that begins with `+`
- stopping the host runner, or taking a checkout off its list: `afkconvoy stop`

## What it answers

Unlike every other command, no JSON: it answers Claude Code the way a hook does.

- exit 0, nothing printed: the call is allowed
- exit 2, a line on stderr: the call is refused; the line is the reason, and Claude Code shows it to the worker — as pi does, given it by the extension, and Codex

A call it cannot read, an environment that lacks one of the stage's facts, or one that names no harness there is, is refused with the reason: nothing is allowed unread.

## What trips you up

- Heredoc bodies and quoted arguments are data, not commands: a report may name `rm` or `git worktree remove`, and a read-only `gh` call may search for stage or intake records with `--jq` or `--template`. Sending a handwritten record through a writing `gh` call is still refused, including through a heredoc. The argument after `sh -c` or `bash -c` is executable shell text and is judged as commands.
- It recognises commands. The same act wrapped in a script, hidden in a variable or in a substitution inside quotes gets past it: it stops a worker that errs, not one that means to get by.
- What it misses is checked after the stage: a commit left on the branch by a stage marked `writes: false`, or by a consult, is a failure, and the issue escalates; a patrol's final commit not on the base branch, working-tree changes, or a PR is a failed run, reported to a human — see `afkconvoy tick --help`.
- Unbound workers and the watch carry only the asking Terminal input guard, and the heavy-commands guard above. The opaque launch command stays intact; the harness adds its hook, extension or plugin. The asking session starts without this guard."#;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every process is someone else's.
    fn nobody(_: u32) -> Process {
        Process::Other
    }

    /// The facts of a stage that writes nothing, with `harness` asking.
    fn facts(name: &str) -> Option<String> {
        let value = match name {
            "AFKCONVOY_STAGE" => "look",
            "AFKCONVOY_WRITES" | "AFKCONVOY_CONSULT" => "false",
            "AFKCONVOY_BASE_BRANCH" => "main",
            "AFKCONVOY_ESCALATE_LABEL" => "ready-for-human",
            "AFKCONVOY_LOCK_LABEL" => "fleet-lock",
            _ => return None,
        };
        Some(value.to_string())
    }

    #[test]
    fn the_harness_that_asks_is_the_one_the_worker_s_environment_names() {
        assert_eq!(asking(None), Ok(Harness::ClaudeCode));
        assert_eq!(asking(Some("claude-code")), Ok(Harness::ClaudeCode));
        assert_eq!(asking(Some("pi")), Ok(Harness::Pi));
        assert_eq!(asking(Some("codex")), Ok(Harness::Codex));
        assert!(asking(Some("aider")).unwrap_err().contains("`aider` is no harness"));
    }

    #[test]
    fn one_call_is_refused_for_the_same_reason_whichever_harness_describes_it() {
        let claude = r#"{"tool_name":"Bash","tool_input":{"command":"git commit -m x"}}"#;
        let pi = r#"{"toolName":"bash","input":{"command":"git commit -m x"}}"#;
        let why = decide(Harness::ClaudeCode, claude, facts, &nobody).unwrap().expect("a refusal");
        assert!(why.contains("`writes: false`: `git commit` is refused"), "{why}");
        assert_eq!(decide(Harness::Pi, pi, facts, &nobody), Ok(Some(why)));
        // A call described in another harness's words cannot be read.
        assert!(
            decide(Harness::Pi, claude, facts, &nobody)
                .unwrap_err()
                .contains("describes no tool call")
        );
        assert!(decide(Harness::ClaudeCode, pi, facts, &nobody).is_err());
        let allowed = r#"{"toolName":"bash","input":{"command":"ls"}}"#;
        assert_eq!(decide(Harness::Pi, allowed, facts, &nobody), Ok(None));
    }

    #[test]
    fn the_heavy_commands_are_those_of_the_worktree_the_worker_s_tmpdir_links_to() {
        let dir = tempfile::tempdir().unwrap();
        let worktree = dir.path().join("w");
        std::fs::create_dir_all(worktree.join(crate::worker_tmp::TMP_DIR)).unwrap();
        std::fs::create_dir_all(worktree.join("docs/agents")).unwrap();
        let tmp = dir.path().join("link");
        std::os::unix::fs::symlink(worktree.join(crate::worker_tmp::TMP_DIR), &tmp).unwrap();
        assert_eq!(heavy_commands(Some(&tmp)), Ok(vec![]), "a worktree with no config");
        std::fs::write(
            worktree.join(crate::config::FILE),
            "```yaml heavy-commands\ncommands: [cargo nextest, ./gate.sh]\n```\n",
        )
        .unwrap();
        let heavy = heavy_commands(Some(&tmp)).unwrap();
        assert_eq!(heavy, ["cargo nextest", "./gate.sh"]);
        assert_eq!(heavy_commands(None), Ok(vec![]));
        assert_eq!(heavy_commands(Some(&worktree)), Ok(vec![]), "no worker's link");
        std::fs::write(
            worktree.join(crate::config::FILE),
            "```yaml heavy-commands\nnope: 1\n```\n",
        )
        .unwrap();
        assert!(heavy_commands(Some(&tmp)).unwrap_err().contains("heavy-commands"));
    }

    #[test]
    fn whom_a_process_is_reads_the_host_runner_first() {
        let read = OnHost { fleets: HashSet::from([10, 77]), runner: Some(77), keeper: Some(76) };
        assert_eq!(whom(&read, 10), Process::Fleets);
        assert_eq!(whom(&read, 77), Process::Runner, "started by a worker, it is still the runner");
        assert_eq!(whom(&read, 76), Process::Runner, "its keeper is the runner's as much");
        assert_eq!(whom(&read, 501), Process::Other);
    }
}
