//! What `run`, `tick` and `plan` check before anything else: a host or a repo
//! the fleet cannot work is refused here, not at its first tick or landing.
//!
//! A probe that passed is noted on the host that made it (`Passed`). `run`
//! probes at every start; `tick` and `plan` trust a note that still stands
//! (`standing`) and probe only without one.

use afkconvoy_core::config::Config;
use afkconvoy_core::gate::outlived;
use afkconvoy_core::layout::{
    Answer, BaseCheck, GATE_KIND, PROBE_STANDS, answer, base_check, choose, no_base_branch,
    no_gate_records, probe_stands,
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

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

/// The warnings of a startup probe that still stands for the repo this
/// command was run in, said again; `None` when there is none, and one is to
/// be made. One stands for an hour (`probe_stands`) on the host that made it,
/// while the config's base branch and team, the remote, and the noted ref
/// layout are what it was asked about — and, for a command that starts
/// workers, `launches` are the worker launch commands it checked. Says on stderr
/// that no probe was made. Whatever keeps the note from being read or
/// compared is the probe's to report: none stands.
pub fn standing(now: u64, launches: &Launches) -> Option<Vec<String>> {
    let read = || -> Result<(Passed, Asked)> {
        let repo = Repo::here()?;
        let config = config::load(&repo.dir.join(config::FILE))?;
        let passed: Passed =
            serde_json::from_str(&std::fs::read_to_string(repo.note(PASSED_NOTE)?)?)?;
        // A command that starts no worker asks nothing of the launch commands.
        let launch = if launches.starts() { launches.key()? } else { passed.asked.launch };
        Ok((passed, Asked::now(&repo, &config, launch)?))
    };
    let (passed, asked) = read().ok()?;
    if !probe_stands(passed.at, now) || passed.asked != asked {
        return None;
    }
    eprintln!(
        "note: no startup probe was made: the one that passed on this host {} min ago stands \
         for {} min (`afkconvoy plan --help`)",
        (now - passed.at) / 60,
        PROBE_STANDS / 60
    );
    Some(passed.warnings)
}

/// Checks this host and the repo this command was run in → what the human
/// should know before the fleet carries on. An error is a host or a repo the
/// fleet cannot work. `launches` are the worker launch commands of the start.
///
/// In order: every command the fleet needs is here and logged in
/// (`tools::check`), before anything is read or written; in a bound repo,
/// every harness the fleet could be asked for can be started, its worker
/// launch command taking what starts a worker in its role
/// (`role::probe`); the base branch exists (none is ever created); no rule of its
/// protection refuses the train's push; where the remote takes the fleet's
/// refs — noted for every later command (`Repo::layout`); whether it takes
/// gate records, and the ones past their day are swept. A probe that passed is
/// noted for `standing`.
pub fn startup(now: u64, launches: &Launches) -> Result<Vec<String>> {
    tools::check(launches).see(See::Troubleshooting)?;
    let repo = Repo::here()?;
    let config = config::load(&repo.dir.join(config::FILE)).see(See::Troubleshooting)?;
    // A bound repo's workers start in their roles, each on the harness its
    // stage names: every one's command must be there, and take that.
    if let Some(bound) = &config.bound {
        role::probe(launches, bound, &config.labels()).see(See::Troubleshooting)?;
    }
    let branch = &config.base_branch;
    let mut warnings = vec![];

    if repo.remote_sha(&format!("refs/heads/{branch}"))?.is_none() {
        return Err(See::Troubleshooting.refusal(no_base_branch(branch)));
    }
    match base_check(branch, &GitHub { dir: repo.dir.clone() }.protection(branch)) {
        BaseCheck::Clear(_) => {}
        BaseCheck::Warn(warning) => warnings.push(warning),
        BaseCheck::Refuse(why) => return Err(See::Troubleshooting.refusal(why)),
    }

    // The probe's own name: two starts probing at once never meet on one ref.
    let token = format!("{now}-{}", std::process::id());
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
    let passed = Passed { at: now, asked: Asked::now(&repo, &config, launches.key()?)?, warnings };
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
