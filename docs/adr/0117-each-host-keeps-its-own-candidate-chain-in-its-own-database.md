---
status: accepted
---

# Each host keeps its own candidate chain and its state in its own database; only publication touches the remote

Correctness never depended on a candidate chain shared on the remote:
publication is one atomic receive-pack transaction that requires the base
branch to be exactly the expected predecessor (ADR-0037), so two hosts can
never land an untested commit. What a shared chain buys is that hosts do not
collide: no gate run wasted on a base another host moved, one round across
hosts, and cancelling a successor on another host. Its price is that every
waiting turn reads the chain from the remote, again and again. The operator
decided that each host is independent and lands its own, and that runs wasted
between hosts are fine.

## The chain is the host's

The candidate chain, its rounds and its groups belong to the host that builds
them, one per repo and target branch. They are rows of the host's database,
`host.db` in its data directory: **the one afkconvoy database of the
host**, which holds the host-local state no other host reads (below). Its
schema has a version (`PRAGMA user_version`); each version is one entry of
its migrations, applied once, in order, in one transaction, by the first open
that finds the database behind; a database newer than the afkconvoy that
opens it is refused.

A chain's write names the revision it was made on and takes only where the
row is still at it, inside one immediate transaction: a turn whose edit was
made on a chain another turn of the host moved reads it again and edits
again. There is no lock file: the transaction is the lock. SQLite rather than
a file under a lock because the host's other state moves into the same
database, and because a reader starts no process and needs no lock.

- A host's round holds only the PRs whose turns ran on that host (ADR-0083).
  It is built on the base tip read live when the candidate is built, and its
  root is published by one atomic push of the base and every source head. If
  another host moved the base meanwhile, the push is refused, and the round is
  built again on the new tip and gated again. That wasted run is accepted.
- Publication holds the chain's transaction while it pushes, so an
  invalidation by a turn of this host and a publication cannot both win; where
  the push took, the chain is written landed in the same transaction. A turn
  that dies between the push and the write leaves a candidate whose
  predecessor is not the base: the next look cancels it `base_moved`, and its
  PR's next turn finds the PR on the base and answers `merged`.
- Waiting turns read the database, which starts no process and asks no
  remote. A write is seen by every waiting turn of the host at once. The remote
  is asked for the base tip when a turn builds a candidate, and when a green
  candidate that the chain says stands on what landed before it is to publish.
  A turn waiting on the chain does not read the PR heads either: a gate that is
  running does (it stops at once where its PR's head moved), and a publication
  is refused by the remote where a head moved.
- The commits of the chain are kept reachable by the local ref
  `refs/convoy/held/<target>`, whose commit has them as parents. A stopped
  green candidate has no worktree, and a later turn of its PR takes it over
  from its commit.
- Kept on the remote: the base branch, claims and heartbeats. Gate records
  are the host's too, rows of its database (ADR-0037, sunfmin/control#1326):
  a tip another host landed is certified again here. A host's claims and
  their lease are unchanged, so an issue is still worked by one host.
- A host runner restarted on the same host finds its database, as it finds
  its claims: a turn that finds its candidate there finishes it or builds it
  again, and does not gate a green candidate again where its run is on record.

`doctor`, `plan` and the panel read the chains of the host's database.

## The landing crate

The landing crate (ADR-0058) says what one publication updates where the
chain is not a ref of the remote — every member's source head kept, and the
target moved — and, for a program of another repo that still keeps its chain
on the remote, the same with the chain's record. afkconvoy does not call the
latter.

## The host's state

The host's other state was loose JSON files in the cache directory, each with
its own lock or none: the checkout list and host settings, the paused
harnesses, worker health, the account book, the patrol runs' usage, the
watch's and the asking session's state, the outbox of human decisions, the trash
directories; and, in each repo's git dir, the landing hold and the fresh
looks. A cache is what can be thrown away; these cannot (decided by Felix,
control session 2026-11-17).

- **The data directory** is `$XDG_DATA_HOME/afkconvoy`, else
  `$HOME/.local/share/afkconvoy` (`copies::data`, the one rule; the local
  tracker's `issues.db` of ADR-0091 is there too). The host runner pins
  XDG_DATA_HOME for what it starts, as it pins XDG_CACHE_HOME.
- **`host.db` is in it**, with every store above as tables of its own. A
  change of a store is one immediate transaction: the transaction replaces
  the store's file lock. Per-repo state is keyed by the repo's canonical
  common git dir, as the chains are.
- **Leases stay kernel locks.** Where a lock was held across work that
  starts processes or asks Orca — worker health's observation
  (`worker-health.lock`), a watch's look or check (`watch/state.lock`), an
  asking step (`asking/state.lock`) — it stays a lock in the cache
  directory, as the worker slots are: no writer may hold a transaction of
  the host's database across that work, for every other writer of the host
  would wait on it. Each read and write under such a lease is a transaction
  of its own. A write begun inside another on the same thread is refused.
- **Files stay files** where they are documents or leases: briefs,
  transcripts, reports and delivery records, evidence, logs, copies,
  worktrees, harness homes, quota observations, the host runner's lock,
  socket and log, and every `convoy-*` note in a worker's worktree.
- **A daily backup.** The host runner writes an online copy (SQLite's
  backup) of `host.db`, and of `issues.db` where there is one, to
  `backups/<YYYY-MM-DD>/` once a day, keeping the newest 14 days. A failure
  is `doctor`'s finding, never a crash.

## Moving in

An older release's files converge into `host.db` as each store is read: the
file is taken in, in the transaction that records its digest, and renamed
`<name>.migrated`, kept. A file already taken in is not taken in again.

During an upgrade the host runner of the older release keeps running, and
reading and writing those files, until it moves onto the new one between
two ticks (ADR-0023), while workers already call the new binary. So the
rename waits: while the host runner's lock is held by an older release, the
file stays where that runner reads it, and a change it writes there is
taken in again; once a runner of this release, or none, holds the host, the
file is moved aside. An old `host.db` in the cache directory is copied by
SQLite's backup into a file beside the new one and linked into place only
where none came meanwhile; turns of the older runner go on with the old
database until it moves, which at worst builds a round twice, as above.

The checkout list of an older release meant, by an entry that says no
tracker, its checkout's own repo. Taken in, such an entry names that repo,
so that it keeps its meaning once no tracker means the host's local tracker
(ADR-0091); one whose repo cannot be read says none still, and `doctor`
names it.

## Consequences

- The cache directory can be removed without losing what the host knows.
- A store is read and changed by every process of the host through one
  seam (`host_db::Store`), and a test world keeps a store through it.
- A process of the older release that still runs after its host runner has
  moved reads no list: such a process is a command started before the
  upgrade, and its next start is of the new release.
