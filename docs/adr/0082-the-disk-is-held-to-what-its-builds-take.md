---
status: accepted
---

# The disk is held to what its builds take, and a repo's builds take their build directory whole

Many worktrees building at once can take a host's disk from plenty free to none in minutes, and ENOSPC then breaks whatever runs — landing turns' cleanup, `tell` steps, a tick log, the host runner itself. A fixed small share free is past the point of no return: what is already building is held back by nothing. And the worktrees of one repo build into one shared build directory, where cargo's locks can deadlock.

## The disk floor

The disk is held to a **disk floor** measured from the host's builds, not a fixed share: the largest fresh build on record in the worktrees of the checkouts on the list, times what may build at once — the queue's worker, fast and slow lanes, and the worktrees a worker was started in. It is never under the share the host says or its default (1%), and never over a quarter of the disk, so a host whose builds would ask more still builds one at a time as space returns. A host that says its own share (`--admit-disk`) holds to that alone: the operator's word stands, `--admit-disk 0` among it. A host on several volumes is judged by the volume closest to its floor.

The one floor holds both new work and heavy commands. Admission starts no worker under it (ADR-0107). The host runner, which holds the exclusive queue (ADR-0023), reads the disk every ten seconds; under the floor it grants no new command — gate, turn, candidate, cut build — until space returns, and running commands keep their lanes. The queue crate stays free of fleet concepts (ADR-0058): its embedder says the disk is low through `Host::disk`, with the bytes free and the floor, and every waiting request shows a `disk` reason.

Rebuildable space is reclaimed before anything is refused. Under the floor, the host runner reclaims the `target/` of each worktree no one builds in — those whose worker's processes ended first, then those nothing wrote for six hours — and only then holds. A `target/` an exclusive command runs in, or written in the last ten minutes, is never reclaimed. It is set aside into the trash beside the worktree, never inside it where git would see it, and deleted by a detached child. A worker whose `target/` was reclaimed builds cold on its next gate.

A worker's own build outside the queue is bounded by its hot limit on writing, a share of the disk free over the floor (ADR-0019).

A disk under its floor, or whose free space falls fast enough to reach its floor within the hour, is an episode: `doctor` reports it as a host finding naming the worktrees that build and the sizes of their `target/`. `afkconvoy queue`, doctor's `host.exclusive` and the panel show the disk hold (`disk`: `{free, floor}`).

## A build takes its build directory whole

The host's cargo is the flux toolchain's fork, which builds every worktree of a repo into one build directory of its own choosing and turns on upstream's unstable fine-grain locking by default. Under it a cargo takes a shared lock on every unit it plans to use and keeps it for its whole run, and waits with no bound for the exclusive lock of a unit it judged dirty: two cargos each holding shared a unit the other waits on wait forever, at no CPU.

So every worker's terminal exports, beside its `TMPDIR`, cargo's opt-out of fine-grain locking (`__CARGO_TEMPORARY_FINE_GRAIN_LOCKING_OPT_OUT=1`), and the gate's command is started with it; one constant says it (`afkconvoy_gate::builds::SERIAL_BUILDS`). A build then takes its build directory's one lock, exclusive, and its artifact directory's after it, in that order: there is nothing to cycle on. A cargo of another program that keeps fine-grain locking holds the build directory's lock shared, so it and one of ours exclude each other whole.

It costs no disk: the worktrees of a repo still share one build directory, and what was built there stands. It costs build time: the compiles of one repo's worktrees run one at a time. Only compiling holds the lock, so a test running lets another build finish meanwhile, and workers mostly compile little at a time on a warm directory. Where the opt-out does not reach, the hot-worker sampler finds the deadlock as a stalled worker (ADR-0019).

## Considered options

- **Raise the default share.** Refused: a share right for one host is wrong for a host with a larger disk or smaller builds; the fact is the builds' size, and it is measured where the fleet already records it (the fresh build).
- **Derive the floor from the peak free-space drop of recent episodes.** Not now: it needs a history kept across host runner restarts, and the first episode would come before any is recorded. The fresh build is on record from a worktree's first green gate.
- **Kill running builds when the disk is low.** Refused: it loses work already paid for, and the hold plus the reclaim stop the fall without it.
- **Reclaim only settled worktrees.** Too late: settled worktrees are removed whole already; the space that fills the disk is the `target/` of worktrees still standing.
- **A build directory per worktree**, set by the worker's environment as `TMPDIR` is. No lock is shared and builds run side by side, but each worktree builds everything once more: as much disk again per worker, which the floor then has to hold, and a cold first build for every worker. Taken where serial compiles prove too slow.
- **Waiting for the fork's fix.** A cargo should release the shared units it no longer needs while it waits for an exclusive one. It is the fork's to fix; the fleet cannot wait on it, and a fixed fork changes nothing here but what serial builds cost.
