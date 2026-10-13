//! Precedents on a bound repo (docs/adr/0011): what a rebuild reads of the
//! proposals, the start of one, and its end. Which rulings are proposed, how
//! many at once, and what makes a PR no proposal is the core's
//! (`afkconvoy_core::precedent`).
//!
//! The fleet opens nothing on the team's repo and merges nothing there: a
//! worker opens the PR, a human merges it, and all the fleet does is close
//! one that is no proposal.

use afkconvoy_core::patrol::run_over;
use afkconvoy_core::precedent::{
    Row, Ruling, Seen, Standing, closing_comment, is_proposal, proposed, proposed_comment, refused,
};
use afkconvoy_core::prompt::{ProposalFields, proposal_prompt};
use afkconvoy_core::role::{Facts, On, Start};
use afkconvoy_core::stage_record::stage_records;
use afkconvoy_core::team::PRECEDENTS;
use afkconvoy_core::tick::Outcome;
use anyhow::{Result, anyhow};

use crate::claims::{Me, drop_run, take_run};
use crate::dispatch::Agent;
use crate::fleet::Fleet;
use crate::orca::{Source, Worktree};
use crate::stage::Flows;

/// What the remote and GitHub say of the proposals of a bound repo: the
/// claims on those still going, and — where the team names who proposes —
/// the rulings to propose and the proposals open on the team's repo. That
/// repo is asked only while a ruling waits to be proposed.
pub fn seen(fleet: &Fleet, read: &Flows) -> Result<Seen> {
    let Some(bound) = fleet.config.bound.as_ref() else { return Ok(Seen::default()) };
    let mut seen = Seen::default();
    for run in &fleet.scan.runs {
        if let Some(ruling) = Ruling::claimed(&run.key) {
            seen.claims.push((ruling, run.clone()));
        }
    }
    // What is wrong with a team is `check`'s to say: here it proposes nothing.
    let Some(precedents) = read.kept().ok().and_then(|kept| kept.team.precedents.as_ref()) else {
        return Ok(seen);
    };
    seen.max_open = Some(precedents.max_open);
    seen.unproposed = read.unproposed();
    if !seen.unproposed.is_empty() {
        for (_, branch) in fleet.gh.open_prs_of(&bound.team.repo)? {
            if is_proposal(&branch) {
                seen.open.push(branch);
            }
        }
    }
    Ok(seen)
}

/// Starts the proposal `row`, due as the rebuild read it: claims it, says on
/// the issue that the ruling is proposed, has orca bring a checkout of the
/// base branch as it stands, and starts a worker there in the role the team
/// names. `Lost` when a peer claimed it, or proposed the ruling since the
/// rebuild.
///
/// The issue is told before the worker is started, so a ruling is proposed at
/// most once: a start that fails after that is an error with the claim still
/// held — a later tick ends it — and the ruling is not proposed again.
pub fn propose(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    read: &Flows,
    row: &Row,
) -> Result<Outcome> {
    let ruling = row.ruling;
    let kept = read.kept()?;
    let bound = fleet.config.bound.as_ref().ok_or_else(|| anyhow!("the repo is not bound"))?;
    // The proposal of a precedent runs on the default: a team names no harness for it.
    let (team, harness) = (&bound.team, bound.harness);
    let launch = agent.command(harness)?;
    let work = &kept
        .team
        .precedents
        .as_ref()
        .ok_or_else(|| anyhow!("the team names nobody who proposes precedents"))?
        .work;
    let role = kept.team.role_of(work);
    let role_copy = kept.role(role, harness)?;
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, &ruling.claim_key())? else {
        return Ok(Outcome::Lost);
    };
    // A peer may have proposed it, and its worker stopped, since the rebuild read the issue.
    let comments = fleet.gh.comments(ruling.issue)?;
    let said = comments.iter().find(|comment| comment.id == ruling.comment);
    let record = said.and_then(|said| stage_records([said.body.as_str()]).pop());
    let already = proposed(comments.iter().map(|c| c.body.as_str())).contains(&ruling.comment);
    let (Some(said), Some((_, record)), false) = (said, record, already) else {
        drop_run(&fleet.repo, &mut fleet.scan, &claim)?;
        return Ok(Outcome::Lost);
    };
    let file = team.join(PRECEDENTS);
    let said_proposed = proposed_comment(ruling, &role.name, team.repo.as_str(), file.path());
    fleet.gh.comment(ruling.issue, &said_proposed)?;

    let base = fleet.base_tip()?;
    let source = Source::of(&fleet.repo)?;
    let created_as = ruling.checkout_name();
    let checkout = match Worktree::named(&source, &created_as)? {
        Some(kept) => {
            kept.clear(&base)?;
            kept
        }
        None => Worktree::cut(&source, &created_as, &base, None)?,
    };
    let path = checkout.path.display().to_string();
    let team_rules = kept.rules();
    let fields = ProposalFields {
        number: ruling.issue,
        link: &said.url,
        ruling: &record.text,
        role: &role.name,
        skill: &work.skill,
        team: team.repo.as_str(),
        file: file.path(),
        branch: &ruling.branch(),
        precedents: &kept.copy.dir.join(PRECEDENTS).display().to_string(),
        team_rules: team_rules.as_deref(),
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        // The worker works in a clone of its own: the line is run from the checkout.
        over_command: &crate::own_command_in(&path).fixed(" over --run ").word(&claim.key).own(),
    };
    let brief = checkout.write_brief(&proposal_prompt(&fields).map_err(|e| anyhow!(e))?)?;
    let told = crate::role::ToldRules::read(harness, &checkout.path, team_rules.as_deref())?;
    let start = Start {
        role_dir: &role_copy.dir.display().to_string(),
        worktree: &path,
        team_dir: &kept.copy.dir.display().to_string(),
        model: None,
        guard: &crate::own_command().fixed(" guard").own(),
        kept: &crate::kept::keep(harness)?.display().to_string(),
        told: told.told(),
        facts: Facts {
            on: On::Proposal { issue: ruling.issue },
            role: &role.name,
            skill: &work.skill,
            // In a clone of the team's repo, of its own: not in the checkout.
            writes: true,
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
    Ok(Outcome::Done)
}

/// Ends the proposal `row` once its run is over — declared over, its worker
/// gone or stopped, or its fleet presumed dead: closes, with the reason, a PR opened
/// from its branch on the team's repo that is no proposal of its ruling,
/// removes the worker's checkout, and unclaims it. A PR that is one is left
/// for a human to merge. `Nothing` while the worker is still at it.
pub fn end(fleet: &mut Fleet, me: &Me, row: &Row) -> Result<Outcome> {
    let Some(claim) = &row.claim else { return Ok(Outcome::Nothing) };
    let ruling = row.ruling;
    let checkout = match row.standing {
        Standing::Running => Worktree::named(&Source::of(&fleet.repo)?, &ruling.checkout_name())?,
        Standing::StaleRun => None,
        _ => return Ok(Outcome::Nothing),
    };
    if let Some(checkout) = &checkout
        && !run_over(claim, checkout.state(me.now)?, me.now)
    {
        return Ok(Outcome::Nothing);
    }
    let team = &fleet.config.bound.as_ref().ok_or_else(|| anyhow!("the repo is not bound"))?.team;
    let comments = fleet.gh.comments(ruling.issue)?;
    let link = comments.iter().find(|c| c.id == ruling.comment).map_or("", |c| c.url.as_str());
    let file = team.join(PRECEDENTS);
    let team = &team.repo;
    for (number, branch) in fleet.gh.open_prs_of(team)? {
        if branch != ruling.branch() {
            continue;
        }
        if let Some(reason) = refused(&fleet.gh.pr_diff_of(team, number)?, link, file.path()) {
            fleet.gh.close_pr_of(team, number, &closing_comment(&reason))?;
        }
    }
    if let Some(checkout) = checkout {
        checkout.remove()?;
    }
    drop_run(&fleet.repo, &mut fleet.scan, claim)?;
    Ok(Outcome::Done)
}
