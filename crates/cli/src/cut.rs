//! A cut (docs/adr/0027): what a rebuild reads of it, a tick's start and end
//! of one, and `afkconvoy cut`, which takes its steps in its own checkout.
//! When a cut is due, which version it makes and how it ended are the core's
//! (`afkconvoy_core::cut`).

use std::fs::OpenOptions;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::config::Config;
use afkconvoy_core::cut::{
    self, CHECKOUT, CLAIM_KEY, Failure, MINOR_LABEL, Record, Row, Seen, Standing, Step, Tag, Took,
};
use afkconvoy_core::line::Line;
use afkconvoy_core::plan::Issue;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::train::{Joined, Of};
use afkconvoy_core::worker::WorkerState;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Me, drop_run, take_run};
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::log::What;
use crate::orca::{Source, Worktree};
use crate::stage::Flows;
use crate::train::{Train, joins_in};

/// How long the step `land` waits between two looks at the landing train.
const LAND_POLL: Duration = Duration::from_secs(30);

/// What the remote and GitHub say of the repo's cut, as a rebuild reads it:
/// the version tags it listed (`Fleet::version_tags`) and the base branch at
/// `base`, the open `issues`, and the PRs `riding` the landing train. A repo
/// that names no cut steps reads nothing past the claim on a cut still going.
pub fn seen(
    fleet: &Fleet,
    read: &Flows,
    issues: &[Issue],
    riding: &[&Joined],
    base: &str,
) -> Result<Seen> {
    let claim = fleet.scan.runs.iter().find(|run| run.key == CLAIM_KEY).cloned();
    let on_train = riding.iter().find(|join| matches!(join.of, Of::Cut(_))).map(|join| join.pr);
    if fleet.config.cut.is_none() {
        return Ok(Seen { claim, on_train, ..Seen::default() });
    }
    let newest = cut::newest(fleet.version_tags());
    let past = match &newest {
        Some(tag) => past(&fleet.repo, tag, base)?,
        None => 0,
    };
    let failure = cut::failure_of(issues, &escalate_label(fleet, read));
    Ok(Seen { on: true, newest, past, claim, failure, on_train })
}

/// How many commits the base branch at `base` holds past the tag `tag`, its
/// commit brought here first where it is not.
fn past(repo: &Repo, tag: &Tag, base: &str) -> Result<u64> {
    repo.bring(&tag.commit, &format!("refs/tags/{}", tag.version.tag()))?;
    let counted = repo.git(&["rev-list", "--count", &format!("{}..{base}", tag.commit)])?;
    counted.parse().with_context(|| format!("git rev-list --count: no count in {counted:?}"))
}

/// The PRs whose landings the commits past `from` up to `to` hold, as the
/// subjects of their first parents name them (`cut::prs_of`).
fn prs_past(repo: &Repo, from: &str, to: &str) -> Result<Vec<u64>> {
    let subjects = repo.git(&["log", "--first-parent", "--format=%s", &format!("{from}..{to}")])?;
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
/// started `now` → its record; `None` where the remote has no version tag,
/// or the base branch holds nothing past it.
fn pick(fleet: &Fleet, base: &str, now: u64) -> Result<Option<Record>> {
    let Some(newest) = cut::newest(&fleet.repo.remote_tags()?) else { return Ok(None) };
    if past(&fleet.repo, &newest, base)? == 0 {
        return Ok(None);
    }
    let prs = prs_past(&fleet.repo, &newest.commit, base)?;
    let labelled = match prs.is_empty() {
        true => vec![],
        false => fleet.gh.merged_prs_labelled(MINOR_LABEL)?,
    };
    let version = cut::pick(newest.version, &prs, &labelled)
        .ok_or_else(|| anyhow!("no version follows {}", newest.version.tag()))?;
    Ok(Some(Record {
        version: version.to_string(),
        from: newest.version.tag(),
        prs,
        minor: version.minor != newest.version.minor,
        started: now,
        pr: None,
        landed: None,
        at: None,
        took: vec![],
        failed: None,
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
/// A step that fails after the claim is an error with the claim still held:
/// a later tick finds no checkout of it, or one whose terminal is gone, and
/// ends it.
pub fn start(fleet: &mut Fleet, me: &Me, agent: &Agent) -> Result<Outcome> {
    if fleet.config.cut.is_none() {
        return Ok(Outcome::Nothing);
    }
    // Before the claim: a host kit that holds no harness starts nothing.
    let first = fleet.first()?;
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, CLAIM_KEY)? else {
        return Ok(Outcome::Lost);
    };
    let base = fleet.base_tip()?;
    let Some(record) = pick(fleet, &base, me.now)? else {
        drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
        return Ok(Outcome::Lost);
    };
    let source = Source::of(&fleet.repo)?;
    if let Some(left) = Worktree::named(&source, CHECKOUT)? {
        left.remove()?;
    }
    let checkout = Worktree::cut(&source, CHECKOUT, &base, None)?;
    checkout.record_cut(&record)?;
    let path = checkout.path.display().to_string();
    let command = crate::own_command_in(&path).fixed(" cut; exit").own();
    agent.start(&checkout, first.harness, &command, me.now)?;
    fleet.cut_said.push(What::CutStarted { version: record.version.clone(), said: record.line() });
    Ok(Outcome::Done)
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
        from: "?".to_string(),
        prs: vec![],
        minor: false,
        started,
        pr: None,
        landed: None,
        at: None,
        took: vec![],
        failed: None,
    });
    let version = record.version.clone();
    match ending.failure(limit) {
        None => {
            fleet.cut_said.push(What::CutEnded { version, said: record.line(), failed: None });
            checkout.remove()?;
        }
        Some(failure) => {
            checkout.close_terminals()?;
            let path = checkout.path.display().to_string();
            let line = goes_on(&path, failure.step);
            // One report stands for a cut until a human closes it.
            let issue = match row.failure {
                Some(open) => open,
                None => fleet.gh.open_issue(
                    &cut::failure_title(&version),
                    &cut::failure_report(&record, &failure, &me.host, &path, &line),
                    &[escalate_label(fleet, read).as_str()],
                )?,
            };
            let failed = format!(
                "at its step `{}`: {} — #{issue} says so, and goes on with `{line}`",
                failure.step.word(),
                failure.why.trim()
            );
            fleet.cut_said.push(What::CutEnded {
                version,
                said: record.line(),
                failed: Some(failed),
            });
        }
    }
    drop_run(&fleet.repo, &mut fleet.scan, claim)?;
    Ok(Outcome::Done)
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
        Standing::Due => format!("due: {}; it starts once the fleet is idle", past()),
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
    let mut cutting = Cutting { repo, config, steps: &steps, gh: &gh, host, checkout: &checkout };
    let first = from.unwrap_or_else(|| record.step());
    record.failed = None;
    for step in first.from() {
        record.at = Some(step);
        checkout.record_cut(&record)?;
        let began = Instant::now();
        if let Err(error) = cutting.take(step, &mut record, &clock) {
            let why = format!("{error:#}");
            record.failed = Some(Failure { step, why: why.clone() });
            checkout.record_cut(&record)?;
            return Ok(json!({"version": record.version, "failed": step.word(), "why": why,
                "said": record.line(), "log": checkout.cut_log()?}));
        }
        record.took.retain(|took| took.step != step);
        record.took.push(Took { step, seconds: began.elapsed().as_secs() });
        record.at = None;
        checkout.record_cut(&record)?;
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
}

impl Cutting<'_> {
    /// Takes `step` of the cut `record`, adding what it learns to it.
    fn take(&mut self, step: Step, record: &mut Record, clock: &impl Fn() -> u64) -> Result<()> {
        match step {
            Step::Set => self.set(record),
            Step::Pr => self.pr(record),
            Step::Gate if record.landed.is_some() => Ok(()),
            Step::Gate => self.gate(clock()),
            Step::Land => self.land(record, clock),
            Step::Tag => self.tag(record),
            Step::Publish => self.publish(record),
        }
    }

    /// Runs the repo's `set` lines, and commits what they changed. Lines that
    /// change nothing find the version set already: by a cut whose end was
    /// cut short after its PR landed. Where that landing is on the base
    /// branch, it is the one this cut tags; else nothing set the version.
    fn set(&self, record: &mut Record) -> Result<()> {
        self.sh(Step::Set, &self.steps.set, &record.version)?;
        self.repo.git(&["add", "-A"])?;
        let changed = self.repo.git_raw(&["status", "--porcelain"])?;
        let title = cut::title(&repo_name(self.repo)?, &record.version);
        if changed.trim().is_empty() {
            let Some(landed) = self.landed_titled(&record.from, &title)? else {
                bail!(
                    "the set lines changed nothing, and no cut's PR titled {title:?} landed past \
                     {}: they set no version",
                    record.from
                );
            };
            record.pr = Some(landed.pr);
            record.landed = Some(landed.commit);
            return Ok(());
        }
        self.repo.git(&["commit", "--quiet", "-m", &title])?;
        Ok(())
    }

    /// The landing of a cut's PR titled `title` the base branch holds past
    /// the tag `from`, where one is.
    fn landed_titled(&self, from: &str, title: &str) -> Result<Option<Joined>> {
        let tag = self.tag_commit(from)?;
        let base = self.base()?;
        let joins = joins_in(self.repo, &[&format!("{tag}..{base}")])?;
        let ends = format!("{title} (#");
        Ok(joins.into_iter().find(|join| {
            matches!(join.of, Of::Cut(_))
                && self.subject(&join.commit).is_ok_and(|s| s.starts_with(&ends))
        }))
    }

    fn subject(&self, commit: &str) -> Result<String> {
        self.repo.git(&["log", "-1", "--format=%s", commit])
    }

    /// The commit the version tag `name` is on, on the remote.
    fn tag_commit(&self, name: &str) -> Result<String> {
        let tags = self.repo.remote_tags()?;
        let found = tags.into_iter().find(|(tag, _)| tag == name);
        let (_, commit) = found.ok_or_else(|| anyhow!("the remote has no tag {name}"))?;
        self.repo.bring(&commit, &format!("refs/tags/{name}"))?;
        Ok(commit)
    }

    /// The base branch's tip on the remote, its commits here.
    fn base(&self) -> Result<String> {
        let branch = &self.config.base_branch;
        let base = self.repo.fetched(&format!("refs/heads/{branch}"))?;
        base.ok_or_else(|| anyhow!("the remote has no branch {branch:?}"))
    }

    /// Pushes the version's commit to a branch of the cut's own and opens
    /// its PR; one already open for that branch is the cut's.
    fn pr(&self, record: &mut Record) -> Result<()> {
        if record.landed.is_some() {
            return Ok(());
        }
        let branch = branch(record);
        self.repo.git(&["push", "--quiet", REMOTE, &format!("HEAD:refs/heads/{branch}")])?;
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

    /// Gates it, the landing train merged in: green, and on record.
    fn gate(&self, now: u64) -> Result<()> {
        let gated = crate::gate::gate(self.repo, self.config, true, self.host, now)?;
        if gated["status"] == "green" && gated["recorded"] == true {
            return Ok(());
        }
        let detail = gated["detail"].as_str().unwrap_or("the gate said nothing");
        match gated["files"].as_array() {
            Some(files) => bail!(
                "{detail} — the files: {}",
                files.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")
            ),
            None => bail!("{detail}"),
        }
    }

    /// Joins its PR to the landing train, on no issue, and waits for it to
    /// land, joining it again whenever it is off the train and not landed: a
    /// train given up and its PRs joined again without it. A fleet lock
    /// standing, or a conflict with the train, fails it.
    fn land(&self, record: &mut Record, clock: &impl Fn() -> u64) -> Result<()> {
        if record.landed.is_some() {
            return Ok(());
        }
        let number = record.pr.ok_or_else(|| anyhow!("the cut opened no PR"))?;
        let head = self.repo.git(&["rev-parse", "HEAD"])?;
        loop {
            let train = Train::read(self.repo, self.config)?;
            if self.repo.within(&head, &train.base)? {
                let tag = self.tag_commit(&record.from)?;
                let joins = joins_in(self.repo, &[&format!("{tag}..{}", train.base)])?;
                let Some(join) = joins.into_iter().rev().find(|join| join.pr == number) else {
                    bail!(
                        "PR #{number} is on {} with no landing of the landing train's: it was \
                         merged outside it, and is no landing a cut tags",
                        self.config.base_branch
                    );
                };
                record.landed = Some(join.commit);
                return Ok(());
            }
            if !train.joins(self.repo)?.iter().any(|join| join.pr == number) {
                self.join(number, clock())?;
            }
            std::thread::sleep(LAND_POLL);
        }
    }

    /// Puts PR `number` on the landing train, as the cut's on `host`.
    fn join(&self, number: u64, now: u64) -> Result<()> {
        let locks = crate::lock::standing_locks(self.gh, self.config)?;
        if !locks.is_empty() {
            let answer = crate::train::locked(json!({}), &locks);
            bail!("{}", answer["detail"].as_str().unwrap_or("a fleet lock stands"));
        }
        let open = self.gh.open_prs()?;
        let pr = open.iter().find(|pr| pr.number == number);
        let pr = pr.ok_or_else(|| anyhow!("PR #{number} is not open: it joins nothing"))?;
        let of = Of::Cut(self.host.to_string());
        let answer = crate::train::join(self.repo, self.config, pr, &of, json!({}), now)?;
        match answer["outcome"].as_str() {
            Some("joined" | "merged") => Ok(()),
            _ => bail!(
                "{} — go on from the step `gate` once it is committed",
                answer["detail"].as_str().unwrap_or("it could not join the landing train")
            ),
        }
    }

    /// Tags the commit that landed it `v<version>` and pushes the tag; the
    /// PRs it holds are read again, up to that commit, its own left out.
    fn tag(&self, record: &mut Record) -> Result<()> {
        let landed =
            record.landed.clone().ok_or_else(|| anyhow!("nothing says where it landed"))?;
        let tag = format!("v{}", record.version);
        let from = self.tag_commit(&record.from)?;
        let mut prs = prs_past(self.repo, &from, &landed)?;
        prs.retain(|pr| Some(*pr) != record.pr);
        record.prs = prs;
        let here =
            self.repo.try_git(&["rev-parse", "-q", "--verify", &format!("{tag}^{{commit}}")])?;
        if !here.ok {
            let message = cut::title(&repo_name(self.repo)?, &record.version);
            self.repo.git(&["tag", "-a", &tag, &landed, "-m", &message])?;
        } else if here.stdout != landed {
            bail!("{tag} is here on {}, not on {landed}, which landed the cut", here.stdout);
        }
        let to = format!("refs/tags/{tag}");
        if self.repo.try_git(&["push", "--quiet", REMOTE, &to])?.ok {
            return Ok(());
        }
        match self.repo.remote_tags()?.into_iter().find(|(name, _)| *name == tag) {
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

    /// Runs `lines` by `sh` in the checkout with the version in
    /// `cut::VERSION_ENV`, what they print added to the cut's log under the
    /// step's name.
    fn sh(&self, step: Step, lines: &str, version: &str) -> Result<()> {
        let file = self.checkout.cut_log()?;
        let mut log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .with_context(|| format!("cannot write {}", file.display()))?;
        writeln!(log, "== {} (VERSION={version})", step.word())?;
        let status = Command::new("sh")
            .args(["-c", lines])
            .current_dir(&self.repo.dir)
            .env(cut::VERSION_ENV, version)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .status()
            .context("sh could not run")?;
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
