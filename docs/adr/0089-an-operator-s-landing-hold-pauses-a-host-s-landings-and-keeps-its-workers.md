---
status: accepted
---

# An operator's landing hold pauses a host's landings and keeps its workers

Amends ADR-0003 and ADR-0011 (sunfmin/control#1258).

On 2026-11-15 the operator batch-landed 28 guild PRs as one PR
(sunfmin/guild#311). `afkconvoy stop` stopped the fleet instance, but its 32
worker sessions went on running `afkconvoy land`, gates and cargo builds
against the shared build directory, which held #311's gate up; the only way to
stop them was SIGSTOP on each harness process. Nothing the fleet had paused a
worker and kept its work:

- `stop` stops the fleet instance only (ADR-0003): it does not drain, and its
  running workers go on, landing as before.
- `stop --release` ends the workers and releases their claims: their work is
  left for another start to take up again.
- A fleet lock (ADR-0011) is on GitHub, for every host: it changes what the
  fleet dispatches and which PR a turn lands, but a worker already running
  still gates and builds.

## The hold

`afkconvoy hold` raises a **landing hold** on the repo on this host:
one note in the repo's git dir (`convoy-landing-hold`), which every worktree
of the repo on the host shares, so a worker's own commands read the hold the
fleet's tick reads, with no GitHub read and no process signalled. It may name
one issue it lets through (`--issue`), the batch's turn; a watch's
`land --watch` is always let through. `hold --lift` removes the note.

While it stands, for every issue but the one it lets through:

- `afkconvoy land --issue` answers `held`, naming the hold, before anything
  is read or merged; a turn already waiting on its chain ends `held` at its
  next wait, as one does at a fleet lock.
- `afkconvoy gate` run by a worker (`AFKCONVOY_ISSUE` set) is refused with an
  error naming the hold. A gate run by hand, or by the watch, still runs.
- The tick leaves the claim alone: it asks for no turn and judges or nudges
  no stopped worker. Claims, worktrees, branches and unpushed work are kept.

Lifting it changes nothing else: the next tick asks again for every held turn
— `held` is a waiting ending — and judges the stopped workers as it would any,
so they go on with no restart.

`plan` answers `landing_hold` while one stands, and `doctor` says it under
`fleet` and as the info finding `a landing hold stands`, in place of
`its fleet lands nothing`: a hold stands to land nothing, so what it holds is
no stall. The watch acts on no such finding; it is lifted by whoever raised it.

## Considered

- **SIGSTOP the workers** — what the operator did by hand. It freezes a
  process mid-command, holding its locks and its place in the exclusive
  queue, and a harness frozen past its API timeouts loses its turn.
- **A hold on GitHub, as a lock is** — it would hold every host's fleet of
  the repo, and every worker's `gate` would read GitHub before it ran. A batch
  lands from one host, and the build directory it contends for is that host's.
- **Make the hold wait instead of refuse** — a worker waiting inside `gate`
  holds its harness turn open with no bound; refusing ends the turn, and the
  fleet already knows how to take a stopped worker up again.
