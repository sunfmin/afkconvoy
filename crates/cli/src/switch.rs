//! Host kit priority and continuation of existing workers. No repository-local
//! choice is read. A checkout that names a harness on the checkout list keeps
//! that name: switch reorders the host kit and does not continue that
//! checkout's workers onto the kit's new first.

use afkconvoy_core::claims::Claim;
use afkconvoy_core::harness::Harness;
use afkconvoy_core::landing_worker::Switch;
use afkconvoy_core::patrol::claimed;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::worker::WorkerState;
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
    target: crate::kit::Named,
    which: Which,
    instance: Option<afkconvoy_core::claims::InstanceId>,
    host: &str,
    now: u64,
    agent: impl Fn(&crate::git::Repo) -> Result<Agent>,
) -> Result<Value> {
    let harness = target.harness;
    let here = repo
        .clone()
        .unwrap_or_else(|| crate::git::Repo::new(std::env::current_dir().unwrap_or_default()));
    let kit = crate::kit::load()?;
    let held = kit.harnesses.iter().any(|held| held.harness == harness);
    let removed = kit.is_removed(harness);
    let launches = crate::launch::Launches::new(&here, true)?;
    let foreseen = crate::launch::Launches::foreseen(&here);
    // A removed harness is validated without worker-launch discovery writing
    // it back before the switch has cleared its removal.
    let checking = if removed { &foreseen } else { &launches };
    let start_fault = match crate::role::startable(checking, harness, PRIORITY_BY) {
        Ok(()) => None,
        Err(error) if !held => {
            let installed = foreseen.of(harness)?.is_some_and(|proposed| proposed.resolves());
            if !installed {
                return Err(error);
            }
            Some(format!("{error:#}"))
        }
        Err(error) => return Err(error),
    };
    let mut result = crate::kit::prefer(harness, target.model.as_deref())?;
    result["added"] = json!(!held);
    result["start_fault"] = json!(start_fault);
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
    let named_paths = crate::checkout_list::List::of_host()
        .and_then(|list| list.read())
        .map(|listed| {
            listed
                .checkouts
                .into_iter()
                .filter(|entry| entry.harness.is_some())
                .map(|entry| entry.checkout)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for path in paths {
        // A checkout that names a harness keeps it. The kit was already
        // reordered. Nothing of that checkout's workers is moved, and an
        // empty answer is not "no claim".
        if named_paths.iter().any(|named| named == &path) && !matches!(which, Which::Nothing) {
            continue;
        }
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
    let before = fleet.first_harness().unwrap_or(Harness::DEFAULT);
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
/// intact. It runs the selected harness's user default: it has no model label.
pub(crate) fn auxiliary(fleet: &Fleet, agent: &Agent, worktree: &Worktree) -> Result<Value> {
    let chosen = fleet.first()?;
    let harness = chosen.harness();
    crate::role::startable(&agent.launches, harness, "worker continuation")?;
    let continued = continuing(Some(worktree), harness);
    worktree.stop_for_continuation()?;
    if worktree.brief()?.exists() {
        crate::dispatch::continue_brief(fleet, worktree, None, None, &continued)?;
    }
    let command = chosen.start_line(agent.command(harness)?.as_str());
    let terminal = agent.start_worker(
        worktree,
        &fleet.gh.tracker,
        (&chosen).into(),
        &command,
        false,
        None,
        fleet.now,
    )?;
    worktree.deliver(&terminal, &worktree.brief()?)?;
    Ok(json!({"worktree": worktree.path, "from": continued.from, "to": harness,
            "terminal": terminal, "skipped": fleet.skipped_before(harness, None, None)}))
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
        "stage": started.get("stage"), "skipped": started["skipped"]}))
}

/// Continues this checkout's workers that are at work on a harness other
/// than `target`. A worker already on `target`, one with no worktree, one
/// whose harness cannot be read, and one that is gone are left. Returns
/// whether every worker that needed to move did. The fleet must already
/// select `target`: a checkout that names one is pinned to it, and one that
/// names none selects the host kit's first.
pub(crate) fn onto(fleet: &mut Fleet, me: &Me, agent: &Agent, target: Harness) -> Result<bool> {
    let mut landed = true;
    let source = Source::of(&fleet.repo)?;
    let issues: Vec<u64> = mine_all(fleet, me).iter().map(|claim| claim.number).collect();
    for issue in issues {
        let Some(worktree) = Worktree::of_issue(&source, issue)? else { continue };
        if !other_than(&worktree, fleet.now, target, &mut landed) {
            continue;
        }
        let on = worktree.harness()?.unwrap_or(target);
        if switch_issue(fleet, me, agent, issue, on).is_err() {
            landed = false;
        }
    }
    for name in my_patrols(fleet, me) {
        let created = afkconvoy_core::patrol::checkout_name(&name);
        let Some(worktree) = Worktree::named(&source, &created)? else { continue };
        if !other_than(&worktree, fleet.now, target, &mut landed) {
            continue;
        }
        let on = worktree.harness()?.unwrap_or(target);
        let said = move |checkout: &Worktree| continuing(Some(checkout), on);
        if crate::patrol::switch(fleet, me, agent, &name, said).is_err() {
            landed = false;
        }
    }
    if let Some(worktree) = Worktree::named(&source, afkconvoy_core::casting::CASTER_WORKTREE)?
        && worktree.terminal()?.is_some()
        && other_than(&worktree, fleet.now, target, &mut landed)
        && auxiliary(fleet, agent, &worktree).is_err()
    {
        landed = false;
    }
    if let Some(worktree) = Worktree::of_train(&source)?
        && worktree.terminal()?.is_some()
        && other_than(&worktree, fleet.now, target, &mut landed)
        && auxiliary(fleet, agent, &worktree).is_err()
    {
        landed = false;
    }
    Ok(landed)
}

/// Whether the worker in `worktree` is at work on a harness other than
/// `target`. Gone, unreadable, and already there are left. Uncertainty
/// about the state or the harness leaves `landed` false so the tick retries.
fn other_than(worktree: &Worktree, now: u64, target: Harness, landed: &mut bool) -> bool {
    let state = match worktree.state(now) {
        Ok(state) => state,
        Err(_) => {
            *landed = false;
            return false;
        }
    };
    if matches!(state, WorkerState::Gone) {
        return false;
    }
    match worktree.harness() {
        Ok(Some(on)) if on != target => true,
        Ok(_) => false,
        Err(_) => {
            *landed = false;
            false
        }
    }
}

/// What `afkconvoy switch --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

`afkconvoy switch --harness codex` moves Codex to the front of the host kit. Run it anywhere, with no repo or instance required. A checkout that names no harness follows this order. A checkout that names a harness keeps that name: switch does not change it, and does not continue that checkout's workers onto the kit's new first. `--all` skips those checkouts. To change priority again, switch again. The kit itself has no separate choice to clear. A checkout's harness is cleared with `afkconvoy run <checkout> --harness default`.

## Who runs it

The operator, on the host whose kit and workers are being changed.

## What it reads and writes

A known harness that is installed but absent is added first on its stock worker launch command, with no model, so workers start on its login's default at once. This includes one the operator removed with `afkconvoy kit --remove`: the switch clears its removal and puts it first, after which relisting, daily refresh and launch discovery keep it as an ordinary entry. The answer comes within seconds, before that harness is listed or researched. Only after the answer does a background refresh list it and hand its listing to research, preserving the order this switch set. No model is sent a prompt: newly listed models are `research pending`, and its workers stay on the default until research places them with dated sources. If the added harness cannot start a worker, the answer's start_fault says why and doctor names the same fault. Unknown or uninstalled harnesses, and existing entries that cannot start, are refused before the kit changes.

`afkconvoy switch --model sonnet` accepts any listed id or researched name of an existing placed model, moves it first within its harness and moves that harness first. A name matching several harnesses or models is refused with candidates; an unknown name is refused with nearest names, leaving the kit unchanged. Repeating a switch is idempotent. Commands, model contents, research and the relative order of other models and harnesses stay intact. Rebuilds, daily refresh and new-model research preserve these orders; new discoveries follow established models. Ticket labels and team rosters are untouched. Role limits still constrain candidates. Paused candidates are skipped without reordering the kit, and become eligible again after a successful recovery probe. A switch with no target leaves running workers alone.

--all continues workers across every checkout on this host's checkout list that names no harness. A checkout that names one is left as it is, workers included. ISSUE or --patrol on a checkout that names one records nothing moved. --instance may identify the owner of a claim in a checkout that names none. Each continuation keeps its claim, stage, worktree, branch, uncommitted work, PR and breaker counts. Its brief includes the previous output. A failure is reported and other workers continue. An old worker that cannot be confirmed stopped is never given a second writer. Every moved worker's answer and tick-log line name every harness ahead of its actual successor, why it was skipped, and what would make it eligible. To remove the harness those workers are on, move them first with `afkconvoy switch --all --harness <harness>`; `kit --remove` refuses while any live fleet worker remains on it and never ends one.


## What it answers

{harness, model, path, order_before, order, changed, added, restored, start_fault, switched, failed}. model is the promoted id, or null for a harness-only switch; added says whether a missing harness was inserted; restored says whether this switch cleared an operator removal. Each moved worker has {from, to, skipped}, where skipped holds {harness, why, fix} for every earlier candidate. Each failure names its checkout and worker. A successful fallback reports the actual harness.

## What trips you up

--all is machine-wide, even when --instance is supplied. --clear is removed: switch to the harness you want first instead. Old repository choice files are ignored. Only `switch --harness` restores an operator-removed harness; a pause, probe, relisting, daily refresh or launch discovery never does."#;
