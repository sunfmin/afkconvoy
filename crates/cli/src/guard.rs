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
/// (`hot::fleets`), and its host runner's pid where one is known.
struct OnHost {
    fleets: HashSet<u32>,
    runner: Option<u32>,
}

/// What `ps` and the host runner's lock say now. A worktree the fleet put
/// a worker in is one whose git dir keeps the note of one.
fn on_host() -> OnHost {
    let procs = crate::hot::processes();
    let ours = |worktree: &std::path::Path| {
        Worktree::at(worktree.to_path_buf(), String::new()).holds_a_worker()
    };
    OnHost { fleets: crate::hot::fleets(&procs, ours), runner: crate::host_runner::pid() }
}

/// Whom `pid` is on the host `read`: the host runner before all, as it
/// may itself have been started by a worker.
fn whom(read: &OnHost, pid: u32) -> Process {
    if read.runner == Some(pid) {
        Process::Runner
    } else if read.fleets.contains(&pid) {
        Process::Fleets
    } else {
        Process::Other
    }
}

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
        let read = OnHost { fleets: HashSet::from([10, 77]), runner: Some(77) };
        assert_eq!(whom(&read, 10), Process::Fleets);
        assert_eq!(whom(&read, 77), Process::Runner, "started by a worker, it is still the runner");
        assert_eq!(whom(&read, 501), Process::Other);
    }
}
