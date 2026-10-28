//! What `run`, `tick` and `plan` check before anything else: a host or a repo
//! the fleet cannot work is refused here, not at its first tick or landing.
//!
//! A probe that passed is noted on the host that made it (`Passed`). `run`
//! probes at every start; `tick` and `plan` trust a note that still stands
//! (`standing`) and probe only without one.
//!
//! A check that found the checkout's tracker private is noted beside it
//! (`Private`): a running fleet instance asks again once it no longer stands
//! (`public_tracker`), and a probe stands only while it does.

use std::path::Path;

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::outlived;
use afkconvoy_core::layout::{
    Answer, BaseCheck, GATE_KIND, PROBE_STANDS, answer, base_check, choose, no_base_branch,
    no_gate_records, probe_stands,
};
use afkconvoy_core::tools::Tool;
use afkconvoy_core::tracker::{Tracker, is_public, public};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::checkout_list::{named_repo, tracker_of};
use crate::config;
use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::launch::Launches;
use crate::role;
use crate::tools;
use crate::topic::{Explain, See};

/// What a probe's commit says it is.
const PROBE_RECORD: &str = "convoy:probe";

/// The note of the last startup probe that passed (`Repo::note`).
const PASSED_NOTE: &str = "convoy-probe";

/// The note of the last check that found the checkout's tracker private
/// (`Private`).
const TRACKER_NOTE: &str = "convoy-tracker";

/// A check that found the checkout's tracker private, as the host that made
/// it notes it: when, and of which tracker — the repo it names, or none for
/// the checkout's own (`noted_as`). It stands for an hour, as a startup probe
/// does (`probe_stands`), and of that tracker alone.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Private {
    /// When it found it private, in epoch seconds.
    at: u64,
    tracker: Option<String>,
}

/// The tracker `tracker` as a note of it names it.
fn noted_as(tracker: &Tracker) -> Option<String> {
    tracker.named().map(|named| named.as_str().to_string())
}

/// Whether a check that found the tracker `tracker` of the checkout `repo`
/// private stands at `now` on this host (`Private`).
fn found_private(repo: &Repo, tracker: &Tracker, now: u64) -> bool {
    let read = || -> Result<Private> {
        Ok(serde_json::from_str(&std::fs::read_to_string(repo.note(TRACKER_NOTE)?)?)?)
    };
    read().is_ok_and(|found| found.tracker == noted_as(tracker) && probe_stands(found.at, now))
}

/// Why the fleet of the checkout `repo` is refused, where GitHub says its
/// tracker is public (`tracker::public`); `None` where it is not. A start's
/// probe asks GitHub every time; one `trusting` — a running fleet instance's,
/// before each tick — asks nothing while a check that found this same
/// tracker private stands, so a tracker made public is found within the
/// hour, and one said anew at once. One found private is noted; one found
/// public is not, and is asked about again at every check, until it is
/// private again or another is said.
pub fn public_tracker(repo: &Repo, now: u64, trusting: bool) -> Result<Option<String>> {
    let gh = GitHub::of(repo)?;
    if trusting && found_private(repo, &gh.tracker, now) {
        return Ok(None);
    }
    let note = repo.note(TRACKER_NOTE)?;
    // Gone before GitHub is asked: no check stands but one that found it private.
    let _ = std::fs::remove_file(&note);
    if let Some(why) = refused_public(repo, &gh)? {
        return Ok(Some(why));
    }
    let found = Private { at: now, tracker: noted_as(&gh.tracker) };
    std::fs::write(&note, serde_json::to_string(&found)?)
        .with_context(|| format!("cannot write {}", note.display()))?;
    Ok(None)
}

/// Why the fleet of the checkout `repo` is refused, where GitHub says the
/// tracker `gh` names is public (`tracker::public`), naming it and the line
/// that says a private one; `None` where it is not.
pub fn refused_public(repo: &Repo, gh: &GitHub) -> Result<Option<String>> {
    if !is_public(&gh.visibility()?) {
        return Ok(None);
    }
    Ok(Some(public(&named(repo, &gh.tracker), &repo.dir.display().to_string())))
}

/// The tracker `tracker` of the checkout `repo`, as a human is told it: the
/// repo it names — of the checkout's own, as its remote names it, where it
/// does.
pub fn named(repo: &Repo, tracker: &Tracker) -> String {
    match tracker.named() {
        Some(named) => named.to_string(),
        None => named_repo(repo).map_or_else(|_| "its own repo".to_string(), |own| own.to_string()),
    }
}

/// A startup probe that passed, as the host that made it notes it: when, what
/// it was asked about, and what it warned of. A probe asked about anything
/// else is another probe.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Passed {
    /// When it passed, in epoch seconds.
    at: u64,
    #[serde(flatten)]
    asked: Asked,
    warnings: Vec<String>,
}

/// What a startup probe is asked about.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Asked {
    base_branch: String,
    /// Where the remote is.
    remote: String,
    /// The team the repo is bound to: a bound repo's workers start in roles.
    team: Option<String>,
    /// The ref layout it settled on, as the repo has it noted.
    layout: String,
    /// The worker launch commands it checked, when the start knew the
    /// default harness's — as their key (`Launches::key`): a command itself
    /// is written to no file.
    launch: Option<u64>,
}

impl Asked {
    /// What a probe of `repo` under `config` is asked about now.
    fn now(repo: &Repo, config: &Config, launch: Option<u64>) -> Result<Asked> {
        Ok(Asked {
            base_branch: config.base_branch.clone(),
            remote: repo.git(&["remote", "get-url", REMOTE])?,
            team: config.bound.as_ref().map(|bound| bound.team.to_string()),
            layout: repo.layout().name().to_string(),
            launch,
        })
    }
}

/// The warnings of a startup probe that still stands for the checkout
/// `repo`, said again; `None` when there is none, and one is to
/// be made. One stands for an hour (`probe_stands`) on the host that made it,
/// while the config's base branch and team, the remote, and the noted ref
/// layout are what it was asked about — and, for a command that starts
/// workers, `launches` are the worker launch commands it checked — and while
/// a check that found the tracker the checkout says now private stands
/// (`Private`). Says on stderr
/// that no probe was made. Whatever keeps the note from being read or
/// compared is the probe's to report: none stands.
/// The startup probe that still stands for `repo` at `now`, read the way
/// `standing` reads it. `None` when none stands. Says nothing.
fn passed_note(repo: &Repo, now: u64, launches: &Launches) -> Option<Passed> {
    let config = config::load(&repo.dir.join(config::FILE)).ok()?;
    let text = std::fs::read_to_string(repo.note(PASSED_NOTE).ok()?).ok()?;
    let passed: Passed = serde_json::from_str(&text).ok()?;
    // A command that starts no worker asks nothing of the launch commands.
    let launch = if launches.starts() { launches.key().ok()? } else { passed.asked.launch };
    let asked = Asked::now(repo, &config, launch).ok()?;
    let tracker = tracker_of(repo).ok()?;
    if !probe_stands(passed.at, now) || passed.asked != asked || !found_private(repo, &tracker, now)
    {
        return None;
    }
    Some(passed)
}

pub fn standing(repo: &Repo, now: u64, launches: &Launches) -> Option<Vec<String>> {
    let passed = passed_note(repo, now, launches)?;
    eprintln!(
        "note: no startup probe was made: the one that passed on this host {} min ago stands \
         for {} min (`afkconvoy plan --help`)",
        (now - passed.at) / 60,
        PROBE_STANDS / 60
    );
    Some(passed.warnings)
}

/// Whether a startup probe still stands for `repo` at `now`, for a command
/// that starts no worker (`Launches::foreseen`). It is `standing` without
/// the note: `doctor` reads it and makes no probe, and says nothing about one.
///
/// A command that starts no worker does not compare launch commands inside
/// `passed_note`. This does: the probe is trusted only when it checked the
/// commands the kit has now. A doctor then starts no harness.
pub fn stands(repo: &Repo, now: u64) -> bool {
    let launches = Launches::foreseen(repo);
    let Some(passed) = passed_note(repo, now, &launches) else { return false };
    match launches.key() {
        Ok(key) => passed.asked.launch == key,
        Err(_) => false,
    }
}

/// Why no checkout is found on a host with no git, which finds one: what a
/// start's first check (`tools::check`) refuses it with in the directory
/// `dir` — git, and every other command the fleet needs that this host lacks,
/// each with the line that installs it, in one refusal. `None` where git is
/// on the `PATH`.
pub fn without_git(dir: &Path) -> Option<anyhow::Error> {
    if tools::on_path(Tool::Git.name()) {
        return None;
    }
    // A checkout git cannot be asked about: what the check asks of it — its
    // remote, whether orca knows it — it answers as of none.
    let unfound = Repo::new(dir.to_path_buf());
    let checked = tools::check(&unfound, &Launches::foreseen(&unfound));
    let refused = checked.err().unwrap_or_else(|| anyhow::anyhow!("git is not on the PATH"));
    Some(See::Troubleshooting.refusal(format!("{refused:#}")))
}

/// Checks this host and the checkout `repo` → what the human
/// should know before the fleet carries on. An error is a host or a repo the
/// fleet cannot work. `launches` are the worker launch commands of the start.
///
/// In order: every command the fleet needs is here and logged in
/// (`tools::check`), before anything is read or written; in a bound repo,
/// every harness the fleet could be asked for can be started, its worker
/// launch command taking what starts a worker in its role
/// (`role::probe`); the base branch exists (none is ever created); no rule of its
/// protection refuses the train's push; the tracker is not public
/// (`public_tracker`), before anything is pushed; where the remote takes the fleet's
/// refs — noted for every later command (`Repo::layout`); whether it takes
/// gate records, and the ones past their day are swept. A probe that passed is
/// noted for `standing`.
pub fn startup(repo: &Repo, now: u64, launches: &Launches) -> Result<Vec<String>> {
    tools::check(repo, launches).see(See::Troubleshooting)?;
    let config = config::load(&config::file_of(repo)).see(See::Troubleshooting)?;
    // A bound repo's workers start in their roles, each on the harness its
    // stage names: every one's command must be there, and take that.
    if let Some(bound) = &config.bound {
        role::probe(repo, launches, bound, &config.labels()).see(See::Troubleshooting)?;
    }
    let branch = &config.base_branch;
    let mut warnings = vec![];

    if repo.remote_sha(&format!("refs/heads/{branch}"))?.is_none() {
        return Err(See::Troubleshooting.refusal(no_base_branch(branch)));
    }
    match base_check(branch, &GitHub::of(repo)?.protection(branch)) {
        BaseCheck::Clear(_) => {}
        BaseCheck::Warn(warning) => warnings.push(warning),
        BaseCheck::Refuse(why) => return Err(See::Troubleshooting.refusal(why)),
    }
    if let Some(why) = public_tracker(repo, now, false)? {
        return Err(See::Troubleshooting.refusal(why));
    }
    crate::publication::check(repo).see(See::Troubleshooting)?;

    // The probe's own name: two starts probing at once never meet on one ref.
    let token = format!("{now}-{}", crate::unique());
    let commit = repo.record_commit(PROBE_RECORD)?;
    let push = |refname: &str| match repo.try_git(&[
        "push",
        "--quiet",
        REMOTE,
        &format!("{commit}:{refname}"),
    ]) {
        Ok(ran) => answer(ran.ok, &ran.stderr),
        Err(error) => Answer::Unreached(format!("{error:#}")),
    };
    let chosen = choose(|layout| push(&layout.probe("", &token)))
        .map_err(|why| See::Troubleshooting.refusal(why))?;
    let layout = chosen.layout();
    warnings.extend(chosen.warning());
    repo.note_layout(layout)?;

    let records = match push(&layout.probe(GATE_KIND, &token)) {
        Answer::Taken => true,
        Answer::Rejected(why) | Answer::Unreached(why) => {
            warnings.push(no_gate_records(layout, &why));
            false
        }
    };
    // Probes — this one's, and any a start killed mid-probe left — and the
    // gate records no landing would believe any more.
    let mut litter = vec![];
    for record in repo.remote_records(&layout.every(""))? {
        let gate_record = layout.kept(&record.name).is_some_and(|kept| kept.starts_with(GATE_KIND));
        // A ref that moved while it was read has no subject yet: not litter.
        let swept = records && gate_record && !record.subject.is_empty();
        if layout.is_probe(&record.name) || (swept && outlived(&record.subject, now)) {
            litter.push(record.name);
        }
    }
    if !litter.is_empty() {
        let mut args = vec!["push", "--quiet", REMOTE, "--delete"];
        args.extend(litter.iter().map(String::as_str));
        // Soft: a ref that stays is litter the next start deletes.
        let _ = repo.try_git(&args);
    }
    let passed = Passed { at: now, asked: Asked::now(repo, &config, launches.key()?)?, warnings };
    let note = repo.note(PASSED_NOTE)?;
    std::fs::write(&note, serde_json::to_string(&passed)?)
        .with_context(|| format!("cannot write {}", note.display()))?;
    Ok(passed.warnings)
}

/// Says each warning where a human reads it; stdout stays the command's answer.
pub fn announce(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}
