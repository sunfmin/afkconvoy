//! A red base tip becomes a dispatchable fleet-lock issue; its worker alone
//! may land a repair while ordinary turns wait outside the worker lanes.
use crate::claims::{Me, drop_run, take_run};
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::remote::Fresh;
use afkconvoy_core::claims::Holder;
use afkconvoy_core::tick::Outcome;
use anyhow::Result;

pub fn red_ref(repo: &Repo, branch: &str) -> String {
    repo.layout().under(&format!("base/red/{branch}"))
}

/// A repair opened for this tip, or an older unmarked lock, may bypass its
/// missing green record. A marked repair for another tip still waits.
pub fn is_for(
    repo: &Repo,
    gh: &crate::gh::GitHub,
    issue: u64,
    tip: &str,
    branch: &str,
) -> Result<bool> {
    let config = crate::config::load(&repo.dir.join(crate::config::FILE))?;
    if crate::target::branch(repo, &config, issue)? != branch {
        return Ok(false);
    }
    let marker = format!("<!--convoy:base-repair tip={tip}-->");
    let read = gh.issue(issue)?;
    Ok(read.body.is_none_or(|body| {
        !body.starts_with("<!--convoy:base-repair ") || body.starts_with(&marker)
    }))
}

/// Tends each base tip the tick's listing of the remote marks red
/// (docs/adr/0043): a mark made since is tended by the next tick.
pub fn tend(fleet: &mut Fleet, me: &Me) -> Result<Outcome> {
    let prefix = red_ref(&fleet.repo, "");
    let mut outcome = Outcome::Nothing;
    for record in fleet.records_at_open(&format!("{prefix}*")) {
        let Some(branch) = record.name.strip_prefix(&prefix) else { continue };
        let next = tend_on(fleet, me, branch)?;
        if next != Outcome::Nothing {
            outcome = next;
        }
    }
    Ok(outcome)
}

fn tend_on(fleet: &mut Fleet, me: &Me, branch: &str) -> Result<Outcome> {
    let mut config = crate::config::load(&fleet.repo.dir.join(crate::config::FILE))?;
    config.base_branch = branch.to_string();
    let base = crate::turn::base(&fleet.repo, &config)?;
    if fleet.remote.sha(&red_ref(&fleet.repo, branch), Fresh::Process)?.as_deref() != Some(&base) {
        return Ok(Outcome::Nothing);
    }
    let declared = crate::turn::tip_config(&fleet.repo, &base)?;
    let key = if branch == fleet.config.base_branch {
        "base-repair".to_string()
    } else {
        // A slash names a tracker issue to the claim reader, never a run.
        format!("base-repair-{:016x}", afkconvoy_core::gate::key(branch))
    };
    if let Some(left) = fleet.scan.runs.iter().find(|run| run.key == key).cloned() {
        if left.holder(Some(&me.instance), &fleet.scan.beats(), me.now) == Holder::Peer {
            return Ok(Outcome::Nothing);
        }
        drop_run(&fleet.repo, &mut fleet.scan, &left)?;
    }
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, &key)? else {
        return Ok(Outcome::Lost);
    };
    let opened = (|| {
        let config = &config;
        let label = config.ready_label.clone();
        let labels = fleet.gh.labels()?;
        for label in [label.as_str(), &config.lock_label, afkconvoy_core::plan::REPAIR_LABEL] {
            if !labels.iter().any(|known| known == label) {
                fleet.gh.create_label(label, "afkconvoy: repair the red base branch")?;
            }
        }
        for issue in crate::lock::standing_locks(&fleet.gh, config)? {
            if is_for(&fleet.repo, &fleet.gh, issue, &base, branch)? {
                let read = fleet.gh.issue(issue)?;
                // A repair carrying the escalate label was escalated as a
                // repair: by its worker or by the fleet. It waits for a human
                // answer; no tick takes that label off (sunfmin/control#498).
                if afkconvoy_core::plan::is_repair(&read.labels)
                    && read.labels.contains(&config.escalate_label)
                {
                    return Ok(Outcome::Nothing);
                }
                let mut changed = false;
                // Older locks can be waiting at a product workflow's human
                // stage. The repair must run under the operator's authority.
                // The repair label goes on last: a tick cut short before it
                // leaves an old lock, which the next tick moves again.
                if read.labels.contains(&config.escalate_label) {
                    fleet.gh.remove_label(issue, &config.escalate_label)?;
                    changed = true;
                }
                for label in [config.ready_label.as_str(), afkconvoy_core::plan::REPAIR_LABEL] {
                    if !read.labels.iter().any(|known| known == label) {
                        fleet.gh.add_label(issue, label)?;
                        changed = true;
                    }
                }
                return Ok(if changed { Outcome::Done } else { Outcome::Nothing });
            }
        }
        let body = format!(
            "<!--convoy:base-repair tip={base}-->\nThe base branch `{}` was gated alone in a clean checkout and is red.\n\nReproduce at `{base}` with `{}`. Find the cause from the gate's log, fix it and add a regression test. Open the repair PR and run `afkconvoy land --issue <n>`. This fleet-lock issue's turn may start on the red tip; it still gates the complete merge commit before landing. Closing this issue frees ordinary turns.\n",
            config.base_branch, declared.gate_command
        );
        let issue = fleet.gh.open_issue(
            &format!("Repair the red {} tip {}", config.base_branch, &base[..12.min(base.len())]),
            &body,
            &[],
        )?;
        crate::target::pin(&fleet.repo, config, issue, Some(branch))?;
        for label in [label.as_str(), &config.lock_label, afkconvoy_core::plan::REPAIR_LABEL] {
            fleet.gh.add_label(issue, label)?;
        }
        Ok(Outcome::Done)
    })();
    drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
    opened
}
