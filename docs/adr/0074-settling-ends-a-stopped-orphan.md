---
status: accepted
---

# Settling ends a stopped orphan in a worker's worktree

Amends ADR-0046 (sunfmin/control#837).

afkconvoy freezes a gate with SIGSTOP before it kills it, so that no process of its tree can fork or be reaped while its tree is found: `exclusive` at its bound, and a candidate's cancellation (ADR-0037). The stop and the kill are one step of one process. When that process goes between them — a test's scheduler killed, a host runner ended, a turn killed mid-cancellation — what it stopped stays stopped for good: handed to pid 1, nothing is left to resume it, and a SIGTERM to it stays pending. ADR-0046 has any process still in a worktree make settling wait, so one such gate keeps the claim and its worktree for ever. On one host a settled worktree waited about 21 hours on some 40 stopped fixture gates of afkconvoy's own tests, until a human killed them.

## Decision

A **stopped orphan** is a process stopped (`T`), handed to pid 1, and working in the worktree. Before settling looks for any process still in a worker's worktree, it ends each stopped orphan there, and every process under one: SIGTERM, then SIGCONT so that the signal is delivered, then SIGKILL a second later. Each one ended is a line of the tick log, `ended stopped orphan pid …, after SIGTERM` or `SIGKILL`. One still there after five seconds is left, and settling waits on it as on any other process.

A running process in the worktree still makes settling wait (ADR-0046): it may still be doing work, and leaves on its own. A stopped orphan cannot.

The tests do their part too. Each test world, as it ends, passing or panicking, resumes and kills every process still working under its directory, and each under one. The gate's own check still fails a run whose tests leave a process in `target/` or in the tests' `TMPDIR`.

## Considered options

- **Have the pauser's exit resume what it paused.** Refused as the only answer: a process killed with SIGKILL runs nothing on its way out, and that is how a test or a timeout ends one.
- **End every orphan in the worktree.** Refused: a running orphan may still be finishing a write; ADR-0046 waits on it.

## Consequences

- A worker's worktree whose gate was frozen by a process that then died is removed by the next settle, and the log names what was ended.
- A process a human stopped in a worker's worktree, and whose terminal then went, is ended by settling: settling is removing that worktree.
