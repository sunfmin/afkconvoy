---
status: accepted
---

# The fleet owns its object store: one fetch at a time, and its own housekeeping

Every worktree of a checkout shares one `.git`: the workers landing at once, the fleet's own reads, the candidates' gates. Many fetches at once into that store, beside git's own background repacks, leave packs git cannot take in, and then every land fails at its first fetch, and every retry fetches again. So the fleet, not git, decides when the store is written.

## One fetch at a time

Every fetch the fleet makes into a repository takes a lock in its git common dir, held for the fetch alone. A land that wants a commit takes it, then looks again whether the commit is there: one fetched while it waited is not fetched again, so many lands that want the same tip make one fetch.

## A corrupt fetch is refetched once, then backs off

A fetch git could not take in (unresolved deltas, an index-pack failure, a bad object, a commit-graph naming a commit the store lacks, a multi-pack-index naming a missing pack) is made again once with `--refetch`, which asks for everything anew. Before it, the store's commit-graph and multi-pack-index are dropped: a repack cut short can leave them naming what is gone, and every read then trusts them. Both only speed up reads; git reads each pack's own index without them, and housekeeping writes them again. No ref is deleted.

A refetch that fails so again is noted in the git dir with its ref, its error, when the failures began and how many there were. Until its backoff ends, which grows with each failure up to a bound, no land or read that brings a commit into that store fetches, and each says what it waits for. The next fetch that succeeds clears the note. While it stands, `doctor` reports `a fetch of the remote keeps failing corrupt`, whose fix is that fetch by hand with `--refetch`.

## The fleet does git's housekeeping itself

A start sets `maintenance.auto false` and `gc.auto 0` in its checkout and disables git's scheduled maintenance tasks that repack. A tick packs the store itself, only once it needs it: more packs than its `gc.autoPackLimit` (git's default where it says none; 0 packs none for its packs alone), or more loose objects than git's auto-gc lets be. It then takes the fetch lock only where no fetch holds it, and asks the host runner to hold the repository whole: an exclusive-queue ticket that no other ticket of the repository runs beside, in any lane, granted only at once and only where none of the repository runs, waits or holds a lease. While it is held, `git gc --auto` packs the store, never detached, and writes its commit-graph again; a gate or turn of the repository that asks meanwhile waits for it. A tick that finds a fetch, a gate, a turn or a land, or no host runner to ask, leaves the packing to a later tick.

The fetch lock alone is not enough: the gates, turns and lands of the repository's worktrees read the same packs as they merge, check out and build, and a repack deletes the packs it replaced under them.
