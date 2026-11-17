//! A promotion (docs/adr/0090): the base branch fast-forwarded to the
//! proving branch's tip once the slow gate passed there. The proving branch
//! holds every round that landed on it fast-green; the base branch moves
//! only by this.
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Proven, Tree};
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::acting::Acting;
use crate::gate::{Detached, Gated};
use crate::git::Repo;
use crate::remote::Fresh;

/// The proving branch and the base branch as one read of the remote saw
/// them, and what landed on the one and is not yet on the other.
pub struct Standing {
    pub base_branch: String,
    pub proving_branch: String,
    pub base: String,
    pub tip: String,
    /// The PRs landed on the proving branch past the base branch's tip.
    pub unpromoted: u64,
    /// When the oldest of them landed, in seconds since the epoch.
    pub oldest: Option<u64>,
    /// Whether the proving branch's tip is a fast-forward of the base
    /// branch's: the one move a promotion makes.
    pub fast_forward: bool,
}

impl Standing {
    /// Where `config`'s proving branch and base branch stand on the remote
    /// now; `None` where it declares no proving branch.
    pub fn read(
        repo: &Repo,
        config: &Config,
        tracker: &afkconvoy_core::tracker::Tracker,
    ) -> Result<Option<Standing>> {
        let Some(proving) = config.proving_branch.clone() else { return Ok(None) };
        let base_ref = format!("refs/heads/{}", config.base_branch);
        let proving_ref = format!("refs/heads/{proving}");
        let refs = repo.remote().refs(&[&base_ref, &proving_ref], Fresh::Live)?;
        let found = |name: &str| refs.iter().find(|r| r.name == name).map(|r| r.sha.clone());
        let base = found(&base_ref)
            .ok_or_else(|| anyhow!("the remote has no branch {:?}", config.base_branch))?;
        let tip = found(&proving_ref).ok_or_else(|| {
            anyhow!(
                "the remote has no proving branch {proving:?}: push it from the base branch \
                 (`git push origin {}:refs/heads/{proving}`)",
                config.base_branch
            )
        })?;
        repo.bring(&base, &base_ref)?;
        repo.bring(&tip, &proving_ref)?;
        let fast_forward = repo.within(&base, &tip)?;
        let landed = match fast_forward && base != tip {
            true => crate::landing::joins_in(repo, tracker, &[&format!("{base}..{tip}")])?,
            false => vec![],
        };
        let mut oldest = None;
        if let Some(first) = landed.first() {
            let at = repo.git(&["show", "-s", "--format=%ct", &first.commit])?;
            oldest = at.trim().parse().ok();
        }
        Ok(Some(Standing {
            base_branch: config.base_branch.clone(),
            proving_branch: proving,
            base,
            tip,
            unpromoted: landed.len() as u64,
            oldest,
            fast_forward,
        }))
    }

    /// Whether the proving branch holds anything the base branch does not.
    pub fn ahead(&self) -> bool {
        self.base != self.tip
    }

    /// What `doctor` and `plan` say of it.
    pub fn json(&self, now: u64) -> Value {
        json!({
            "proving_branch": self.proving_branch,
            "proving_tip": self.tip,
            "base_branch": self.base_branch,
            "base_tip": self.base,
            "unpromoted": self.unpromoted,
            "oldest_waited_seconds": self.oldest.map(|at| now.saturating_sub(at)),
            "fast_forward": self.fast_forward,
        })
    }

    /// The same in one line.
    pub fn line(&self, now: u64) -> String {
        let short = |sha: &str| sha.get(..12).unwrap_or(sha).to_string();
        let mut said = format!(
            "proving branch {} at {}, base branch {} at {}",
            self.proving_branch,
            short(&self.tip),
            self.base_branch,
            short(&self.base)
        );
        if !self.fast_forward {
            said.push_str(": not a fast-forward of the base branch, so no promotion moves it");
            return said;
        }
        said.push_str(&format!("; {} landed PR(s) not yet promoted", self.unpromoted));
        if let Some(at) = self.oldest {
            said.push_str(&format!(", the oldest waiting {}s", now.saturating_sub(at)));
        }
        said
    }
}

/// `afkconvoy promote`: gates the proving branch's tip with its slow gate —
/// its whole gate where no fast run of that tree is on record — and, green,
/// fast-forwards the base branch to it. Nothing else ever moves the base
/// branch of a repo with a proving branch.
pub fn promote(repo: &Repo, host: &str, now: u64) -> Result<Value> {
    let config = crate::config::of(repo)?;
    let tracker = crate::gh::GitHub::of(repo)?.tracker;
    let Some(standing) = Standing::read(repo, &config, &tracker)? else {
        return Ok(json!({
            "outcome": "none",
            "detail": "this repo declares no proving_branch: it lands on its base branch, and \
                       nothing is promoted (afkconvoy config --help)",
        }));
    };
    let mut answer = standing.json(now);
    if !standing.ahead() {
        answer["outcome"] = json!("current");
        answer["detail"] = json!("the base branch is at the proving branch's tip");
        return Ok(answer);
    }
    if !standing.fast_forward {
        answer["outcome"] = json!("diverged");
        answer["detail"] = json!(format!(
            "the base branch {} is not an ancestor of the proving branch's tip: a promotion \
             never moves it to a commit that is not a fast-forward of it; it stays at {}",
            standing.base_branch, standing.base
        ));
        say(repo, &answer);
        return Ok(answer);
    }
    let Some(_lock) = Lock::take(repo)? else {
        answer["outcome"] = json!("promoting");
        answer["detail"] = json!("another promotion of this checkout runs; it answers for it");
        return Ok(answer);
    };
    let tip = standing.tip.clone();
    // The tip's own declaration, as a landing on the base branch reads it.
    let declared = crate::turn::tip_config(repo, &tip, &standing.base_branch)?;
    let whole = GateCommand::new(&declared.gate_command);
    let tree = Tree::named(&repo.git(&["rev-parse", &format!("{tip}^{{tree}}")])?);
    let output = repo.common_dir()?.join(format!("convoy-promotion-{now}-{}.log", crate::unique()));
    let (gated, ran) = gate(repo, &declared, &tip, &tree, host, now, &output)?;
    answer["gate"] = json!(ran);
    answer["gate_command"] = json!(declared.gate_command);
    match gated {
        Gated::Green { run, .. } => {
            let _ = std::fs::remove_file(&output);
            let proven = Proven::by(&tree, &whole, &run, now)
                .ok_or_else(|| anyhow!("the whole gate's record of {tip} does not stand"))?;
            if !crate::landing::push_base(repo, &standing.base_branch, &tip, &proven)? {
                answer["outcome"] = json!("target_moved");
                answer["detail"] = json!(format!(
                    "the remote refused the fast-forward of {} to {tip}: it moved meanwhile; \
                     the next promotion reads it again",
                    standing.base_branch
                ));
            } else {
                answer["outcome"] = json!("promoted");
                answer["detail"] = json!(format!(
                    "fast-forwarded {} from {} to {tip}, {} landed PR(s)",
                    standing.base_branch, standing.base, standing.unpromoted
                ));
            }
        }
        Gated::Red { red, .. } => {
            answer["outcome"] = json!("gate_red");
            answer["failing"] = json!(red.tests);
            answer["gate_output"] = json!(output);
            answer["detail"] = json!(format!(
                "{}; the base branch {} stays at {}",
                red.said(&format!("the proving branch's tip {tip}")),
                standing.base_branch,
                standing.base
            ));
        }
        Gated::Unsettled { unsettled, .. } => {
            answer["outcome"] = json!("gate_unsettled");
            answer["gate_output"] = json!(output);
            answer["detail"] = json!(format!(
                "the proving branch's tip {tip} is unsettled: {}; nothing is recorded, and the \
                 base branch {} stays at {}",
                unsettled.how(),
                standing.base_branch,
                standing.base
            ));
        }
    }
    say(repo, &answer);
    Ok(answer)
}

/// Gates the proving branch's tip `tip` whole, in a worktree of its own: a
/// green run of the whole gate on record answers; else, where the fast
/// gate's run of the tree is on record, its slow gate runs and the two are
/// joined into a record of the whole gate (docs/adr/0048's join); else the
/// whole gate runs → what it answered, and which of those it was.
fn gate(
    repo: &Repo,
    declared: &Config,
    tip: &str,
    tree: &Tree,
    host: &str,
    now: u64,
    output: &std::path::Path,
) -> Result<(Gated, &'static str)> {
    let whole = GateCommand::new(&declared.gate_command);
    if let Some(run) = crate::gate::recorded(repo, tree, &whole, now)? {
        return Ok((Gated::Green { run, source: "recorded" }, "recorded"));
    }
    let fast = declared.gates.as_ref().map(|gates| {
        (GateCommand::new(&gates.fast.command), GateCommand::new(&gates.slow.command), gates)
    });
    let joined = match &fast {
        Some((fast, _, _)) => crate::gate::recorded(repo, tree, fast, now)?.is_some(),
        None => false,
    };
    let (command, seconds, ran) = match (&fast, joined) {
        (Some((_, _, gates)), true) => {
            (GateCommand::new(&gates.slow.command), gates.slow.seconds, "slow")
        }
        _ => (GateCommand::new(&declared.gate_command), declared.gate_seconds(), "whole"),
    };
    let limit = Duration::from_secs(seconds);
    let (gated, removal) = crate::gate::detached(repo, tip, Detached::Promotion, |checkout| {
        crate::gate::gated(checkout, &command, host, now, limit, None, Some(output))
    })?;
    if let Some(error) = removal {
        eprintln!("warning: {error:#}");
    }
    let gated = gated?;
    if let (Gated::Green { .. }, Some((fast, slow, _)), true) = (&gated, &fast, joined) {
        if let Some(why) =
            crate::gate::record_whole(repo, tree, tip, (&whole, fast, slow), None, now)?
        {
            bail!("the slow gate passed on {tip}, but the whole gate is not on record: {why}");
        }
        let run = crate::gate::recorded(repo, tree, &whole, now)?
            .context("the whole gate's record was written but does not read back")?;
        return Ok((Gated::Green { run, source: "joined" }, ran));
    }
    Ok((gated, ran))
}

/// Says in the fleet's log how a promotion ended: what `run` echoes, a line
/// of the tick log.
fn say(repo: &Repo, answer: &Value) {
    let outcome = answer["outcome"].as_str().unwrap_or("");
    let detail = answer["detail"].as_str().unwrap_or("");
    Acting::new("promote")
        .in_checkout(Some(repo.clone()))
        .say(crate::clock(), format!("promotion {outcome}: {detail}"));
}

/// One promotion of a checkout at a time: a lock on a file in its git dir,
/// let go as the process ends, whatever ends it.
struct Lock(#[allow(dead_code)] std::fs::File);

impl Lock {
    fn file(repo: &Repo) -> Result<PathBuf> {
        Ok(repo.common_dir()?.join("convoy-promote.lock"))
    }

    /// The lock, or `None` while another process holds it.
    fn take(repo: &Repo) -> Result<Option<Lock>> {
        let path = Lock::file(repo)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("cannot open {}", path.display()))?;
        // SAFETY: flock only reads the descriptor this function owns.
        let took = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        Ok(took.then_some(Lock(file)))
    }

    /// Whether a promotion of the checkout runs now.
    fn held(repo: &Repo) -> bool {
        match Lock::take(repo) {
            Ok(Some(_)) => false,
            Ok(None) => true,
            Err(_) => false,
        }
    }
}

/// What a tick does of the proving branch (docs/adr/0090): where it is ahead
/// of the base branch, the tick starts a promotion of its own, detached from
/// it, unless one runs → whether it started one. Its answer goes to the
/// checkout's promotion log; how it ended is said in the fleet's log.
pub fn tend(
    repo: &Repo,
    config: &Config,
    tracker: &afkconvoy_core::tracker::Tracker,
) -> Result<bool> {
    let Some(standing) = Standing::read(repo, config, tracker)? else { return Ok(false) };
    if !standing.ahead() || !standing.fast_forward || Lock::held(repo) {
        return Ok(false);
    }
    let log = repo.common_dir()?.join("convoy-promotion.log");
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("cannot open {}", log.display()))?;
    let program = std::env::current_exe().context("cannot find this afkconvoy")?;
    let mut child = crate::process::spawn(
        Command::new(program)
            .arg("promote")
            .arg(&repo.dir)
            .current_dir(&repo.dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log),
    )
    .context("cannot start a promotion")?;
    // Reaped once it ends; nothing waits on its answer here.
    std::thread::spawn(move || child.wait());
    Ok(true)
}

/// What `afkconvoy promote --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Any time, in a checkout of a repo whose config names a `proving_branch` (`afkconvoy config --help`). A tick runs it by itself, detached from the tick, whenever the proving branch is ahead of the base branch and no promotion of the checkout runs; run it by hand to promote now.

## Who runs it

The fleet, at its ticks, and the operator.

## What it does

In a repo with a proving branch, a round lands on the proving branch once its fast gate is green: its root merged onto the proving branch's tip and fast-gated only, each PR answering `merged` and its issue closing (`afkconvoy land --help`). The base branch moves only by a promotion, which is this command:

1. It reads the base branch's tip and the proving branch's tip on the remote. The same commit: `current`, nothing to do. The base branch's tip not an ancestor of the proving branch's: `diverged`, and nothing moves — a promotion never moves the base branch to a commit that is not a fast-forward of it.
2. It gates the proving branch's tip, in a worktree of its own detached at it, by the gate that tip's config declares. A green run of `gate_command` on record for its tree answers. Else, where the fast gate's green run of that tree is on record — the last round's landing put it there — it runs the slow gate alone and puts `gate_command` on record from the two runs, as at a round's tip (ADR-0048). Else it runs `gate_command` whole.
3. Green: it pushes the proving branch's tip to the base branch, a plain push the remote refuses unless it is a fast-forward: `promoted`. Red: the base branch stays where it was, and the answer and the fleet's log say so: `gate_red`, with the failing tests and the run's whole output. A run red only on TIMEOUTs the gate cannot classify on an overloaded host: `gate_unsettled`, nothing recorded.

One promotion of a checkout runs at a time: another answers `promoting`. A repo with no proving branch answers `none`.

## What it reads and writes

It reads the remote's base branch and proving branch and the gate records, and writes a gate record of the tip's tree and, green, the base branch. Its run's output is in the checkout's git dir (`convoy-promotion-<time>-<n>.log`), kept where it is red; a tick's promotion appends its answer to `convoy-promotion.log` there.

## What it answers

{"outcome", "detail", "proving_branch", "proving_tip", "base_branch", "base_tip", "unpromoted", "oldest_waited_seconds", "fast_forward"}, and once it gated, {"gate"} — `recorded`, `slow` or `whole`, what ran — and {"gate_command"}; red, {"failing"}, the tests its output names failing, and {"gate_output"}, the file that keeps it. `outcome` is one of: none, current, diverged, promoting, promoted, gate_red, gate_unsettled, target_moved — the remote refused the fast-forward because the base branch moved meanwhile.

## What trips you up

- A red promotion is not repaired in this version: fast-green PRs keep landing on the proving branch, and the base branch waits until a promotion of a later tip is green.
- A repo that moves its base branch by hand, past the proving branch, is `diverged` until the proving branch holds the base branch's tip again."#;
