//! Patrols on a bound repo (docs/adr/0008): what a rebuild reads of each one
//! the repo names, the start of a run, and its end. When a patrol runs and
//! what makes a run a failed one is the core's (`afkconvoy_core::patrol`).

use afkconvoy_core::intake::opened_by;
use afkconvoy_core::patrol::{
    Row, Seen, Standing, broke, checkout_name, claim_key, failure_of, failure_report,
    failure_title, is_due, read_run, run_over, run_ref, write_run,
};
use afkconvoy_core::prompt::{PatrolFields, patrol_prompt};
use afkconvoy_core::role::{Facts, On, Start, flows};
use afkconvoy_core::team::PRECEDENTS;
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, anyhow};

use crate::claims::{Me, drop_run, take_run};
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::gh::OpenIssues;
use crate::git::REMOTE;
use crate::orca::{Source, Worktree};
use crate::stage::Flows;
use crate::team::unknown_patrols;

/// What the remote and GitHub say of each patrol the repo names, in the
/// order it names them. A repo that names none, and one whose team does not
/// load, has none to run: what is wrong with a team is `check`'s to say. A
/// name the team has no patrol for is an error.
pub fn seen(fleet: &Fleet, read: &Flows, open: &OpenIssues) -> Result<Vec<Seen>> {
    let Some(bound) = fleet.config.bound.as_ref().filter(|bound| !bound.patrols.is_empty()) else {
        return Ok(vec![]);
    };
    let Ok(kept) = read.kept() else { return Ok(vec![]) };
    let named = kept
        .team
        .patrols_named(&bound.patrols)
        .map_err(|unknown| unknown_patrols(bound.team.as_str(), &kept.team, &unknown))?;
    // An issue handed to a human left its workflow, and is open all the same.
    let escalate = &kept.team.escalate_label;
    let mut opened = read.opened();
    let issues = &open.rows;
    for issue in issues {
        if flows(&issue.labels).is_empty() && issue.labels.contains(escalate) {
            let comments = open.comments(issue.number);
            opened.extend(opened_by(comments.iter().map(|comment| comment.body.as_str())));
        }
    }
    let seen = |patrol: &&afkconvoy_core::team::Patrol| Seen {
        name: patrol.name.clone(),
        every: patrol.every,
        max_open: patrol.max_open,
        last_run: fleet.scan.last_runs.get(&patrol.name).copied(),
        claim: fleet.scan.runs.iter().find(|run| run.key == claim_key(&patrol.name)).cloned(),
        open: opened.iter().filter(|by| **by == patrol.name).count() as u64,
        failure: failure_of(issues, &patrol.name, escalate),
    };
    Ok(named.iter().map(seen).collect())
}

/// Runs the patrol `row`, due as the rebuild read it: claims the run, has
/// orca cut the patrol a checkout of the base branch as it stands, starts a
/// worker in the patrol's role there, and records the run. A checkout an
/// earlier run left — one that failed, or whose end was cut short — is
/// removed first: a run starts in a new one. `Lost` when a peer
/// claimed it, or ran it since the rebuild.
///
/// A step that fails is an error with the claim still held and no run
/// recorded: a later tick finds the run over and ends it, and the patrol is
/// due again.
pub fn run(fleet: &mut Fleet, me: &Me, agent: &Agent, read: &Flows, row: &Row) -> Result<Outcome> {
    let name = row.name.as_str();
    let kept = read.kept()?;
    let patrol =
        kept.team.patrol(name).ok_or_else(|| anyhow!("the team has no patrol {name:?}"))?;
    let role = kept.team.role_of(&patrol.work);
    let role_copy = kept.role(role)?;
    let bound = fleet.config.bound.as_ref().ok_or_else(|| anyhow!("the repo is not bound"))?;
    let harness = patrol.harness(bound.harness);
    // Before the claim: a patrol whose harness cannot be started here runs nothing.
    let launch = agent.command(harness)?;
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, &claim_key(name))? else {
        return Ok(Outcome::Lost);
    };
    // A peer's run may have started and ended since the rebuild read the remote.
    let recorded = run_ref(fleet.repo.layout(), name);
    let last = fleet.repo.remote_records(&recorded)?.pop();
    if !is_due(row.every, last.and_then(|record| read_run(&record.subject)), me.now) {
        drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
        return Ok(Outcome::Lost);
    }
    let base = fleet.base_tip()?;
    let source = Source::of(&fleet.repo)?;
    let created_as = checkout_name(name);
    if let Some(left) = Worktree::named(&source, &created_as)? {
        left.remove()?;
    }
    let checkout = Worktree::cut(&source, &created_as, &base, None)?;
    checkout.keep_started()?;

    let path = checkout.path.display().to_string();
    // A worker starts in its role's copy: its commands are run from the checkout.
    let own = || crate::own_command_in(&path);
    let workflows: Vec<&str> =
        kept.team.workflows.iter().map(|workflow| workflow.name.as_str()).collect();
    let fields = PatrolFields {
        patrol: name,
        role: &role.name,
        skill: &patrol.work.skill,
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        precedents: &kept.copy.dir.join(PRECEDENTS).display().to_string(),
        workflows: &workflows,
        room: row.max_open.saturating_sub(row.open),
        intake_command: &own().fixed(" intake --issue <number> --workflow <name>").own(),
        lock_command: &own().fixed(" lock --issue <number>").own(),
        over_command: &own().fixed(" over --run ").word(&claim.key).own(),
    };
    let brief = checkout.write_brief(&patrol_prompt(&fields).map_err(|e| anyhow!(e))?)?;
    let start = Start {
        role_dir: &role_copy.dir.display().to_string(),
        worktree: &path,
        team_dir: &kept.copy.dir.display().to_string(),
        model: patrol.model.as_deref(),
        guard: &crate::own_command().fixed(" guard").own(),
        report: harness.report(),
        facts: Facts {
            on: On::Patrol(name),
            role: &role.name,
            skill: &patrol.work.skill,
            writes: false,
            consult: false,
            base_branch: &fleet.config.base_branch,
            escalate_label: &kept.team.escalate_label,
            lock_label: &fleet.config.lock_label,
            role_commit: &role_copy.commit,
            team_commit: &kept.copy.commit,
        },
    };
    let terminal = checkout.start(&harness.in_role(launch.as_str(), &start), me.now)?;
    checkout.await_ready(&terminal, agent.ready_timeout)?;
    checkout.deliver(&terminal, &brief)?;

    let sha = fleet.repo.record_commit(&write_run(me.now))?;
    fleet.repo.git(&["push", "--quiet", "--force", REMOTE, &format!("{sha}:{recorded}")])?;
    fleet.scan.last_runs.insert(name.to_string(), me.now);
    Ok(Outcome::Done)
}

/// Ends the run of the patrol `row` once it is over: a peer's presumed dead
/// is only unclaimed; mine, declared over or its worker gone or stopped, is
/// checked for what
/// a patrol never leaves — a commit, a PR — and unclaimed. A run that left
/// one has failed: an issue is opened for a human, and no other is touched.
/// Its worker is ended with it: the checkout is removed, its terminal with
/// it — but a failed run's is kept, with only the terminal closed, for the
/// human its report gives the path to. `Nothing` while the worker is still
/// at it.
pub fn end(fleet: &mut Fleet, me: &Me, read: &Flows, row: &Row) -> Result<Outcome> {
    let Some(claim) = &row.claim else { return Ok(Outcome::Nothing) };
    let checkout = match row.standing {
        Standing::Running => Worktree::named(&Source::of(&fleet.repo)?, &checkout_name(&row.name))?,
        Standing::StaleRun => None,
        _ => return Ok(Outcome::Nothing),
    };
    if let Some(checkout) = checkout {
        if !run_over(claim, checkout.state(me.now)?, me.now) {
            return Ok(Outcome::Nothing);
        }
        let on = checkout.branch_now()?;
        let prs: Vec<u64> = fleet
            .gh
            .open_prs()?
            .iter()
            .filter(|pr| pr.head_branch == checkout.branch || Some(&pr.head_branch) == on.as_ref())
            .map(|pr| pr.number)
            .collect();
        let reason = broke(checkout.started()?.as_deref(), &checkout.tip()?, &prs);
        // One report stands for a patrol until a human closes it.
        if let Some(reason) = reason.as_ref().filter(|_| row.failure.is_none()) {
            let path = checkout.path.display().to_string();
            fleet.gh.open_issue(
                &failure_title(&row.name),
                &failure_report(&row.name, reason, &me.host, &path),
                &[read.kept()?.team.escalate_label.as_str()],
            )?;
        }
        match reason {
            Some(_) => checkout.close_terminals()?,
            None => checkout.remove()?,
        }
    }
    drop_run(&fleet.repo, &mut fleet.scan, claim)?;
    Ok(Outcome::Done)
}
