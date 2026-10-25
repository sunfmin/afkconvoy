# A failure escalates; nothing is retried

afk-fleet discards a failed attempt — closes its PR, deletes its branch, removes its worktree — and starts a fresh worker from the base, up to `retry` times, before handing the issue to a human. afkconvoy hands it over at the first failure, with the PR, branch and worktree kept as evidence.

With the checks gate and the adversarial verify gone (ADR-0004), what is left to fail is a worker that gave up or went silent past its nudge. A fresh worker reads the same issue and tends to stop at the same place, at the cost of everything the first one did.

## Consequences

- No `afk-attempt/*` labels, no `retry` key, no discard of an attempt, and none of the bookkeeping that made a failure count once.
- `giving-up` and `needs-decision` lead to the same place, so they are one verdict.
- Continuation is unaffected: a worker that died is still continued from its progress. That answers "did the worker die", not "is the work failing".

## Amended: a dispatch whose worker never started is continued

A terminal created is no worker started: the harness must become ready and
take its brief. Until then, the dispatch is incomplete. The next tick starts
the worker again under the same claim, in the same worktree, preserving its
branch and local work. This is continuation of a claim left without a worker,
not a fresh attempt at failed work. A second failed start escalates and
releases the claim, with the startup errors as its reason. An incomplete
start is never judged silent, and a nudge requires a harness ready for a
prompt immediately before it is sent.

## Amended by ADR-0010 and ADR-0011

A failure is a worker that crashed, went silent past its nudge, could not turn the gate green, declared no valid verdict, or crossed a guard. Those still escalate at once and are never retried.

A `reject` verdict is not a failure. It is an edge of the workflow: the earlier stage runs again, with the reason, in the same worktree with everything kept. What was wrong with afk-fleet's retry — a fresh worker reads the same issue and stops at the same place — does not apply, because the stage that runs again has been told something new. It is bounded by the breakers instead of by a retry count.

## Amended: what a fleet lock explains runs again

One failure is not the issue's own: a broken base branch, which every issue meets and a fleet lock is raised for (ADR-0011). A stage's `escalate`, or its silence past a nudge, from before the lock closed and not acted on since is not carried out once the last lock has closed: the stage runs again, and that counts as no rework. This is not the retry the decision above refuses — the stage has been told something new, and the base branch it runs on is another one. A human handed such an issue could only answer "run the gate again".

It is decided each tick from the stage records and the time the lock's issue closed, and stored nowhere: the fleet leaves a stage record saying a lock stood, which is what puts the issue at its stage again (ADR-0010). What a stage declares after the lock closed is carried out as it stands, so nothing runs twice for one lock. A `pass`, a `blocked` and an `already-satisfied` are not what a broken base branch produces, and stand.
