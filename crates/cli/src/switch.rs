//! Moving running work onto another harness (`afkconvoy switch`): this
//! host's choice of harness for the repo, kept where no other host reads it,
//! and each worker named ended and one of that harness started in its place —
//! in the same worktree, on the same branch, at the same stage — briefed to
//! continue what is there. A claim, its branch, its worktree and its PR are
//! untouched, and nothing the tick counts moves: no restart, no rework, no
//! nudge.
//!
//! The choice is this host's, never the repo's: whoever else works the repo
//! runs the harness they have, under their own accounts. Every worker this
//! host starts for the repo afterwards is of the chosen harness, whatever the
//! config, a stage, a patrol or an issue's casting names, until a switch
//! chooses again or `switch --clear` forgets the choice, and each worker runs
//! what its range and casting say again (docs/adr/0020).

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
use crate::git::Repo;
use crate::judge::mine;
use crate::orca::{Source, Worktree};
use crate::role::startable;
use crate::watch::{join_record, record_join};

/// Where this host keeps the harness a switch chose for the repo: in the
/// repo's git dir, which every worktree of it on this host shares and no
/// push carries.
const CHOSEN_FILE: &str = "convoy-chosen-harness";

/// What calls for the chosen harness, as a fault of it is said.
pub const CHOSEN_BY: &str = "this host's `afkconvoy switch`";

/// How many of the last lines of a worker's terminal its successor is given.
const SAID_LINES: usize = 40;

/// The harness a switch on this host chose for the repo of `repo`, when one did.
pub fn chosen(repo: &Repo) -> Result<Option<Harness>> {
    let file = repo.note(CHOSEN_FILE)?;
    let kept = std::fs::read_to_string(&file).unwrap_or_default();
    match kept.trim() {
        "" => Ok(None),
        key => Harness::read(key).map(Some).ok_or_else(|| {
            anyhow!("{} names no harness: {}", file.display(), Harness::unknown(key))
        }),
    }
}

/// Keeps `harness` as this host's choice for the repo of `repo`; `None`
/// forgets it, and the range's answer to the casting stands again.
fn choose(repo: &Repo, harness: Option<Harness>) -> Result<()> {
    let file = repo.note(CHOSEN_FILE)?;
    match harness {
        Some(harness) => std::fs::write(&file, harness.key())
            .with_context(|| format!("cannot write {}", file.display())),
        None if file.exists() => {
            std::fs::remove_file(&file).with_context(|| format!("cannot remove {}", file.display()))
        }
        None => Ok(()),
    }
}

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

/// `afkconvoy switch`: chooses `to` as this host's harness for the repo —
/// `None` forgets the choice (`--clear`) — then moves the workers `which`
/// names onto what each now runs: `to`, or, cleared, what its range and
/// casting say. A harness chosen that cannot be started here is refused
/// before anything is chosen or any worker ended; so is a claim or a run I do
/// not hold.
pub fn switch(
    fleet: &mut Fleet,
    me: &Me,
    agent: &Agent,
    to: Option<Harness>,
    which: Which,
) -> Result<Value> {
    let was = fleet.chosen;
    // What a worker started before any switch kept its harness ran on.
    let before = fleet.harness();
    if let Some(to) = to {
        startable(&agent.launches, to, CHOSEN_BY)?;
    }
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
    choose(&fleet.repo, to)?;
    *fleet = Fleet::open(fleet.now)?;
    let chosen = json!({"was": was, "now": to});
    let one = |moved: Value| {
        let mut moved = moved;
        moved["chosen"] = chosen.clone();
        moved
    };
    Ok(match which {
        Which::Nothing => json!({"harness": to, "chosen": chosen, "switched": []}),
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
            json!({"harness": to, "chosen": chosen, "switched": switched, "failed": failed})
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
fn continuing(worktree: Option<&Worktree>, before: Harness) -> Continuing {
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

/// Moves the worker of my claim on `issue` onto what it runs now — this host's
/// chosen harness, else what its range and casting say: a worker
/// started on it as a continuation of what survives (`dispatch`), which closes
/// whatever terminal the worktree had, whatever its worker was doing. The
/// switch is recorded on the issue's open PR, beside its join's record.
fn switch_issue(
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
