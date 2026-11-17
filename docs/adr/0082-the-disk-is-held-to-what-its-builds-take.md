---
status: accepted
---

# The disk is held to what its builds take

Amends ADR-0019 and ADR-0072 (sunfmin/control#1126).

On 2026-11-15 Studio64's 460 GB data volume went from about 95 GB free to 0 twice in twenty minutes, while 23 worktrees ran cargo, rustc or flux at once: workers' own builds, fast gates, candidate turns and a slow gate. Each time ENOSPC broke what was running — landing turns' cleanup, `tell` steps, a fleet instance's tick log, the host runner itself. Admission held new workers back under 1% free (4.9 GB), which is past the point of no return: what is already building is not held back by anything, and the exclusive queue went on granting gates and turns on a 2 GB disk.

## Decision

The disk is held to a **disk floor** measured from the host's builds, not a fixed share: the largest fresh build on record in the worktrees of the checkouts on the list, times what may build at once — the queue's worker, fast and slow lanes, and the worktrees a worker was started in. It is never under the share the host says or its default (1%), and never over a quarter of the disk, so a host whose builds would ask more still builds one at a time as space returns. A host that says its own share (`--admit-disk`) holds to that alone: the operator's word stands, `--admit-disk 0` among it.

The one floor holds both new work and heavy commands. Admission starts no worker under it (ADR-0072). The host runner, which holds the exclusive queue (ADR-0023), reads the disk every ten seconds; under the floor it grants no new command — gate, turn, candidate, cut build — until space returns, and running commands keep their lanes. The queue crate stays free of fleet concepts (ADR-0059): its embedder says the disk is low through `Host::disk`, with the bytes free and the floor, and every waiting request shows a `disk` reason.

Rebuildable space is reclaimed before anything is refused. Under the floor, the host runner reclaims the `target/` of each worktree no one builds in — those whose worker's processes ended first, then those nothing wrote for six hours — and only then holds. A `target/` an exclusive command runs in, or written in the last ten minutes, is never reclaimed. It is set aside into the trash beside the worktree, never inside it where git would see it, and deleted by a detached child (sunfmin/control#987).

A worker's own build outside the queue is bounded by its hot limit on writing, now a share of the disk free *over the floor*: it is hot before the disk reaches the floor, not after it is full.

A disk under its floor, or whose free space falls faster than 2 GiB a minute over five minutes, is an episode — the fall only where, at that rate, it would bring the disk to its floor within the hour (sunfmin/control#1301): `doctor` reports it as a host finding naming the worktrees that build and the sizes of their `target/`.

## Considered options

- **Raise the default share.** Refused: 15% is right for Studio64 today and wrong for a host with a larger disk or smaller builds; the fact is the builds' size, and it is measured where the fleet already records it (the fresh build).
- **Derive the floor from the peak free-space drop of recent episodes.** Not now: it needs a history kept across host runner restarts, and the first episode would come before any is recorded. The fresh build is on record from a worktree's first green gate.
- **Kill running builds when the disk is low.** Refused: it loses work already paid for, and the hold plus the reclaim stop the fall without it.
- **Reclaim only settled worktrees.** Too late: settled worktrees are removed whole already; the space that filled the disk was the `target/` of worktrees still standing.

## Consequences

- `afkconvoy queue`, doctor's `host.exclusive` and the panel show the disk hold (`disk`: `{free, floor}`) and each waiting request's `disk` reason.
- A worker whose `target/` was reclaimed builds cold on its next gate.
- Hosts on several volumes are judged by the volume closest to its floor.
