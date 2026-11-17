---
status: accepted
---

# A failure escalates; nothing is retried

afk-fleet discards a failed attempt — closes its PR, deletes its branch, removes its worktree — and starts a fresh worker from the base, up to `retry` times, before handing the issue to a human. afkconvoy never retries failed work. It hands the issue on at the first failure, with the PR, branch and worktree kept as evidence.

A fresh worker reads the same issue and tends to stop at the same place, at the cost of everything the first one did. So there are no `afk-attempt/*` labels, no `retry` key and no discard of an attempt.

A **failure** is a worker that crashed, went silent past its nudge, could not turn the gate green, declared no valid verdict, or crossed a guard. `giving-up` and `needs-decision` lead to the same place, so they are one verdict.

## What is not a failure

- **Continuation.** A worker that died is continued from its progress. That answers "did the worker die", not "is the work failing". A dispatch whose worker never took its brief is likewise started again under the same claim, in the same worktree; a second failed start escalates.
- **A `reject` verdict.** It is an edge of the workflow (ADR-0010): the earlier stage runs again, with the reason, in the same worktree with everything kept. The stage that runs again has been told something new, and the breakers bound it (ADR-0011).
- **What a fleet lock explains.** A broken base branch is not the issue's own failure (ADR-0011). A stage's `escalate`, or its silence past a nudge, from before the lock closed and not acted on since, is not carried out once the last lock has closed: the stage runs again, counting as no rework. It is decided each tick from the stage records and the time the lock's issue closed, and stored nowhere. A `pass`, a `blocked` and an `already-satisfied` stand.

## Every hand-off goes to a named consult first

Most of what reached a human was something the team's senior role could have answered as well: read the worktree and the issue's edges, run the gate again, and rule. So every way the fleet hands an issue on goes first to the consult the issue's stage names: a failure the fleet saw (a worker silent past its nudge, a commit a stage that may not write left, an `already-satisfied` over a branch that holds work, a `blocked` that will never clear), a verdict the stage may not declare, a tripped breaker, and a workflow that changed under the issue. A human is reached only when the consult lets the issue through.

Only a consult a team names exists. A stage that names none, a stage `by: human`, and every unbound repo escalate at once, and the host's watch answers what waits (ADR-0040).

- **What a consult is given** is what the human's comment would have carried — the reason and the evidence — with the issue, its worktree, its stage records, where its worker's terminal and the gate are, and the fleet's own commands. No check per kind of failure and no suggested answer.
- **What it may do** is what a human does with an escalated issue, through the same commands (`waiting::ANSWERING` is the one list both are given), and any read-only command, the gate included: rule, send the issue to another stage, close it, change its blocked-by, or open an issue.
- **What it may not do** is commit, push, or change the host. The guard refuses those, and a commit it left all the same is its own failure.
- **Bounds.** A consult's ruling is no rework and starts no count over: only a human's word does. The next hand-off of the same kind goes to a human. A consult's own failure goes to a human. The chain is one role long.
- **Record.** A failure the fleet saw leaves no record of the worker's, so the fleet leaves a stage record stating it, whose text is what a human would have been told. The issue's position is derived from it as from any record (ADR-0010).

## A fleet fault skips the consult

A consult cannot change the host, and starts on the same host and harness as the worker it would answer for. Asking it about a fault of the host repeats the fault and delays the fix. So a **fleet fault** escalates to a human at once.

Exactly two hand-offs are fleet faults: `its worker start failed again` and `its harness's worktree trust gap`. Either way the worker never took its brief, and the fix is on the host: trusting a worktree, switching a harness, waiting out a usage limit, mending a build server. The escalation comment and `doctor`'s `waiting_on_human` entry name the fault and say the fix is on the host. Its escalation marker records which fault it was, so a later tick leaves it with the human. An issue whose task every harness in its range refused skips the consult in the same way: a consult would read the same task on the same exhausted kit.

Every other hand-off still goes to its consult first. A silent worker had its brief, so the work may be the cause. Nothing new retries: the operator fixes the host and hands the issue back, and another failed start escalates at once again.
