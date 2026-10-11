# A failure escalates; nothing is retried

afk-fleet discards a failed attempt — closes its PR, deletes its branch, removes its worktree — and starts a fresh worker from the base, up to `retry` times, before handing the issue to a human. afkconvoy hands it over at the first failure, with the PR, branch and worktree kept as evidence.

With the checks gate and the adversarial verify gone (ADR-0004), what is left to fail is a worker that gave up or went silent past its nudge. A fresh worker reads the same issue and tends to stop at the same place, at the cost of everything the first one did.

## Consequences

- No `afk-attempt/*` labels, no `retry` key, no discard of an attempt, and none of the bookkeeping that made a failure count once.
- `giving-up` and `needs-decision` lead to the same place, so they are one verdict.
- Continuation is unaffected: a worker that died is still continued from its progress. That answers "did the worker die", not "is the work failing".
