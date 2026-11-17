---
status: accepted
---

# Each host keeps its own candidate chain, in its own database; only publication touches the remote

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
`host.db` under the cache directory: **the one afkconvoy database of the
host**, where the host-local state that no other host reads is to go. Its
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
- Kept on the remote: the base branch, claims, heartbeats and gate records.
  Gate records are still reused across hosts (ADR-0037). A host's claims and
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
