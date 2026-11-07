//! `show` and `rehearse`: a loaded team's workflow drawn, and walked through
//! a list of verdicts with no worker started. Both only put into words what
//! the core decides (`afkconvoy_core::graph`, `afkconvoy_core::position`).

use afkconvoy_core::casting;
use afkconvoy_core::graph;
use afkconvoy_core::position::{ConsultOn, Declared, Place, Position};
use afkconvoy_core::team::{HUMAN, Team, Workflow};
use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::team::Loaded;

/// The workflow of the team that is named `name`.
fn named<'t>(loaded: &'t Loaded, name: &str) -> Result<&'t Workflow> {
    let Some(workflow) = loaded.team.workflow(name) else {
        let has: Vec<&str> = loaded.team.workflows.iter().map(|w| w.name.as_str()).collect();
        if has.is_empty() {
            bail!("the team {} has no workflow `{name}`: it has none", loaded.name);
        }
        bail!("the team {} has no workflow `{name}`; it has: {}", loaded.name, has.join(", "));
    };
    Ok(workflow)
}

/// The drawing of the workflow named `name`, or of every workflow of the
/// team, a blank line between two.
pub fn show(loaded: &Loaded, name: Option<&str>) -> Result<String> {
    let draw = |flow: &Workflow| graph::draw(&loaded.team, flow);
    let drawn: Vec<String> = match name {
        Some(name) => vec![draw(named(loaded, name)?)],
        None => loaded.team.workflows.iter().map(draw).collect(),
    };
    if drawn.is_empty() {
        bail!("the team {} has no workflow to draw", loaded.name);
    }
    Ok(drawn.join("\n") + "\n" + &graph::roster(&loaded.team))
}

/// Where an issue of the workflow named `name` is after each of `declared`,
/// and where it ends.
pub fn rehearse(loaded: &Loaded, name: &str, declared: &[Declared]) -> Result<Value> {
    let workflow = named(loaded, name)?;
    let mut place = Place::enter(workflow);
    let enters = position(&loaded.team, workflow, place.position);
    let mut steps: Vec<Value> = vec![];
    for declared in declared {
        let stage = place.position.stage().map(|stage| stage.name.as_str());
        let valid = place.takes(*declared);
        let next = place.after(*declared);
        // A breaker is tripped by what was declared, not by what follows it.
        let breaker = match next.position.breaker() {
            Some(breaker) if valid => Some(breaker.key()),
            _ => None,
        };
        let mut in_a_row = Map::new();
        for stage in workflow.stages().iter().filter(|stage| stage.reject().is_some()) {
            in_a_row.insert(stage.name.clone(), json!(next.consecutive_rework(stage)));
        }
        steps.push(json!({
            "declared": declared.word(),
            "stage": stage,
            "valid": valid,
            "breaker": breaker,
            "position": position(&loaded.team, workflow, next.position),
            "rework": next.rework(),
            "consecutive_rework": in_a_row,
        }));
        place = next;
    }
    Ok(json!({
        "workflow": workflow.name,
        "max_rework": workflow.max_rework,
        "max_consecutive_rework": workflow.max_consecutive_rework,
        "enters": enters,
        "steps": steps,
        "ends": position(&loaded.team, workflow, place.position),
    }))
}

/// A position as a command answers it: what it is, at which stage, who acts
/// on it, and — where a worker does — on which harness with no casting on
/// this host: a worker in a role on its range's first. None on a host without
/// a host kit.
pub(crate) fn position(team: &Team, workflow: &Workflow, position: Position) -> Value {
    let is = match position {
        Position::Perform(..) => "perform",
        Position::Waiting(_) => "waiting",
        Position::Consult(..) => "consult",
        Position::ParkedForEvidence(_) => "parked-for-evidence",
        Position::Parked(_) | Position::LandingParked => "parked",
        Position::Landing => "landing",
        Position::Ended => "ended",
        Position::Escalating(..) | Position::LandingEscalating => "escalating",
    };
    let worker = match position {
        Position::Landing => workflow.land.as_ref(),
        _ => position.worker(),
    };
    let by = match position {
        Position::Waiting(_) | Position::Escalating(..) | Position::LandingEscalating => {
            Some(HUMAN)
        }
        _ => worker.map(|work| team.role_of(work).name.as_str()),
    };
    // What runs is this host's host kit's to say: none, on a host without one.
    let harness = worker.and_then(|work| {
        let range = crate::kit::load().ok()?.range();
        let range = casting::range(team.role_of(work), &range);
        range.first().map(|held| held.harness)
    });
    // Why it is with a human — or with its stage's consult, for a hand-off
    // that would have gone to one.
    let reason = match position {
        Position::Escalating(stage, reason)
        | Position::Consult(stage, _, ConsultOn::HandOff(reason)) => Some(reason.said(stage)),
        Position::LandingEscalating => {
            Some(afkconvoy_core::position::LANDING_ESCALATION.to_string())
        }
        _ => None,
    };
    // Landing has left the workflow's stages. Its new waiting positions
    // name `land` for the declaration they need; ordinary landing keeps
    // its existing stage-less answer.
    let stage = match position {
        Position::LandingParked | Position::LandingEscalating => Some(afkconvoy_core::team::LAND),
        _ => position.stage().map(|stage| stage.name.as_str()),
    };
    json!({
        "is": is,
        "stage": stage,
        "by": by,
        "do": worker.map(|work| work.skill.as_str()),
        "harness": harness,
        "reason": reason,
    })
}

/// What `afkconvoy rehearse --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const REHEARSE_HELP: &str = r#"## When to run it

While designing a workflow, to test it without starting a worker or spending anything: name the workflow and what is declared on an issue, in order, and it says where the issue would be after each and where it ends. Use it to see that a rejection leads back where you meant, and how many it takes to trip a breaker.

```
afkconvoy rehearse feature pass pass reject pass pass
```

## Who runs it

The operator, or whoever designs the team: in a checkout of a bound repo, or anywhere with `--team` or `--dir`.

## What it reads and writes

It writes nothing and starts nothing. It reads a team as `check` does, with the same `--team` and `--dir`: see `afkconvoy check --help`. No issue is read: the issue is one that has just entered the workflow, at its first stage.

After the workflow's name comes what is declared, each by whoever the issue is with at that point. The five verdicts:

- `pass`: follows the stage's `on.pass`: to a stage, `land` or `done`
- `reject`: follows the stage's `on.reject`, back to an earlier stage
- `blocked`: parks the issue; the same stage runs again afterwards
- `already-satisfied`: ends the workflow
- `escalate`: goes to the stage's consult if its `escalate:` names one, else to a human

and `ruling`: what a consult, or a human on an escalated issue, decides. The stage that asked runs again under it.

## What it answers

{"workflow", "max_rework", "max_consecutive_rework", "enters", "steps", "ends"}

- `enters`, `ends`: the position before anything is declared, and after everything
- `steps`: one per thing declared, in order: {"declared", "stage", "valid", "breaker", "position", "rework", "consecutive_rework"}

Of a step, `stage` is the stage it was declared in, null once the issue is landing or ended, and `position` where the issue is after it. A position is {"is", "stage", "by", "do", "harness", "reason"}:

- is: perform: a worker is to perform `stage`, as the skill `do` of the role `by`
- is: waiting: `stage` is a human's (`by: human`): it waits for their verdict
- is: consult: a worker is to rule on the `escalate` of `stage`, as `do` of `by`
- is: parked: parked at `stage`, which runs again when its blockers close
- is: landing: the workflow's `land` lands in a turn, as `do` of `by`
- is: ended: nothing more to do
- is: escalating: handed to a human, with the `reason`; under their `ruling` the `stage` runs again

`harness` is the harness the worker of the position is a session of, null where no worker acts: the stage's — the one it names, or the default of this checkout's config — for a worker that performs it, and the default for a consult and a landing.

`valid` is false for what may not be declared where the issue is: a `reject` from a stage with no `on.reject`, a `ruling` nobody asked for, anything but `ruling` or `escalate`, `close` or `park` from a consult. An invalid verdict escalates the issue at a stage. Landing accepts `blocked` and `escalate`; a ruling returns an escalated issue to Landing. Other declarations at landing, anything declared on an ended issue, and anything but a `ruling` on one that is escalating change nothing.

`rework` is the issue's rework in total: its rejections and its consults' rulings. `consecutive_rework` is, for each stage that may reject, its rejections in a row: a `pass` of that stage starts its count over. A human's `ruling` starts every count over. `breaker` is null, or the breaker this step tripped — "max_rework" or "max_consecutive_rework": the rework that would have passed it is refused and not counted, and the issue escalates at the stage that would have run again.

Errors, as {"error": …} with a non-zero exit: a word that is none of the six; the team has no workflow of that name; the team has a fault — {"error", "faults"}, as `check` answers it; and every error of `check`.

## What trips you up

- It rehearses only a team that loads. Run `check` first.
- A count never passes its breaker. With `max_rework: 3` the fourth rework is the one refused: the step says `"breaker": "max_rework"` and `"rework": 3`.
- Running again after `blocked` is no rework."#;

/// What `afkconvoy show --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const SHOW_HELP: &str = r#"## When to run it

After `check` accepts a team, to review a workflow as a whole: where an issue enters, which role performs each stage, and where every `pass` and `reject` leads. Paste it into a GitHub comment inside a code block to have it reviewed there.

With `--issue`, to ask where one issue is in its workflow: how it came in, its position, what follows it, and the rework it has left before a breaker stops it.

```
afkconvoy show --issue 7
```

Or `afkconvoy show 7`: a workflow is not named by a number, so one given is an issue's.

In an unbound repo, to ask where an issue stands on the one path every issue there takes — ready, held back, claimed, landing, waiting on a human, or closed. An unbound repo has no workflow to draw: `show` with no issue there is refused.

## Who runs it

The operator, or whoever designs the team: in a checkout of a bound repo, or anywhere with `--team` or `--dir`. With an issue, also the operator in a checkout of an unbound repo.

## What it reads and writes

It writes nothing. It reads a team as `check` does, with the same `--team` and `--dir`: the team's files, and every role's repo from GitHub. See `afkconvoy check --help`. With a workflow's name it draws that one; with none, every workflow of the team.

With `--issue` it also reads, in a checkout of the issue's repo, the issue's labels and its comments. The workflow is the one the issue's flow label names — for an escalated issue, whose flow label was swapped out, the one its escalation's comment says it left. The position is read off the issue's stage records — the comments `afkconvoy verdict`, `rule` and `send` write — and off nothing else: not its `stage:` label, which the fleet writes for humans to filter by and never reads. How it came in is read off its intake record, the comment `afkconvoy intake` writes.

In an unbound repo, with no `--team` or `--dir`, it reads no team: it reads what `afkconvoy plan` reads — the open issues and their labels, blockers and comments, the open PRs, and the claims and heartbeats on the remote — as of no fleet instance, every claim a peer's.

## What it answers

Not JSON: a drawing, as plain text. One workflow:

```
feature — flow:feature — max_rework 3, max_consecutive_rework 2

┌───▶ plan      dev: plan
│       ↓ pass → build
│ ┌─▶ build     dev: implement · escalate → reviewer: rule
│ │     ↓ pass → review
│ └── review    reviewer: review · writes nothing · reject → build
│       ↓ pass → sign-off
└──── sign-off  human · reject → plan
        ↓ pass → land
      land      dev: land

roster

  dev       claude-code opus (most work) · claude-code fable (hard or wide work)
            cast: Hard or wide work on fable; the rest on opus.
  reviewer  the host kit whole
```

The first line is the workflow, its flow label and its two breakers. Then one row per stage, in the order its file lists them — an issue enters at the first — with who performs it, as `<role>: <skill>` or `human`, and what else its file sets. Under each stage is where its `pass` leads. A `reject` is said on its stage's row and drawn as the line that leaves that row to the left and comes back in, at the arrow, to the stage it leads to. The last rows are the ends: `land` with who lands, and `done`. Under the drawings, once, is the team's roster: a row per role, each harness and model it may run in order of preference — `any model` where the entry names none — with what it is good at; `the host kit whole` for a role the roster names no entry for (`afkconvoy help roster`). What a role's range is on this host is `afkconvoy check`'s to answer.

With `--issue`, JSON:

{"issue", "workflow", "intake", "position", "follows", "rework", "consecutive_rework", "records", "refusals"}

Also `delivery` (null for an ordinary task) and its resolved `target_branch`.

- `intake`: how the issue came in. {"by": "worker", "role", "from", "url"}: a worker in that role, on the issue `from`, took it in with `afkconvoy intake`, and `url` is the comment that records it; `patrol` in place of `from` where a patrol's worker did. {"by": "human"}: its flow label has no intake record for it
- `position`: where the issue is: {"is", "stage", "by", "do", "harness", "reason"}, as `afkconvoy rehearse --help` spells out
- `follows`: the position a `pass` declared now would lead to; null where none can be declared: the issue is landing, ended, escalating or with a consult
- `rework`: {"had", "left"}: its rework so far, and what `max_rework` still allows
- `consecutive_rework`: for each stage that may reject, {"had", "left"}: its rejections in a row, and what `max_consecutive_rework` still allows
- `refusals`: the harnesses that refused this issue, each with `harness` and the refusal line as `evidence`; every later start skips them at every stage
- `records`: its stage records, oldest first: {"stage", "verdict", "reads", "by", "claim", "role_commit", "team_commit", "harness", "url"}

Of a record, `by` says who left it: `worker` or — asked to rule on the stage's `escalate` — `consult`, under the `claim` and the two commits it names, as a session of the `harness` it names, or `human`, for whom all four are null. `verdict` is what it states: one of the five verdicts, `ruling`, or `sent`. And `reads` says how it counted:

- `verdict`: it is the verdict of the stage its claim was dispatched to, or what a human declared or ruled at the stage the issue was at
- `sent`: a human sent the issue to its stage
- `other-stage`: it names a stage the issue was not at: it counts for nothing
- `claim-spent`: a verdict was already declared under its claim: it is not one
- `unknown-stage`: it names a stage the workflow no longer has: the issue escalates

In an unbound repo, with an issue, JSON:

{"issue", "workflow": null, "stands", "claim", "pr", "delivery", "target_branch"}

- `stands`: {"is", …}, where the issue is on its one path, from the same facts a tick acts on. `is` is `ready`: a worker can be started on it now; `held`, with the `reason` `plan` gives it in `excluded` — no ready label, an epic label, open blockers, a fleet lock, an open linked PR not the fleet's; `claimed`: a fleet instance holds it, its worker at work; `landing`: a fleet instance holds it and its PR is the fleet's, which waits to land in a turn; `waiting_on_human`, escalated or a failed run's report, with the `why`, `url` and `answer` of its row in `plan`'s `waiting_on_human`; or `closed`, with GitHub's `state_reason` and `closed_by`
- `claim`: {"instance", "live"} while a fleet instance holds the issue: which one, and whether its heartbeat is within the lease — a claim that is not is work for a live fleet instance to take. Null while none does
- `pr`: the open PR that closes the issue, {"number", "branch", "fleet"}, `fleet` whether its branch is one the fleet recorded on the issue: only such a PR is landed by the fleet. Null while there is none

Errors, as {"error": …} with a non-zero exit: the team has a fault — {"error", "faults"}, as `check` answers it; the team has no workflow of that name, or none at all; the issue has no position — it carries several flow labels, one that names no workflow of the team, or none with no escalation that says which it left; there is no such issue (`no such issue: #7 (HTTP 404)`), or GitHub says it was deleted or transferred; and every error of `check`.

## What trips you up

- It draws only a team that loads. Run `check` first: a fault is reported, not drawn.
- `blocked`, `already-satisfied` and `escalate` are not drawn: they mean the same at every stage. `afkconvoy rehearse` shows where each leads.
- `--issue` reads the team as it stands now, not as it stood when a record was written: a workflow changed since can leave records that name no stage of it."#;
