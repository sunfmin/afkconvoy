---
status: accepted
---

# No human stands in a workflow by default

A workflow runs from intake to landing without a human. A human is reached only by exception: an `escalate` verdict no consult ruled on, a breaker, a failure. A stage may be given to a human (`by: human`), but that is a choice a workflow makes, not the shape the fleet assumes.

This reverses where the design started — a human's label as the approval of every issue, a human's sign-off before every landing. Each made a person a step every issue waits on.

## Consequences

- A worker may take an issue in: a patrol that proposes work, a stage that splits its issue. With nobody approving, what bounds the work the fleet gives itself is a limit on such issues still open (`intake_limit`), counted from the record intake leaves. It is set high, to stop a runaway and not to meter a normal day. Issues a human took in are not counted.
- Rework is bounded by two breakers, since no reviewer of the reviewers exists: a total per issue, and rejections in a row by one stage. A breaker escalates straight to a human; one more role in the loop is one more round.
- An `escalate` verdict goes first to a consult, where the stage names one, and only then to a human. The chain is one role long.
- Whether a rejection raises new findings or old ones is not counted. Only the reviewer could say, and nothing could check it.
- A ruling answers its issue and resumes its stage. Lasting rules are amended through ordinary work in their owning ADR, team or role rules with their sources preserved (ADR-0039, sunfmin/control#141). This supersedes the former automatic proposal and human acceptance lifecycle.
- Trunk broken is the one fault every issue shares, and with no human watching it would escalate them one by one. A fleet lock stops everything but the fix, and is itself the issue that carries the fix.
- A human who is needed is not told (ADR-0008); the escalate label and the `stage:` label are what there is to look at.

## Amended: a red base branch raises the lock in an unbound repo too

A worker in an unbound repo, or its inspection, that proves the base branch red the same way gated alone in a clean checkout finds the open issue for the fix or opens one, makes it dispatchable with the ready label, and raises the fleet lock on it with `afkconvoy lock`. No role is needed. The train worker does the same in either repo; in a bound repo it takes the issue in to a workflow first. The issue is the tracker's, with the repo label where needed (ADR-0027). A red landing train whose base branch is green raises no lock. The worker parks its own issue on the fix's; the train worker has none to park and stops until told to carry on.
