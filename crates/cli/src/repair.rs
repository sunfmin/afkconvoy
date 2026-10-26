//! A red base tip becomes a dispatchable fleet-lock issue; its worker alone
//! may land a repair while ordinary turns wait outside the worker lanes.
use crate::claims::{Me, drop_run, take_run};
use crate::fleet::Fleet;
use crate::git::Repo;
use afkconvoy_core::claims::Holder;
use afkconvoy_core::tick::Outcome;
use anyhow::Result;

pub fn red_ref(repo: &Repo, branch: &str) -> String {
    repo.layout().under(&format!("base/red/{branch}"))
}

/// Only the repair opened for this red tip may bypass its missing green
/// record. Another lock issue still waits for the base to be repaired.
pub fn is_for(gh: &crate::gh::GitHub, issue: u64, tip: &str) -> Result<bool> {
    let marker = format!("<!--convoy:base-repair tip={tip}-->");
    let read = gh.issue(issue)?;
    Ok(afkconvoy_core::plan::is_repair(&read.labels)
        && read.body.is_some_and(|body| body.starts_with(&marker)))
}

pub fn tend(fleet: &mut Fleet, me: &Me) -> Result<Outcome> {
    let base = fleet.base_tip()?;
    if fleet.repo.remote_sha(&red_ref(&fleet.repo, &fleet.config.base_branch))?.as_deref()
        != Some(&base)
    {
        return Ok(Outcome::Nothing);
    }
    let key = "base-repair";
    if let Some(left) = fleet.scan.runs.iter().find(|run| run.key == key).cloned() {
        if left.holder(Some(&me.instance), &fleet.scan.beats(), me.now) == Holder::Peer {
            return Ok(Outcome::Nothing);
        }
        drop_run(&fleet.repo, &mut fleet.scan, &left)?;
    }
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, key)? else {
        return Ok(Outcome::Lost);
    };
    let opened = (|| {
        for issue in crate::lock::standing_locks(&fleet.gh, &fleet.config)? {
            if is_for(&fleet.gh, issue, &base)? {
                return Ok(Outcome::Nothing);
            }
        }
        let config = &fleet.config;
        let label = config.ready_label.clone();
        let labels = fleet.gh.labels()?;
        for label in [label.as_str(), &config.lock_label, afkconvoy_core::plan::REPAIR_LABEL] {
            if !labels.iter().any(|known| known == label) {
                fleet.gh.create_label(label, "afkconvoy: repair the red base branch")?;
            }
        }
        let body = format!(
            "<!--convoy:base-repair tip={base}-->\nThe base branch `{}` was gated alone in a clean checkout and is red.\n\nReproduce at `{base}` with `{}`. Find the cause from the gate's log, fix it and add a regression test. Open the repair PR and run `afkconvoy land --issue <n>`. This fleet-lock issue's turn may start on the red tip; it still gates the complete merge commit before landing. Closing this issue frees ordinary turns.\n",
            config.base_branch, config.gate_command
        );
        fleet.gh.open_issue(
            &format!("Repair the red {} tip {}", config.base_branch, &base[..12.min(base.len())]),
            &body,
            &[&label, &config.lock_label, afkconvoy_core::plan::REPAIR_LABEL],
        )?;
        Ok(Outcome::Done)
    })();
    drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
    opened
}
