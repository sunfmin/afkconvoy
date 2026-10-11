//! What `run`, `tick` and `plan` check before anything else: a repo the fleet
//! cannot work is refused here, not at its first landing.

use afkconvoy_core::gate::outlived;
use afkconvoy_core::layout::{
    Answer, BaseCheck, GATE_KIND, answer, base_check, choose, no_base_branch, no_gate_records,
};
use anyhow::{Result, anyhow, bail};

use crate::config;
use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};

/// What a probe's commit says it is.
const PROBE_RECORD: &str = "convoy:probe";

/// Checks the repo this command was run in → what the human should know
/// before the fleet carries on. An error is a repo the fleet cannot work.
///
/// In order: the base branch exists (none is ever created); no rule of its
/// protection refuses the train's push; where the remote takes the fleet's
/// refs — noted for every later command (`Repo::layout`); whether it takes
/// gate records, and the ones past their day are swept.
pub fn startup(now: u64) -> Result<Vec<String>> {
    let repo = Repo::here()?;
    let config = config::load(&repo.dir.join(config::FILE))?;
    let branch = &config.base_branch;
    let mut warnings = vec![];

    if repo.remote_sha(&format!("refs/heads/{branch}"))?.is_none() {
        bail!(no_base_branch(branch));
    }
    match base_check(branch, &GitHub { dir: repo.dir.clone() }.protection(branch)) {
        BaseCheck::Clear(_) => {}
        BaseCheck::Warn(warning) => warnings.push(warning),
        BaseCheck::Refuse(why) => bail!(why),
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
    let chosen = choose(|layout| push(&layout.probe("", &token))).map_err(|why| anyhow!(why))?;
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
    Ok(warnings)
}

/// Says each warning where a human reads it; stdout stays the command's answer.
pub fn announce(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}
