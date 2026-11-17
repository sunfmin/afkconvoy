//! A promotion (docs/adr/0090): the base branch fast-forwarded to the
//! proving branch's tip once the slow gate passed there. The proving branch
//! holds every round that landed on it fast-green; the base branch moves
//! only by this.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::{GateCommand, Judged};
use afkconvoy_landing::proving::{self, Outcome, Tips};
use afkconvoy_landing::turn::Declared;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::acting::Acting;
use crate::gate::{Detached, Gated};
use crate::git::Repo;
use crate::remote::{Fresh, Push};

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
    /// How many of them each host landed, by the host its landing commit
    /// names (sunfmin/control#1334): an older release's names none, and is
    /// counted in `unpromoted` alone.
    pub hosts: BTreeMap<String, u64>,
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
        let hosts = hosts_of(repo, &landed)?;
        // The oldest change the base branch does not hold: the first PR
        // landed past it, or, where none is known as one, the first commit
        // the proving branch took past it.
        let mut oldest = None;
        if let Some(first) = landed.first() {
            let at = repo.git(&["show", "-s", "--format=%ct", &first.commit])?;
            oldest = at.trim().parse().ok();
        } else if fast_forward && base != tip {
            let range = format!("{base}..{tip}");
            let times = repo.git(&["log", "--first-parent", "--format=%ct", &range])?;
            oldest = times.lines().filter_map(|at| at.trim().parse().ok()).min();
        }
        Ok(Some(Standing {
            base_branch: config.base_branch.clone(),
            proving_branch: proving,
            base,
            tip,
            unpromoted: landed.len() as u64,
            oldest,
            hosts,
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
            "landed_on": self.hosts,
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

/// How long the oldest change on the proving branch, not yet on the base
/// branch, waits for an idle host before its promotion starts anyway, where
/// the host says none (`afkconvoy run --promotion-seconds`).
pub const BOUND_SECONDS: u64 = 3600;

/// The host as a promotion reads it whether to start: as the exclusive queue
/// and admission read it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Host {
    /// The commands of any repo waiting in the exclusive queue.
    pub waiting: usize,
    /// Those running there.
    pub running: usize,
    /// How busy the cores count as, in percent; `None`: no reading.
    pub busy: Option<u64>,
    /// The busy share the queue starts a command under.
    pub share: u64,
    /// Why the host could not be read; it then counts as busy.
    pub unread: Option<String>,
}

impl Host {
    /// This host now, as its host runner says it; with none running, its
    /// queue is empty and its cores unread.
    pub fn read() -> Host {
        let share = crate::host_settings::of_host().unwrap_or_default().busy_share();
        match crate::host_runner::shared() {
            Ok(Some(shared)) => Host::of(&shared.exclusive, shared.exclusive_room, share),
            Ok(None) => Host { share, ..Host::default() },
            Err(error) => Host { share, unread: Some(format!("{error:#}")), ..Host::default() },
        }
    }

    /// The host whose queue holds `queue`, its room `room`, the busy share
    /// `share` where the room says none.
    pub fn of(
        queue: &[afkconvoy_core::exclusive::Ticket],
        room: Option<afkconvoy_core::exclusive::Room>,
        share: u64,
    ) -> Host {
        let running = queue.iter().filter(|ticket| ticket.ran.is_some()).count();
        Host {
            waiting: queue.len() - running,
            running,
            busy: room.and_then(|room| room.busy()),
            share: room.map_or(share, |room| room.share),
            unread: None,
        }
    }

    /// Why the host is not idle; `None`: it is — no command of any repo
    /// waits or runs in its exclusive queue, and its cores are under the busy
    /// share.
    pub fn busy_because(&self) -> Option<String> {
        if let Some(why) = &self.unread {
            return Some(format!("host unread: {why}"));
        }
        if self.waiting + self.running > 0 {
            return Some(format!(
                "host busy: {} worker commands waiting, {} running in the exclusive queue",
                self.waiting, self.running
            ));
        }
        match self.busy {
            Some(busy) if busy >= self.share => Some(format!(
                "host busy: its cores {busy}% busy, at or over the busy share {}%",
                self.share
            )),
            _ => None,
        }
    }
}

/// Whether a promotion of a proving branch starts now (docs/adr/0090): one
/// is due while the branch holds what the base branch does not, and starts
/// while the host is idle, or once the oldest of it has waited the bound.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Due {
    /// Nothing to promote, or nothing a promotion could move.
    Not,
    /// Due, and the host is idle.
    Idle,
    /// Due, the host busy, and the oldest change waited `waited` seconds,
    /// the bound `bound` or more: it starts anyway.
    Past { waited: u64, bound: u64, busy: String },
    /// Due, and the host busy: it waits for it to be idle, `left` seconds
    /// more at most.
    Busy { busy: String, left: u64 },
}

impl Due {
    /// Of `standing` on `host`, the bound `bound`, at `now`.
    pub fn of(standing: &Standing, host: &Host, bound: u64, now: u64) -> Due {
        if !standing.ahead() || !standing.fast_forward {
            return Due::Not;
        }
        let Some(busy) = host.busy_because() else { return Due::Idle };
        let waited = standing.oldest.map_or(0, |at| now.saturating_sub(at));
        match waited >= bound {
            true => Due::Past { waited, bound, busy },
            false => Due::Busy { busy, left: bound - waited },
        }
    }

    /// Whether it starts a promotion.
    pub fn starts(&self) -> bool {
        matches!(self, Due::Idle | Due::Past { .. })
    }

    /// What the tick, `plan` and `doctor` say of it; `None`: nothing is due.
    pub fn said(&self) -> Option<String> {
        Some(match self {
            Due::Not => return None,
            Due::Idle => "the promotion starts: the host is idle".to_string(),
            Due::Past { waited, bound, busy } => format!(
                "the promotion starts anyway: the oldest unpromoted change waited {waited}s, \
                 past the bound of {bound}s ({busy})"
            ),
            Due::Busy { busy, left } => format!(
                "a due promotion has not started: {busy}; it starts once the host is idle, or \
                 in {left}s at the bound"
            ),
        })
    }

    /// The same as `plan` and `doctor` answer it: {"due", "said"}.
    pub fn json(&self) -> Value {
        let due = match self {
            Due::Not => "not",
            Due::Idle => "idle",
            Due::Past { .. } => "past_bound",
            Due::Busy { .. } => "host_busy",
        };
        json!({"due": due, "said": self.said()})
    }
}

/// `afkconvoy promote`: gates the proving branch's tip with its slow gate —
/// its whole gate where no fast run of that tree is on record — and, green,
/// fast-forwards the base branch to it. Nothing else ever moves the base
/// branch of a repo with a proving branch.
///
/// The promotion is the landing crate's (`proving::promote`), which this
/// host runs (`Runner`): its lane in the exclusive queue, its gate in a
/// worktree detached at the tip. Several hosts land on one proving branch
/// (sunfmin/control#1334), so the tip is read from the remote before the
/// gate's lane is asked for, and again once it is given: the tip gated is
/// every host's landings up to then, in one run. What follows is the
/// fleet's: the answer, the repair issue of a red tip, closed by a green
/// one, and the line in the fleet's log.
pub fn promote(repo: &Repo, host: &str, now: u64) -> Result<Value> {
    let config = crate::config::of(repo)?;
    let tracker = crate::gh::GitHub::of(repo)?.tracker;
    if config.proving_branch.is_none() {
        return Ok(json!({
            "outcome": "none",
            "detail": "this repo declares no proving_branch: it lands on its base branch, and \
                       nothing is promoted (afkconvoy config --help)",
        }));
    }
    let output = repo.common_dir()?.join(format!("convoy-promotion-{now}-{}.log", crate::unique()));
    let runner = Runner {
        repo,
        config: &config,
        tracker: &tracker,
        host,
        now,
        output: &output,
        standing: RefCell::new(None),
        records: crate::gate::OnRemote(repo),
    };
    let promotion = proving::promote(&runner, now)?;
    let standing =
        runner.standing.into_inner().context("a promotion reads the remote's tips first")?;
    let tip = standing.tip.clone();
    let mut answer = standing.json(now);
    answer["outcome"] = json!(promotion.outcome.word());
    let declared = match promotion.ran {
        Some(ran) => {
            let declared = crate::turn::tip_config(repo, &tip, &standing.base_branch)?;
            answer["gated_tip"] = json!(tip);
            answer["gate"] = json!(ran.word());
            answer["gate_command"] = json!(declared.gate_command);
            Some(declared)
        }
        None => None,
    };
    let base_branch = &standing.base_branch;
    let mut said = true;
    let detail = match &promotion.outcome {
        Outcome::Current => {
            said = false;
            "the base branch is at the proving branch's tip".to_string()
        }
        Outcome::Promoting => {
            said = false;
            "another promotion of this checkout runs; it answers for it".to_string()
        }
        Outcome::Diverged { base } if declared.is_none() => format!(
            "the base branch {base_branch} is not an ancestor of the proving branch's tip: a \
             promotion never moves it to a commit that is not a fast-forward of it; it stays at \
             {base}"
        ),
        Outcome::Diverged { base } => format!(
            "the base branch {base_branch} moved to {base}, which is not an ancestor of the gated \
             tip {tip}: a promotion never moves it to what is not a fast-forward of it"
        ),
        Outcome::Promoted { from } => {
            format!("fast-forwarded {base_branch} from {from} to {tip}, {}", covered(&standing))
        }
        Outcome::AlreadyPromoted { base } => format!(
            "the gated tip {tip} is in the base branch {base_branch} already, at {base}: another \
             promotion took it there; nothing is pushed"
        ),
        Outcome::TargetMoved { .. } => format!(
            "the remote refused the fast-forward of {base_branch} to {tip} {} times: it moved \
             each time; the next promotion reads it again",
            proving::PUBLISH_TRIES
        ),
        Outcome::Unrecorded(why) => {
            bail!("the gate passed on {tip}, but the whole gate is not on record: {why}")
        }
        Outcome::GateRed(red) => {
            answer["failing"] = json!(red.tests);
            answer["gate_output"] = json!(output);
            let said = red.said(&format!("the proving branch's tip {tip}"));
            let declared = declared.as_ref().context("a red promotion ran a gate")?;
            let issue = repair(repo, &tracker, &standing, declared, (&said, &red.tests, &output))?;
            answer["repair_issue"] = json!(issue);
            format!("{said}; the base branch {base_branch} stays at {}", standing.base)
        }
        Outcome::GateUnsettled(unsettled) => {
            answer["gate_output"] = json!(output);
            format!(
                "the proving branch's tip {tip} is unsettled: {}; nothing is recorded, and the \
                 base branch {base_branch} stays at {}",
                unsettled.how(),
                standing.base
            )
        }
    };
    answer["detail"] = json!(detail);
    match &promotion.outcome {
        Outcome::Promoted { from: base }
        | Outcome::AlreadyPromoted { base }
        | Outcome::TargetMoved { base }
        | Outcome::Diverged { base } => {
            answer["base_tip"] = json!(base);
        }
        _ => {}
    }
    let green = matches!(
        promotion.outcome,
        Outcome::Promoted { .. } | Outcome::AlreadyPromoted { .. } | Outcome::TargetMoved { .. }
    ) || (matches!(promotion.outcome, Outcome::Diverged { .. }) && declared.is_some());
    if green {
        let _ = std::fs::remove_file(&output);
        answer["pushed"] = json!(matches!(promotion.outcome, Outcome::Promoted { .. }));
    }
    // The gated tip is in the base branch, by this promotion or another
    // host's: a repair of a red promotion is done.
    if matches!(promotion.outcome, Outcome::Promoted { .. } | Outcome::AlreadyPromoted { .. })
        && let Some(issue) = crate::promotion_repair::green(repo, &standing.proving_branch, &tip)?
    {
        answer["repair_closed"] = json!(issue);
    }
    if said {
        say(repo, &standing, &answer);
    }
    Ok(answer)
}

/// Opens the repair issue of the red proving tip `standing` read, or
/// comments on the one open (docs/adr/0090), naming every issue landed past
/// the base branch, the failing tests and the gate's `output` → its number.
fn repair(
    repo: &Repo,
    tracker: &afkconvoy_core::tracker::Tracker,
    standing: &Standing,
    declared: &Config,
    (said, failing, output): (&str, &[String], &Path),
) -> Result<Value> {
    let tip = &standing.tip;
    let landed: Vec<(Option<u64>, u64)> =
        crate::landing::joins_in(repo, tracker, &[&format!("{}..{tip}", standing.base)])?
            .iter()
            .map(|landed| (landed.issue(), landed.pr))
            .collect();
    let text = std::fs::read_to_string(output).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let excerpt = lines[lines.len().saturating_sub(80)..].join("\n");
    let issue = crate::promotion_repair::red(
        repo,
        declared,
        &crate::promotion_repair::Red {
            proving: &standing.proving_branch,
            tip,
            said,
            failing,
            output,
            excerpt: &excerpt,
            landed: &landed,
            gate_command: &declared.gate_command,
        },
    )?;
    Ok(json!(issue))
}

/// A promotion as this host runs it (`proving::Proving`): the remote read
/// live, one promotion of a checkout at a time (`Lock`), its gate's lane in
/// the host's exclusive queue, and its gate run in a worktree detached at
/// the tip, its whole output kept in `output`.
struct Runner<'a> {
    repo: &'a Repo,
    config: &'a Config,
    tracker: &'a afkconvoy_core::tracker::Tracker,
    host: &'a str,
    now: u64,
    output: &'a Path,
    /// What the last read of the tips saw: what the answer says.
    standing: RefCell<Option<Standing>>,
    records: crate::gate::OnRemote<'a>,
}

impl<'a> proving::Proving for Runner<'a> {
    type Error = anyhow::Error;
    type Records = crate::gate::OnRemote<'a>;
    type Lane = (Lock, Option<crate::exclusive::Permit>);

    fn records(&self) -> &Self::Records {
        &self.records
    }

    fn tips(&self) -> Result<Tips> {
        let standing = Standing::read(self.repo, self.config, self.tracker)?
            .context("the repo's proving branch went from its config while the promotion waited")?;
        let tips = Tips { base: standing.base.clone(), proving: standing.tip.clone() };
        *self.standing.borrow_mut() = Some(standing);
        Ok(tips)
    }

    fn base(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        let base_ref = format!("refs/heads/{branch}");
        let base = self
            .repo
            .remote()
            .refs(&[&base_ref], Fresh::Live)?
            .into_iter()
            .find(|r| r.name == base_ref)
            .map(|r| r.sha)
            .ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))?;
        self.repo.bring(&base, &base_ref)?;
        Ok(base)
    }

    fn holds(&self, head: &str, other: &str) -> Result<bool> {
        self.repo.within(other, head)
    }

    fn tree(&self, commit: &str) -> Result<String> {
        self.repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])
    }

    /// The tip's own declaration, as a landing on the base branch reads it.
    fn declared(&self, tip: &str) -> Result<Declared> {
        let config = crate::turn::tip_config(self.repo, tip, &self.config.base_branch)?;
        Ok(Declared {
            seconds: config.gate_seconds(),
            whole: config.gate_command,
            gates: config.gates,
        })
    }

    fn lane(&self, declared: &Declared) -> Result<Option<Self::Lane>> {
        let Some(lock) = Lock::take(self.repo)? else { return Ok(None) };
        let heavy = crate::config::heavy_gate(&declared.whole)
            || declared.gates.as_ref().is_some_and(|gates| {
                crate::config::heavy_gate(&gates.fast.command)
                    || crate::config::heavy_gate(&gates.slow.command)
            });
        let permit = match crate::exclusive::held() {
            true => None,
            false => Some(crate::exclusive::Permit::promotion(
                self.repo,
                "promote: the proving branch's tip",
                heavy,
            )?),
        };
        Ok(Some((lock, permit)))
    }

    fn gate(&self, tip: &str, command: &GateCommand, seconds: u64) -> Result<Judged> {
        let limit = Duration::from_secs(seconds);
        let (gated, removal) = crate::gate::detached(self.repo, tip, Detached::Promotion, |at| {
            crate::gate::gated(at, command, self.host, self.now, limit, None, Some(self.output))
        })?;
        if let Some(error) = removal {
            eprintln!("warning: {error:#}");
        }
        Ok(match gated? {
            Gated::Green { .. } => Judged::Green,
            Gated::Red { red, .. } => Judged::Red(red),
            Gated::Unsettled { unsettled, .. } => Judged::Unsettled(unsettled),
        })
    }

    fn push(&self, tip: &str, base: &str) -> Result<bool> {
        let base_ref = format!("refs/heads/{}", self.config.base_branch);
        Ok(self.repo.remote().try_push(&Push::set(tip, &base_ref).leased(base))?.ok)
    }
}

/// How many landings a promotion of `standing` covers, and whose: `3
/// landing(s), from a 2 and b 1`.
fn covered(standing: &Standing) -> String {
    let mut said = format!("{} landing(s)", standing.unpromoted);
    if !standing.hosts.is_empty() {
        let hosts: Vec<String> =
            standing.hosts.iter().map(|(host, n)| format!("{host} {n}")).collect();
        said.push_str(&format!(", from {}", hosts.join(", ")));
    }
    let named: u64 = standing.hosts.values().sum();
    if named < standing.unpromoted {
        said.push_str(&format!(
            "; {} name no host, landed by an older release",
            standing.unpromoted - named
        ));
    }
    said
}

/// How many of `landed` each host's turn made, as its landing commit names
/// it (`landed_on`).
fn hosts_of(
    repo: &Repo,
    landed: &[afkconvoy_core::landing::LandedPr],
) -> Result<BTreeMap<String, u64>> {
    let mut hosts = BTreeMap::new();
    if landed.is_empty() {
        return Ok(hosts);
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%B%x00"];
    args.extend(landed.iter().map(|join| join.commit.as_str()));
    let log = repo.git_raw(&args)?;
    for message in log.split('\0') {
        if let Some(host) = afkconvoy_core::landing::landing_host(message) {
            *hosts.entry(host.to_string()).or_insert(0) += 1;
        }
    }
    Ok(hosts)
}

/// Says in the fleet's log how a promotion ended: what `run` echoes, a line
/// of the tick log, naming the proving tip it read and how many landings,
/// from which hosts, it covers.
fn say(repo: &Repo, standing: &Standing, answer: &Value) {
    let outcome = answer["outcome"].as_str().unwrap_or("");
    let detail = answer["detail"].as_str().unwrap_or("");
    let line = format!(
        "promotion {outcome}: {detail} (proving tip {}, {})",
        standing.tip,
        covered(standing)
    );
    Acting::new("promote").in_checkout(Some(repo.clone())).say(crate::clock(), line);
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

/// Whether a promotion of `standing` starts now, on this host as it is now,
/// by the host's bound: what the tick does and `plan` and `doctor` say.
pub fn due(standing: &Standing, now: u64) -> Due {
    let bound = crate::host_settings::of_host().unwrap_or_default().promotion_seconds();
    match standing.ahead() && standing.fast_forward {
        true => Due::of(standing, &Host::read(), bound, now),
        false => Due::Not,
    }
}

/// What a tick does of the proving branch (docs/adr/0090): where it is ahead
/// of the base branch and the host is idle, or the oldest change it holds
/// past the base branch has waited the bound, the tick starts a promotion of
/// its own, detached from it, unless one runs → what the tick's line says of
/// it; `None`: nothing. Its answer goes to the checkout's promotion log; how
/// it ended is said in the fleet's log.
pub fn tend(
    repo: &Repo,
    config: &Config,
    tracker: &afkconvoy_core::tracker::Tracker,
    now: u64,
) -> Result<Option<String>> {
    let Some(standing) = Standing::read(repo, config, tracker)? else { return Ok(None) };
    if !standing.ahead() || !standing.fast_forward || Lock::held(repo) {
        return Ok(None);
    }
    let due = due(&standing, now);
    if !due.starts() {
        return Ok(due.said());
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
    Ok(due.said())
}

/// What `afkconvoy promote --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Any time, in a checkout of a repo whose config names a `proving_branch` (`afkconvoy config --help`). A tick runs it by itself, detached from the tick, where the proving branch is ahead of the base branch and no promotion of the checkout runs, while the host is idle: no command of any repo waits or runs in the exclusive queue, and the host's CPU reading is under the busy share (`afkconvoy exclusive --help`). Once the oldest change the proving branch holds past the base branch has waited the promotion bound, {const:PROMOTION_SECONDS}s or what `afkconvoy run --promotion-seconds N` says, the tick starts it anyway, its slow gate queued as any heavy command. A promotion started on an idle host is not ended when a command then waits. The tick's line, `plan` and `doctor` (`proving.promotion`: `due` — `idle`, `host_busy`, `past_bound` or `not` — and `said`) say why a due promotion has not started, as `host busy: N worker commands waiting, M running in the exclusive queue`, and, past the bound, that it starts anyway. Run it by hand to promote now, whatever the host.

## Who runs it

The fleet, at its ticks, and the operator.

## What it does

In a repo with a proving branch, a round lands on the proving branch once its fast gate is green: its root merged onto the proving branch's tip and fast-gated only, each PR answering `merged` and its issue closing (`afkconvoy land --help`). Every host of the repo lands on the one proving branch, each push requiring the tip its round was built on; a push refused because another host moved it is built again on the new tip and gated again. Each landing commit names the host whose turn made it (`Landed on <host>`). The base branch moves only by a promotion, which is this command:

1. It reads the base branch's tip and the proving branch's tip on the remote. The same commit: `current`, nothing to do. The base branch's tip not an ancestor of the proving branch's: `diverged`, and nothing moves — a promotion never moves the base branch to a commit that is not a fast-forward of it.
2. It asks for its lane in the host's exclusive queue, a slow lane, and once given it reads both tips from the remote again: the tip it gates is every host's landings up to then, in one run. Read again, `current` and `diverged` answer as in step 1.
3. It gates that tip, in a worktree of its own detached at it, by the gate that tip's config declares. A green run of `gate_command` on record for its tree answers. Else, where the fast gate's green run of that tree is on record — the last round's landing put it there — it runs the slow gate alone and puts `gate_command` on record from the two runs, as at a round's tip (ADR-0083). Else it runs `gate_command` whole.
4. Green: it reads the base branch again. Where it holds the gated tip already — another host promoted it meanwhile — nothing is pushed: `already_promoted`. Where it is still an ancestor of the tip, it pushes the tip to it, a push the remote refuses unless the base branch is still at the value it read and the tip is a fast-forward of it: `promoted`. Refused, it reads the base branch again, three times at most, then answers `target_moved`. The base branch never takes a merge. Red: the base branch stays where it was, and the answer and the fleet's log say so: `gate_red`, with the failing tests and the run's whole output. A run red only on TIMEOUTs the gate cannot classify on an overloaded host: `gate_unsettled`, nothing recorded.

One promotion of a checkout runs at a time: another answers `promoting`. A repo with no proving branch answers `none`.

## What it reads and writes

It reads the remote's base branch and proving branch and the gate records, and writes a gate record of the tip's tree and, green, the base branch. Its run's output is in the checkout's git dir (`convoy-promotion-<time>-<n>.log`), kept where it is red; a tick's promotion appends its answer to `convoy-promotion.log` there. Its line in the fleet's log names the proving tip it read last and how many landings, from which hosts, it covers.

## What it answers

{"outcome", "detail", "proving_branch", "proving_tip", "base_branch", "base_tip", "unpromoted", "landed_on", "oldest_waited_seconds", "fast_forward"} — `proving_tip` the tip it read last, `unpromoted` how many landings past the base branch it covers, and `landed_on` how many of them each host landed, by the host its landing commit names (an older release's names none, and is counted in `unpromoted` alone) — and once it gated, {"gated_tip"}, the tip it gated, {"gate"} — `recorded`, `slow` or `whole`, what ran — and {"gate_command"}; green, {"pushed"}, whether it pushed the base branch; red, {"failing"}, the tests its output names failing, and {"gate_output"}, the file that keeps it. `outcome` is one of: none, current, diverged, promoting, promoted, already_promoted, gate_red, gate_unsettled, target_moved — the remote refused the fast-forward each time it read the base branch again.

## What trips you up

- A red promotion opens one repair issue (`repair_issue`), labelled as the fleet's repairs are, naming the failing tests, the gate output and every issue landed since the last promotion; a second red while it is open is a comment on it. The fleet starts a repair worker on it; its fix lands on the proving branch by an ordinary turn, and the next green promotion closes the issue (`repair_closed`). Fast-green PRs keep landing meanwhile.
- A repo that moves its base branch by hand, past the proving branch, is `diverged` until the proving branch holds the base branch's tip again."#;

#[cfg(test)]
mod tests {
    use super::*;

    fn standing(oldest: u64) -> Standing {
        Standing {
            base_branch: "main".into(),
            proving_branch: "proving".into(),
            base: "a".into(),
            tip: "b".into(),
            unpromoted: 1,
            oldest: Some(oldest),
            hosts: BTreeMap::new(),
            fast_forward: true,
        }
    }

    fn host(waiting: usize, running: usize, busy: Option<u64>) -> Host {
        Host { waiting, running, busy, share: 90, unread: None }
    }

    #[test]
    fn a_promotion_starts_while_the_host_is_idle_or_once_the_oldest_change_waited_the_bound() {
        let idle = Due::of(&standing(1000), &host(0, 0, Some(10)), 3600, 1010);
        assert_eq!(idle, Due::Idle);
        let busy = Due::of(&standing(1000), &host(2, 1, Some(10)), 3600, 1010);
        assert!(!busy.starts());
        let said = busy.said().unwrap();
        assert!(said.contains("host busy: 2 worker commands waiting, 1 running"), "{said}");
        assert!(said.contains("in 3590s at the bound"), "{said}");
        let cores = Due::of(&standing(1000), &host(0, 0, Some(95)), 3600, 1010);
        assert!(cores.said().unwrap().contains("cores 95% busy"), "{cores:?}");
        let past = Due::of(&standing(1000), &host(1, 0, None), 3600, 4600);
        assert!(past.starts() && past.said().unwrap().contains("starts anyway"), "{past:?}");
        let unread = Host { unread: Some("no answer".into()), ..host(0, 0, None) };
        assert!(!Due::of(&standing(1000), &unread, 3600, 1010).starts());
        let current = Standing { tip: "a".into(), ..standing(1000) };
        assert_eq!(Due::of(&current, &host(0, 0, None), 3600, 1010), Due::Not);
    }
}
