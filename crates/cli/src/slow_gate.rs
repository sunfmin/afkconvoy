//! A slow gate's effect half: what a rebuild reads of the landings' recorded
//! gate runs and of the issues opened for a slow gate, and the opening of
//! one. Whether the gate is slow is the core's (`afkconvoy_core::slow_gate`).

use std::collections::HashMap;

use afkconvoy_core::claims::{Holder, RunClaim};
use afkconvoy_core::gate::{GateCommand, GreenRun, Tree, record_ref};
use afkconvoy_core::layout::GATE_KIND;
use afkconvoy_core::slow_gate::{
    CLAIM_KEY, LABEL, LANDINGS, Landing, Reported, SlowGate, TITLE, report, slow,
};
use afkconvoy_core::tick::Outcome;
use anyhow::Result;

use crate::claims::{Me, drop_run, take_run};
use crate::fleet::Fleet;
use crate::gh::GitHub;
use crate::git::{RemoteRecord, Repo};

/// How many commits down the base branch a landing is looked for. A recorded
/// gate run is swept a day after it was made, so the landings that still have
/// one are a day's.
const READ_BACK: &str = "--max-count=1000";

/// The base branch's landings below `base` that have a run of `command` on
/// record among `records` — the fleet's refs as one listing saw them — newest
/// first. A tree that landed twice is the landing of its newest commit: one
/// run is counted once.
///
/// The base branch is not walked while the remote holds fewer records than
/// make a gate slow.
fn landings(
    repo: &Repo,
    command: &GateCommand,
    records: &[RemoteRecord],
    base: &str,
) -> Result<Vec<Landing>> {
    let layout = repo.layout();
    let gate_record = |record: &&RemoteRecord| {
        layout.kept(&record.name).is_some_and(|kept| kept.starts_with(GATE_KIND))
    };
    let mut on_record: HashMap<&str, &str> = records
        .iter()
        .filter(gate_record)
        .map(|record| (record.name.as_str(), record.subject.as_str()))
        .collect();
    if on_record.len() < LANDINGS {
        return Ok(vec![]);
    }
    let listed = repo.git(&["log", "--first-parent", "--format=%H %T", READ_BACK, base])?;
    let mut landings = vec![];
    for (commit, tree) in listed.lines().filter_map(|row| row.split_once(' ')) {
        let tree = Tree::named(tree);
        let subject = on_record.remove(record_ref(layout, &tree, command).as_str());
        if let Some(run) = subject.and_then(|subject| GreenRun::recorded(subject, &tree, command)) {
            landings.push(Landing { commit: commit.to_string(), run });
        }
    }
    Ok(landings)
}

/// The issues the fleet opened for a slow gate, as GitHub has them now.
fn reported(gh: &GitHub) -> Result<Reported> {
    Ok(Reported::among(&gh.carrying(LABEL)?))
}

/// What a rebuild reads for the slow gate → the landings below `base` with a
/// recorded run of the gate command, and the issues opened for a slow gate.
/// GitHub is asked for those only while the landings say the gate is slow:
/// they change nothing otherwise.
pub fn seen(
    fleet: &Fleet,
    records: &[RemoteRecord],
    base: &str,
) -> Result<(Vec<Landing>, Reported)> {
    let command = GateCommand::new(&fleet.config.gate_command);
    let landings = landings(&fleet.repo, &command, records, base)?;
    let reported = match slow(
        &landings,
        fleet.config.gate_seconds(),
        fleet.config.gate_budget_seconds,
        &Reported::NONE,
    ) {
        Some(_) => reported(&fleet.gh)?,
        None => Reported::NONE,
    };
    Ok((landings, reported))
}

/// Claims the opening of the issue → the claim, or `None` when a live peer
/// holds it. A claim a tick left when it was cut short — mine, or a fleet's
/// presumed dead — is deleted first: it opens nothing any more.
fn claim(fleet: &mut Fleet, me: &Me) -> Result<Option<RunClaim>> {
    let left = fleet.scan.runs.iter().find(|run| run.key == CLAIM_KEY).cloned();
    if let Some(left) = left {
        if left.holder(Some(&me.instance), &fleet.scan.beats(), me.now) == Holder::Peer {
            return Ok(None);
        }
        drop_run(&fleet.repo, &mut fleet.scan, &left)?;
    }
    take_run(&fleet.repo, &mut fleet.scan, me, CLAIM_KEY)
}

/// Opens the issue that says the gate is `slow`, as the rebuild found it,
/// carrying the ready label: the fleet works it. The opening is claimed on the
/// remote, as a patrol's run is, so of several hosts that found the gate slow
/// one opens it. `Lost` when a peer holds that claim, or opened the issue
/// since the rebuild.
pub fn open(fleet: &mut Fleet, me: &Me, slow: &SlowGate) -> Result<Outcome> {
    let Some(claim) = claim(fleet, me)? else { return Ok(Outcome::Lost) };
    let config = &fleet.config;
    let opened = reported(&fleet.gh).and_then(|reported| {
        if reported.open.is_some() {
            return Ok(Outcome::Lost);
        }
        let body = report(slow, &config.gate_command, &config.base_branch);
        fleet.gh.open_issue(TITLE, &body, &[config.ready_label.as_str(), LABEL])?;
        Ok(Outcome::Done)
    });
    // Whatever came of it, the claim is over: the issue is what stands.
    drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
    opened
}
