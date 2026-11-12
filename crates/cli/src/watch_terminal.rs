//! The watch's launch journal. Orca's title is a discovery hint, never an
//! ownership key. A creation intent precedes the external call; its shell
//! acknowledges its runtime handle before starting the guarded harness.

use std::path::PathBuf;

use afkconvoy_core::line::Line;
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{copies::cache, host_watch::Watch, orca::Worktree};

pub(crate) const TITLE: &str = "afkconvoy watch";
const FILE: &str = "watch/terminal.json";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LaunchState {
    Creating,
    Ready,
    Sending,
    Running,
    Idle,
    Ended,
}

/// The snapshot repairs a missing state/ID after a runner crash. Once the
/// state has a running watch, that watch remains authoritative for progress.
#[derive(Serialize, Deserialize)]
pub(crate) struct Record {
    pub watch: Watch,
    pub status: LaunchState,
    /// Public Orca metadata from our creation receipt or verified handshake.
    pub terminal: Option<Value>,
    pub problem: Option<String>,
    pub command: String,
    /// One durable Orca mutation identity per round, allocated before input.
    #[serde(default)]
    pub request: Option<String>,
    #[serde(default)]
    pub receipt: Option<Value>,
    #[serde(default)]
    pub offered: Option<u64>,
    #[serde(default)]
    pub execution_evidence: Option<Value>,
}

fn file() -> Result<PathBuf> {
    Ok(cache()?.join(FILE))
}

pub(crate) fn read() -> Result<Option<Record>> {
    let path = file()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("cannot read watch association {}", path.display()))?,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn save(record: &Record) -> Result<()> {
    let path = file()?;
    std::fs::create_dir_all(path.parent().expect("association parent"))?;
    let temporary = path.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(record)?)?;
    std::fs::rename(temporary, path)?;
    // Keep each round's delivery evidence after the resident journal moves
    // to another round. This is an archive of the journal, never a second
    // source from which the scheduler sends input.
    let round = crate::watch_report::directory(&record.watch.id)?;
    std::fs::create_dir_all(&round)?;
    let delivery = delivery_file(&record.watch)?;
    let temporary = delivery.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(&json!({
            "round": record.watch.id, "session": crate::host_watch::session_identity(&record.watch),
            "terminal": record.watch.terminal, "status": record.status,
            "request": record.request, "receipt": record.receipt,
            "offered": record.offered, "started": record.watch.started,
        "execution_evidence": record.execution_evidence,
        }))?,
    )?;
    std::fs::rename(temporary, delivery)?;
    Ok(())
}

fn delivery_file(watch: &Watch) -> Result<PathBuf> {
    Ok(crate::watch_report::directory(&watch.id)?
        .join(format!("delivery-{}.json", watch.attempts.len() + 1)))
}

pub(crate) fn delivery_json(watch: &Watch) -> Value {
    match delivery_file(watch).and_then(|path| Ok(serde_json::from_slice(&std::fs::read(path)?)?)) {
        Ok(value) => value,
        Err(_) => Value::Null,
    }
}

pub(crate) fn active() -> Result<Option<Watch>> {
    Ok(read()?.filter(|r| r.status != LaunchState::Ended).map(|r| r.watch))
}

fn handshake(watch: &Watch) -> PathBuf {
    watch.transcript.with_extension("terminal")
}

pub(crate) fn claim(watch: &Watch) -> PathBuf {
    watch.transcript.with_extension("claim")
}

pub(crate) fn matches(record: &Record, watch: &Watch) -> bool {
    crate::host_watch::session_identity(&record.watch) == crate::host_watch::session_identity(watch)
}

/// Refuses unknown same-name sessions before creating anything. The host
/// runner's host lock serializes callers; the shell's mkdir also fences a
/// duplicated external create request from running a second harness.
pub(crate) fn prepare(watch: &Watch, command: &str) -> Result<()> {
    ensure!(active()?.is_none(), "a watch launch is still unresolved; no second agent started");
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    let candidates: Vec<_> =
        worktree.watch_terminals()?.into_iter().filter(|t| t["title"] == TITLE).collect();
    ensure!(
        candidates.is_empty(),
        "unverified terminals named {TITLE}: {}; no agent started",
        candidates
            .iter()
            .map(|t| t["handle"].as_str().unwrap_or("unknown"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    save(&Record {
        watch: watch.clone(),
        status: LaunchState::Creating,
        terminal: None,
        problem: None,
        command: command.into(),
        request: None,
        receipt: None,
        offered: None,
        execution_evidence: None,
    })
}

/// Wraps the existing guarded command without interpreting or replacing it.
/// mkdir is intentionally retained after exit: this launch may run once.
/// A missing Orca handle cannot acknowledge ownership and runs no harness.
pub(crate) fn launch_line(watch: &Watch, line: &str) -> String {
    let claim = claim(watch).display().to_string();
    let receipt = handshake(watch).display().to_string();
    let temporary = format!("{receipt}.tmp");
    Line::new()
        .fixed("if mkdir ").word(&claim)
        .fixed("; then if [ -n \"${ORCA_TERMINAL_HANDLE-}\" ]; then printf '%s\\n' \"$ORCA_TERMINAL_HANDLE\" > ")
        .word(&temporary).fixed(" && mv ").word(&temporary).fixed(" ").word(&receipt)
        .fixed(" && { ").launch(line)
        .fixed("; }; else printf '%s\\n' 'watch launch has no ORCA_TERMINAL_HANDLE; recovery required' >&2; fi; else printf '%s\\n' 'watch launch already claimed; no second agent started' >&2; fi")
        .done()
}

pub(crate) fn created(watch: &Watch, handle: &str) -> Result<()> {
    let mut record = read()?.context("watch launch has no intent")?;
    ensure!(matches(&record, watch), "watch launch intent changed");
    // Query public metadata rather than assuming titles or runtime handles
    // survive an Orca restart. Retain the intent if this query fails.
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    let rows = worktree.watch_terminals()?;
    let row = rows
        .into_iter()
        .find(|row| row["handle"] == handle)
        .context("created watch terminal is not listed; recovery required")?;
    record.watch.terminal = handle.into();
    record.terminal = Some(row);
    record.status = LaunchState::Ready;
    save(&record)
}

/// Exact saved handle first, then stable public pane identity or the shell's
/// own acknowledgement. Unknown or multiple discovery candidates stay open.
/// None means a previously confirmed terminal is now absent.
pub(crate) fn locate(watch: &Watch) -> Result<Option<String>> {
    let worktree = Worktree::at(watch.directory.clone(), String::new());
    let rows = worktree.watch_terminals()?;
    let record = read()?.filter(|record| matches(record, watch));
    let metadata = record.as_ref().and_then(|r| r.terminal.as_ref());
    let exact = if watch.terminal.is_empty() {
        record.as_ref().map_or("", |r| r.watch.terminal.as_str())
    } else {
        &watch.terminal
    };
    if !exact.is_empty()
        && let Some(row) = rows.iter().find(|row| row["handle"] == exact)
    {
        if let Some(saved) = metadata
            && reincarnated(saved, row)
        {
            return retire(&worktree, record.expect("metadata is the record's"), row);
        }
        if let Some(saved) = metadata {
            for key in ["ptyId", "incarnationId"] {
                ensure!(
                    saved[key].is_null() || saved[key] == row[key],
                    "watch terminal {} changed {key}; ownership is unverified",
                    watch.terminal
                );
            }
        }
        if record.is_none() {
            // An upgrade may retain a pre-journal watch. Its persisted
            // exact handle is afkconvoy's ownership evidence; capture the
            // public identity now, without changing its running session.
            save(&Record {
                watch: watch.clone(),
                status: LaunchState::Running,
                terminal: Some(row.clone()),
                problem: None,
                command: String::new(),
                request: None,
                receipt: None,
                offered: None,
                execution_evidence: None,
            })?;
        }
        return Ok(Some(exact.to_string()));
    }
    let acknowledged =
        claim(watch).is_dir().then(|| std::fs::read_to_string(handshake(watch)).ok()).flatten();
    let candidates: Vec<_> = rows
        .iter()
        .filter(|row| {
            row["title"] == TITLE
                || metadata.is_some_and(|saved| {
                    !saved["ptyId"].is_null() && saved["ptyId"] == row["ptyId"]
                })
                || acknowledged.as_ref().is_some_and(|handle| row["handle"] == handle.trim())
        })
        .collect();
    if candidates.is_empty() {
        if !exact.is_empty() {
            return Ok(None);
        }
        bail!("watch creation has no receipt or confirmed terminal; creation will not be repeated");
    }
    ensure!(
        candidates.len() == 1,
        "multiple watch terminal candidates: {}; recovery required",
        candidates
            .iter()
            .map(|r| r["handle"].as_str().unwrap_or("unknown"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let row = candidates[0];
    let acknowledged_here =
        acknowledged.as_ref().is_some_and(|handle| row["handle"] == handle.trim());
    if !acknowledged_here
        && let Some(saved) = metadata
        && reincarnated(saved, row)
    {
        return retire(&worktree, record.expect("metadata is the record's"), row);
    }
    let owned = metadata.is_some_and(|saved| {
        !saved["ptyId"].is_null()
            && saved["ptyId"] == row["ptyId"]
            && saved["incarnationId"] == row["incarnationId"]
    }) || acknowledged_here;
    ensure!(owned, "terminal {} is named {TITLE} but its ownership is unverified", row["handle"]);
    let handle = row["handle"].as_str().context("watch terminal has no handle")?.to_string();
    if let Some(mut record) = record {
        record.watch.terminal = handle.clone();
        record.terminal = Some(row.clone());
        if record.status == LaunchState::Creating {
            record.status = LaunchState::Ready;
        }
        save(&record)?;
    }
    Ok(Some(handle))
}

/// Our confirmed pane, by its saved ptyId, under a new Orca incarnation:
/// Orca restarted or restored it. Its resident harness, screen and input
/// are not that incarnation's, and a saved request key binds the old one.
fn reincarnated(saved: &Value, row: &Value) -> bool {
    !saved["ptyId"].is_null()
        && saved["ptyId"] == row["ptyId"]
        && !saved["incarnationId"].is_null()
        && saved["incarnationId"] != row["incarnationId"]
}

/// Ends a re-incarnated resident: its marked executor first, wherever it
/// survived, then its pane, which is ours by ptyId and would otherwise
/// block the next creation by its title. The journal records the session
/// ended, so its confirmed terminal is absent (None) and a fresh watch
/// may start (ADR-0070). Nothing is adopted and nothing is resent.
fn retire(worktree: &Worktree, mut record: Record, row: &Value) -> Result<Option<String>> {
    crate::host_watch::stop_marked(&crate::host_watch::session_identity(&record.watch))?;
    let handle = row["handle"].as_str().context("watch terminal has no handle")?;
    worktree.close_terminal(handle)?;
    ensure!(
        !worktree.live(handle)?,
        "the re-incarnated watch terminal {handle} is still live after it was closed"
    );
    record.status = LaunchState::Ended;
    record.problem = None;
    save(&record)?;
    Ok(None)
}

pub(crate) fn set_status(watch: &Watch, status: LaunchState) -> Result<()> {
    if let Some(mut record) = read()?.filter(|r| matches(r, watch)) {
        record.watch = watch.clone();
        record.status = status;
        record.problem = None;
        save(&record)?;
    }
    Ok(())
}

pub(crate) fn begin_round(watch: &Watch) -> Result<()> {
    let mut record = read()?.context("the resident watch has no launch journal")?;
    ensure!(
        matches(&record, watch) && record.status == LaunchState::Idle,
        "the resident session has an unresolved round; nothing sent"
    );
    record.watch = watch.clone();
    record.status = LaunchState::Ready;
    record.request = None;
    record.receipt = None;
    record.offered = None;
    record.execution_evidence = None;
    save(&record)
}

pub(crate) fn problem(why: &str) -> Result<()> {
    if let Some(mut record) = read()? {
        record.problem = Some(why.into());
        save(&record)?;
    }
    Ok(())
}

pub(crate) fn recovery(why: &str) -> String {
    format!(
        "{why}. No replacement is started and existing work is retained. Run `afkconvoy doctor --all` and `orca terminal list --worktree path:<watch-directory> --json`; inspect candidates with `orca terminal show --terminal <handle> --json`. Rename unrelated same-name terminals with `orca terminal rename --terminal <handle> --title <title> --json`, then let the runner retry. For an uncertain launch or prompt, inspect the existing session and submit its complete report using `afkconvoy watched --watch <id> --text-file <report.json>`; this completes only that round and retains a healthy resident executor. An unresolved association still pauses further delivery. Keep the brief, transcript and launch claim; do not resend creation or send a fresh prompt. The runner may replay only its exact saved Terminal request key."
    )
}

pub(crate) fn json() -> Value {
    match read() {
        Ok(Some(record)) => json!({"title": TITLE, "watch": record.watch.id,
            "session": crate::host_watch::session_identity(&record.watch),
            "directory": record.watch.directory, "terminal": record.watch.terminal,
            "identity": record.terminal, "status": record.status, "problem": record.problem,
            "request": record.request, "receipt": record.receipt}),
        Ok(None) => json!({"title": TITLE, "status": "unassociated"}),
        Err(error) => {
            json!({"title": TITLE, "status": "uncertain", "problem": recovery(&format!("{error:#}"))})
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    fn watch(directory: PathBuf) -> Watch {
        serde_json::from_value(json!({"id": format!("smoke-{}", crate::unique()),
            "directory": directory, "look": {}, "items": [], "started": crate::clock(),
            "harness": "claude-code", "model": null, "terminal": "",
            "transcript": directory.join("session.log")}))
        .unwrap()
    }

    #[test]
    fn the_real_launch_shell_claims_execution_once_even_with_concurrent_duplicate_calls() {
        let dir = tempfile::tempdir().unwrap();
        let watch = watch(dir.path().into());
        let output = dir.path().join("executions");
        let line =
            Line::new().fixed("printf 'executed\\n' >> ").word(&output.display().to_string()).own();
        let line = launch_line(&watch, &line);
        let spawn = || {
            Command::new("sh")
                .args(["-c", &line])
                .env("ORCA_TERMINAL_HANDLE", "own-terminal")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        };
        let mut first = spawn();
        let mut duplicate = spawn();
        first.wait().unwrap();
        duplicate.wait().unwrap();
        assert_eq!(std::fs::read_to_string(output).unwrap(), "executed\n");
        assert_eq!(std::fs::read_to_string(handshake(&watch)).unwrap(), "own-terminal\n");
    }

    #[test]
    fn a_shell_without_an_orca_handle_never_starts_an_unrecoverable_executor() {
        let dir = tempfile::tempdir().unwrap();
        let watch = watch(dir.path().into());
        let output = dir.path().join("executions");
        let line = Line::new().fixed("touch ").word(&output.display().to_string()).own();
        Command::new("sh")
            .args(["-c", &launch_line(&watch, &line)])
            .env_remove("ORCA_TERMINAL_HANDLE")
            .output()
            .unwrap();
        assert!(!output.exists());
        assert!(!handshake(&watch).exists());
    }

    /// Opt in from a worktree registered with the real Orca. This exercises
    /// the production creation/handshake/lookup path with a harmless marked
    /// interactive executor, without asking a model to operate the host.
    #[test]
    #[ignore = "requires visible local Orca; creates two temporary terminals in the current worktree"]
    fn real_orca_recovers_a_visible_terminal_and_preserves_input_while_refusing_ambiguity() {
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let mut watch = watch(directory.clone());
        watch.transcript = cache().unwrap().join("smoke-session.log");
        let worktree = Worktree::at(directory, String::new());
        let marker = format!("afkconvoy-watch-session-{}-1", watch.id);
        let command = afkconvoy_core::host_watch::session(
            "export AFKCONVOY_WATCH=host; printf 'watch recovery smoke: ready\\n'; while IFS= read -r input; do printf 'retained input: %s\\n' \"$input\"; done",
            &watch.transcript.display().to_string(),
            &format!("{}-1", watch.id),
        );
        prepare(&watch, &command).unwrap();
        let terminal = worktree.start_watch(&launch_line(&watch, &command), TITLE).unwrap();
        // Deliberately discard the create receipt. Only the startup
        // acknowledgement is allowed to associate this candidate.
        let until = Instant::now() + Duration::from_secs(15);
        while !handshake(&watch).exists() {
            assert!(Instant::now() < until, "no terminal handshake");
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(locate(&watch).unwrap().unwrap(), terminal);
        watch.terminal = terminal.clone();
        set_status(&watch, LaunchState::Running).unwrap();
        let sent = Command::new("orca")
            .args([
                "terminal",
                "send",
                "--terminal",
                &terminal,
                "--text",
                "human progress",
                "--enter",
                "--json",
            ])
            .output()
            .unwrap();
        assert!(sent.status.success());
        // A replacement host runner reconstructs solely from on-disk state.
        let recovered = active().unwrap().unwrap();
        assert_eq!(locate(&recovered).unwrap().unwrap(), terminal);
        let processes = crate::hot::listed_checked().unwrap();
        let executors: Vec<_> = processes
            .iter()
            .filter(|p| p.line.split_whitespace().any(|word| word == marker))
            .collect();
        assert_eq!(executors.len(), 1, "one marked executor");
        let mut screen = vec![];
        while Instant::now() < until {
            screen = worktree.screen(&terminal, 80).unwrap();
            if screen.iter().any(|line| line.contains("retained input: human progress")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(
            screen.iter().any(|line| line.contains("retained input: human progress")),
            "{screen:?}"
        );
        let other =
            worktree.start_watch("printf 'unrelated smoke session\\n'; sleep 120", TITLE).unwrap();
        // Exact ID wins even alongside a same-title operator terminal.
        assert_eq!(locate(&recovered).unwrap().unwrap(), terminal);
        // Without the exact receipt, discovery must pause on the two names.
        let mut record = read().unwrap().unwrap();
        record.watch.terminal.clear();
        save(&record).unwrap();
        let mut lost = recovered;
        lost.terminal.clear();
        assert!(
            locate(&lost).unwrap_err().to_string().contains("multiple watch terminal candidates")
        );
        assert!(worktree.live(&other).unwrap());
        println!(
            "real Orca: terminal={terminal}, unrelated={other}, executor_pid={}, marker={marker}; input retained; lost creation reply recovered; exact lookup and runner reconstruction reused the session; ambiguous discovery paused",
            executors[0].pid
        );
        worktree.close_terminal(&other).unwrap();
        worktree.close_terminal(&terminal).unwrap();
        set_status(&watch, LaunchState::Ended).unwrap();
    }
}
