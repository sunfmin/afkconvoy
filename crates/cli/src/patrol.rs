//! Patrols on a bound repo (docs/adr/0008): what a rebuild reads of each one
//! the repo names, a human's ask for a run now, the start of a run, and its
//! end. When a patrol runs and
//! what makes a run a failed one is the core's (`afkconvoy_core::patrol`).

use afkconvoy_core::intake::opened_by;
use afkconvoy_core::lock;
use afkconvoy_core::patrol::{
    Ran, Row, Seen, Standing, asked_ref, broke, checkout_name, claim_key, failure_of,
    failure_report, failure_title, held_until, is_due, read_asked, read_run, run_over, run_ref,
    write_run,
};
use afkconvoy_core::prompt::{PatrolFields, patrol_prompt};
use afkconvoy_core::role::{Facts, On, Start};
use afkconvoy_core::selection::Selection;
use afkconvoy_core::tick::Outcome;
use afkconvoy_core::tracker::Tracker;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::{Ask, Me, drop_ask, drop_run, push_ask, take_run};
use crate::copies::Copies;
use crate::dispatch::{Agent, Continuing};
use crate::fleet::Fleet;
use crate::gh::OpenIssues;
use crate::git::Repo;
use crate::orca::{Source, Worktree};
use crate::remote::{Fresh, Push};
use crate::stage::Flows;
use crate::team::{Kept, keep, unknown_patrols};
use crate::topic::See;

/// The moment a patrol's landings should be counted back from, when a patrol
/// this repo names bounds how many issues it lands in a day. `None` when it
/// names no such patrol, or the team cannot be read: the open read then
/// leaves the landings unasked, and [`seen`] asks if the team loads.
pub fn landings_since(repo: &Repo, now: u64) -> Option<u64> {
    let config = crate::config::load(&crate::config::file_of(repo)).ok()?;
    let bound = config.bound.as_ref().filter(|bound| !bound.patrols.is_empty())?;
    let team = crate::role::current(repo, bound, &config.labels())?;
    let patrols = team.patrols_named(&bound.patrols).ok()?;
    patrols.iter().any(|patrol| patrol.max_landed_a_day.is_some()).then_some(now)
}

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
        .map_err(|unknown| unknown_patrols(&bound.team.to_string(), &kept.team, &unknown))?;
    // An issue handed to a human left its workflow, and is open all the same.
    let escalate = &kept.team.escalate_label;
    let mut opened = read.opened();
    let issues = &open.rows;
    for issue in issues {
        if kept.team.flows(&issue.labels).is_empty() && issue.labels.contains(escalate) {
            let comments = open.comments(issue.number);
            opened.extend(opened_by(comments.iter().map(|comment| comment.body.as_str())));
        }
    }
    let landed = if named.iter().any(|patrol| patrol.max_landed_a_day.is_some()) {
        match &fleet.patrol_landings {
            Some(landed) => landed.clone(),
            None => fleet.gh.patrol_landings(fleet.now)?,
        }
    } else {
        vec![]
    };
    let seen = |patrol: &&afkconvoy_core::team::Patrol| Seen {
        name: patrol.name.clone(),
        every: patrol.every,
        max_runs_a_day: patrol.max_runs_a_day,
        max_landed_a_day: patrol.max_landed_a_day,
        landed: landed.iter().filter(|(name, _)| *name == patrol.name).map(|(_, at)| *at).collect(),
        max_open: patrol.max_open,
        last_run: fleet.scan.last_runs.get(&patrol.name).cloned(),
        claim: fleet.scan.runs.iter().find(|run| run.key == claim_key(&patrol.name)).cloned(),
        open: opened.iter().filter(|by| **by == patrol.name).count() as u64,
        failure: failure_of(issues, &patrol.name, escalate),
        asked: fleet.scan.asks.get(&patrol.name).map(|ask| ask.ts),
    };
    Ok(named.iter().map(seen).collect())
}

/// `afkconvoy patrol-now <name>`: a human's ask for a run of the patrol `name`
/// of the fleet of the checkout `repo` now, on `host` at `now`. It is recorded on the remote for the next tick of
/// whichever host claims the run, past `every` and `max_runs_a_day`; the
/// fleet is woken as the command ends (`crate::acting`). Refused, with
/// nothing recorded, when what holds an asked run holds it: a fleet lock, the
/// patrol's `max_open` or `max_landed_a_day`, a failed run nobody answered, a run still going.
///
///   {"patrol", "asked", "ref"}
pub fn ask(repo: &Repo, name: &str, host: &str, now: u64) -> Result<Value> {
    let (mut fleet, open) = crate::fleet::Fleet::read(repo, now)?;
    let read = Flows::read(&fleet, &open.issues)?;
    let patrols = seen(&fleet, &read, &open.issues)?;
    let Some(patrol) = patrols.iter().find(|patrol| patrol.name == name) else {
        let names: Vec<&str> = patrols.iter().map(|patrol| patrol.name.as_str()).collect();
        return Err(See::Patrols.refusal(format!(
            "this repo runs no patrol {name:?}: it runs {}. A repo runs the patrols its config \
             names, under `patrols`",
            if names.is_empty() { "none".to_string() } else { names.join(", ") }
        )));
    };
    let locks = lock::standing(&open.issues.rows, &fleet.config);
    let held = afkconvoy_core::patrol::ask(patrol, &fleet.scan.beats(), now, !locks.is_empty());
    if let Err(held) = held {
        return Err(See::Patrols.refusal(refused(patrol, held, &locks, &fleet.gh.tracker)));
    }
    let refname = push_ask(&fleet.repo, &mut fleet.scan, name, host, now)?;
    Ok(json!({"patrol": name, "asked": now, "ref": refname}))
}

/// Why an ask for a run of `patrol` is refused, held as `held`, and what
/// clears it; `locks` are the fleet locks standing, issues of `tracker`.
fn refused(patrol: &Seen, held: Standing, locks: &[u64], tracker: &Tracker) -> String {
    let name = &patrol.name;
    match held {
        Standing::Locked => {
            let numbers: Vec<String> = locks.iter().map(|&lock| tracker.issue(lock)).collect();
            format!(
                "a fleet lock stands ({}): no patrol runs while one does, asked for or not. It \
                 ends when the last lock issue closes",
                numbers.join(", ")
            )
        }
        Standing::Full => format!(
            "the open issues the patrol `{name}` opened have reached its `max_open` ({} of {}): \
             it runs once one of them closes, or once `max_open` in patrols/{name}.md is raised",
            patrol.open, patrol.max_open
        ),
        Standing::Landed => format!(
            "the issues the patrol `{name}` opened reached its `max_landed_a_day` ({}): \
             ask again once enough completions are 24 hours old; `afkconvoy plan` says until when, \
             or raise `max_landed_a_day` in patrols/{name}.md",
            patrol.max_landed_a_day.unwrap_or_default()
        ),
        Standing::Failed => match patrol.failure {
            Some(failure) => format!(
                "the last run of the patrol `{name}` failed, and {} reports it: it runs once you \
                 have looked and closed it ({})",
                tracker.issue(failure),
                tracker.gh_issue("close", failure).own()
            ),
            None => format!("the last run of the patrol `{name}` failed"),
        },
        Standing::Running | Standing::PeerRunning => format!(
            "a run of the patrol `{name}` is still going: ask again once it has ended — once its \
             worker declares it over with `afkconvoy over`, or is gone, or has stopped for a \
             grace period; `afkconvoy plan` says when it has"
        ),
        Standing::Due | Standing::NotDue | Standing::Spent | Standing::StaleRun => {
            format!("the patrol `{name}` cannot be asked for now")
        }
    }
}

/// Runs the patrol `row`, due as the rebuild read it: claims the run, has
/// orca cut the patrol a checkout of the base branch as it stands, starts a
/// worker in the patrol's role there, and records the run — one a human
/// asked for as such, the ask then answered and deleted. A checkout an
/// earlier run left — one that failed, or whose end was cut short — is
/// removed first: a run starts in a new one. `Lost` when a peer
/// claimed it, or ran it since the rebuild — what was asked for included.
///
/// A step that fails is an error with the claim still held and no run
/// recorded: a later tick finds the run over and ends it, and the patrol is
/// due again.
pub fn run(fleet: &mut Fleet, me: &Me, agent: &Agent, read: &Flows, row: &Row) -> Result<Outcome> {
    let name = row.name.as_str();
    let kept = read.kept()?;
    let none = || See::Patrols.refusal(format!("the team has no patrol {name:?}"));
    let patrol = kept.team.patrol(name).ok_or_else(none)?;
    let role = kept.team.role_of(&patrol.work);
    // A patrol has no issue and no model label: the user default of the
    // harness its role's range selects.
    let chosen = fleet.select_in(Some(role), &[], None)?;
    let harness = chosen.harness();
    kept.role(role, harness)?;
    // Before the claim: a patrol whose harness cannot be started here runs nothing.
    let launch = agent.command(harness)?;
    let Some(claim) = take_run(&fleet.repo, &mut fleet.scan, me, &claim_key(name))? else {
        return Ok(Outcome::Lost);
    };
    // A peer's run may have started and ended since the rebuild read the remote.
    let recorded = run_ref(fleet.repo.layout(), name);
    let last = fleet.remote.records(&[&recorded], Fresh::Process)?.pop();
    let last = last.and_then(|record| read_run(&record.subject));
    // …and answered the ask it is due by: one standing still passes the schedule.
    let ask = fleet.remote.records(&[&asked_ref(fleet.repo.layout(), name)], Fresh::Process)?.pop();
    let ask =
        ask.and_then(|record| Some(Ask { ts: read_asked(&record.subject)?, sha: record.sha }));
    if ask.is_none()
        && (!is_due(row.every, last.as_ref().map(|ran| ran.ts), me.now)
            || held_until(row.max_runs_a_day, last.as_ref(), me.now).is_some())
    {
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

    let path = checkout.path.display().to_string();
    // A worker starts in its role's copy: its commands are run from the checkout.
    let own = || crate::own_command_in(&path);
    let workflows: Vec<&str> =
        kept.team.workflows.iter().map(|workflow| workflow.name.as_str()).collect();
    let team_rules = kept.rules();
    let fields = PatrolFields {
        patrol: name,
        role: &role.name,
        skill: &patrol.work.skill,
        worktree: &path,
        base_branch: &fleet.config.base_branch,
        team_rules: team_rules.as_deref(),
        workflows: &workflows,
        room: row.max_open.saturating_sub(row.open),
        intake_command: &own().fixed(" intake --issue <number> --workflow <name>").own(),
        lock_command: &own().fixed(" lock --issue <number>").own(),
        over_command: &own().fixed(" over --run ").word(&claim.key).own(),
        tracker: &fleet.gh.tracker,
    };
    let brief = checkout.write_brief(&patrol_prompt(&fields).map_err(|e| anyhow!(e))?)?;
    let line = in_role(fleet, kept, name, &chosen, launch.as_str(), &checkout)?;
    let terminal = agent.start_worker(
        &checkout,
        &fleet.gh.tracker,
        (&chosen).into(),
        &line,
        true,
        None,
        me.now,
    )?;
    checkout.deliver(&terminal, &brief)?;

    // The record carries the runs of the day before it: they are counted from it alone.
    let ran = Ran::after(last.as_ref(), me.now, ask.is_some());
    let sha = fleet.repo.record_commit(&write_run(&ran))?;
    fleet.remote.push(&Push::set(&sha, &recorded).force())?;
    fleet.scan.last_runs.insert(name.to_string(), ran);
    if let Some(ask) = &ask {
        drop_ask(&fleet.repo, &mut fleet.scan, name, ask)?;
    }
    Ok(Outcome::Done)
}

/// The line the terminal in `checkout` runs to start the worker of a run of
/// the patrol `name` on what `cast` says, with `launch`, the worker launch
/// command of its harness. What this host keeps for it is put in place first.
fn in_role(
    fleet: &Fleet,
    kept: &Kept,
    name: &str,
    chosen: &Selection,
    launch: &str,
    checkout: &Worktree,
) -> Result<String> {
    let none = || See::Patrols.refusal(format!("the team has no patrol {name:?}"));
    let patrol = kept.team.patrol(name).ok_or_else(none)?;
    let role = kept.team.role_of(&patrol.work);
    let harness = chosen.harness();
    let role_copy = kept.role(role, harness)?;
    let path = checkout.path.display().to_string();
    let team_rules = kept.rules();
    let told = crate::role::ToldRules::read(
        harness,
        &checkout.path,
        team_rules.as_deref(),
        &role_copy.dir,
    )?;
    let host_user = role.host_user;
    let (kept_dir, shadowed) = crate::kept::for_worker(
        harness,
        host_user,
        &role_copy.dir,
        &checkout.path,
        &told.told(),
        &crate::own_command().fixed(" guard").own(),
    )?;
    let start = Start {
        role_dir: &role_copy.dir.display().to_string(),
        worktree: &path,
        team_dir: &kept.copy.dir.display().to_string(),
        model: chosen.model.as_deref(),
        guard: &crate::own_command().fixed(" guard").own(),
        kept: &kept_dir,
        host_user,
        dsh_profile: role.dsh_profile,
        host_rules: &crate::kept::host_rules(harness),
        shadowed: &shadowed,
        told: told.told(),
        facts: Facts {
            on: On::Patrol(name),
            role: &role.name,
            skill: &patrol.work.skill,
            writes: false,
            consult: false,
            base_branch: &fleet.config.base_branch,
            target_branch: None,
            escalate_label: &kept.team.escalate_label,
            lock_label: &fleet.config.lock_label,
            role_commit: &role_copy.commit,
            team_commit: &kept.copy.commit,
        },
    };
    Ok(harness.in_role(launch, &start))
}

/// Ends the worker of my run of the patrol `name` and starts one in its
/// checkout of the harness it runs now — this host's chosen one, else the
/// patrol's — briefed as it was, and told what of the run is there
/// (`continuing`). The run, its claim and its record are untouched: no new
/// run starts, and it ends as any does.
pub(crate) fn switch(
    fleet: &Fleet,
    me: &Me,
    agent: &Agent,
    name: &str,
    said: impl FnOnce(&Worktree) -> Continuing,
) -> Result<Value> {
    let key = claim_key(name);
    let mine = fleet.scan.runs.iter().find(|run| {
        run.key == key
            && run.owner.as_ref().is_some_and(|owner| owner.instance == me.instance.as_str())
    });
    if mine.is_none() {
        bail!("{} holds no run of the patrol {name:?}; nothing was switched", me.instance);
    }
    let source = Source::of(&fleet.repo)?;
    let Some(checkout) = Worktree::named(&source, &checkout_name(name))? else {
        bail!("the run of the patrol {name:?} has no checkout on this host; nothing was switched");
    };
    let bound = fleet.config.bound.as_ref().ok_or_else(|| anyhow!("the repo is not bound"))?;
    let kept = keep(&fleet.gh, &Copies::of_host()?, &bound.team, &fleet.config.labels())?;
    let none = || See::Patrols.refusal(format!("the team has no patrol {name:?}"));
    let patrol = kept.team.patrol(name).ok_or_else(none)?;
    let role = kept.team.role_of(&patrol.work);
    let chosen = fleet.select_in(Some(role), &[], None)?;
    let harness = chosen.harness();
    let skipped = fleet.skipped_before(harness, Some(role), None);
    let launch = agent.command(harness)?;
    let line = in_role(fleet, &kept, name, &chosen, launch.as_str(), &checkout)?;
    let continuing = said(&checkout);
    checkout.stop_for_continuation()?;
    crate::dispatch::continue_brief(fleet, &checkout, None, None, &continuing)?;
    let terminal = agent.start_worker(
        &checkout,
        &fleet.gh.tracker,
        (&chosen).into(),
        &line,
        true,
        None,
        me.now,
    )?;
    checkout.deliver(&terminal, &checkout.brief()?)?;
    Ok(json!({"patrol": name, "from": continuing.from, "to": harness,
        "worktree": checkout.path, "terminal": terminal, "skipped": skipped}))
}

/// Ends the run of the patrol `row` once it is over: a peer's presumed dead
/// is only unclaimed; mine, declared over or its worker gone or stopped, is
/// checked for what a patrol never leaves — a commit not on the base branch,
/// working-tree changes, a PR — and unclaimed. A run that left one has failed:
/// an issue is opened for a human, and no other is touched.
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
        if !claim.over && crate::health::holding(&checkout) {
            return Ok(Outcome::Nothing);
        }
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
        let repo = Repo::new(checkout.path.clone());
        let tip = checkout.tip()?;
        let base = fleet.base_tip()?;
        let left = (!repo.within(&tip, &base)?).then_some(tip.as_str());
        let dirty = !repo.git_raw(&["status", "--porcelain"])?.trim().is_empty();
        let reason = broke(left, dirty, &prs);
        let started =
            row.last_run.or_else(|| claim.owner.as_ref().map(|owner| owner.ts)).unwrap_or(me.now);
        crate::patrol_usage::record(
            &fleet.gh.repository()?,
            &checkout,
            &row.name,
            started,
            me.now,
        )?;
        // One report stands for a patrol until a human closes it.
        if let Some(reason) = reason.as_ref().filter(|_| row.failure.is_none()) {
            let path = checkout.path.display().to_string();
            fleet.gh.open_report(
                &failure_title(&row.name),
                &failure_report(&row.name, reason, &me.host, &path),
                reason,
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

/// What `afkconvoy patrol-now --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When you want a patrol's run now rather than when its interval has passed — a review right after several landings, say — without editing the team's patrol file, which changes the schedule for every repo that runs it:

```
afkconvoy patrol-now sweep
```

The next tick of whichever host claims the run starts it, and this command wakes the `run` of this host so that tick starts at once.

## Who runs it

The operator, or any human, in a checkout of a bound repo whose config names the patrol under `patrols`. No worker is told to: a patrol's worker ends its run with `afkconvoy over`.

## What it reads and writes

It reads the team, the patrol's file, the remote's records of the patrol and what is open on GitHub — what `afkconvoy plan` reads of a patrol. Then it pushes one record to the remote, a ref of the fleet's, `asked/<name>`, saying when it asked and from which host, and wakes the `run` of this host (`afkconvoy wake --help`). It changes no config file, starts no worker and claims nothing itself.

A run asked for this way:

- passes `every` and `max_runs_a_day`: it runs though the interval has not passed since the last run, and though the day's runs have reached the bound;
- is still held by what holds a due run: a fleet lock, the patrol's open issues having reached `max_open`, its completed issues having reached `max_landed_a_day`, a failed run whose report is still open, and a run of it still going. Each refuses the ask, and nothing is recorded;
- is claimed on the remote as a due run is: of several hosts asked at once, one runs it and the others' ticks say `lost`;
- is recorded as the patrol's run, with `asked=human` in its record: its start is the last run `every` counts from, and one of the day's starts `max_runs_a_day` counts. The ask is deleted as the run is recorded.

Until a tick starts the run, `afkconvoy plan` shows `asked` (normally `due`; new completions may hold it again): when the ask was made. The panel says a human asked for it.

## What it answers

{"patrol", "asked", "ref", "woke"} — the ask is on the remote at `ref`, made at `asked` (epoch seconds); `woke` is whether a `run` on this host was told.

Errors, as {"error", "see"} with a non-zero exit, nothing recorded:

- a fleet lock stands: no patrol runs until the last lock issue closes;
- the open issues the patrol opened have reached its `max_open`: it runs once one closes, or `max_open` is raised;
- its last run failed and the issue that reports it is open: close it once you have looked;
- a run of it is still going: ask again once it has ended;
- the repo runs no patrol of that name: the config's `patrols` lists those it runs.

Also: "is not in a git checkout", or git's own words for a checkout it refuses; the config, the team, the remote or GitHub cannot be read; the remote does not take the push.

## What trips you up

- It runs nothing itself: a `run` does, at its next tick. With no `run` on any host (`woke: false` here, and none elsewhere), the ask waits on the remote for the first tick of any of them.
- An ask made while nothing held the patrol stands until a run answers it, whatever comes to hold the patrol meanwhile: a lock raised since holds it, and it runs once the lock has closed. `afkconvoy plan` shows it held, with `asked`.
- Asking twice before the run asks once: one run answers both.
- The schedule goes on from the run asked for: the next due run is `every` after its start, not after the last scheduled one."#;
