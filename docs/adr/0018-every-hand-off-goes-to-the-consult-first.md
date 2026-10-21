---
status: accepted
---

# Every hand-off to a human goes to the stage's consult first

ADR-0011 asked a consult only of an `escalate`; everything else went straight to a human, and a consult could only rule or let through. In practice most of what reached a human that way was something the team's senior role could have answered as well: a worker counted silent while it waited on its own gate on a full disk, a `blocked` that would have closed a cycle with its own blocker. The human answering each read the worktree and the issue's edges, ran the gate again and ruled — nothing a consult cannot do.

So every way the fleet hands an issue to a human goes to the consult of the stage it is at first: a failure the fleet saw (a worker silent past its nudge, a commit a stage that may not write left, an `already-satisfied` over a branch that holds work, a `blocked` that will never clear), a verdict the stage may not declare, a tripped breaker, and a workflow that changed under the issue. A human is reached only when the consult lets the issue through.

## Where no consult is named, the stock consult stands in

Most stages name no consult, and every unbound repo has none to name, so most hand-offs would still have gone straight to a human. Afkconvoy carries a consult of its own for them: the **stock consult**. It is no role of any team and carries no opinion of how a product is built: its whole instruction is one short text in the binary, to look into what happened as the human handed it would and decide what they would. It names no kind of failure and suggests no answer; what it is told and what it may run are what follows here for any consult.

- **In a bound repo** a stage that names no consult has the stock consult. `escalate: { to: human }` names none: that stage hands every hand-off to a human, as a stage that named none did before. A stage `by: human` has no consult. The stock consult runs on the stage's harness, started in a directory of its own whose rules are the instruction, and the guard holds it as it holds any consult.
- **In an unbound repo** there are no stages and no records: an escalated issue goes to the stock consult first, under a claim of its own, on the fleet's default harness. The fleet leaves a marker that it asked it, and reads its answer off the issue as a human's would be read: the ready label put back hands the issue back to the fleet, a closed issue is left alone, an issue it opened and recorded as a blocker holds it back, and its `escalate` verdict lets it through to a human. Nothing guards an unbound worker, so the fleet checks the branch after: a commit it left is its failure. The config's `escalate_to: human` opts the repo out, every hand-off going to a human as before.

## The consult stands where a human stands

- **What it is given** is what the human's comment would have carried — the reason and the evidence — with the issue, its worktree, its stage records, where its worker's terminal and the gate are, and the fleet's own commands. No check per kind of failure, and no suggested answer: how it decides is its role's, in principles.
- **What it may do** is what a human does with an escalated issue, through the same commands: every command the `humans` help names for answering one (`waiting::ANSWERING` is the one list both are given), and any read-only command, the gate included. It rules, sends the issue to another stage, closes it, changes its blocked-by, or opens an issue.
- **What it may not do** is commit, push, or change the host: the guard refuses those, and a commit it left all the same is its own failure.

## Bounds

- A consult's ruling on a hand-off is no rework and starts no count over: only a human's word does. The next hand-off of the same kind — a breaker tripping again, the same failure again — goes to a human.
- A consult's own failure goes to a human. The chain is still one role long.

## How it is recorded

A failure the fleet saw leaves no record of the worker's, so the fleet leaves one: a stage record by the fleet stating the failure, whose text is what a human would have been told. The issue's position is derived from it as from any record (ADR-0010): with the consult, or with a human where the consult ruled on that kind already or is what failed. A breaker, a verdict the stage may not declare and a changed workflow need no new record: the position derived from the records puts the issue with the consult.

An issue already waiting on a human from a hand-off no consult saw is put to its consult once, by the first acting tick: the fleet leaves the record of its escalation and hands it back to the fleet. One a consult had its word on stays with the human.

## Amends

- ADR-0005: a failure no longer escalates at once. It goes to the consult of its stage first — the stock consult where none is named, and in an unbound repo — and to a human from there, or where the stage or the repo says `human`.
- ADR-0010: a workflow changed under an issue goes to the consult of the stage it was at before a human.
- ADR-0011: "a breaker escalates straight to a human" now holds of a stage that says `escalate: { to: human }`, and of a breaker that trips again after its consult ruled. A stage that names no consult has the stock consult.
