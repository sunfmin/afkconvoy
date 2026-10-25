//! Host kit priority and continuation of existing workers. No repository-local choice is read.

use afkconvoy_core::claims::Claim;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::joining::Switch;
use afkconvoy_core::patrol::claimed;
use afkconvoy_core::plan::closing_pr;
use anyhow::{Context, Result, anyhow, bail};
use serde_json::{Value, json};

use crate::claims::Me;
use crate::dispatch::{Agent, Continuing, StartFrom, dispatch_continuing};
use crate::fleet::Fleet;
use crate::judge::mine;
use crate::orca::{Source, Worktree};

use crate::watch::{join_record, record_join};

/// Promotes a kit entry, then continues named workers, independently of the
/// directory the command was run in. Failures never stop the remaining checkouts.
pub fn on_host(
    repo: Option<crate::git::Repo>,
    harness: Harness,
    which: Which,
    instance: Option<afkconvoy_core::claims::InstanceId>,
    host: &str,
    now: u64,
    agent: impl Fn(&crate::git::Repo) -> Result<Agent>,
) -> Result<Value> {
    let here = repo
        .clone()
        .unwrap_or_else(|| crate::git::Repo::new(std::env::current_dir().unwrap_or_default()));
    let launches = crate::launch::Launches::new(&here, true)?;
    crate::role::startable(&launches, harness, PRIORITY_BY)?;
    let mut result = crate::kit::prefer(harness)?;
    let mut switched = vec![];
    let mut failed = vec![];
    let mut paths: Vec<_> = match &which {
        Which::Nothing => vec![],
        Which::All => crate::checkout_list::List::of_host()?
            .read()?
            .checkouts
            .into_iter()
            .map(|e| e.checkout)
            .collect(),
        _ => vec![repo.as_ref().context("a targeted worker requires a checkout")?.dir.clone()],
    };
    if matches!(which, Which::All)
        && let Some(repo) = &repo
        && !paths.contains(&repo.dir)
    {
        paths.push(repo.dir.clone());
    }
    for path in paths {
        let continued = (|| -> Result<Value> {
            let repo = crate::git::Repo::new(path.clone());
            let mut fleet = Fleet::open(&repo, now)?;
            let agent = agent(&repo)?;
            let mut owners: Vec<_> =
                fleet.scan.claims.iter().filter_map(|c| c.owner.clone()).collect();
            owners.extend(fleet.scan.runs.iter().filter_map(|r| r.owner.clone()));
            owners.retain(|owner| {
                owner.host == host
                    && (matches!(which, Which::All)
                        || instance.as_ref().is_none_or(|id| id.as_str() == owner.instance))
            });
            owners.sort_by(|a, b| a.instance.cmp(&b.instance));
            owners.dedup_by(|a, b| a.instance == b.instance);
            let mut moved = vec![];
            let mut errors = vec![];
            for owner in owners {
                let me = Me {
                    instance: afkconvoy_core::claims::InstanceId::parse(&owner.instance)
                        .map_err(|e| anyhow!(e))?,
                    host: host.into(),
                    now,
                };
                let target = match &which {
                    Which::Issue(n) => Which::Issue(*n),
                    Which::Patrol(name) => Which::Patrol(name.clone()),
                    _ => Which::All,
                };
                match switch(&mut fleet, &me, &agent, target) {
                    Ok(value) if value["switched"].is_array() => {
                        moved.extend(value["switched"].as_array().cloned().unwrap_or_default());
                        errors.extend(value["failed"].as_array().cloned().unwrap_or_default());
                    }
                    Ok(value) => moved.push(value),
                    Err(e) => {
                        errors.push(json!({"instance": owner.instance, "error": format!("{e:#}")}))
                    }
                }
            }
            if matches!(which, Which::All) {
                let source = Source::of(&repo)?;
                for (name, worktree, caster) in [
                    ("landing train", Worktree::of_train(&source)?, false),
                    (
                        "caster",
                        Worktree::named(&source, afkconvoy_core::casting::CASTER_WORKTREE)?,
                        true,
                    ),
                ] {
                    if let Some(worktree) =
                        worktree.filter(|w| w.terminal().ok().flatten().is_some())
                    {
                        match auxiliary(&fleet, &agent, &worktree, caster) {
                            Ok(value) => moved.push(value),
                            Err(e) => {
                                errors.push(json!({"worker": name, "error": format!("{e:#}")}))
                            }
                        }
                    }
                }
            }
            if moved.is_empty() && errors.is_empty() && !matches!(which, Which::All) {
                bail!("no claim on the targeted worker is held on this host");
            }
            Ok(json!({"switched": moved, "failed": errors}))
        })();
        match continued {
            Ok(value) => {
                for mut one in value["switched"].as_array().cloned().unwrap_or_default() {
                    one["checkout"] = json!(path);
                    switched.push(one);
                }
                for mut one in value["failed"].as_array().cloned().unwrap_or_default() {
                    one["checkout"] = json!(path);
                    failed.push(one);
                }
            }
            Err(e) => failed.push(json!({"checkout": path, "error": format!("{e:#}")})),
        }
    }
    result["switched"] = json!(switched);
    result["failed"] = json!(failed);
    Ok(result)
}

/// What calls for the kit promotion.
pub const PRIORITY_BY: &str = "this host's kit priority";
const SAID_LINES: usize = 80;

/// The workers a switch moves.
pub enum Which {
    /// None: only the choice of harness for the workers started from now on.
    Nothing,
    /// The worker of my claim on one issue.
    Issue(u64),
    /// The worker of my run of one patrol.
    Patrol(String),
    /// Every worker of a claim of mine — on an issue, on a patrol's run.
    All,
}

/// Continue the named workers using current host kit order. Host promotion
/// has already completed. Each successor is validated before its old writer
/// is stopped, and ownership is checked before any continuation.
pub fn switch(fleet: &mut Fleet, me: &Me, agent: &Agent, which: Which) -> Result<Value> {
    // What a worker started before any switch kept its harness ran on.
    let before = fleet.first().map_or(Harness::DEFAULT, |first| first.harness);
    match &which {
        Which::Issue(issue) => {
            if mine(fleet, me, *issue)?.is_none() {
                bail!("{} holds no claim on issue #{issue}; nothing was switched", me.instance);
            }
        }
        Which::Patrol(name) => {
            if !holds_run(fleet, me, name) {
                bail!("{} holds no run of the patrol {name:?}; nothing was switched", me.instance);
            }
        }
        Which::Nothing | Which::All => {}
    }
    *fleet = Fleet::open(&fleet.repo, fleet.now)?;
    let one = |moved: Value| moved;
    Ok(match which {
        Which::Nothing => json!({"switched": []}),
        Which::Issue(issue) => one(switch_issue(fleet, me, agent, issue, before)?),
        Which::Patrol(name) => {
            let said = |checkout: &Worktree| continuing(Some(checkout), before);
            one(crate::patrol::switch(fleet, me, agent, &name, said)?)
        }
        Which::All => {
            let (mut switched, mut failed) = (vec![], vec![]);
            let issues: Vec<u64> = mine_all(fleet, me).iter().map(|claim| claim.number).collect();
            for issue in issues {
                match switch_issue(fleet, me, agent, issue, before) {
                    Ok(moved) => switched.push(moved),
                    Err(e) => failed.push(json!({"issue": issue, "error": format!("{e:#}")})),
                }
            }
            for name in my_patrols(fleet, me) {
                let said = |checkout: &Worktree| continuing(Some(checkout), before);
                match crate::patrol::switch(fleet, me, agent, &name, said) {
                    Ok(moved) => switched.push(moved),
                    Err(e) => failed.push(json!({"patrol": name, "error": format!("{e:#}")})),
                }
            }
            json!({"switched": switched, "failed": failed})
        }
    })
}

/// My claims on issues.
fn mine_all(fleet: &Fleet, me: &Me) -> Vec<Claim> {
    let claims = fleet.scan.claims.iter().filter(|claim| claim.is_held_by(&me.instance));
    claims.cloned().collect()
}

/// The patrols I hold a run of.
fn my_patrols(fleet: &Fleet, me: &Me) -> Vec<String> {
    let runs = fleet.scan.runs.iter().filter(|run| {
        run.owner.as_ref().is_some_and(|owner| owner.instance == me.instance.as_str())
    });
    runs.filter_map(|run| claimed(&run.key).map(str::to_string)).collect()
}

/// Whether I hold a run of the patrol `name`.
fn holds_run(fleet: &Fleet, me: &Me, name: &str) -> bool {
    my_patrols(fleet, me).iter().any(|held| held == name)
}

/// What the successor of the worker in `worktree` is told of it: the harness
/// it ran on — `before` where its start kept none — and the last lines its
/// terminal shows, read before that terminal is closed. A terminal that
/// cannot be read said nothing.
pub(crate) fn continuing(worktree: Option<&Worktree>, before: Harness) -> Continuing {
    let Some(worktree) = worktree else { return Continuing { from: before, said: vec![] } };
    let from = worktree.harness().ok().flatten().unwrap_or(before);
    let terminal = worktree.terminal().ok().flatten();
    let screen = terminal.and_then(|terminal| worktree.screen(&terminal, SAID_LINES).ok());
    let mut said = screen.unwrap_or_default();
    while said.last().is_some_and(|line| line.trim().is_empty()) {
        said.pop();
    }
    let blank = said.iter().take_while(|line| line.trim().is_empty()).count();
    Continuing { from, said: said.split_off(blank) }
}

/// Continues a worker with no issue in the same checkout and with its brief
/// intact. Train and caster records are untouched.
pub(crate) fn auxiliary(
    fleet: &Fleet,
    agent: &Agent,
    worktree: &Worktree,
    caster: bool,
) -> Result<Value> {
    let cast = fleet.first()?;
    crate::role::startable(&agent.launches, cast.harness, "worker continuation")?;
    let continued = continuing(Some(worktree), cast.harness);
    worktree.stop_for_continuation()?;
    if worktree.brief()?.exists() {
        crate::dispatch::continue_brief(fleet, worktree, None, &continued)?;
    }
    let command = if caster {
        crate::own_command().fixed(" cast; exit").own()
    } else {
        cast.command(agent.command(cast.harness)?.as_str())
    };
    let terminal = agent.start(worktree, cast.harness, &command, fleet.now)?;
    if !caster {
        worktree.await_ready(&terminal, agent.ready_timeout)?;
        worktree.deliver(&terminal, &worktree.brief()?)?;
    }
    Ok(
        json!({"worktree": worktree.path, "from": continued.from, "to": cast.harness, "terminal": terminal}),
    )
}

/// Moves the worker of my claim on `issue` onto what it runs now — this host's
/// chosen harness, else what its range and casting say: a worker
/// started on it as a continuation of what survives (`dispatch`), which closes
/// whatever terminal the worktree had, whatever its worker was doing. The
/// switch is recorded on the issue's open PR, beside its join's record.
pub(crate) fn switch_issue(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    issue: u64,
    before: Harness,
) -> Result<Value> {
    if mine(fleet, me, issue)?.is_none() {
        bail!("{} holds no claim on issue #{issue}", me.instance);
    }
    let worktree = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)?;
    let continued = continuing(worktree.as_ref(), before);
    let from = continued.from;
    crate::dispatch::continuation_harness(fleet, agent, issue)?;
    if let Some(worktree) = &worktree {
        worktree.stop_for_continuation()?;
    }
    let started = dispatch_continuing(fleet, me, agent, issue, StartFrom::Auto, Some(&continued))?;
    if started["started"] != true {
        bail!("issue #{issue}: no worker was started: {started}");
    }
    let to = started["harness"].as_str().and_then(Harness::read).unwrap_or(from);
    let pr = closing_pr(&fleet.gh.open_prs()?, issue).map(|pr| pr.number);
    if let Some(pr) = pr {
        let (mut record, comments) = join_record(fleet, pr)?;
        record.switched.push(Switch {
            at: fleet.now,
            from: from.key().to_string(),
            to: to.key().to_string(),
            by: me.instance.as_str().to_string(),
        });
        record_join(fleet, pr, &record, &comments)?;
    }
    Ok(json!({"issue": issue, "from": from, "to": to, "worktree": started["worktree"],
        "branch": started["branch"], "terminal": started["terminal"], "pr": pr,
        "stage": started.get("stage")}))
}
