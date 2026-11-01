//! Remote candidate ownership. Every write extends one ref; a competing
//! writer's child cannot fast-forward it. Publishing the base and the next
//! record is one atomic push, so invalidation and publication cannot both win.
use afkconvoy_core::account::End;
use afkconvoy_core::candidate::{Barrier, Candidate, Chain, Phase, Process, Publication, Slow};
use afkconvoy_core::claims::{HEARTBEAT_KIND, read_heartbeat};
use afkconvoy_core::config::Config;
use afkconvoy_core::ending::Ending;
use afkconvoy_core::gate::{GateCommand, Proven, Tree};
use afkconvoy_core::landing::{Of, merge_message};
use afkconvoy_core::lease::stamp_stale;
use afkconvoy_core::lock::keeps_off;
use afkconvoy_core::plan::PullRequest;
use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::exclusive::Permit;
use crate::gh::{GitHub, Tracker};
use crate::git::Repo;
use crate::remote::{Fresh, Push};

/// How long a turn waiting on the chain first waits before it reads it again.
const EVERY: Duration = Duration::from_millis(100);

/// The longest it waits between two reads (`Waiting`).
const AT_MOST: Duration = Duration::from_secs(1);

/// A turn waiting on the chain: for its round, its predecessor or another
/// turn of its PR. Each read asks the remote live and starts git, so a wait
/// that sees nothing change grows, twice as long each time up to `AT_MOST`,
/// and a change starts it again at `EVERY`. Those reads ran beside the gate
/// a turn waits on, and in this repo's tests beside the other tests'
/// (sunfmin/control#309).
struct Waiting {
    seen: Option<Chain>,
    next: Duration,
}

impl Waiting {
    fn new() -> Waiting {
        Waiting { seen: None, next: EVERY }
    }

    /// Waits after reading `chain`.
    fn after(&mut self, chain: &Chain) {
        std::thread::sleep(self.wait(chain));
    }

    /// How long to wait after reading `chain`.
    fn wait(&mut self, chain: &Chain) -> Duration {
        if self.seen.as_ref() == Some(chain) {
            self.next = (self.next * 2).min(AT_MOST);
        } else {
            self.seen = Some(chain.clone());
            self.next = EVERY;
        }
        self.next
    }
}

/// Selection must also refuse a candidate left after its claim ended.
pub(crate) fn has_task(repo: &Repo, issue: u64) -> Result<bool> {
    let prefix = repo.layout().under("candidates/");
    for record in repo.remote().records(&[&format!("{prefix}*")], Fresh::Process)? {
        let chain: Chain = serde_json::from_str(&record.subject)?;
        if chain.candidates.iter().any(|c| c.issue == Some(issue) && c.active()) {
            return Ok(true);
        }
    }
    Ok(false)
}

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
    // Live: a write follows this read, and must start from the remote as it is.
    let refs = repo.remote().refs(&[&state_ref, &base_ref], Fresh::Live)?;
    let found = |name: &str| refs.iter().find(|r| r.name == name).map(|r| r.sha.clone());
    let sha = found(&state_ref);
    let base = found(&base_ref).ok_or_else(|| {
        anyhow!("target branch {:?} disappeared; restore it before continuing", config.base_branch)
    })?;
    repo.bring(&base, &base_ref)?;
    if let Some(sha) = &sha {
        repo.bring(sha, &state_ref)?;
    }
    let chain = match &sha {
        Some(sha) => serde_json::from_str(&repo.message(sha)?)?,
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
/// The chain goes to git on stdin: as an argument it passed macOS's `ARG_MAX`
/// once it held 258 candidates, and nothing landed (sunfmin/control#308).
/// A candidate nothing holds any more is not carried on: the previous
/// record, its first parent, keeps its commit reachable.
fn commit(repo: &Repo, snapshot: &Snapshot) -> Result<String> {
    let tree = repo.git(&["hash-object", "-t", "tree", "/dev/null"])?;
    let mut chain = snapshot.chain.clone();
    chain.prune(released);
    let text = serde_json::to_string(&chain)?;
    let mut args = vec!["commit-tree", &tree, "-F", "-"];
    if let Some(sha) = &snapshot.sha {
        args.extend(["-p", sha]);
    }
    for c in &chain.candidates {
        args.extend(["-p", &c.commit]);
    }
    repo.git_with_input(&args, &text)
}

/// Nothing holds an ended candidate any more: its owner, a turn of this host,
/// is observed gone, and its worktree is removed. An owner on another host,
/// or one not observed, still holds it.
fn released(c: &Candidate) -> bool {
    let gone = c.owner.host == crate::this_host()
        && matches!(identity(c.owner.pid), Ok(found) if found.as_ref() != Some(&c.owner));
    gone && !Path::new(&c.worktree).exists() && !crate::process_scope::path(&c.worktree).exists()
}

fn write(repo: &Repo, config: &Config, snapshot: &Snapshot) -> Result<bool> {
    let next = commit(repo, snapshot)?;
    let mut attempt = 0;
    loop {
        let pushed = repo.remote().try_push(&Push::set(&next, &refname(repo, config)))?;
        if pushed.ok {
            return Ok(true);
        }
        if repo.remote().sha(&refname(repo, config), Fresh::Process)? != snapshot.sha {
            return Ok(false);
        }
        // A remote fault is not contention. Never spin on a rejected record,
        // but for a lock another push held at that moment (sunfmin/control#246).
        attempt += 1;
        if !lock_failure(&pushed.stderr) || attempt >= LOCK_ATTEMPTS {
            bail!(
                "candidate record could not be written: {}; run land again",
                pushed.stderr.trim()
            );
        }
        std::thread::sleep(Duration::from_millis(250 << attempt));
    }
}

/// How many times a record write is tried while the remote's ref lock fails.
const LOCK_ATTEMPTS: u32 = 5;

/// A push the remote rejected because another push held the ref's lock:
/// GitHub says `(failed)` or `cannot lock ref`, not a rejected fast-forward.
pub(crate) fn lock_failure(stderr: &str) -> bool {
    stderr.lines().any(|line| {
        let line = line.trim();
        (line.starts_with("! [remote rejected]") && line.ends_with("(failed)"))
            || line.contains("cannot lock ref")
            || line.contains("failed to lock")
    })
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
        let _ = change(self.repo, self.config, |chain| {
            chain.barriers.retain(|b| b.id != self.id);
            chain.pending.retain(|b| b.id != self.id);
        });
    }
}

/// One candidate as a look shows it. A finished candidate carries no gate
/// excerpt. One gating now (`Validating`) may still carry `excerpt`.
fn shown(chain: &Chain, c: &Candidate, branch: &str) -> Value {
    let mut row = json!({
        "phase": c.phase,
        "commit": c.commit,
        "issue": c.issue,
        "pr": c.pr,
        "predecessor": c.predecessor,
        "waiting": c.waiting(),
        "target_branch": branch,
        "id": c.id,
        "cause": c.cause,
        "cancelled_by": c.cancelled_by,
        "round": chain.round_of(&c.id).map(|tip| &tip.id),
    });
    if matches!(c.phase, Phase::Validating | Phase::Slow)
        && let Some(excerpt) = c.result.as_ref().and_then(|result| result.get("excerpt"))
    {
        row["excerpt"] = excerpt.clone();
    }
    row
}

pub(crate) fn view(
    repo: &Repo,
    _config: &Config,
    records: &[crate::remote::RemoteRecord],
) -> Result<Value> {
    let prefix = repo.layout().under("candidates/");
    let mut rows = vec![];
    for record in records.iter().filter(|record| record.name.starts_with(&prefix)) {
        let branch = record.name.strip_prefix(&prefix).context("invalid candidate ref")?;
        let chain: Chain = serde_json::from_str(&record.subject)?;
        for c in &chain.candidates {
            rows.push(shown(&chain, c, branch));
        }
    }
    Ok(json!(rows))
}

/// Whether a land turn of `branch` holds a candidate on the chain now: a
/// valid one of that branch, in `records`, whose turn runs on this host and
/// is alive. Such a turn waits on the chain — its gate, the candidates ahead,
/// its round — however long that takes, and holds no lane while it waits for
/// its round (sunfmin/control#244).
pub(crate) fn landing(records: &[crate::remote::RemoteRecord], prefix: &str, branch: &str) -> bool {
    let chains = records
        .iter()
        .filter(|record| record.name.starts_with(prefix))
        .filter_map(|record| serde_json::from_str::<Chain>(&record.subject).ok());
    chains.into_iter().any(|chain| {
        chain.candidates.iter().any(|c| {
            c.valid()
                && c.phase != Phase::Landed
                && c.branch == branch
                && c.owner.host == crate::this_host()
                && identity(c.owner.pid).ok().flatten().as_ref() == Some(&c.owner)
        })
    })
}

/// A process this user may not read is another user's, never the recorded
/// owner. Any other unreadable observation never proves death. Kernel start
/// time prevents PID reuse; host identity is physical, independent of --host.
#[cfg(target_os = "macos")]
pub(crate) fn identity(pid: u32) -> Result<Option<Process>> {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: the exact libproc structure and its bounded output buffer.
    let got = unsafe {
        libc::proc_pidinfo(pid as i32, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size)
    };
    if got == 0
        && matches!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH | libc::EPERM))
    {
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

/// Whether `process` runs: on this host, by its identity. One on another
/// host cannot be read from here, and counts as running; whether that host
/// is alive at all is `reconcile`'s to read (`dead_hosts`).
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

/// A turn's look at the chain at `now`, which keeps it true: says this host
/// is alive (`Chain::beat`), takes over what a dead host owns (`dead_hosts`,
/// `Chain::abandon`), invalidates a candidate whose PR head moved or whose
/// owner on this host is gone, except an adoptable stopped fast-green one,
/// drops this host's dead barriers and pending
/// turns, and stops this host's invalid candidates (`cleanup`): its own too,
/// when another host took it over while this one was away.
fn reconcile(repo: &Repo, config: &Config, now: u64) -> Result<Chain> {
    let here = crate::this_host();
    loop {
        let snapshot = read(repo, config)?;
        let mut updated = snapshot;
        let before = updated.chain.clone();
        updated.chain.beat(&here, now);
        for (host, heard) in dead_hosts(repo, &updated.chain, &here, now)? {
            match heard {
                Some(ts) => updated.chain.heard(&host, ts),
                None => updated.chain.abandon(&host),
            }
        }
        let mut invalid = vec![];
        for c in updated.chain.candidates.iter().filter(|c| c.valid()) {
            if repo.remote().sha(&format!("refs/heads/{}", c.branch), Fresh::Live)?.as_deref()
                != Some(&c.original_head)
            {
                invalid.push((c.id.clone(), "head_changed"));
            } else if !c.adoptable() && !alive(&c.owner)? {
                invalid.push((c.id.clone(), "owner_died"));
            }
        }
        let mut dead_barriers = vec![];
        for barrier in updated.chain.barriers.iter().chain(&updated.chain.pending) {
            if !alive(&barrier.owner)? {
                dead_barriers.push(barrier.id.clone());
            }
        }
        updated.chain.barriers.retain(|b| !dead_barriers.contains(&b.id));
        updated.chain.pending.retain(|b| !dead_barriers.contains(&b.id));
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

/// The hosts but `here` that own a member of `chain` and stamped it no
/// later than the lease before `now` → each with what the fleet heartbeats
/// on this repo's remote say instead: the newest stamp of a fleet instance
/// on it, where that is within the lease, or `None`: the host is dead. The
/// heartbeats are read live, and only when a host is to be judged by them:
/// a host whose turns run this version stamps the chain itself.
fn dead_hosts(
    repo: &Repo,
    chain: &Chain,
    here: &str,
    now: u64,
) -> Result<Vec<(String, Option<u64>)>> {
    let quiet: Vec<&str> = chain
        .owners()
        .into_iter()
        .filter(|host| *host != here && !chain.beats(host, now))
        .collect();
    if quiet.is_empty() {
        return Ok(vec![]);
    }
    let prefix = repo.layout().under(HEARTBEAT_KIND);
    let mut newest: BTreeMap<String, u64> = BTreeMap::new();
    for record in repo.remote().records(&[&format!("{prefix}*")], Fresh::Live)? {
        if let Some(beat) = read_heartbeat(&record.subject, record.sha.clone()) {
            let ts = newest.entry(beat.host).or_insert(beat.ts);
            *ts = (*ts).max(beat.ts);
        }
    }
    Ok(quiet
        .into_iter()
        .map(|host| {
            let fresh = newest.get(host).copied().filter(|&ts| !stamp_stale(ts, now));
            (host.to_string(), fresh)
        })
        .collect())
}

/// Used before queue capacity is released after a candidate owner dies.
/// A stopped fast-green candidate has no process to clean up and is adopted
/// by its next turn instead of cancelling its successors here.
pub(crate) fn abandoned(dir: &str) -> Result<()> {
    let repo = Repo::new(dir.into());
    let prefix = repo.layout().under("candidates/");
    for record in repo.remote().records(&[&format!("{prefix}*")], Fresh::Process)? {
        let mut config = crate::config::load(&crate::config::file_of(&repo))?;
        config.base_branch =
            record.name.strip_prefix(&prefix).context("invalid candidate ref")?.to_string();
        let chain: Chain = serde_json::from_str(&record.subject)?;
        for c in chain.candidates.iter().filter(|c| c.active()) {
            if !c.adoptable() && !alive(&c.owner)? {
                invalidate(&repo, &config, &c.id, "owner_died")?;
            }
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
    /// A round's slow gate: its tests keep the threads its profile gives
    /// them, and are not held to a share of the host's cores.
    pub slow: bool,
    /// The turn's clock: what each look at the chain is made at.
    pub clock: &'a dyn Fn() -> u64,
}

impl Gate<'_> {
    /// Where the candidate's gate builds: the target directory of the
    /// worktree its turn runs in, which that worker's own gate has just
    /// built. A candidate's worktree is new, and a build there is cold.
    pub fn target(&self) -> std::path::PathBuf {
        self.repo.dir.join("target")
    }

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
                    // A round's slow gate starts in `Slow`, and stays there.
                    if c.phase == Phase::Building {
                        c.phase = Phase::Validating;
                    }
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
        let chain = reconcile(self.repo, self.config, (self.clock)())?;
        Ok(!chain.candidates.iter().any(|c| c.id == self.id && c.valid()))
    }
}

/// Closes a landed candidate's issue and writes its work account. The
/// landing is published: what fails here is said in `settle_error`, and the
/// tick settles a landed issue still open (sunfmin/control#247), so the turn
/// still answers `merged`.
fn settle(repo: &Repo, gh: &GitHub, issue: Option<u64>, now: u64, answer: &mut Value) {
    let Some(n) = issue else { return };
    let settled =
        gh.close_issue(n).and_then(|()| crate::account::leave(repo, gh, n, End::Landed, now));
    if let Err(error) = settled {
        answer["settle_error"] = json!(format!("{error:#}"));
    }
}

fn cancelled(mut answer: Value, c: &Candidate, chain: &Chain) -> Value {
    if c.cause.as_deref() == Some("round_red") {
        let tip = chain.candidates.iter().find(|t| Some(&t.id) == c.cancelled_by.as_ref());
        answer["outcome"] = json!(Ending::RoundRed);
        answer["status"] = json!("cancelled");
        answer["recorded"] = json!(false);
        answer["candidate"] = json!(c.id);
        answer["cancelled_by"] = json!(c.cancelled_by);
        answer["cause"] = json!(c.cause);
        answer["round_pr"] = json!(tip.map(|t| t.pr));
        answer["detail"] = json!(
            "the slow gate ran red on the round at PR `round_pr`, which holds this change; its worker fixes the round: stop"
        );
        return answer;
    }
    answer["outcome"] = json!(if c.cause.as_deref() == Some("base_moved") {
        Ending::TargetMoved
    } else {
        Ending::Cancelled
    });
    answer["status"] = json!("cancelled");
    answer["recorded"] = json!(false);
    answer["candidate"] = json!(c.id);
    answer["predecessor"] = json!(c.predecessor);
    answer["cancelled_by"] = json!(c.cancelled_by);
    answer["cause"] = json!(c.cause);
    answer["detail"] = json!(
        "candidate invalidated; source branch preserved; ask for another turn when its blockers permit; this is not this worker's red gate"
    );
    if c.cause.as_deref() == Some("owner_died") && c.owner.host == crate::this_host() {
        answer["detail"] = json!(format!(
            "another host took this candidate over: no turn on this host said it was alive within the lease ({}s) — it slept, or lost the network — so nothing of it lands, and its gate here was stopped; source branch preserved; ask for another turn",
            afkconvoy_core::lease::LEASE
        ));
    }
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

/// Re-reads every candidate chain and removes the worktrees of those that
/// have stopped.
///
/// A turn that has just written a chain calls this. That write is past the
/// look's moment, so the listing the look holds is not this chain
/// (docs/adr/0043).
pub(crate) fn remove_stopped_worktrees(repo: &Repo) -> Result<()> {
    let prefix = repo.layout().under("candidates/");
    let records = repo.remote().records(&[&format!("{prefix}*")], Fresh::Process)?;
    remove_listed(repo, &records)
}

/// Candidate worktrees live outside the checkout so a gate's tools cannot
/// discover the checkout's configuration above their own. The common git
/// dir identifies this repository across all its worktrees and path aliases.
fn worktree_root(repo: &Repo) -> Result<PathBuf> {
    let common = std::fs::canonicalize(repo.common_dir()?)?;
    let key = afkconvoy_core::gate::key(&common.to_string_lossy());
    Ok(crate::copies::cache()?.join("candidates").join(format!("{key:016x}")))
}

/// A configured cache can itself be inside a checkout. Refuse it before
/// reserving a turn or assigning a candidate, even if it is another repo's.
/// Ask git so linked worktrees and configuration discovery boundaries count.
fn prepare_worktree_root(repo: &Repo) -> Result<PathBuf> {
    let root = worktree_root(repo)?;
    std::fs::create_dir_all(&root)?;
    let root = std::fs::canonicalize(root)?;
    let found = Repo::new(root.clone()).try_git(&["rev-parse", "--show-toplevel"])?;
    ensure!(
        !found.ok,
        "candidate cache {} is inside checkout {}; set XDG_CACHE_HOME to a directory outside every checkout and run `afkconvoy land` again",
        root.display(),
        found.stdout
    );
    ensure!(
        found.stderr.starts_with("fatal: not a git repository"),
        "cannot check candidate cache {}: {}; set XDG_CACHE_HOME to a directory outside every checkout and run `afkconvoy land` again",
        root.display(),
        found.stderr.trim()
    );
    Ok(root)
}

/// Removes the worktree of every stopped candidate named by `records`.
///
/// `records` are candidate chains already read: the look's listing, or a
/// turn's re-read of the chain it just wrote. A landed, red, or cancelled
/// candidate loses its worktree. One that is building, validating, green and
/// waiting, or cancelling keeps it. So does one whose turn, another process,
/// still runs: a cancelled turn still runs git there before it answers
/// (sunfmin/control#312), and removes it itself when it returns. A process
/// that still has the directory as its working directory keeps it too; the
/// next call removes it once that process is gone. A directory whose git registration is already gone is
/// removed as well. Nothing here reads or writes the candidate chain. An
/// issue's, watch's, or cut's worktree is outside the candidate roots and is
/// left alone. Both the host cache and the old common-git-dir root are
/// recognised, so a later version finishes an earlier one's removal.
///
/// A process listing that fails removes nothing: it is no evidence the
/// directory is free.
pub(crate) fn remove_listed(repo: &Repo, records: &[crate::remote::RemoteRecord]) -> Result<()> {
    let prefix = repo.layout().under("candidates/");
    let roots = [worktree_root(repo)?, repo.common_dir()?.join("convoy-candidates")];
    let mut pending = vec![];
    for record in records {
        if !record.name.starts_with(&prefix) {
            continue;
        }
        let chain: Chain = serde_json::from_str(&record.subject)?;
        for candidate in &chain.candidates {
            if candidate.active() {
                continue;
            }
            let path = PathBuf::from(&candidate.worktree);
            if !roots.iter().any(|root| candidate_directory(root, &path, &candidate.id)) {
                continue;
            }
            if (path.exists() || crate::process_scope::path(&candidate.worktree).exists())
                && !another_turn_runs(&candidate.owner)?
            {
                pending.push((path, candidate.worktree.clone()));
            }
        }
    }
    if pending.is_empty() {
        return Ok(());
    }
    let busy = crate::hot::working_directories()?;
    for (path, recorded) in pending {
        if crate::hot::occupies(&busy, &path) {
            continue;
        }
        remove_candidate_worktree(repo, &path, &recorded);
    }
    // A removal that deleted the directory and left the registration, or the
    // reverse, converges here. A worktree that is still on disk stays listed.
    let _ = repo.try_git(&["worktree", "prune"]);
    Ok(())
}

/// Whether `owner`, a candidate's turn, is a process other than this one
/// still running on this host. The worktree is that turn's until it returns.
fn another_turn_runs(owner: &Process) -> Result<bool> {
    Ok(owner.host == crate::this_host() && owner.pid != std::process::id() && alive(owner)?)
}

/// `path` is this repo's candidate directory for `id`, and nothing else.
fn candidate_directory(root: &Path, path: &Path, id: &str) -> bool {
    if path.file_name().and_then(|name| name.to_str()) != Some(id) {
        return false;
    }
    let Some(parent) = path.parent() else { return false };
    if parent == root {
        return true;
    }
    match (std::fs::canonicalize(parent), std::fs::canonicalize(root)) {
        (Ok(parent), Ok(root)) => parent == root,
        _ => false,
    }
}

/// Drops `path` and, once the directory is gone, its process-scope file.
///
/// Directories are opened for removal first (`copies::removable`): git deletes
/// the gitdir before the worktree, and stops on a directory the owner cannot
/// write, leaving the registration gone and the directory behind.
fn remove_candidate_worktree(repo: &Repo, path: &Path, recorded: &str) {
    crate::copies::removable(path);
    let text = path.to_string_lossy();
    let _ = repo.try_git(&["worktree", "remove", "--force", text.as_ref()]);
    if path.exists() {
        crate::copies::removable(path);
        let _ = std::fs::remove_dir_all(path);
    }
    if path.exists() {
        eprintln!(
            "warning: candidate worktree {} is still there and will be removed on a later tick",
            path.display()
        );
        return;
    }
    let _ = std::fs::remove_file(crate::process_scope::path(recorded));
}

/// Removes stopped candidate worktrees when the turn ends, including when it
/// is cancelled. A turn killed outright leaves them for the next tick.
struct Reap<'a>(&'a Repo);

impl Drop for Reap<'_> {
    fn drop(&mut self) {
        let _ = remove_stopped_worktrees(self.0);
    }
}

/// The source head has been pushed and the base certified by the caller.
/// Assignment happens only after capacity is granted, by one atomic record.
pub(crate) fn run(request: Request<'_>, mut answer: Value) -> Result<Value> {
    let Request { repo, config, tracker, pr, of, host, now } = request;
    let _reap = Reap(repo);
    let began = std::time::Instant::now();
    let clock = || now.saturating_add(began.elapsed().as_secs());
    let gh = GitHub::of(repo)?;
    let issue = if let Of::Issue(n) = of { Some(*n) } else { None };
    let original = repo
        .remote()
        .fetched(&format!("refs/heads/{}", pr.head_branch), Fresh::Process)?
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
            answer["outcome"] = json!(Ending::Blocked);
            answer["blocked_by"] = json!(blockers);
            return Ok(answer);
        }
    }
    let root = prepare_worktree_root(repo)?;
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
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_nanos();
    let id = format!(
        "{:016x}-{stamp}-{}",
        afkconvoy_core::gate::key(&crate::this_host()),
        crate::unique()
    );
    // A barrier's place, or a pending turn's: a round waits for every turn
    // that asked to land (sunfmin/control#244). It says this host is alive.
    let here = crate::this_host();
    let reserve = |chain: &mut Chain| {
        let places = if barrier { &mut chain.barriers } else { &mut chain.pending };
        if !places.iter().any(|b| b.id == id) {
            places.push(Barrier { id: id.clone(), owner: owner.clone() });
        }
        chain.beat(&here, clock());
    };
    change(repo, config, reserve)?;
    let _reservation = Reservation { repo, config, id: id.clone() };
    // A fast-green candidate's gate is confirmed gone, so a later turn of its
    // PR can own its immutable evidence without cancelling its successors.
    let existing = read(repo, config)?
        .chain
        .candidates
        .iter()
        .rev()
        .find(|c| c.pr == pr.number && c.original_head == original && c.adoptable())
        .cloned();
    let mut adopted = None;
    if let Some(c) = existing
        && c.owner != owner
        && !alive(&c.owner)?
    {
        let previous = c.id.clone();
        let old_owner = c.owner.clone();
        change(repo, config, |chain| {
            if let Some(c) = chain
                .candidates
                .iter_mut()
                .find(|c| c.id == previous && c.owner == old_owner && c.adoptable())
            {
                c.owner = owner.clone();
                c.abandoned = false;
                adopted = Some(c.clone());
                chain.pending.retain(|b| b.id != id);
                chain.barriers.retain(|b| b.id != id);
            }
        })?;
    }
    // Do not reserve a host lane while waiting for a barrier/window or for
    // another invocation of this PR. Dead owners are reconciled first.
    let mut waiting = Waiting::new();
    loop {
        let mut chain = reconcile(repo, config, clock())?;
        // Another host read this one dead while it was away — it slept, or
        // lost the network — and dropped its place: it takes one again, at
        // the back.
        if adopted.is_none() && !chain.barriers.iter().chain(&chain.pending).any(|b| b.id == id) {
            chain = change(repo, config, reserve)?;
        }
        if let Some(c) =
            chain.candidates.iter().rev().find(|c| c.pr == pr.number && c.original_head == original)
        {
            if c.phase == Phase::Landed {
                let mut result = c.result.clone().unwrap_or(answer);
                settle(repo, &gh, issue, clock(), &mut result);
                result["outcome"] = json!(Ending::Merged);
                result["commit"] = json!(c.commit);
                return Ok(result);
            }
            if c.active() {
                if adopted.as_ref().is_some_and(|adopted| adopted.id == c.id) {
                    break;
                }
                waiting.after(&chain);
                continue;
            }
        }
        if (!barrier && chain.barriers.is_empty())
            || (barrier
                && chain.can_assign(true)
                && chain.barriers.first().is_some_and(|b| b.id == id))
        {
            break;
        }
        waiting.after(&chain);
    }
    let recovering = adopted.is_some();
    let id = adopted.as_ref().map_or_else(|| id.clone(), |c| c.id.clone());
    let mut permit = if recovering {
        None
    } else {
        Some(if barrier {
            Permit::take(repo, "turn: serial barrier", heavy, true)?
        } else {
            Permit::candidate(repo, "turn: validate candidate, record, land", heavy)?
        })
    };
    let path = root.join(&id);
    let candidate = if let Some(candidate) = adopted {
        candidate
    } else {
        loop {
            let locks = crate::lock::standing_locks(&gh, config)?;
            if issue.map_or(!locks.is_empty(), |n| keeps_off(&locks, n)) {
                return Ok(crate::turn::locked(answer, &locks));
            }
            let barrier = barrier || !locks.is_empty();
            let mut snapshot = read(repo, config)?;
            let current = snapshot.base.clone();
            if snapshot.chain.candidates.iter().any(|c| c.pr == pr.number && c.active()) {
                answer["outcome"] = json!(Ending::CandidateActive);
                answer["detail"] = json!(
                    "this PR already has an active candidate; its owner alone may validate and publish it"
                );
                return Ok(answer);
            }
            let declared = crate::turn::tip_config(repo, &current)?;
            let base_command = GateCommand::new(&declared.gate_command);
            let base_tree = Tree::named(&repo.git(&["rev-parse", &format!("{current}^{{tree}}")])?);
            let repairing = match issue.filter(|n| locks.contains(n)) {
                Some(n) => crate::repair::is_for(repo, &gh, n, &current, &config.base_branch)?,
                None => false,
            };
            if !repairing
                && crate::gate::recorded(repo, &base_tree, &base_command, clock())?.is_none()
            {
                answer["outcome"] = json!(Ending::BaseUncertified);
                answer["detail"] = json!(
                    "the base changed before assignment and needs certification; ask for another turn"
                );
                return Ok(answer);
            }
            // A barrier or an unconfirmed cancellation may hold the window.
            if !snapshot.chain.can_assign(barrier) {
                drop(permit);
                answer["outcome"] = json!(Ending::WaitingForCapacity);
                answer["detail"] = json!("the candidate window is held; ask for another turn");
                return Ok(answer);
            }
            snapshot.chain.observe_base(&current);
            let predecessor = snapshot.chain.predecessor(&current);
            let message = merge_message(&pr.title, pr.number, of, tracker);
            // A cut adds one commit: the set commit stays on the cut's branch.
            // An issue turn and a watch still land a merge.
            let built = match of {
                Of::Cut(_) => crate::turn::cut_onto(repo, &predecessor, &original, &message, now)?,
                _ => crate::turn::merged_onto(repo, &predecessor, &original, &message, now)?,
            };
            let Some(commit) = built else {
                if predecessor != current {
                    // The worker resolves it: its branch then carries the candidate ahead.
                    let files = crate::turn::merge_in(repo, &predecessor, "the candidate ahead")?;
                    answer["outcome"] = json!(Ending::SpeculativeConflict);
                    answer["status"] = json!("conflict");
                    answer["files"] = json!(files);
                    answer["recorded"] = json!(false);
                    answer["worktree"] = json!(repo.dir);
                    answer["merging"] = json!(predecessor);
                    answer["detail"] = json!(
                        "merging onto the candidate ahead conflicts; resolve every file, git add it, COMMIT the merge, and ask for a turn again"
                    );
                    return Ok(answer);
                }
                let files = crate::turn::merge_in(repo, &current, "the base branch's tip")?;
                answer["outcome"] = json!(Ending::Conflict);
                answer["status"] = json!("conflict");
                answer["files"] = json!(files);
                answer["recorded"] = json!(false);
                answer["worktree"] = json!(repo.dir);
                answer["detail"] = json!(
                    "resolve every file, git add it, COMMIT the merge, and ask for a turn again"
                );
                return Ok(answer);
            };
            let declared = crate::turn::tip_config(repo, &commit)?;
            if crate::config::heavy_gate(&declared.gate_command) && !heavy {
                answer["outcome"] = json!(Ending::GateLaneChanged);
                answer["detail"] = json!(
                    "the predecessor changed the gate to a worker lane; request a new turn after it lands"
                );
                return Ok(answer);
            }
            let tree = repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])?;
            // Where the commit splits its gate, the candidate runs the fast gate,
            // and a round at it the slow one (docs/adr/0048).
            let (command, seconds, slow) = match &declared.gates {
                Some(gates) => (
                    gates.fast.command.clone(),
                    gates.fast.seconds,
                    Some(Slow {
                        command: gates.slow.command.clone(),
                        seconds: gates.slow.seconds,
                        whole: declared.gate_command.clone(),
                    }),
                ),
                None => (declared.gate_command.clone(), declared.gate_seconds(), None),
            };
            let c = Candidate {
                id: id.clone(),
                pr: pr.number,
                issue,
                branch: pr.head_branch.clone(),
                original_head: original.clone(),
                predecessor,
                commit,
                tree,
                command,
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
                abandoned: false,
                result: None,
                slow,
                slowed: false,
            };
            snapshot.chain.candidates.push(c.clone());
            snapshot.chain.barriers.retain(|b| b.id != id);
            snapshot.chain.pending.retain(|b| b.id != id);
            if write(repo, config, &snapshot)? {
                break c;
            }
        }
    };
    answer["candidate"] = json!(id);
    answer["merged"] = json!(candidate.predecessor);
    answer["predecessor"] = json!(candidate.predecessor);
    let mut result = if recovering {
        candidate.result.clone().expect("a fast-green candidate has its gate result")
    } else {
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
        let hook = Gate { repo, config, id: &id, slow: false, clock: &clock };
        let result = crate::gate::candidate_gate(
            &checkout,
            &command,
            host,
            clock(),
            Duration::from_secs(candidate.seconds),
            issue,
            &hook,
        );
        match result {
            Ok(result) => result,
            Err(error) => {
                invalidate(repo, config, &id, "gate_interrupted")?;
                return Err(error);
            }
        }
    };
    for (key, value) in answer.as_object().into_iter().flatten() {
        result[key] = value.clone();
    }
    let chain = if recovering {
        read(repo, config)?.chain
    } else {
        let green = result["status"] == "green" && result["recorded"] == true;
        let chain = change(repo, config, |chain| chain.finish(&id, green, result.clone()))?;
        cleanup(repo, config, &chain)?;
        stop_candidate(repo, config, &id)?;
        chain
    };
    let current = chain.candidates.iter().find(|c| c.id == id).expect("assigned candidate");
    if current.phase == Phase::Red {
        result["outcome"] = json!(Ending::GateRed);
        return Ok(result);
    }
    // A green candidate waits for its round, or its predecessor, holding no
    // worker lane: the lanes bound the gates running, not the PRs waiting.
    if !barrier {
        permit = None;
    }
    let mut waiting = Waiting::new();
    loop {
        let chain = reconcile(repo, config, clock())?;
        let c = chain.candidates.iter().find(|c| c.id == id).expect("assigned candidate");
        // A round at a candidate behind this one may have landed it.
        if c.phase == Phase::Landed {
            settle(repo, &gh, issue, clock(), &mut result);
            result["outcome"] = json!(Ending::Merged);
            result["commit"] = json!(c.commit);
            result["detail"] = json!(
                "landed with the round at a candidate behind it, as a fast-forward; source head preserved — stop"
            );
            return Ok(result);
        }
        if !c.valid() {
            return Ok(cancelled(result, c, &chain));
        }
        let locks = crate::lock::standing_locks(&gh, config)?;
        if issue.map_or(!locks.is_empty(), |n| keeps_off(&locks, n)) {
            invalidate(repo, config, &id, "fleet_lock")?;
            return Ok(crate::turn::locked(result, &locks));
        }
        if chain.round_tip().is_some_and(|tip| tip.id == id) {
            let round_of = Round { repo, config, tracker, id: &id, host, issue, heavy };
            if let Some(red) = round(round_of, clock)? {
                for (key, value) in red.as_object().into_iter().flatten() {
                    result[key] = value.clone();
                }
                return Ok(result);
            }
            continue;
        }
        let base = crate::turn::base(repo, config)?;
        if !chain.ripe(&id, &base) {
            waiting.after(&chain);
            continue;
        }
        if let Some(n) = issue {
            let blockers = gh.open_blockers(n)?;
            if !blockers.is_empty() {
                invalidate(repo, config, &id, "issue_blocked")?;
                result["outcome"] = json!(Ending::Blocked);
                result["blocked_by"] = json!(blockers);
                return Ok(result);
            }
        }
        let current_pr = gh.pr_of_branch(&pr.head_branch)?.context(
            "internal PR disappeared before publication; restore it before requesting another turn",
        )?;
        if let Err(error) = crate::turn::ensure_base(&current_pr, &config.base_branch) {
            invalidate(repo, config, &id, "pr_base_changed")?;
            return Err(error);
        }
        if let Some(n) = issue {
            ensure!(
                crate::target::branch(repo, config, n)? == config.base_branch,
                "pinned target changed during validation; restore its record before continuing"
            );
        }
        let mut snapshot = read(repo, config)?;
        let members: Vec<Candidate> = snapshot.chain.members(&id).into_iter().cloned().collect();
        let Some(c) = members.last() else {
            bail!("candidate record disappeared");
        };
        // The whole gate on its tree: its own gate, or its round's two parts.
        let whole = GateCommand::new(c.slow.as_ref().map_or(&c.command, |slow| &slow.whole));
        let tree = Tree::named(&c.tree);
        let run = crate::gate::recorded(repo, &tree, &whole, clock())?;
        let mut heads = true;
        for m in &members {
            let head = repo.remote().sha(&format!("refs/heads/{}", m.branch), Fresh::Live)?;
            heads &= head.as_deref() == Some(&m.original_head);
        }
        let Some(publication) = Publication::new(
            members.iter().all(Candidate::valid),
            snapshot.chain.ripe(&id, &base) && run.is_some(),
            members.first().is_some_and(|m| m.predecessor == base),
            true,
            heads,
            members.iter().all(|m| m.stopped),
        ) else {
            invalidate(repo, config, &id, "publication_evidence_changed")?;
            let chain = read(repo, config)?.chain;
            let c = chain.candidates.iter().find(|c| c.id == id).expect("assigned candidate");
            return Ok(cancelled(result, c, &chain));
        };
        let run = run.expect("publication requires green record");
        let proof = Proven::by(&tree, &whole, &run, clock())
            .ok_or_else(|| anyhow!("candidate green record expired"))?;
        let landed = c.commit.clone();
        snapshot.chain.land(&id);
        let next = commit(repo, &snapshot)?;
        let pushed = publish(repo, config, &members, &snapshot, &next, &proof, &publication)?;
        if !pushed {
            if crate::turn::base(repo, config)? != base {
                continue;
            }
            if repo.remote().sha(&refname(repo, config), Fresh::Process)? == snapshot.sha {
                result["outcome"] = json!(Ending::TargetMoved);
                invalidate(repo, config, &id, "push_rejected")?;
                return Ok(result);
            }
            continue;
        }
        settle(repo, &gh, issue, clock(), &mut result);
        result["outcome"] = json!(Ending::Merged);
        result["commit"] = json!(landed);
        result["landed"] = json!(members.iter().map(|m| m.pr).collect::<Vec<_>>());
        result["detail"] = json!(
            "landed the exact recorded candidate as a fast-forward; source head preserved — stop"
        );
        let prs: Vec<String> = members.iter().map(|m| format!("#{}", m.pr)).collect();
        crate::acting::Acting::new("land").on_issue(issue).in_checkout(Some(repo.clone())).say(
            crate::clock(),
            format!("turn landed PR {} on {} at {landed}", prs.join(", "), config.base_branch),
        );
        drop(permit);
        return Ok(result);
    }
}

/// What a round's slow gate runs with: the tip's turn.
struct Round<'a> {
    repo: &'a Repo,
    config: &'a Config,
    tracker: &'a Tracker,
    id: &'a str,
    host: &'a str,
    issue: Option<u64>,
    heavy: bool,
}

/// Runs the round at the tip `id`: the slow gate, once, on the tip's commit,
/// in the worktree its fast gate ran in → `None` when the round is green, or
/// was not this turn's to run; the tip turn's answer when it is red.
///
/// Green puts the whole gate on record for the tip's tree, from its fast
/// and slow runs. Red is not bisected: the worker's branch is brought up to
/// the round's tip, which holds every change of the round, and the worker
/// fixes the round from its issues and the failing tests (docs/adr/0048).
fn round(round: Round<'_>, clock: impl Fn() -> u64) -> Result<Option<Value>> {
    let Round { repo, config, tracker, id, host, issue, heavy } = round;
    // The slow gate takes a worker lane again; a barrier still holds its own.
    let _lane = if crate::exclusive::held() {
        None
    } else {
        Some(Permit::slow_gate(repo, "turn: a round's slow gate", heavy)?)
    };
    let mut started = false;
    let chain = change(repo, config, |chain| started = chain.start_round(id))?;
    if !started {
        return Ok(None);
    }
    let members: Vec<Candidate> = chain.members(id).into_iter().cloned().collect();
    let tip = members.last().context("round tip disappeared")?.clone();
    let slow = tip.slow.clone().context("a round needs a slow gate")?;
    let checkout = Repo::new(PathBuf::from(&tip.worktree));
    // The fast gate's scope was confirmed free when it stopped; the slow
    // gate's processes are owned through a scope of their own.
    let scope = crate::process_scope::path(&tip.worktree);
    if scope.exists() {
        ensure!(crate::process_scope::free(&scope)?, "the fast gate's processes are still alive");
        std::fs::remove_file(&scope)?;
    }
    let command = GateCommand::new(&slow.command);
    let hook = Gate { repo, config, id, slow: true, clock: &clock };
    let result = crate::gate::candidate_gate(
        &checkout,
        &command,
        host,
        clock(),
        Duration::from_secs(slow.seconds),
        issue,
        &hook,
    );
    let mut result = match result {
        Ok(result) => result,
        Err(error) => {
            invalidate(repo, config, id, "gate_interrupted")?;
            return Err(error);
        }
    };
    let mut green = result["status"] == "green" && result["recorded"] == true;
    if green {
        let tree = Tree::named(&tip.tree);
        let parts = (&GateCommand::new(&slow.whole), &GateCommand::new(&tip.command), &command);
        if let Some(why) =
            crate::gate::record_whole(repo, &tree, &tip.commit, parts, issue, clock())?
        {
            green = false;
            result["recorded"] = json!(false);
            result["not_recorded"] = json!(why);
        }
    }
    let chain = change(repo, config, |chain| chain.finish(id, green, result.clone()))?;
    cleanup(repo, config, &chain)?;
    stop_candidate(repo, config, id)?;
    if chain.candidates.iter().any(|c| c.id == id && c.phase != Phase::Red) {
        return Ok(None);
    }
    let files = crate::turn::merge_in(repo, &tip.commit, "the round's tip")?;
    result["outcome"] = json!(Ending::GateRed);
    result["gate"] = json!("slow");
    result["round"] = json!(
        members
            .iter()
            .map(|m| json!({"pr": m.pr, "issue": m.issue.map(|n| tracker.issue(n))}))
            .collect::<Vec<_>>()
    );
    result["files"] = json!(files);
    result["detail"] = json!(if files.is_empty() {
        "the slow gate ran red on the round at this candidate; your branch now holds the round's tip, every change of it: do not split it; read the issues of `round` and the failing tests, fix the round, commit and ask for a turn again"
    } else {
        "the slow gate ran red on the round at this candidate; bringing your branch up to the round's tip conflicts: resolve `files`, commit, then fix the round from the issues of `round` and the failing tests, and ask for a turn again"
    });
    Ok(Some(result))
}

/// Publishes `members`, the candidates a landing takes, in one atomic push:
/// the base fast-forwards from the first one's predecessor to the last one's
/// commit, the record advances, and every source head is checked unchanged.
fn publish(
    repo: &Repo,
    config: &Config,
    members: &[Candidate],
    snapshot: &Snapshot,
    state: &str,
    proven: &Proven,
    _publication: &Publication,
) -> Result<bool> {
    let (Some(first), Some(last)) = (members.first(), members.last()) else {
        bail!("a landing takes at least one candidate");
    };
    let base = &first.predecessor;
    let old_state = snapshot.sha.as_deref().context("candidate record disappeared")?;
    ensure!(
        repo.git(&["rev-parse", &format!("{}^{{tree}}", last.commit)])? == proven.tree(),
        "candidate tree differs from recorded proof"
    );
    let mut expected = base;
    for m in members {
        ensure!(
            m.predecessor == *expected
                && repo.git(&["rev-parse", &format!("{}^1", m.commit)])? == *expected,
            "candidate does not fast-forward its expected predecessor"
        );
        expected = &m.commit;
    }
    ensure!(
        repo.git(&["rev-parse", &format!("{state}^1")])? == old_state,
        "candidate state does not extend its expected record"
    );
    let state_ref = refname(repo, config);
    let base_ref = format!("refs/heads/{}", config.base_branch);
    let sources: Vec<String> = members.iter().map(|m| format!("refs/heads/{}", m.branch)).collect();
    let mut updates: Vec<crate::remote::Update<'_>> = members
        .iter()
        .zip(&sources)
        .map(|(m, name)| crate::remote::Update {
            name,
            old: &m.original_head,
            new: &m.original_head,
        })
        .collect();
    updates.push(crate::remote::Update { name: &state_ref, old: old_state, new: state });
    updates.push(crate::remote::Update { name: &base_ref, old: base, new: &last.commit });
    repo.remote().publish(&updates)
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn an_unreadable_pid_is_not_the_candidate_owner() {
        let recorded = super::Process {
            host: crate::this_host(),
            pid: 1,
            started: "the recorded turn owner".into(),
        };
        assert!(!super::alive(&recorded).unwrap());
    }

    #[test]
    fn a_branch_whose_live_turn_holds_a_valid_candidate_is_landing() {
        let me = super::identity(std::process::id()).unwrap().unwrap();
        let chain = |phase: &str, owner: &super::Process, cancelled: Option<&str>| {
            let c = serde_json::json!({
                "id": "c0", "pr": 1, "issue": 1, "branch": "b", "original_head": "h",
                "predecessor": "base", "commit": "m", "tree": "t", "command": "fast",
                "seconds": 60, "phase": phase, "owner": owner, "gate": null,
                "descendants": [], "worktree": "w", "barrier": false,
                "cancelled_by": cancelled, "cause": null, "stopped": false, "result": null,
            });
            let subject = serde_json::json!({"candidates": [c]}).to_string();
            vec![crate::remote::RemoteRecord {
                name: "refs/convoy/candidates/main".into(),
                sha: "s".into(),
                subject,
            }]
        };
        let landing = |records: &[crate::remote::RemoteRecord], branch| {
            super::landing(records, "refs/convoy/candidates/", branch)
        };
        assert!(landing(&chain("green", &me, None), "b"), "waiting for its round");
        assert!(!landing(&chain("green", &me, None), "other"), "another branch's");
        assert!(!landing(&chain("green", &me, Some("c9")), "b"), "cancelled");
        let dead = super::Process { pid: u32::MAX - 1, ..me.clone() };
        assert!(!landing(&chain("slow", &dead, None), "b"), "its turn is gone");
    }

    #[test]
    fn a_wait_on_a_chain_that_does_not_change_grows_to_a_second_and_a_change_starts_it_again() {
        use std::time::Duration;
        let ms = Duration::from_millis;
        let mut waiting = super::Waiting::new();
        let still = super::Chain::default();
        let waits: Vec<_> = (0..6).map(|_| waiting.wait(&still)).collect();
        assert_eq!(waits, [ms(100), ms(200), ms(400), ms(800), ms(1000), ms(1000)]);
        let moved = super::Chain {
            pending: vec![super::Barrier {
                id: "b".into(),
                owner: super::identity(std::process::id()).unwrap().unwrap(),
            }],
            ..super::Chain::default()
        };
        assert_eq!(waiting.wait(&moved), ms(100));
        assert_eq!(waiting.wait(&moved), ms(200));
    }

    #[test]
    fn a_ref_lock_held_by_another_push_is_told_from_a_remote_fault() {
        let held = "To https://github.com/o/r.git\n ! [remote rejected]   31515b8c -> refs/convoy/candidates/main (failed)\nerror: failed to push some refs";
        assert!(super::lock_failure(held));
        assert!(super::lock_failure(
            "error: cannot lock ref 'refs/convoy/candidates/main': is at 1 but expected 2"
        ));
        assert!(!super::lock_failure(
            " ! [remote rejected] HEAD -> refs/convoy/candidates/main (permission denied)"
        ));
        assert!(!super::lock_failure(" ! [rejected]        HEAD -> main (non-fast-forward)"));
        assert!(!super::lock_failure(
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host"
        ));
    }
}
