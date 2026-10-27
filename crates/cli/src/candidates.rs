//! Remote candidate ownership. Every write extends one ref; a competing
//! writer's child cannot fast-forward it. Publishing the base and the next
//! record is one atomic push, so invalidation and publication cannot both win.
use afkconvoy_core::candidate::{Barrier, Candidate, Chain, Phase, Process, Publication};
use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Proven, Tree};
use afkconvoy_core::landing::{Of, merge_message};
use afkconvoy_core::lock::keeps_off;
use afkconvoy_core::plan::PullRequest;
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
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
    announce(&chain);
    Ok(Snapshot { sha, base, chain })
}

thread_local! { static ANNOUNCED: std::cell::RefCell<Option<(afkconvoy_core::candidate::Validation, std::time::Instant)>> = const { std::cell::RefCell::new(None) }; }

fn announce(chain: &Chain) {
    if !crate::exclusive::held() {
        return;
    }
    let Some(candidate) = chain
        .candidates
        .iter()
        .rev()
        .find(|c| c.owner.pid == std::process::id() && c.owner.host == crate::this_host())
    else {
        return;
    };
    let validation = candidate.observation();
    ANNOUNCED.with_borrow_mut(|last| {
        if last
            .as_ref()
            .is_some_and(|(v, at)| *v == validation && at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        let _ = crate::runner_socket::ask(
            &crate::runner_socket::Request::Candidate {
                pid: std::process::id(),
                validation: validation.clone(),
            },
            Duration::from_millis(500),
        );
        *last = Some((validation, std::time::Instant::now()));
    });
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
            announce(&snapshot.chain);
            return Ok(snapshot.chain);
        }
    }
}

struct Reservation<'a> {
    repo: &'a Repo,
    config: &'a Config,
    id: String,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let _ = change(self.repo, self.config, |chain| chain.barriers.retain(|b| b.id != self.id));
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

/// An unreadable observation never proves death. Kernel start time prevents
/// PID reuse; host identity is physical, independent of --host.
#[cfg(target_os = "macos")]
pub(crate) fn identity(pid: u32) -> Result<Option<Process>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the exact libproc structure and its bounded output buffer.
    let got = unsafe {
        libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size)
    };
    if got == 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Ok(None);
    }
    ensure!(got == size, "cannot confirm process {pid}: {}", std::io::Error::last_os_error());
    // SAFETY: a complete kernel write of numeric fields and byte arrays.
    let info = unsafe { info.assume_init() };
    if info.pbi_status == libc::SZOMB {
        return Ok(None);
    }
    Ok(Some(Process {
        host: crate::this_host(),
        pid,
        started: format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec),
    }))
}

#[cfg(target_os = "linux")]
pub(crate) fn identity(pid: u32) -> Result<Option<Process>> {
    let stat = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let (_, rest) = stat.rsplit_once(')').context("kernel process identity is malformed")?;
    let fields: Vec<_> = rest.split_whitespace().collect();
    if fields.first() == Some(&"Z") {
        return Ok(None);
    }
    let start = fields.get(19).context("kernel process start time is missing")?;
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    Ok(Some(Process { host: crate::this_host(), pid, started: format!("{}:{start}", boot.trim()) }))
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn identity(pid: u32) -> Result<Option<Process>> {
    let out = crate::process::output(std::process::Command::new("ps").args([
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

/// A private environment mark follows descendants across process groups and
/// reparenting. It is supplied only to this gate, never the turn's git/ps calls.
pub(crate) const PROCESS_ENV: &str = "AFKCONVOY_CANDIDATE_PROCESS";

/// Freeze and durably identify the whole owned tree BEFORE killing any of it.
/// Another process may finish this cancellation from the saved identities.
fn signal(repo: &Repo, config: &Config, candidate: &Candidate) -> Result<Vec<Process>> {
    let scope = crate::process_scope::path(&candidate.worktree);
    let has_scope = scope.exists();
    ensure!(
        candidate.gate.is_none() || has_scope,
        "gate process ownership is missing; its lane remains occupied"
    );
    let mut owned: Vec<Process> = candidate
        .descendants
        .iter()
        .filter_map(|p| match alive(p) {
            Ok(true) => Some(Ok(p.clone())),
            Ok(false) => None,
            Err(e) => Some(Err(e)),
        })
        .collect::<Result<_>>()?;
    let marker = format!("{PROCESS_ENV}={}", candidate.id);
    let group = match &candidate.gate {
        Some(gate) if gate.host == crate::this_host() => {
            let current = identity(gate.pid)?;
            if current.as_ref() == Some(gate)
                && has_scope
                && crate::process_scope::owns(gate.pid, &scope)?
            {
                // SAFETY: the group's identity belongs to this gate, with
                // PID reuse excluded by its recorded process start time.
                unsafe {
                    libc::kill(-(gate.pid as i32), libc::SIGSTOP);
                }
                if current.is_some() && !owned.contains(gate) {
                    owned.push(gate.clone());
                }
                Some(gate.pid)
            } else {
                None
            }
        }
        _ => None,
    };
    for p in &owned {
        // SAFETY: checked identities belonging to this candidate alone.
        unsafe {
            libc::kill(p.pid as i32, libc::SIGSTOP);
        }
    }
    loop {
        let all = crate::hot::listed_checked()?;
        let mut found = vec![];
        for p in &all {
            if p.pid == candidate.owner.pid
                || p.pid == std::process::id()
                || owned.iter().any(|known| known.pid == p.pid)
            {
                continue;
            }
            if owned.iter().any(|known| known.pid == p.ppid)
                || p.line.split_whitespace().any(|word| word == marker)
                || (has_scope && crate::process_scope::owns(p.pid, &scope)?)
            {
                found.push(p.pid);
            }
        }
        let mut added = false;
        for pid in found {
            if let Some(process) = identity(pid)? {
                // SAFETY: an owned descendant, identified above.
                unsafe {
                    libc::kill(pid as i32, libc::SIGSTOP);
                }
                owned.push(process);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
    change(repo, config, |chain| {
        for c in &mut chain.candidates {
            if c.id == candidate.id {
                for process in &owned {
                    if !c.descendants.contains(process) {
                        c.descendants.push(process.clone());
                    }
                }
            }
        }
    })?;
    for process in owned.iter().rev() {
        if identity(process.pid)?.as_ref() == Some(process) {
            // SAFETY: the frozen process still has the saved identity.
            unsafe {
                libc::kill(process.pid as i32, libc::SIGKILL);
            }
        }
    }
    if let Some(group) = group {
        // SAFETY: kill any group members that exited during discovery.
        unsafe {
            libc::kill(-(group as i32), libc::SIGKILL);
        }
    }
    Ok(owned)
}

fn confirmed(processes: &[Process], scope: &std::path::Path) -> Result<()> {
    let began = std::time::Instant::now();
    loop {
        let mut live = false;
        for process in processes {
            live |= alive(process)?;
        }
        if scope.exists() {
            live |= !crate::process_scope::free(scope)?;
        }
        if !live {
            return Ok(());
        }
        ensure!(
            began.elapsed() < Duration::from_secs(10),
            "candidate exit is unconfirmed; its lane stays occupied"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn stop_candidate(repo: &Repo, config: &Config, id: &str) -> Result<()> {
    let chain = read(repo, config)?.chain;
    let c = chain
        .candidates
        .iter()
        .find(|c| c.id == id)
        .ok_or_else(|| anyhow!("candidate disappeared"))?;
    if c.stopped {
        return Ok(());
    }
    let processes = signal(repo, config, c)?;
    confirmed(&processes, &crate::process_scope::path(&c.worktree))?;
    change(repo, config, |chain| {
        for c in &mut chain.candidates {
            if c.id == id {
                c.stopped = true;
                if c.phase == Phase::Cancelling {
                    c.phase = Phase::Cancelled;
                }
            }
        }
    })?;
    Ok(())
}

/// Invalidate the suffix atomically, signal ALL its gates, then confirm each
/// exit. A slow root's cleanup cannot postpone signalling its descendants.
fn cleanup(repo: &Repo, config: &Config, chain: &Chain) -> Result<()> {
    let mut signalled = vec![];
    for c in
        chain.candidates.iter().filter(|c| !c.stopped && !c.valid() && c.phase != Phase::Landed)
    {
        if c.owner.host != crate::this_host() || (c.gate.is_none() && alive(&c.owner)?) {
            continue;
        }
        signalled.push((
            c.id.clone(),
            signal(repo, config, c)?,
            crate::process_scope::path(&c.worktree),
        ));
    }
    for (id, processes, scope) in signalled {
        confirmed(&processes, &scope)?;
        change(repo, config, |chain| {
            for c in &mut chain.candidates {
                if c.id == id && !c.valid() {
                    c.stopped = true;
                    if c.phase == Phase::Cancelling {
                        c.phase = Phase::Cancelled;
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
        let mut dead_barriers = vec![];
        for barrier in &updated.chain.barriers {
            if !alive(&barrier.owner)? {
                dead_barriers.push(barrier.id.clone());
            }
        }
        let before = updated.chain.clone();
        updated.chain.barriers.retain(|b| !dead_barriers.contains(&b.id));
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
    pub fn scope(&self) -> Result<std::path::PathBuf> {
        let chain = read(self.repo, self.config)?.chain;
        let candidate = chain
            .candidates
            .iter()
            .find(|c| c.id == self.id)
            .context("candidate disappeared before gate start")?;
        Ok(crate::process_scope::path(&candidate.worktree))
    }
    /// Called at the first observed red exit or limit, before log draining,
    /// gate-record I/O or process teardown can delay dependent cancellation.
    pub fn failed(
        &self,
        exit_code: i32,
        timed_out: bool,
        seconds: u64,
        termination: Option<crate::gate::Termination>,
    ) -> Result<()> {
        let evidence = json!({"status":"red", "exit_code":exit_code, "timed_out":timed_out, "seconds":seconds, "termination":termination});
        let chain =
            change(self.repo, self.config, |chain| chain.finish(self.id, false, evidence.clone()))?;
        cleanup(self.repo, self.config, &chain)
    }
    pub fn process_marker(&self) -> &str {
        self.id
    }

    pub fn stop(&self) -> Result<()> {
        stop_candidate(self.repo, self.config, self.id)
    }
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
    let began = std::time::Instant::now();
    let clock = || now.saturating_add(began.elapsed().as_secs());
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
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let id = format!(
        "{:016x}-{stamp}-{}",
        afkconvoy_core::gate::key(&crate::this_host()),
        crate::unique()
    );
    let _reservation = if barrier {
        change(repo, config, |chain| {
            if !chain.barriers.iter().any(|b| b.id == id) {
                chain.barriers.push(Barrier { id: id.clone(), owner: owner.clone() });
            }
        })?;
        Some(Reservation { repo, config, id: id.clone() })
    } else {
        None
    };
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
        if (!barrier && chain.barriers.is_empty())
            || (barrier
                && chain.can_assign(limit, true)
                && chain.barriers.first().is_some_and(|b| b.id == id))
        {
            break;
        }
        std::thread::sleep(EVERY);
    }
    let permit = if barrier {
        Permit::take(repo, "turn: serial barrier", heavy, true)?
    } else {
        Permit::candidate(repo, "turn: validate candidate, record, land", heavy)?
    };
    let path = repo.common_dir()?.join("convoy-candidates").join(&id);
    let candidate = loop {
        let locks = crate::lock::standing_locks(&gh, config)?;
        if issue.map_or(!locks.is_empty(), |n| keeps_off(&locks, n)) {
            return Ok(crate::turn::locked(answer, &locks));
        }
        let barrier = barrier || !locks.is_empty();
        let limit = if heavy { crate::host_settings::of_host()?.gate_lanes() } else { 1 };
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
        let repairing = match issue.filter(|n| locks.contains(n)) {
            Some(n) => crate::repair::is_for(&gh, n, &current)?,
            None => false,
        };
        if !repairing && crate::gate::recorded(repo, &base_tree, &base_command, clock())?.is_none()
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
            descendants: vec![],
            worktree: path.to_string_lossy().into_owned(),
            barrier,
            cancelled_by: None,
            cause: None,
            stopped: false,
            result: None,
        };
        snapshot.chain.candidates.push(c.clone());
        snapshot.chain.barriers.retain(|b| b.id != id);
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
        clock(),
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
    let mut result = result;
    for (key, value) in answer.as_object().into_iter().flatten() {
        result[key] = value.clone();
    }
    let green = result["status"] == "green" && result["recorded"] == true;
    let chain = change(repo, config, |chain| chain.finish(&id, green, result.clone()))?;
    cleanup(repo, config, &chain)?;
    stop_candidate(repo, config, &id)?;
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
        let run = crate::gate::recorded(repo, &tree, &command, clock())?;
        let head = repo.remote_sha(&format!("refs/heads/{}", c.branch))?;
        let Some(publication) = Publication::new(
            c.valid(),
            c.phase == Phase::Green && run.is_some(),
            c.predecessor == base,
            true,
            head.as_deref() == Some(&c.original_head),
            c.stopped,
        ) else {
            invalidate(repo, config, &id, "publication_evidence_changed")?;
            return Ok(cancelled(result, c));
        };
        let run = run.expect("publication requires green record");
        let proof = Proven::by(&tree, &command, &run, clock())
            .ok_or_else(|| anyhow!("candidate green record expired"))?;
        let landed = c.commit.clone();
        c.phase = Phase::Landed;
        let publishing = c.clone();
        let next = commit(repo, &snapshot)?;
        let pushed = publish(repo, config, &publishing, &snapshot, &next, &proof, &publication)?;
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
        let _ = std::fs::remove_file(crate::process_scope::path(&candidate.worktree));
        drop(permit);
        return Ok(result);
    }
}

fn publish(
    repo: &Repo,
    config: &Config,
    candidate: &Candidate,
    snapshot: &Snapshot,
    state: &str,
    proven: &Proven,
    _publication: &Publication,
) -> Result<bool> {
    let commit = &candidate.commit;
    let head = &candidate.original_head;
    let branch = &candidate.branch;
    let base = &candidate.predecessor;
    let old_state = snapshot.sha.as_deref().context("candidate record disappeared")?;
    ensure!(
        repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])? == proven.tree(),
        "candidate tree differs from recorded proof"
    );
    ensure!(
        repo.git(&["rev-parse", &format!("{commit}^1")])? == *base,
        "candidate does not fast-forward its expected predecessor"
    );
    ensure!(
        repo.git(&["rev-parse", &format!("{state}^1")])? == old_state,
        "candidate state does not extend its expected record"
    );
    let state_ref = refname(repo, config);
    let base_ref = format!("refs/heads/{}", config.base_branch);
    let source_ref = format!("refs/heads/{branch}");
    crate::publication::push(
        repo,
        &[
            crate::publication::Update { name: &source_ref, old: head, new: head },
            crate::publication::Update { name: &state_ref, old: old_state, new: state },
            crate::publication::Update { name: &base_ref, old: base, new: commit },
        ],
    )
}
