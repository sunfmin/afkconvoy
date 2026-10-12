---
status: proposed
---

# No human stands in a workflow by default

A workflow runs from intake to landing without a human. A human is reached only by exception: an `escalate` verdict no consult ruled on, a breaker, a failure. A stage may be given to a human (`by: human`), but that is a choice a workflow makes, not the shape the fleet assumes.

This reverses where the design started — a human's label as the approval of every issue, a human's sign-off before every landing. Each made a person a step every issue waits on.

## Consequences

- A worker may take an issue in: a patrol that proposes work, a stage that splits its issue. With nobody approving, what bounds the work the fleet gives itself is a limit on such issues still open (`intake_limit`), counted from the record intake leaves. It is set high, to stop a runaway and not to meter a normal day. Issues a human took in are not counted.
- Rework is bounded by two breakers, since no reviewer of the reviewers exists: a total per issue, and rejections in a row by one stage. A breaker escalates straight to a human; one more role in the loop is one more round.
- An `escalate` verdict goes first to a consult, where the stage names one, and only then to a human. The chain is one role long.
- Whether a rejection raises new findings or old ones is not counted. Only the reviewer could say, and nothing could check it.
- A human's ruling on an escalated issue may become a precedent: a worker proposes it as a change to the team's precedents, and a human merging that change is what accepts it. Open proposals are held under a limit.
- Trunk broken is the one fault every issue shares, and with no human watching it would escalate them one by one. A fleet lock stops everything but the fix, and is itself the issue that carries the fix.
- A human who is needed is not told (ADR-0008); the escalate label and the `stage:` label are what there is to look at.
