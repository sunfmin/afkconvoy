---
status: accepted
---

# The host's state is in its database, in the data directory, backed up daily

Amends ADR-0117 (`host.db` under the cache directory). Decided by Felix,
control session 2026-11-17.

ADR-0117 put the candidate chain into `host.db` and said the rest of the
host-local state would follow. That state was loose JSON files in the cache
directory, each with its own lock or none: the checkout list and host
settings, the paused harnesses, worker health, the account book, the patrol
runs' usage, the watch's and the asking session's state, the outboxes, the
trash directories; and, in each repo's git dir, the landing hold and the
fresh looks. A cache is what can be thrown away; these cannot.

## The decision

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
two ticks (ADR-0032), while workers already call the new binary. So the
rename waits: while the host runner's lock is held by an older release, the
file stays where that runner reads it, and a change it writes there is
taken in again; once a runner of this release, or none, holds the host, the
file is moved aside. An old `host.db` in the cache directory is copied by
SQLite's backup into a file beside the new one and linked into place only
where none came meanwhile; turns of the older runner go on with the old
database until it moves, which at worst builds a round twice — ADR-0117
accepts that.

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
