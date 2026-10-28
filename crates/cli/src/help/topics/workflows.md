A workflow is the stages one issue passes through and where each verdict leads, ending in a landed PR or in nothing more to do. Read this to write a file of a team's `workflows/`, or to tell why an issue went where it did.

## A workflow's file

`workflows/<name>.md` is the workflow `<name>`: an issue that carries its flow label is at the first stage. That is `flow:<name>`, or what `label` names — `ready-for-agent` takes issues opened the usual way; an epic stays out. Above the yaml block, say what the workflow is for and what an issue of it holds: `afkconvoy flows` answers that prose to whoever opens an issue, and `afkconvoy check` warns of a file that says nothing there.

```yaml
label: <label>                     # optional; default flow:<name>
max_rework: {const:MAX_REWORK}                      # default
max_consecutive_rework: {const:MAX_CONSECUTIVE_REWORK}          # default
stages:
  - name: <unique; not land, done or human>
    by: <role> | human
    do: <skill>                    # required unless by: human
    writes: false                  # optional; default true
    escalate: { to: <role>, do: <skill> } | { to: human }  # optional: the consult
    on: { pass: <stage> | land | done, reject: <earlier stage> }   # reject optional
land: { by: <role>, do: <skill> }  # required exactly when a pass leads to land
```

## Stages

A stage is one skill of one role, performed by one new worker: `by` is a short name of the team file, `do` an entry of that role's `.claude/skills/`. The worker of a stage knows nothing of the one before it but what the stage records say, so a stage that judges never inherits the reasoning of the one that wrote.

A stage names neither a harness nor a model. What its worker runs is the first harness of its role's range that the host kit holds, and the issue's one model label of that harness, or that harness's user default ([`afkconvoy help roster`](help:roster)).

`writes: false` has the guard refuse the stage's worker a commit and a push: mark every stage that judges so.

## Verdicts, and where each leads

A worker ends its stage with `afkconvoy verdict`, which leaves a stage record on the issue. Where an issue is, is derived from those records each tick and stored nowhere: `afkconvoy show --issue <n>` reads it.

- `pass`: where the stage's `on.pass` says: a stage, `land` or `done`
- `reject`: back to the stage `on.reject` names: always an earlier one
- `blocked`: parked; the same stage runs again when its blockers close
- `already-satisfied`: the workflow ends, with nothing landed
- `escalate`: to the stage's consult (below), or a human where it says so

Only `pass` and `reject` are keys of `on`: the other three mean the same at every stage. `done` ends the workflow with nothing to land; `land` hands the issue's PR to the landing (below). A stage with no `reject` may not declare one. Following `pass` alone from any stage must come to `land` or `done`, and every stage must be reached from the first: a loop back is made with `reject` only.

A stage may split its issue: open issues, take each in, then declare `blocked` on them. When they close, it judges the whole. The team file's `split` key may be false to omit this procedure from briefs ([`afkconvoy help team`](help:team)); true is the default.

A blocker, defect or split piece for another repo of the host goes on the tracker, with that repo's `repo:<owner>/<repo>` label. Run `afkconvoy flows` in that repo's checkout and use its opening line. In a bound repo omit the flow label while opening, then take it in there with `afkconvoy intake`. A blocker outside the tracker, or carrying no single repo label of a checkout serving it, is one no fleet works: `plan` and `doctor` name it with transfer and label lines. Only such blockers never hold a named consult or a declared hand-off back; a build worker still waits. The consult's brief names them.

A stage that finds a defect outside its issue records the evidence on an existing issue or opens one, takes it in, and declares `blocked` on it. An unrecorded base tip is also certified before any landing turn. If it is red, the fleet dispatches an operator repair outside product workflows, with a fleet lock. Its own turn alone may start on that red tip, and still must gate green before landing.

## Rework and its breakers

Rework is a stage running again because a later stage rejected its work or a consult ruled on it. It is counted per issue from the stage records, from where a human last ruled on the issue or sent it to a stage. Two breakers bound it:

- `max_rework`: the rework of one issue, in total
- `max_consecutive_rework`: the rejections in a row by one stage

A count never passes its breaker: the rework that would is not started, and the issue is handed on (below), naming the breaker and every rework's stage and reason. `afkconvoy rehearse` shows which verdict trips one, with no worker started.

## A consult

A stage's consult is asked before a human is for a hand-off about the work: its `escalate`, a tripped breaker, a failure, a workflow changed under it. A fleet fault — a worker start that failed again, a worktree trust gap — escalates at once: the fix is on the host. Every other hand-off starts a named consult where that human would stand, free to run what they would ([`afkconvoy help humans`](help:humans)). `escalate: { to, do }` names a more senior role, run within its range ([`afkconvoy help roster`](help:roster)); where a stage names none, or says `{ to: human }`, the issue escalates for the host's watch to answer with the same commands a human uses. There is no stock consult. A consult rules (`afkconvoy rule`: the stage runs again; on an `escalate` that is one more rework in total), sends, closes, parks or lets it through. Close and park add no rework ([`afkconvoy help humans`](help:humans) says how to bring a park back). The same hand-off again goes to a human. A consult writes no code.

## A stage `by: human`

No human stands in a workflow unless it names one. A stage `by: human` has no `do`; it starts no worker and holds no claim. The issue waits there until a human declares the stage's verdict with `afkconvoy verdict`.

Anywhere else a human is reached by an escalation: what a consult let through, or what a stage with `to: human` hands on. The human answers with `afkconvoy rule` — the stage runs again under the ruling — or `afkconvoy send`, to a stage of their choosing.

## A landing

A workflow in which a pass leads to `land` names who lands: `land: { by, do }`. The tick after that `pass` starts a new worker in the role `by`, with the skill `do`, within its role's range. It lands the issue's PR in a turn (`afkconvoy land --help`), using the same protocol as every other PR. A `land:` no pass leads to is a fault, and so is a `pass: land` with no `land:`.

## Where to read on

- [`afkconvoy help team`](help:team): the roles a `by` names, and the rest of a team's directory
- [`afkconvoy help roles`](help:roles): what a role is, and how to write a good one
- [`afkconvoy help patrols`](help:patrols): how an issue comes to carry a flow label with no human
- [`afkconvoy help glossary`](help:glossary): each term used here, with its one definition
