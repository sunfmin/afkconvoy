---
status: accepted
---

# A store is packed only while the queue holds its repository whole

Amends ADR-0079 (sunfmin/control#1101).

ADR-0079 turned git's own gc and maintenance off a fleet's checkout, and has
each tick run git's auto-gc under the fetch lock. The fetch lock keeps a
repack off a fetch, but not off the gates, turns and lands of the
repository's worktrees, which read the same packs as they merge, check out
and build: a repack deletes the packs it replaced under them.

A tick packs the store only once it needs it: it holds more packs than its
`gc.autoPackLimit` (50 where it says none; 0 packs none for its packs alone)
or more loose objects than git's auto-gc lets be. It takes the fetch lock
only where no fetch holds it. Then it asks the host runner to hold the
repository whole for it: a ticket of the exclusive queue that no other
ticket of its repository runs beside, in any lane, granted only at once and
only where none of the repository runs, waits or holds a lease — never one
that waits for its turn. One that finds a fetch, a gate, a turn or a land
leaves the packing to a later tick; so does one with no host runner to ask,
for nothing then keeps the repository's commands off it. While it is held,
`git gc --auto` packs the store, never detached, and writes its commit-graph
again; a gate or turn of the repository that asks meanwhile waits for it, as
for any ticket of its lane.
