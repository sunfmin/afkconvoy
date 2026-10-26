//! Remote candidate ownership. Every write extends one ref; a competing
//! writer's child cannot fast-forward it. Publishing the base and the next
//! record is one atomic push, so invalidation and publication cannot both win.
use afkconvoy_core::candidate::{Candidate, Chain, Phase, Process, Publication};
use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Proven, Tree};
use afkconvoy_core::landing::{Of, merge_message};
use afkconvoy_core::lock::keeps_off;
use afkconvoy_core::plan::PullRequest;
use anyhow::{Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::process::Command;
use std::time::Duration;

use crate::exclusive::Permit;
use crate::gh::{GitHub, Tracker};
use crate::git::{REMOTE, Repo};

const EVERY: Duration = Duration::from_millis(100);

fn refname(repo: &Repo, config: &Config) -> String {
    repo.layout().under(&format!("candidates/{}", config.base_branch))
}

pub(crate) struct Snapshot {
    sha: Option<String>,
    base: String,
    pub chain: Chain,
}

fn read(repo: &Repo, config: &Config) -> Result<Snapshot> {
    let state_ref = refname(repo, config);
    let base_ref = format!("refs/heads/{}", config.base_branch);
    let refs = repo.git(&["ls-remote", REMOTE, &state_ref, &base_ref])?;
    let found = |name: &str| {
        refs.lines()
            .filter_map(|line| line.split_once('\t'))
            .find(|(_, r)| *r == name)
            .map(|(sha, _)| sha.to_string())
    };
    let sha = found(&state_ref);
    let base = found(&base_ref).ok_or_else(|| anyhow!("base branch disappeared"))?;
    repo.bring(&base, &base_ref)?;
    if let Some(sha) = &sha {
        repo.bring(sha, &state_ref)?;
    }
    let chain = match &sha {
        Some(sha) => serde_json::from_str(&repo.git(&["show", "-s", "--format=%B", sha])?)?,
        None => Chain::default(),
    };
    Ok(Snapshot { sha, base, chain })
}

/// No force push, including the state ref. Its first parent is the previous
/// record. Extra parents retain every candidate and its preserved source head.
fn commit(repo: &Repo, snapshot: &Snapshot) -> Result<String> {
    let tree = repo.git(&["hash-object", "-t", "tree", "/dev/null"])?;
    let text = serde_json::to_string(&snapshot.chain)?;
    let mut args = vec!["commit-tree", &tree, "-m", &text];
    if let Some(sha) = &snapshot.sha {
        args.extend(["-p", sha]);
    }
    for c in &snapshot.chain.candidates {
        args.extend(["-p", &c.commit]);
    }
    repo.git(&args)
}

fn write(repo: &Repo, config: &Config, snapshot: &Snapshot) -> Result<bool> {
    let next = commit(repo, snapshot)?;
    let pushed =
        repo.try_git(&["push", "--quiet", REMOTE, &format!("{next}:{}", refname(repo, config))])?;
    if pushed.ok {
        return Ok(true);
    }
    // A remote fault is not contention. Never spin on a rejected record.
    if repo.remote_sha(&refname(repo, config))? == snapshot.sha {
        bail!("candidate record could not be written: {}; run land again", pushed.stderr.trim());
    }
    Ok(false)
}

fn change(repo: &Repo, config: &Config, mut edit: impl FnMut(&mut Chain)) -> Result<Chain> {
    loop {
        let mut snapshot = read(repo, config)?;
        let before = snapshot.chain.clone();
        edit(&mut snapshot.chain);
        if before == snapshot.chain || write(repo, config, &snapshot)? {
            return Ok(snapshot.chain);
        }
    }
}

pub(crate) fn view(repo: &Repo, config: &Config) -> Result<Value> {
    let snapshot = read(repo, config)?;
    let rows: Vec<_> = snapshot
        .chain
        .candidates
        .iter()
        .map(|c| {
            let mut row = serde_json::to_value(c).expect("candidate is serializable");
            row["waiting"] = json!(c.waiting());
            row
        })
        .collect();
    Ok(json!(rows))
}

/// `ps` must answer successfully. An unreadable observation never proves
/// death. Start time prevents a recycled PID from owning or being killed for
/// an earlier candidate. Host identity is physical, independent of --host.
pub(crate) fn identity(pid: u32) -> Result<Option<Process>> {
    let out = crate::process::output(Command::new("ps").args([
        "-p",
        &pid.to_string(),
        "-o",
        "lstart=,stat=",
    ]))?;
    if !out.status.success() {
        ensure!(
            out.status.code() == Some(1) && out.stdout.is_empty(),
            "cannot confirm process {pid}: ps failed"
        );
        return Ok(None);
    }
    let line = String::from_utf8_lossy(&out.stdout);
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() < 6 {
        bail!("cannot confirm process {pid}: ps gave no identity");
    }
    if fields[5].starts_with('Z') {
        return Ok(None);
    }
    Ok(Some(Process { host: crate::this_host(), pid, started: fields[..5].join(" ") }))
}

fn alive(process: &Process) -> Result<bool> {
    if process.host != crate::this_host() {
        return Ok(true);
    }
    Ok(identity(process.pid)?.as_ref() == Some(process))
}

/// Kill a registered group and escaped descendants, freezing before walking
/// children. The root stays stopped until all known descendants have exited.
/// If interrupted, a later owner walks the still-stopped root again.
pub(crate) fn terminate(gate: &Process) -> Result<()> {
    if gate.host != crate::this_host() {
        return Ok(());
    }
    if let Some(current) = identity(gate.pid)?
        && current != *gate
    {
        return Ok(());
    }
    // A live process group retains its ID even if its original leader exited.
    // SAFETY: this group was created for the recorded candidate gate alone.
    unsafe {
        libc::kill(-(gate.pid as i32), libc::SIGSTOP);
    }
    let mut stopped = vec![gate.pid];
    loop {
        let all = crate::hot::listed_checked()?;
        let children: Vec<_> = all
            .iter()
            .filter(|p| stopped.contains(&p.ppid) && !stopped.contains(&p.pid))
            .map(|p| p.pid)
            .collect();
        if children.is_empty() {
            break;
        }
        for pid in children {
            // SAFETY: a descendant of the stopped group, which cannot fork.
            unsafe {
                libc::kill(pid as i32, libc::SIGSTOP);
            }
            stopped.push(pid);
        }
    }
    for pid in stopped.iter().rev().filter(|pid| **pid != gate.pid) {
        // SAFETY: the process was discovered under the frozen gate above.
        unsafe {
            libc::kill(*pid as i32, libc::SIGKILL);
        }
    }
    // SAFETY: end all remaining group members, then the stopped root.
    unsafe {
        libc::kill(-(gate.pid as i32), libc::SIGKILL);
        libc::kill(gate.pid as i32, libc::SIGKILL);
    }
    let began = std::time::Instant::now();
    loop {
        let out = crate::process::output(Command::new("ps").args(["-axo", "pid=,pgid=,stat="]))?;
        ensure!(out.status.success(), "cannot confirm candidate gate exit: ps failed");
        let text = String::from_utf8_lossy(&out.stdout);
        let live = text.lines().any(|line| {
            let f: Vec<_> = line.split_whitespace().collect();
            f.len() == 3
                && !f[2].starts_with('Z')
                && (f[1].parse::<u32>().ok() == Some(gate.pid)
                    || f[0].parse::<u32>().ok().is_some_and(|pid| stopped.contains(&pid)))
        });
        if !live {
            break;
        }
        ensure!(
            began.elapsed() < Duration::from_secs(10),
            "candidate gate exit is unconfirmed; its lane stays occupied"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

/// Invalidation is durable BEFORE signalling any dependent gate. Every
/// caller may complete interrupted cleanup, but no caller may resurrect it.
fn cleanup(repo: &Repo, config: &Config, chain: &Chain) -> Result<()> {
    for c in
        chain.candidates.iter().filter(|c| !c.stopped && !c.valid() && c.phase != Phase::Landed)
    {
        if c.owner.host != crate::this_host() {
            continue;
        }
        if let Some(gate) = &c.gate {
            terminate(gate)?;
        } else if alive(&c.owner)? {
            continue;
        }
        change(repo, config, |state| {
            for next in &mut state.candidates {
                if next.id == c.id && !next.valid() {
                    next.stopped = true;
                    if next.phase == Phase::Cancelling {
                        next.phase = Phase::Cancelled;
                    }
                }
            }
        })?;
    }
    Ok(())
}

pub(crate) fn invalidate(repo: &Repo, config: &Config, id: &str, cause: &str) -> Result<()> {
    let chain = change(repo, config, |chain| chain.invalidate(id, cause))?;
    cleanup(repo, config, &chain)
}

fn reconcile(repo: &Repo, config: &Config) -> Result<Chain> {
    loop {
        let snapshot = read(repo, config)?;
        let mut invalid = vec![];
        for c in snapshot.chain.candidates.iter().filter(|c| c.valid()) {
            if repo.remote_sha(&format!("refs/heads/{}", c.branch))?.as_deref()
                != Some(&c.original_head)
            {
                invalid.push((c.id.clone(), "head_changed"));
            } else if !alive(&c.owner)? {
                invalid.push((c.id.clone(), "owner_died"));
            }
        }
        let mut updated = snapshot;
        let before = updated.chain.clone();
        updated.chain.observe_base(&updated.base);
        for (id, cause) in &invalid {
            updated.chain.invalidate(id, cause);
        }
        if before != updated.chain && !write(repo, config, &updated)? {
            continue;
        }
        cleanup(repo, config, &updated.chain)?;
        return Ok(read(repo, config)?.chain);
    }
}

/// Used before queue capacity is released after a candidate owner dies.
pub(crate) fn abandoned(dir: &str) -> Result<()> {
    let repo = Repo::new(dir.into());
    let config = crate::config::load(&crate::config::file_of(&repo))?;
    let chain = read(&repo, &config)?.chain;
    for c in chain.candidates.iter().filter(|c| c.active()) {
        if !alive(&c.owner)? {
            invalidate(&repo, &config, &c.id, "owner_died")?;
        }
    }
    Ok(())
}

/// The run hook records the process group before its shell is released to
/// execute any build. A crash before registration leaves only a waiting shell
/// that exits when its parent dies; no unrecorded gate starts.
pub(crate) struct Gate<'a> {
    pub repo: &'a Repo,
    pub config: &'a Config,
    pub id: &'a str,
}

impl Gate<'_> {
    pub fn started(&self, pid: u32) -> Result<()> {
        let process =
            identity(pid)?.ok_or_else(|| anyhow!("candidate gate vanished before registration"))?;
        let chain = change(self.repo, self.config, |chain| {
            for c in &mut chain.candidates {
                if c.id == self.id && c.valid() {
                    c.gate = Some(process.clone());
                    c.phase = Phase::Validating;
                }
            }
        })?;
        ensure!(
            chain.candidates.iter().any(|c| c.id == self.id && c.valid()),
            "candidate was cancelled before its gate started"
        );
        Ok(())
    }

    pub fn cancelled(&self) -> Result<bool> {
        let chain = reconcile(self.repo, self.config)?;
        Ok(!chain.candidates.iter().any(|c| c.id == self.id && c.valid()))
    }
}

fn cancelled(mut answer: Value, c: &Candidate) -> Value {
    answer["outcome"] =
        json!(if c.cause.as_deref() == Some("base_moved") { "target_moved" } else { "cancelled" });
    answer["status"] = json!("cancelled");
    answer["recorded"] = json!(false);
    answer["candidate"] = json!(c.id);
    answer["predecessor"] = json!(c.predecessor);
    answer["cancelled_by"] = json!(c.cancelled_by);
    answer["cause"] = json!(c.cause);
    answer["detail"] = json!(
        "candidate invalidated; source branch preserved; ask for another turn when its blockers permit; this is not this worker's red gate"
    );
    answer
}

pub(crate) struct Request<'a> {
    pub repo: &'a Repo,
    pub config: &'a Config,
    pub tracker: &'a Tracker,
    pub pr: &'a PullRequest,
    pub of: &'a Of,
    pub host: &'a str,
    pub now: u64,
}

/// The source head has been pushed and the base certified by the caller.
/// Assignment happens only after capacity is granted, by one atomic record.
pub(crate) fn run(request: Request<'_>, mut answer: Value) -> Result<Value> {
    let Request { repo, config, tracker, pr, of, host, now } = request;
    let gh = GitHub::of(repo)?;
    let issue = if let Of::Issue(n) = of { Some(*n) } else { None };
    let original = repo
        .fetched(&format!("refs/heads/{}", pr.head_branch))?
        .ok_or_else(|| anyhow!("PR head is missing"))?;
    let owner =
        identity(std::process::id())?.ok_or_else(|| anyhow!("cannot identify turn owner"))?;
    let locks = crate::lock::standing_locks(&gh, config)?;
    if issue.map_or(!locks.is_empty(), |n| keeps_off(&locks, n)) {
        return Ok(crate::turn::locked(answer, &locks));
    }
    if let Some(n) = issue {
        let blockers = gh.open_blockers(n)?;
        if !blockers.is_empty() {
            answer["outcome"] = json!("blocked");
            answer["blocked_by"] = json!(blockers);
            return Ok(answer);
        }
    }
    let barrier = !locks.is_empty() || !matches!(of, Of::Issue(_));
    let starting = read(repo, config)?;
    let base_config = crate::turn::tip_config(repo, &starting.base)?;
    let heavy = crate::config::heavy_gate(&config.gate_command)
        || crate::config::heavy_gate(&base_config.gate_command)
        || starting
            .chain
            .candidates
            .iter()
            .any(|c| c.valid() && crate::config::heavy_gate(&c.command));
    let limit = if heavy { crate::host_settings::of_host()?.gate_lanes() } else { 1 };
    // Do not reserve a host lane while waiting for a barrier/window or for
    // another invocation of this PR. Dead owners are reconciled first.
    loop {
        let chain = reconcile(repo, config)?;
        if let Some(c) =
            chain.candidates.iter().rev().find(|c| c.pr == pr.number && c.original_head == original)
        {
            if c.phase == Phase::Landed {
                if let Some(issue) = issue {
                    gh.close_issue(issue)?;
                }
                let mut result = c.result.clone().unwrap_or(answer);
                result["outcome"] = json!("merged");
                result["commit"] = json!(c.commit);
                return Ok(result);
            }
            if c.active() {
                std::thread::sleep(EVERY);
                continue;
            }
        }
        if chain.can_assign(limit, barrier) {
            break;
        }
        std::thread::sleep(EVERY);
    }
    let permit = if barrier {
        Permit::take(repo, "turn: serial barrier", heavy, true)?
    } else {
        Permit::candidate(repo, "turn: validate candidate, record, land", heavy)?
    };
    let id = crate::unique();
    let path = repo.common_dir()?.join("convoy-candidates").join(&id);
    let candidate = loop {
        let mut snapshot = read(repo, config)?;
        let current = snapshot.base.clone();
        if snapshot.chain.candidates.iter().any(|c| c.pr == pr.number && c.active()) {
            answer["outcome"] = json!("waiting_for_predecessor");
            answer["detail"] = json!(
                "this PR already has an active candidate; its owner alone may validate and publish it"
            );
            return Ok(answer);
        }
        let declared = crate::turn::tip_config(repo, &current)?;
        let base_command = GateCommand::new(&declared.gate_command);
        let base_tree = Tree::named(&repo.git(&["rev-parse", &format!("{current}^{{tree}}")])?);
        if locks.is_empty()
            && crate::gate::recorded(repo, &base_tree, &base_command, now)?.is_none()
        {
            answer["outcome"] = json!("target_moved");
            answer["detail"] = json!(
                "the base changed before assignment and needs certification; ask for another turn"
            );
            return Ok(answer);
        }
        // A competitor may have used the last window place while we queued.
        if !snapshot.chain.can_assign(limit, barrier) {
            drop(permit);
            answer["outcome"] = json!("waiting_for_capacity");
            answer["detail"] = json!("the bounded candidate window is full; ask for another turn");
            return Ok(answer);
        }
        snapshot.chain.observe_base(&current);
        let predecessor = snapshot.chain.predecessor(&current);
        let message = merge_message(&pr.title, pr.number, of, tracker);
        let Some(commit) = crate::turn::merged_onto(repo, &predecessor, &original, &message, now)?
        else {
            if predecessor != current {
                answer["outcome"] = json!("waiting_for_predecessor");
                answer["detail"] =
                    json!("speculative merge conflicts; ask again after its predecessor lands");
                return Ok(answer);
            }
            let files = crate::turn::merge_in(repo, &current, "the base branch's tip")?;
            answer["outcome"] = json!("conflict");
            answer["status"] = json!("conflict");
            answer["files"] = json!(files);
            answer["recorded"] = json!(false);
            answer["worktree"] = json!(repo.dir);
            answer["detail"] =
                json!("resolve every file, git add it, COMMIT the merge, and ask for a turn again");
            return Ok(answer);
        };
        let declared = crate::turn::tip_config(repo, &commit)?;
        if crate::config::heavy_gate(&declared.gate_command) && !heavy {
            answer["outcome"] = json!("waiting_for_predecessor");
            answer["detail"] = json!(
                "the predecessor changed the gate to a worker lane; request a new turn after it lands"
            );
            return Ok(answer);
        }
        let tree = repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])?;
        let seconds = declared.gate_seconds();
        let c = Candidate {
            id: id.clone(),
            pr: pr.number,
            issue,
            branch: pr.head_branch.clone(),
            original_head: original.clone(),
            predecessor,
            commit,
            tree,
            command: declared.gate_command,
            seconds,
            phase: Phase::Building,
            owner: owner.clone(),
            gate: None,
            worktree: path.to_string_lossy().into_owned(),
            barrier,
            cancelled_by: None,
            cause: None,
            stopped: false,
            result: None,
        };
        snapshot.chain.candidates.push(c.clone());
        if write(repo, config, &snapshot)? {
            break c;
        }
    };
    answer["candidate"] = json!(id);
    answer["merged"] = json!(candidate.predecessor);
    answer["predecessor"] = json!(candidate.predecessor);
    std::fs::create_dir_all(path.parent().expect("candidate parent"))?;
    repo.git(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        &path.to_string_lossy(),
        &candidate.commit,
    ])?;
    let checkout = Repo::new(path.clone());
    let command = GateCommand::new(&candidate.command);
    let hook = Gate { repo, config, id: &id };
    let result = crate::gate::candidate_gate(
        &checkout,
        &command,
        host,
        now,
        Duration::from_secs(candidate.seconds),
        &hook,
    );
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            invalidate(repo, config, &id, "gate_interrupted")?;
            return Err(error);
        }
    };
    // All processes must be gone even when a successful script leaves children.
    if let Some(gate) = read(repo, config)?
        .chain
        .candidates
        .iter()
        .find(|c| c.id == id)
        .and_then(|c| c.gate.as_ref())
    {
        terminate(gate)?;
    }
    change(repo, config, |chain| {
        for c in &mut chain.candidates {
            if c.id == id {
                c.stopped = true;
            }
        }
    })?;
    let mut result = result;
    for (key, value) in answer.as_object().into_iter().flatten() {
        result[key] = value.clone();
    }
    let green = result["status"] == "green" && result["recorded"] == true;
    let chain = change(repo, config, |chain| chain.finish(&id, green, result.clone()))?;
    cleanup(repo, config, &chain)?;
    let current = chain.candidates.iter().find(|c| c.id == id).expect("assigned candidate");
    if current.phase == Phase::Red {
        result["outcome"] = json!("gate_red");
        return Ok(result);
    }
    loop {
        let chain = reconcile(repo, config)?;
        let c = chain.candidates.iter().find(|c| c.id == id).expect("assigned candidate");
        if !c.valid() {
            return Ok(cancelled(result, c));
        }
        let locks = crate::lock::standing_locks(&gh, config)?;
        if issue.map_or(!locks.is_empty(), |n| keeps_off(&locks, n)) {
            invalidate(repo, config, &id, "fleet_lock")?;
            return Ok(crate::turn::locked(result, &locks));
        }
        let base = crate::turn::base(repo, config)?;
        if c.predecessor != base {
            std::thread::sleep(EVERY);
            continue;
        }
        if let Some(n) = issue {
            let blockers = gh.open_blockers(n)?;
            if !blockers.is_empty() {
                invalidate(repo, config, &id, "issue_blocked")?;
                result["outcome"] = json!("blocked");
                result["blocked_by"] = json!(blockers);
                return Ok(result);
            }
        }
        let mut snapshot = read(repo, config)?;
        let Some(c) = snapshot.chain.candidates.iter_mut().find(|c| c.id == id) else {
            bail!("candidate record disappeared");
        };
        let tree = Tree::named(&c.tree);
        let run = crate::gate::recorded(repo, &tree, &command, crate::clock())?;
        let head = repo.remote_sha(&format!("refs/heads/{}", c.branch))?;
        let Some(publication) = Publication::new(
            c.valid(),
            c.phase == Phase::Green && run.is_some(),
            c.predecessor == base,
            true,
            head.as_deref() == Some(&c.original_head),
        ) else {
            invalidate(repo, config, &id, "publication_evidence_changed")?;
            return Ok(cancelled(result, c));
        };
        let run = run.expect("publication requires green record");
        let proof = Proven::by(&tree, &command, &run, crate::clock())
            .ok_or_else(|| anyhow!("candidate green record expired"))?;
        let landed = c.commit.clone();
        c.phase = Phase::Landed;
        let next = commit(repo, &snapshot)?;
        let pushed = publish(repo, config, &landed, &next, &proof, &publication)?;
        if !pushed {
            if crate::turn::base(repo, config)? != base {
                continue;
            }
            if repo.remote_sha(&refname(repo, config))? == snapshot.sha {
                result["outcome"] = json!("target_moved");
                invalidate(repo, config, &id, "push_rejected")?;
                return Ok(result);
            }
            continue;
        }
        if let Some(n) = issue {
            gh.close_issue(n)?;
        }
        result["outcome"] = json!("merged");
        result["commit"] = json!(landed);
        result["detail"] = json!(
            "landed the exact recorded candidate as a fast-forward; source head preserved — stop"
        );
        crate::acting::Acting::new("land").on_issue(issue).in_checkout(Some(repo.clone())).say(
            crate::clock(),
            format!("turn landed PR #{} on {} at {landed}", pr.number, config.base_branch),
        );
        // Candidate artifacts are disposable only after success. Failed ones
        // retain their evidence; source worktrees are never removed here.
        let _ = repo.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
        drop(permit);
        return Ok(result);
    }
}

fn publish(
    repo: &Repo,
    config: &Config,
    candidate: &str,
    state: &str,
    proven: &Proven,
    _publication: &Publication,
) -> Result<bool> {
    ensure!(
        repo.git(&["rev-parse", &format!("{candidate}^{{tree}}")])? == proven.tree(),
        "candidate tree differs from recorded proof"
    );
    Ok(repo
        .try_git(&[
            "push",
            "--quiet",
            "--atomic",
            REMOTE,
            &format!("{state}:{}", refname(repo, config)),
            &format!("{candidate}:refs/heads/{}", config.base_branch),
        ])?
        .ok)
}
