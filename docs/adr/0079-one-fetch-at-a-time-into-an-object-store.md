---
status: accepted
---

# One fetch at a time into an object store

sunfmin/control#1102.

On Studio64 the guild fleet landed nothing for an hour and a half. Every
`land` failed at its first step, fetching `refs/convoy/candidates/main`:
`fatal: pack has 4 unresolved deltas`. Each worker asked for a turn again
every 10–40 seconds, and each retry fetched again: sixteen fetches at once
into the one `.git` the checkout's worktrees share, beside a cruft repack and
a geometric repack of git's own maintenance on the same store. One fetch by
hand succeeded in 1.3 seconds.

## Fetches take the fetch lock

Every fetch the fleet makes into a repository takes a lock in its git common
dir, held for the fetch alone. A land that brings a commit takes it, then
looks again whether the commit is there: one fetched while it waited is not
fetched again. Eight lands that want the chain's tip at once make one fetch.

## A corrupt fetch is refetched once, then backs off

A fetch whose pack git could not take in — unresolved deltas, an index-pack
failure, a bad object — is made again once with `--refetch`, which asks for
everything anew. One that fails so again is noted in the git dir with its
ref, its error, when the failures began and how many there were. Until its
backoff ends — a minute, doubled with each failure after, at most half an
hour — no land or read that brings a commit into that store fetches; each
says what it waits for. The
next fetch that succeeds clears the note. While it stands, `doctor` reports
`a fetch of the remote keeps failing corrupt`, whose fix is the fetch by
hand with `--refetch`.

## The fleet does git's housekeeping itself

A start sets `maintenance.auto false` and `gc.auto 0` in its checkout, and
disables the scheduled maintenance tasks that repack. Each tick runs git's
auto-gc under the fetch lock, only where no fetch holds it, never detached,
so no repack rewrites the packs a fetch reads.
