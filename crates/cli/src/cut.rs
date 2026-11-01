//! A cut (docs/adr/0028): what a rebuild reads of it, a tick's start and end
//! of one, and `afkconvoy cut`, which takes its steps in its own checkout.
//! When a cut is due, which version it makes and how it ended are the core's
//! (`afkconvoy_core::cut`).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::config::Config;
use afkconvoy_core::cut::{
    self, CHECKOUT, CLAIM_KEY, Failure, MINOR_LABEL, Record, Row, Seen, Standing, Step, Tag, Took,
};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::landing::{LandedPr, Of};
use afkconvoy_core::line::Line;
use afkconvoy_core::plan::Issue;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Me, drop_run, take_run};
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::landing::joins_in;
use crate::log::What;
use crate::orca::{Source, Worktree};
use crate::remote::{Fresh, Push};
use crate::stage::Flows;

/// How many seconds the step `land` waits between two looks at the base branch.
const LAND_POLL: u32 = 30;

/// What a rebuild reads of the cut: version tags, base tip, open issues
/// and a running cut's claim. Without declared steps only its claim remains.
pub fn seen(fleet: &Fleet, read: &Flows, issues: &[Issue], base: &str) -> Result<Option<Seen>> {
    let claim = fleet.scan.runs.iter().find(|run| run.key == CLAIM_KEY).cloned();
    let Some(steps) = &fleet.config.cut else {
        let going = claim.is_some();
        return Ok(going.then(|| Seen { claim, ..Seen::default() }));
    };
    let newest = cut::newest(fleet.version_tags());
    let past = match &newest {
        Some(tag) => past(&fleet.repo, tag, base)?,
        None => 0,
    };
    let failure = cut::failure_of(issues, &escalate_label(fleet, read));
    let initial = steps.initial_version();
    let oldest_landing = match &newest {
        Some(tag) if past > 0 => {
            let dates = fleet.repo.git(&[
                "log",
                "--first-parent",
                "--reverse",
                "--format=%ct",
                &history(Some(&tag.commit), base),
            ])?;
            dates.lines().next().map(str::parse).transpose().context("the oldest landing's time")?
        }
        _ => None,
    };
    Ok(Some(Seen {
        on: true,
        newest,
        initial,
        past,
        oldest_landing,
        busy_after_seconds: steps.busy_after_seconds,
        claim,
        failure,
    }))
}

/// How many commits the base branch at `base` holds past the tag `tag`, its
/// commit brought here first where it is not.
fn past(repo: &Repo, tag: &Tag, base: &str) -> Result<u64> {
    repo.bring(&tag.commit, &format!("refs/tags/{}", tag.version.tag()))?;
    let counted = repo.git(&["rev-list", "--count", &format!("{}..{base}", tag.commit)])?;
    counted.parse().with_context(|| format!("git rev-list --count: no count in {counted:?}"))
}

/// The history up to `to`, past `from` where a version tag exists; all of it
/// for the initial cut.
fn history(from: Option<&str>, to: &str) -> String {
    match from {
        Some(from) => format!("{from}..{to}"),
        None => to.to_string(),
    }
}

/// The PRs whose landings the commits past `from` up to `to` hold, as the
/// subjects of their first parents name them (`cut::prs_of`).
fn prs_past(repo: &Repo, from: Option<&str>, to: &str) -> Result<Vec<u64>> {
    let subjects = repo.git(&["log", "--first-parent", "--format=%s", &history(from, to)])?;
    Ok(cut::prs_of(subjects.lines()))
}

/// The label an issue that hands work to a human carries: a bound repo's
/// team names it, an unbound repo's config.
fn escalate_label(fleet: &Fleet, read: &Flows) -> String {
    match (&fleet.config.bound, read.kept()) {
        (Some(_), Ok(kept)) => kept.team.escalate_label.clone(),
        _ => fleet.config.escalate_label.clone(),
    }
}

/// What a cut started now makes, past the newest version tag on the remote
/// as it is now and holding the PRs the base branch at `base` holds past it,
/// started `now` → its record. Without a version tag it makes the declared
/// initial version; `None` without that declaration, or with no work past a tag.
fn pick(fleet: &Fleet, base: &str, now: u64) -> Result<Option<Record>> {
    let newest = cut::newest(&fleet.remote.tags(Fresh::Process)?);
    let (version, from, prs, minor) = match newest {
        Some(newest) => {
            if past(&fleet.repo, &newest, base)? == 0 {
                return Ok(None);
            }
            let prs = prs_past(&fleet.repo, Some(&newest.commit), base)?;
            let labelled = match prs.is_empty() {
                true => vec![],
                false => fleet.gh.merged_prs_labelled(MINOR_LABEL)?,
            };
            let version = cut::pick(newest.version, &prs, &labelled)
                .ok_or_else(|| anyhow!("no version follows {}", newest.version.tag()))?;
            (version, Some(newest.version.tag()), prs, version.minor != newest.version.minor)
        }
        None => {
            let Some(version) = fleet.config.cut.as_ref().and_then(cut::Steps::initial_version)
            else {
                return Ok(None);
            };
            (version, None, prs_past(&fleet.repo, None, base)?, false)
        }
    };
    Ok(Some(Record {
        version: version.to_string(),
        from,
        prs,
        minor,
        started: now,
        pr: None,
        landed: None,
        at: None,
        took: vec![],
        failed: None,
        superseded: None,
    }))
}

/// Starts a cut, due as the rebuild read it: claims it, picks its version
/// from the version tags as they are now — a peer may have cut since the
/// rebuild — has orca cut it a checkout of the base branch as it stands,
/// keeps what the cut is there (`cut::Record`), and starts `afkconvoy cut`
/// in a terminal of it, under the host kit's first harness's worker
/// environment, as a caster is. A checkout an earlier cut left is removed
/// first. `Lost` when a peer claimed it, or nothing is left to cut.
///
/// A step that fails after the claim gives the claim back and is an error:
/// held, a later tick would read the record a cut before it left in the
/// checkout as this one's, and say that cut's end again.
pub fn start(fleet: &mut Fleet, me: &Me, agent: &Agent, busy: bool) -> Result<Outcome> {
    if fleet.config.cut.is_none() {
        return Ok(Outcome::Nothing);
    }
    // Before the claim: a host kit that holds no harness starts nothing.
    let first = fleet.first_harness()?;
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, CLAIM_KEY)? else {
        return Ok(Outcome::Lost);
    };
    let started = started(fleet, me, agent, first);
    if !matches!(started, Ok(Some(_)))
        && let Err(kept) = drop_run(&fleet.repo, &mut fleet.scan, &claim)
    {
        let failed = started.err().map(|error| format!("{error:#}; ")).unwrap_or_default();
        bail!("{failed}its claim could not be given back: {kept:#}");
    }
    let Some(record) = started? else { return Ok(Outcome::Lost) };
    fleet.cut_said.push(What::CutStarted {
        version: record.version.clone(),
        said: record.line(),
        busy,
    });
    Ok(Outcome::Done)
}

/// What `start` does once it holds the claim → the cut's record, its
/// checkout cut and its terminal started; `None` where nothing is left to cut.
fn started(fleet: &Fleet, me: &Me, agent: &Agent, first: Harness) -> Result<Option<Record>> {
    let base = fleet.base_tip()?;
    let Some(record) = pick(fleet, &base, me.now)? else { return Ok(None) };
    let source = Source::of(&fleet.repo)?;
    if let Some(left) = Worktree::named(&source, CHECKOUT)? {
        left.remove()?;
    }
    let checkout = Worktree::cut(&source, CHECKOUT, &base, None)?;
    checkout.record_cut(&record)?;
    let path = checkout.path.display().to_string();
    let command = crate::own_command_in(&path).fixed(" cut; exit").own();
    let title = format!("afkconvoy cut v{}", record.version);
    agent.start(&checkout, first, &command, Some(&title), me.now)?;
    Ok(Some(record))
}

/// Ends the cut `row` once it is over: a peer's presumed dead is only
/// unclaimed; mine is read off its checkout (`cut::ending`) — it took every
/// step, failed at one, ran out of time, or its terminal is gone. One that
/// took every step is said, and its checkout removed. One that ended short
/// is said too, opens one issue under the escalate label saying which step
/// failed, why, and the line that goes on from there — no cut starts while
/// it is open — and keeps its checkout for that line, its terminal closed.
/// A cut with no checkout here started nothing: it is only unclaimed.
/// `Nothing` while it still runs.
pub fn end(fleet: &mut Fleet, me: &Me, read: &Flows, row: &Row) -> Result<Outcome> {
    let Some(claim) = &row.claim else { return Ok(Outcome::Nothing) };
    match row.standing {
        Standing::StaleRun => {
            drop_run(&fleet.repo, &mut fleet.scan, claim)?;
            return Ok(Outcome::Done);
        }
        Standing::Running => {}
        _ => return Ok(Outcome::Nothing),
    }
    let Some(checkout) = Worktree::named(&Source::of(&fleet.repo)?, CHECKOUT)? else {
        drop_run(&fleet.repo, &mut fleet.scan, claim)?;
        return Ok(Outcome::Done);
    };
    let record = checkout.cut_record()?;
    let gone = checkout.state(me.now)? == WorkerState::Gone;
    let started = claim.owner.as_ref().map_or(me.now, |owner| owner.ts);
    let limit = cut::time_limit(fleet.config.gate_seconds());
    let Some(ending) = cut::ending(record.as_ref(), started, gone, me.now, limit) else {
        return Ok(Outcome::Nothing);
    };
    // A record nobody can read names no version: the issue still stands for it.
    let record = record.unwrap_or_else(|| Record {
        version: "?".to_string(),
        from: Some("?".to_string()),
        prs: vec![],
        minor: false,
        started,
        pr: None,
        landed: None,
        at: None,
        took: vec![],
        failed: None,
        superseded: None,
    });
    let version = record.version.clone();
    match ending.failure(limit) {
        None => {
            fleet.cut_said.push(What::CutEnded {
                version,
                said: record.line(),
                failed: None,
                superseded: record.superseded.clone(),
            });
            checkout.remove()?;
        }
        Some(failure) => {
            checkout.close_terminals()?;
            let path = checkout.path.display().to_string();
            let line = goes_on(&path, failure.step);
            // One report stands for a cut until a human closes it.
            let issue = match row.failure {
                Some(open) => open,
                None => fleet.gh.open_report(
                    &cut::failure_title(&version),
                    &cut::failure_report(&record, &failure, &me.host, &path, &line),
                    &format!("the step `{}` failed: {}", failure.step.word(), failure.why.trim()),
                    &[escalate_label(fleet, read).as_str()],
                )?,
            };
            let failed = format!(
                "at its step `{}`: {} — {} says so, and goes on with `{line}`",
                failure.step.word(),
                failure.why.trim(),
                fleet.gh.tracker.issue(issue)
            );
            fleet.cut_said.push(What::CutEnded {
                version,
                said: record.line(),
                failed: Some(failed),
                superseded: None,
            });
        }
    }
    drop_run(&fleet.repo, &mut fleet.scan, claim)?;
    Ok(if record.superseded.is_some() { Outcome::Superseded } else { Outcome::Done })
}

/// The line that goes on with the cut in the checkout `path` from `step`.
fn goes_on(path: &str, step: Step) -> String {
    Line::new().fixed("cd ").word(path).fixed(" && afkconvoy cut --from ").word(step.word()).own()
}

/// What the panel says of the repo's cut `row`, in words, with the last cut
/// the tick log of `repo` holds where none is going on here; `None` where the
/// repo turns cutting off and no cut is going.
pub fn says(repo: &Repo, row: &Row) -> Option<String> {
    let past = || match &row.newest_tag {
        Some(tag) => format!("{} commits past {tag}", row.past),
        None => "no version tag".to_string(),
    };
    let said = match row.standing {
        Standing::Off => return None,
        Standing::Running => {
            let checkout = Source::of(repo)
                .and_then(|source| Worktree::named(&source, CHECKOUT))
                .ok()
                .flatten();
            match checkout.and_then(|checkout| checkout.cut_record().ok().flatten()) {
                Some(record) => {
                    format!("cutting, at its step `{}`: {}", record.step().word(), record.line())
                }
                None => "cutting".to_string(),
            }
        }
        Standing::PeerRunning => "a peer is cutting".to_string(),
        Standing::StaleRun => {
            "the peer that was cutting went silent: the next tick unclaims it".to_string()
        }
        Standing::Due => match (row.oldest_landing_age_seconds, row.busy_in_seconds) {
            (Some(age), Some(wait)) => format!(
                "due: {}; oldest landing {age}s ago; starts when idle or in {wait}s while busy",
                past()
            ),
            _ => format!("due: {}; it starts once the fleet is idle", past()),
        },
        Standing::Locked => format!("{}; a fleet lock stands, and no cut starts", past()),
        Standing::Failed => match row.failure {
            Some(issue) => {
                format!("the last cut failed: #{issue} says which step, and how to go on")
            }
            None => "the last cut failed".to_string(),
        },
        Standing::Tagged => format!("{}: nothing to cut", past()),
        Standing::Untagged => "the remote has no version tag to count a version from".to_string(),
    };
    let last = crate::log::Log::of(repo).ok().and_then(|log| log.read()).and_then(|entries| {
        entries.into_iter().rev().find_map(|entry| match entry.what {
            What::CutEnded { said, superseded: Some(why), .. } => {
                Some(format!("{said} — stopped at `tag`, without tagging: {why}"))
            }
            What::CutEnded { said, failed, .. } => Some(match failed {
                Some(failed) => format!("{said} — FAILED {failed}"),
                None => said,
            }),
            _ => None,
        })
    });
    Some(match last.filter(|_| row.standing != Standing::Running) {
        Some(last) => format!("{said} · last cut: {last}"),
        None => said,
    })
}

/// `afkconvoy cut`: takes the steps of the cut whose checkout `repo` is —
/// from the step after the last it took, or from `from` — to its last,
/// timing each, and keeping what it did in the checkout as it goes for the
/// fleet to read once it ends. A step that fails ends it there, saying why.
/// One that took its last closes the open issue that said this version's cut
/// failed. `config` is the checkout's own, whose cut steps it takes; on
/// `host`, at the time `clock` says, each second of its sleeps taking
/// `second`.
pub fn run(
    repo: &Repo,
    config: &Config,
    from: Option<Step>,
    host: &str,
    second: Duration,
    clock: impl Fn() -> u64,
) -> Result<Value> {
    let checkout = cut_checkout(repo)?;
    let Some(mut record) = checkout.cut_record()? else {
        bail!(
            "{} says no cut: only the checkout a fleet started a cut in takes its steps",
            checkout.path.display()
        );
    };
    let steps = config.cut.clone().ok_or_else(|| {
        anyhow!(
            "{} names no cut steps: a ```yaml cut block in {} says them",
            repo.dir.display(),
            crate::config::FILE
        )
    })?;
    let gh = GitHub::of(repo)?;
    let mut cutting =
        Cutting { repo, config, steps: &steps, gh: &gh, host, checkout: &checkout, second };
    let first = from.unwrap_or_else(|| record.step());
    let taking = first.from();
    let names: Vec<_> = taking.iter().map(|step| step.word()).collect();
    let mut summary = record.line();
    if record.prs.is_empty() {
        summary.push_str(" · holding no PRs");
    }
    eprintln!(
        "[afkconvoy] cut: {summary}; steps: {}; log: {}; record: {}",
        names.join(", "),
        checkout.cut_log()?.display(),
        checkout.cut_record_file()?.display()
    );
    record.failed = None;
    if record.superseded.is_some() {
        return Ok(stopped(&record));
    }
    for step in taking {
        record.at = Some(step);
        checkout.record_cut(&record)?;
        let began = Instant::now();
        eprintln!("[afkconvoy] cut: {} started", step.word());
        let taken = cutting.take(step, &mut record, &clock);
        let seconds = began.elapsed().as_secs();
        if let Err(error) = taken {
            let why = format!("{error:#}");
            eprintln!("[afkconvoy] cut: {} failed after {seconds}s: {why}", step.word());
            record.failed = Some(Failure { step, why: why.clone() });
            checkout.record_cut(&record)?;
            return Ok(json!({"version": record.version, "failed": step.word(), "why": why,
                "said": record.line(), "log": checkout.cut_log()?}));
        }
        if record.superseded.is_some() {
            checkout.record_cut(&record)?;
            return Ok(stopped(&record));
        }
        record.took.retain(|took| took.step != step);
        record.took.push(Took { step, seconds });
        record.at = None;
        checkout.record_cut(&record)?;
        eprintln!("[afkconvoy] cut: {} passed after {seconds}s", step.word());
    }
    let title = cut::failure_title(&record.version);
    let open = gh.open_issues()?;
    let closed: Vec<u64> =
        open.rows.iter().filter(|issue| issue.title == title).map(|issue| issue.number).collect();
    for issue in &closed {
        gh.close_issue(*issue)?;
    }
    Ok(json!({"version": record.version, "tag": format!("v{}", record.version),
        "prs": record.prs, "pr": record.pr, "landed": record.landed, "took": record.took,
        "closed": closed, "said": record.line()}))
}

/// A patch overtaken by a minor landing is an expected stop, with no tag.
fn stopped(record: &Record) -> Value {
    json!({"version": record.version, "stopped": "tag", "why": record.superseded,
        "said": record.line()})
}

/// The checkout `repo`, refused unless it is a cut's.
fn cut_checkout(repo: &Repo) -> Result<Worktree> {
    let checkout = Worktree::of(repo)?;
    let named = checkout.path.file_name().and_then(|name| name.to_str()).unwrap_or("");
    if !afkconvoy_core::worker::is_worktree_named(named, CHECKOUT) {
        bail!(
            "{} is no cut's checkout: a cut takes its steps in the checkout the fleet started it \
             in, `{CHECKOUT}`",
            checkout.path.display()
        );
    }
    Ok(checkout)
}

/// What a cut's steps are taken with.
struct Cutting<'a> {
    repo: &'a Repo,
    config: &'a Config,
    steps: &'a cut::Steps,
    gh: &'a GitHub,
    host: &'a str,
    checkout: &'a Worktree,
    /// How long one second of its sleeps takes.
    second: Duration,
}

impl Cutting<'_> {
    /// Takes `step` of the cut `record`, adding what it learns to it.
    fn take(&mut self, step: Step, record: &mut Record, clock: &impl Fn() -> u64) -> Result<()> {
        match step {
            Step::Set => self.set(record),
            Step::Pr => self.pr(record),
            Step::Gate if record.landed.is_some() => Ok(()),
            Step::Gate => {
                let number = record.pr.ok_or_else(|| anyhow!("the cut opened no PR"))?;
                self.join(record.from.as_deref(), number, clock())
            }
            Step::Land => self.land(record, clock),
            Step::Tag => self.tag(record),
            Step::Publish => self.publish(record),
        }
    }

    /// Runs the repo's `set` lines, and commits what they changed. Lines that
    /// change nothing find the version set already: by a cut whose end was
    /// cut short after its PR landed. Where that landing is on the base
    /// branch, it is the one this cut tags. An initial version already set
    /// still gets its own commit, gate and landing through a landing turn.
    fn set(&self, record: &mut Record) -> Result<()> {
        self.sh(Step::Set, &self.steps.set, &record.version)?;
        self.repo.git(&["add", "-A"])?;
        let changed = self.repo.git_raw(&["status", "--porcelain"])?;
        let title = cut::title(&repo_name(self.repo)?, &record.version);
        if changed.trim().is_empty() {
            if let Some(landed) = self.landed_titled(record.from.as_deref(), &title)? {
                record.pr = Some(landed.pr);
                record.landed = Some(landed.commit);
                return Ok(());
            }
            if let Some(from) = &record.from {
                bail!(
                    "the set lines changed nothing, and no cut's PR titled {title:?} landed past \
                     {from}: they set no version"
                );
            }
        }
        self.repo.git(&["commit", "--quiet", "--allow-empty", "-m", &title])?;
        Ok(())
    }

    /// The landings on `base` past the previous tag, or all landings for an
    /// initial cut. Every step reads the same history of the cut's origin.
    fn landings(&self, from: Option<&str>, base: &str) -> Result<Vec<LandedPr>> {
        let tag = from.map(|tag| self.tag_commit(tag)).transpose()?;
        joins_in(self.repo, &self.gh.tracker, &[&history(tag.as_deref(), base)])
    }

    /// The landing of a cut's PR titled `title` the base branch holds past
    /// the tag `from`, or anywhere on it for an initial cut, where one is.
    fn landed_titled(&self, from: Option<&str>, title: &str) -> Result<Option<LandedPr>> {
        let base = self.base()?;
        let joins = self.landings(from, &base)?;
        let ends = format!("{title} (#");
        Ok(joins.into_iter().find(|join| {
            matches!(join.of, Of::Cut(_))
                && self.subject(&join.commit).is_ok_and(|s| s.starts_with(&ends))
        }))
    }

    /// The cut's landing of PR `number` on the base branch past `from`.
    fn landed_pr(&self, from: Option<&str>, number: u64) -> Result<Option<LandedPr>> {
        let base = self.base()?;
        let joins = self.landings(from, &base)?;
        Ok(joins.into_iter().rev().find(|join| join.pr == number && matches!(join.of, Of::Cut(_))))
    }

    fn subject(&self, commit: &str) -> Result<String> {
        self.repo.git(&["log", "-1", "--format=%s", commit])
    }

    /// The commit the version tag `name` is on, on the remote.
    fn tag_commit(&self, name: &str) -> Result<String> {
        let tags = self.repo.remote().tags(Fresh::Process)?;
        let found = tags.into_iter().find(|(tag, _)| tag == name);
        let (_, commit) = found.ok_or_else(|| anyhow!("the remote has no tag {name}"))?;
        self.repo.bring(&commit, &format!("refs/tags/{name}"))?;
        Ok(commit)
    }

    /// The base branch's tip on the remote, its commits here.
    fn base(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        let base = self.repo.remote().fetched(&format!("refs/heads/{branch}"), Fresh::Process)?;
        base.ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))
    }

    /// Pushes the version's commit to a branch of the cut's own and opens
    /// its PR; one already open for that branch is the cut's.
    fn pr(&self, record: &mut Record) -> Result<()> {
        if record.landed.is_some() {
            return Ok(());
        }
        let branch = branch(record);
        self.repo.remote().push(&Push::set("HEAD", &format!("refs/heads/{branch}")))?;
        let open = self.gh.open_prs()?;
        if let Some(pr) = open.iter().find(|pr| pr.head_branch == branch) {
            record.pr = Some(pr.number);
            return Ok(());
        }
        let title = cut::title(&repo_name(self.repo)?, &record.version);
        let body = cut::pr_body(record, self.host);
        record.pr = Some(self.gh.open_pr(&self.config.base_branch, &branch, &title, &body)?);
        Ok(())
    }

    /// Recovers the cut's commit from the base branch after its landing turn.
    /// An interrupted cut can take another turn if its PR has not landed.
    fn land(&self, record: &mut Record, clock: &impl Fn() -> u64) -> Result<()> {
        if record.landed.is_some() {
            return Ok(());
        }
        let number = record.pr.ok_or_else(|| anyhow!("the cut opened no PR"))?;
        let from = record.from.clone();
        let head = self.repo.git(&["rev-parse", "HEAD"])?;
        loop {
            if let Some(join) = self.landed_pr(from.as_deref(), number)? {
                record.landed = Some(join.commit);
                return Ok(());
            }
            let base = self.base()?;
            if self.repo.within(&head, &base)? {
                bail!(
                    "PR #{number} is on {} without a recorded landing commit: it was \
                     merged outside a turn, and is no landing a cut tags",
                    self.config.base_branch
                );
            }
            self.join_turn(number, clock())?;
            std::thread::sleep(self.second * LAND_POLL);
        }
    }

    /// Puts PR `number` on the base branch, as the cut's on `host`. A landing
    /// already there is the one this cut tags: taking the turn again would
    /// add a second commit.
    fn join(&self, from: Option<&str>, number: u64, now: u64) -> Result<()> {
        if self.landed_pr(from, number)?.is_some() {
            return Ok(());
        }
        self.join_turn(number, now)
    }

    /// Takes the cut's turn for PR `number`.
    fn join_turn(&self, number: u64, now: u64) -> Result<()> {
        let locks = crate::lock::standing_locks(self.gh, self.config)?;
        if !locks.is_empty() {
            let answer = crate::turn::locked(json!({}), &locks);
            bail!("{}", answer["detail"].as_str().unwrap_or("a fleet lock stands"));
        }
        let open = self.gh.open_prs()?;
        let pr = open.iter().find(|pr| pr.number == number);
        let pr = pr.ok_or_else(|| anyhow!("PR #{number} is not open: it joins nothing"))?;
        let of = Of::Cut(self.host.to_string());
        let answer = crate::turn::join(
            self.repo,
            self.config,
            &self.gh.tracker,
            pr,
            &of,
            json!({}),
            (self.host, now),
        )?;
        match answer["outcome"].as_str() {
            Some("merged") => Ok(()),
            _ => bail!(
                "{} — go on from the step `gate` once it is committed",
                answer["detail"].as_str().unwrap_or("it could not land in a turn")
            ),
        }
    }

    /// Tags the commit that landed it `v<version>` and pushes the tag; the
    /// PRs it holds are read again, up to that commit, its own left out. A
    /// patch that holds a PR labelled `MINOR_LABEL` — one that landed while
    /// it ran — is tagged nothing: cut again, the version is the next minor.
    fn tag(&self, record: &mut Record) -> Result<()> {
        let landed =
            record.landed.clone().ok_or_else(|| anyhow!("nothing says where it landed"))?;
        let tag = format!("v{}", record.version);
        let from = record.from.as_deref().map(|tag| self.tag_commit(tag)).transpose()?;
        let mut prs = prs_past(self.repo, from.as_deref(), &landed)?;
        prs.retain(|pr| Some(*pr) != record.pr);
        record.prs = prs;
        if record.from.is_some() && !record.minor {
            let prs = prs_past(self.repo, from.as_deref(), &self.base()?)?;
            let labelled = self.gh.merged_prs_labelled(MINOR_LABEL)?;
            if let Some(pr) = prs.iter().find(|pr| labelled.contains(pr)) {
                record.superseded = Some(format!(
                    "#{pr}, labelled `{MINOR_LABEL}`, landed while the cut ran, and {tag} is a \
                     patch: the next tick cuts the next minor"
                ));
                return Ok(());
            }
        }
        let here =
            self.repo.try_git(&["rev-parse", "-q", "--verify", &format!("{tag}^{{commit}}")])?;
        if !here.ok {
            let message = cut::title(&repo_name(self.repo)?, &record.version);
            self.repo.git(&["tag", "-a", &tag, &landed, "-m", &message])?;
        } else if here.stdout != landed {
            bail!("{tag} is here on {}, not on {landed}, which landed the cut", here.stdout);
        }
        let to = format!("refs/tags/{tag}");
        if self.repo.remote().try_push(&Push::set(&to, &to))?.ok {
            return Ok(());
        }
        match self.repo.remote().tags(Fresh::Process)?.into_iter().find(|(name, _)| *name == tag) {
            Some((_, commit)) if commit == landed => Ok(()),
            Some((_, commit)) => bail!("the remote has {tag} on {commit} already, not on {landed}"),
            None => bail!("{tag} could not be pushed"),
        }
    }

    /// With the tag checked out, runs the repo's `publish` lines.
    fn publish(&self, record: &Record) -> Result<()> {
        let tag = format!("v{}", record.version);
        self.repo.git(&["checkout", "--quiet", "--detach", &tag])?;
        self.sh(Step::Publish, &self.steps.publish, &record.version)
    }

    /// Runs `lines` by `sh -e` in the checkout with the version in
    /// `cut::VERSION_ENV`, both output streams kept in order in the cut's log
    /// and streamed to stderr before the step ends. The first line that fails
    /// ends them, its status theirs:
    /// by `sh` alone the lines after it would run, and the last one's would
    /// stand for them all.
    fn sh(&self, step: Step, lines: &str, version: &str) -> Result<()> {
        let file = self.checkout.cut_log()?;
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .with_context(|| format!("cannot write {}", file.display()))?;
        writeln!(log, "== {} (VERSION={version})", step.word())?;
        let mut output = File::open(&file)?;
        output.seek(SeekFrom::End(0))?;
        let mut child = crate::process::spawn(
            Command::new("sh")
                .args(["-e", "-c", lines])
                .current_dir(&self.repo.dir)
                .env(cut::VERSION_ENV, version)
                .stdin(Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log),
        )
        .context("sh could not run")?;
        // Follow the same file the commands have always written. A child
        // left in the background can keep that file open without keeping
        // this step waiting on an output pipe after its shell exits.
        let streamed = (|| -> std::io::Result<_> {
            let mut buffer = [0; 8192];
            let mut terminal = std::io::stderr().lock();
            let mut newline = true;
            loop {
                let status = child.try_wait()?;
                loop {
                    let read = match output.read(&mut buffer) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        read => read?,
                    };
                    if read == 0 {
                        break;
                    }
                    terminal.write_all(&buffer[..read])?;
                    newline = buffer[read - 1] == b'\n';
                }
                terminal.flush()?;
                if let Some(status) = status {
                    if !newline {
                        writeln!(terminal)?;
                    }
                    return Ok(status);
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })();
        if streamed.is_err() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let status = streamed.context("cannot stream the cut step's output")?;
        if !status.success() {
            let code = status.code().map_or("a signal".to_string(), |code| code.to_string());
            bail!("its lines exited {code}; what they printed is in {}", file.display());
        }
        Ok(())
    }
}

/// The branch a cut's PR is pushed to: its own, of its version and its start.
fn branch(record: &Record) -> String {
    format!("{CHECKOUT}/v{}-{}", record.version, record.started)
}

/// The repo's name, as its remote `origin` names it: `afkconvoy`.
fn repo_name(repo: &Repo) -> Result<String> {
    let of = crate::checkout_list::repo_of(repo)?;
    Ok(of.rsplit('/').next().unwrap_or(&of).to_string())
}
