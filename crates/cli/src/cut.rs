//! A cut (docs/adr/0028): what a rebuild reads of it, a tick's start and end
//! of one, and `afkconvoy cut`, which takes its steps in its own checkout.
//! When a cut is due, which version it makes and how it ended are the core's
//! (`afkconvoy_core::cut`).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::blocked::CutWait;
use afkconvoy_core::config::Config;
use afkconvoy_core::cut::{
    self, CHECKOUT, CLAIM_KEY, Failure, MINOR_LABEL, Record, RedGate, Row, Seen, Standing, Step,
    Tag, Took,
};
use afkconvoy_core::gate::{GateCommand, failing_tests, quoted};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::line::Line;
use afkconvoy_core::plan::Issue;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::worker::WorkerState;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Me, drop_run, take_run};
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::gate::Gated;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::log::What;
use crate::orca::{Source, Worktree};
use crate::remote::{Fresh, Push};
use crate::stage::Flows;

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

/// Whether a cut will come for an issue that waits on the next one: one is
/// running; or the base branch holds landed work past the newest version tag,
/// or the repo declares its initial version on an untagged remote, so one is
/// due or will be — a failed cut too, which holds the next back only until a
/// human answers. A repo that names no cut steps is never cut, and one with
/// nothing past its newest tag has no cut to come until more lands.
pub fn wait(fleet: &Fleet) -> Result<CutWait> {
    let Some(steps) = &fleet.config.cut else {
        return Ok(CutWait::Never("the repo names no cut steps".to_string()));
    };
    if fleet.scan.runs.iter().any(|run| run.key == CLAIM_KEY) {
        return Ok(CutWait::Coming);
    }
    Ok(match cut::newest(fleet.version_tags()) {
        Some(tag) if past(&fleet.repo, &tag, &fleet.base_tip()?)? > 0 => CutWait::Coming,
        Some(tag) => CutWait::Never(format!(
            "nothing has landed past {} and no cut is running: the version it names is the \
             newest there is",
            tag.version.tag()
        )),
        None if steps.initial_version().is_some() => CutWait::Coming,
        None => CutWait::Never(
            "the remote has no version tag and the repo declares no initial \
             version"
                .to_string(),
        ),
    })
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
        branch: None,
        commit: None,
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
    // The one evaluation this tick starts the cut by, and says (#532).
    let failed = crate::probe::check_cut_needs(&fleet.repo, &fleet.config, me.now, true)?;
    let held = failed.is_some();
    fleet.cut_needs = Some(failed);
    if held {
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
    agent.start(&checkout, &fleet.gh.tracker, first, &command, Some(&title), me.now)?;
    Ok(Some(record))
}

/// Ends the cut `row` once it is over. Mine is read off its checkout
/// (`cut::ending`) — it took every step, failed at one, ran out of time, or
/// its terminal is gone. One that took every step is said, and its checkout
/// removed. One that ended short is said too, opens one issue under the
/// escalate label saying which step failed, why, and the line that goes on
/// from there — no cut starts while it is open — and keeps its checkout for
/// that line, its terminal closed. A cut whose checkout is gone from here,
/// and a peer's presumed dead, ended short as well, out of sight: each opens
/// that issue too, named by the version its branch says, and is unclaimed.
/// Unclaimed without that issue, the next idle tick would start the same
/// version again on its own. `Nothing` while it still runs.
pub fn end(fleet: &mut Fleet, me: &Me, read: &Flows, row: &Row) -> Result<Outcome> {
    let Some(claim) = &row.claim else { return Ok(Outcome::Nothing) };
    let started = claim.owner.as_ref().map_or(me.now, |owner| owner.ts);
    let unseen = |why: &str| Ending::Gone(why.to_string());
    let (checkout, record, ending, limit) = match row.standing {
        Standing::StaleRun => {
            let host = claim.owner.as_ref().map_or("?", |owner| owner.host.as_str());
            let why = format!("the host that was cutting, {host}, went silent");
            (None, None, Some(unseen(&why)), 0)
        }
        Standing::Running => match Worktree::named(&Source::of(&fleet.repo)?, CHECKOUT)? {
            None => (None, None, Some(unseen("its checkout is gone from this host")), 0),
            Some(checkout) => {
                let record = checkout.cut_record()?;
                let gone = checkout.state(me.now)? == WorkerState::Gone;
                let limit = cut::time_limit(fleet.config.gate_seconds());
                let ending =
                    cut::ending(record.as_ref(), started, gone, me.now, limit).map(Ending::Read);
                (Some(checkout), record, ending, limit)
            }
        },
        _ => return Ok(Outcome::Nothing),
    };
    let Some(ending) = ending else { return Ok(Outcome::Nothing) };
    // A record out of reach names the version its branch does, where it
    // pushed one: the issue still stands for it.
    let record = match record {
        Some(record) => record,
        None => Record {
            version: cut::version_started(&cut_heads(&fleet.repo)?, started)
                .unwrap_or_else(|| "?".to_string()),
            from: Some("?".to_string()),
            prs: vec![],
            minor: false,
            started,
            branch: None,
            commit: None,
            at: None,
            took: vec![],
            failed: None,
            superseded: None,
        },
    };
    let version = record.version.clone();
    let failure = match &ending {
        Ending::Read(ending) => ending.failure(limit),
        Ending::Gone(why) => Some(Failure::at(record.step(), why.clone())),
    };
    match failure {
        None => {
            // Its version is published and installed: what waited on it is
            // worked again. Before the checkout goes, so that a release that
            // fails is taken up by the next tick, which reads the cut again.
            if record.superseded.is_none() {
                release_waiting(fleet)?;
            }
            fleet.cut_said.push(What::CutEnded {
                version,
                said: record.line(),
                failed: None,
                superseded: record.superseded.clone(),
            });
            if let Some(checkout) = &checkout {
                checkout.remove()?;
            }
        }
        Some(failure) => {
            if let Some(checkout) = &checkout {
                checkout.close_terminals()?;
            }
            let path = match &checkout {
                Some(checkout) => checkout.path.display().to_string(),
                None => "(gone)".to_string(),
            };
            let line = match &checkout {
                Some(_) => goes_on(&path, failure.step),
                None => "close this issue: the next cut takes up its branch".to_string(),
            };
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

/// Takes the label that waits on the next cut (`cut::WAITING_LABEL`) off
/// every open issue that carries it, read as it is now: an issue parked on the
/// cut since the rebuild returns too. Each returns to the frontier at the
/// stage it left.
fn release_waiting(fleet: &Fleet) -> Result<()> {
    for issue in fleet.gh.open_with(cut::WAITING_LABEL)? {
        fleet.gh.remove_label(issue, cut::WAITING_LABEL)?;
    }
    Ok(())
}

/// How a cut that is over ended: as its checkout says, or out of sight, why.
enum Ending {
    Read(cut::Ending),
    Gone(String),
}

/// Every branch a cut pushed on the remote, each with its commit.
fn cut_heads(repo: &Repo) -> Result<Vec<(String, String)>> {
    let pattern = format!("refs/heads/{CHECKOUT}/*");
    let refs = repo.remote().refs(&[&pattern], Fresh::Process)?;
    Ok(refs
        .into_iter()
        .filter_map(|r| Some((r.name.strip_prefix("refs/heads/")?.to_string(), r.sha)))
        .collect())
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
    let said = match &row.not_cutting_here {
        Some(failed) => format!("{said}; {}", failed.said()),
        None => said,
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
/// `host`, at the time `clock` says.
pub fn run(
    repo: &Repo,
    config: &Config,
    from: Option<Step>,
    host: &str,
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
        Cutting { repo, config, steps: &steps, gh: &gh, host, checkout: &checkout, red: None };
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
            let red = cutting.red.take();
            let output = red.as_ref().map(|red| red.output.clone());
            record.failed = Some(Failure { step, why: why.clone(), red });
            checkout.record_cut(&record)?;
            let mut answer = json!({"version": record.version, "failed": step.word(), "why": why,
                "said": record.line(), "log": checkout.cut_log()?});
            if let Some(output) = output {
                answer["gate_output"] = json!(output);
            }
            return Ok(answer);
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
        "prs": record.prs, "branch": record.branch, "commit": record.commit, "took": record.took,
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
    /// What the gate step's red run named, for the failure it ends the cut with.
    red: Option<RedGate>,
}

impl Cutting<'_> {
    /// Takes `step` of the cut `record`, adding what it learns to it.
    fn take(&mut self, step: Step, record: &mut Record, clock: &impl Fn() -> u64) -> Result<()> {
        match step {
            Step::Set => self.set(record),
            Step::Push => self.push(record),
            Step::Gate => self.gate(record, clock()),
            Step::Tag => self.tag(record),
            Step::Publish => self.publish(record),
        }
    }

    /// On the snapshot the checkout was cut at, the base tip as it stood:
    /// runs the repo's `set` lines and commits what they changed, even
    /// nothing — the version's commit, whose one parent is the snapshot. A
    /// cut of this version that already tagged, or pushed its branch, made
    /// that commit before: this one takes it up, and makes no second.
    fn set(&self, record: &mut Record) -> Result<()> {
        let tag = format!("v{}", record.version);
        let tagged = self.repo.remote().tags(Fresh::Process)?.into_iter().find(|(n, _)| *n == tag);
        let taken = match tagged {
            Some((_, commit)) => {
                self.repo.bring(&commit, &format!("refs/tags/{tag}"))?;
                Some((None, commit))
            }
            None => cut::pushed(&cut_heads(self.repo)?, &record.version)
                .map(|(branch, commit)| (Some(branch), commit)),
        };
        if let Some((branch, commit)) = taken {
            if let Some(branch) = &branch {
                self.repo.bring(&commit, &format!("refs/heads/{branch}"))?;
            }
            self.repo.git(&["reset", "--quiet", "--hard", &commit])?;
            record.branch = branch.or(record.branch.take());
            record.commit = Some(commit);
            return Ok(());
        }
        self.sh(Step::Set, &self.steps.set, &record.version)?;
        self.repo.git(&["add", "-A"])?;
        let title = cut::title(&repo_name(self.repo)?, &record.version);
        self.repo.git(&["commit", "--quiet", "--allow-empty", "-m", &title])?;
        record.commit = Some(self.repo.git(&["rev-parse", "HEAD"])?);
        Ok(())
    }

    /// The version's commit, as its step `set` made it or took it up.
    fn commit(&self, record: &Record) -> Result<String> {
        match &record.commit {
            Some(commit) => Ok(commit.clone()),
            None => self.repo.git(&["rev-parse", "HEAD"]),
        }
    }

    /// Pushes the version's commit to the cut's own branch: never the base
    /// branch. A branch already there on that commit is the cut's.
    fn push(&self, record: &mut Record) -> Result<()> {
        let commit = self.commit(record)?;
        let branch = record.branch.clone().unwrap_or_else(|| branch(record));
        let to = format!("refs/heads/{branch}");
        let there = self.repo.remote().sha(&to, Fresh::Process)?;
        if there.as_deref() != Some(commit.as_str()) {
            self.repo.remote().push(&Push::set(&commit, &to))?;
        }
        record.branch = Some(branch);
        record.commit = Some(commit);
        Ok(())
    }

    /// Gates the version's commit, the whole gate — the fast gate and the
    /// slow gate — in this checkout, in a worker lane as any heavy command
    /// is (`gate::gated`): a run on record for its tree answers. It takes no
    /// landing turn, is no candidate on any chain and reserves no barrier,
    /// and its lane shares the repo as a candidate's does
    /// (`Permit::cut_gate`): turns land on the base branch while it runs,
    /// whether it passes, fails or hangs. A run made now writes its whole
    /// output to a file of its own in the checkout's git dir, which no later
    /// run writes; a red one fails with what it named (`RedGate`).
    fn gate(&mut self, record: &mut Record, now: u64) -> Result<()> {
        let commit = self.commit(record)?;
        // An earlier afkconvoy's record names the commit its turn landed:
        // the base branch brings it here.
        if !self.repo.try_git(&["cat-file", "-e", &format!("{commit}^{{commit}}")])?.ok {
            self.base()?;
        }
        if self.repo.git(&["rev-parse", "HEAD"])? != commit {
            self.repo.git(&["checkout", "--quiet", "--detach", &commit])?;
        }
        let command = GateCommand::new(&self.config.gate_command);
        let limit = Duration::from_secs(self.config.gate_seconds());
        let heavy = crate::config::heavy_gate(command.text());
        let _lane = crate::exclusive::Permit::cut_gate(self.repo, command.text(), heavy)?;
        let output = self.gate_output(now)?;
        match crate::gate::gated(self.repo, &command, self.host, now, limit, None, Some(&output))? {
            Gated::Green { .. } => Ok(()),
            Gated::Red { ran, excerpt, termination } => {
                let how = match termination {
                    Some(termination) => termination.description(),
                    None if ran.timed_out => "it reached the gate limit".to_string(),
                    None => format!("it exited {}", ran.exit_code),
                };
                // The whole run, as it kept it; its excerpt where it kept none.
                let lines: Vec<String> = match std::fs::read_to_string(&output) {
                    Ok(text) => text.lines().map(str::to_string).collect(),
                    Err(_) => excerpt,
                };
                let tests = failing_tests(&lines);
                let named = match tests.as_slice() {
                    [] => "; its output names no failing test".to_string(),
                    tests => format!("; failing: {}", tests.join(", ")),
                };
                let output = output.display().to_string();
                self.red = Some(RedGate { tests, quote: quoted(&lines), output: output.clone() });
                bail!("the gate is red on {commit}: {how}{named}; its output is in {output}")
            }
        }
    }

    /// A file of its own for the output of the gate run started at `now`:
    /// in the checkout's git dir, beside the cut's log, named for when it
    /// started — never one an earlier run wrote.
    fn gate_output(&self, now: u64) -> Result<PathBuf> {
        let log = self.checkout.cut_log()?;
        let dir = log.parent().ok_or_else(|| anyhow!("{} has no directory", log.display()))?;
        let mut n = 0;
        loop {
            let name = match n {
                0 => format!("convoy-cut-gate-{now}.log"),
                n => format!("convoy-cut-gate-{now}-{n}.log"),
            };
            let file = dir.join(name);
            if !file.exists() {
                return Ok(file);
            }
            n += 1;
        }
    }

    /// Tags the version's commit `v<version>` and pushes the tag; the PRs it
    /// holds are read again, up to that commit. A patch that holds a PR
    /// labelled `MINOR_LABEL` — one that landed while it ran — is tagged
    /// nothing: cut again, the version is the next minor.
    fn tag(&self, record: &mut Record) -> Result<()> {
        let commit = self.commit(record)?;
        let tag = format!("v{}", record.version);
        let from = record.from.as_deref().map(|tag| self.tag_commit(tag)).transpose()?;
        record.prs = prs_past(self.repo, from.as_deref(), &commit)?;
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
            self.repo.git(&["tag", "-a", &tag, &commit, "-m", &message])?;
        } else if here.stdout != commit {
            bail!("{tag} is here on {}, not on {commit}, the version's commit", here.stdout);
        }
        let to = format!("refs/tags/{tag}");
        if self.repo.remote().try_push(&Push::set(&to, &to))?.ok {
            return Ok(());
        }
        match self.repo.remote().tags(Fresh::Process)?.into_iter().find(|(name, _)| *name == tag) {
            Some((_, there)) if there == commit => Ok(()),
            Some((_, there)) => bail!("the remote has {tag} on {there} already, not on {commit}"),
            None => bail!("{tag} could not be pushed"),
        }
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

/// The branch a cut commits its version on: its own, of its version and its
/// start (`cut::branch`).
fn branch(record: &Record) -> String {
    cut::branch(&record.version, record.started)
}

/// The repo's name, as its remote `origin` names it: `afkconvoy`.
fn repo_name(repo: &Repo) -> Result<String> {
    let of = crate::checkout_list::repo_of(repo)?;
    Ok(of.rsplit('/').next().unwrap_or(&of).to_string())
}
