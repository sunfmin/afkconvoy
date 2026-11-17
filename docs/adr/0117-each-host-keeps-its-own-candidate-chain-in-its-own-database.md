---
status: accepted
---

# Each host keeps its own candidate chain, in its own database; only publication touches the remote

sunfmin/control#1321. Amends ADR-0037 (the chain on the remote, cancelling
another host's candidates), ADR-0083 (a round across hosts) and ADR-0111 (the
shared chain reading of #1321 is gone, its lock reading kept). Linked from
ADR-0060, since the landing crate changes.

## The decision

Felix, in the control session on 2026-11-17, about 11:18 JST:

> 我觉着每台机器互不相干，自己land 自己的白跑什么的没关系。

(Each host is independent and lands its own. Wasted runs between hosts are
fine.) And about 11:35 JST, of where it is kept:

> 本地的那些配置项也都放到 SQLite 数据库里面。不放到文件里了。

(Host-local settings and state go into an SQLite database, not into files.)

The question before the first was why the chain is read from the remote when
one host lands into it. Correctness never depended on it: publication is one
atomic receive-pack transaction that requires the base branch to be exactly
the expected predecessor (ADR-0037), so two hosts can never land an untested
commit. What the shared chain bought is that hosts do not collide: no gate run
wasted on a base another host moved, one round across hosts, and cancelling a
successor on another host. That is given up. On 2026-11-17 the only live
heartbeats of afkconvoy and guild were one host's, and 9 waiting turns read
`refs/convoy/candidates/main` from GitHub about once a second each: 43% of
the host's 2,500 to 3,000 process starts a second were git.

## The chain is the host's

The candidate chain, its rounds and its groups belong to the host that builds
them, one per repo and target branch. They are rows of the host's database,
`host.db` under the cache directory: **the one afkconvoy database of the
host** (`host_db`), where the host-local state that no other host reads is
to go, one issue at a time (the checkout list, host settings, the kit, worker
health, slots, quota notes, gate records). Its schema has a version
(`PRAGMA user_version`); each version is one entry of `MIGRATIONS`, applied
once, in order, in one transaction, by the first open that finds the database
behind; a database newer than the afkconvoy that opens it is refused.

The chain is the table `chains(repo, branch, revision, chain)`. A write names
the revision it was made on and takes only where the row is still at it,
inside one immediate transaction, as the remote record's did: a turn whose edit
was made on a chain another turn of the host moved reads it again and edits
again. There is no lock file: the transaction is the lock. SQLite rather than
a file under a lock because the host's other state moves into the same
database, and because a reader starts no process and needs no lock.

- A host's round holds only the PRs whose turns ran on that host. It is built
  on the base tip read live when the candidate is built, and its root is
  published by the same atomic push as before, of the base and every source
  head, with no chain record in it. If another host moved the base meanwhile,
  the push is refused, the round is built again on the new tip and gated again.
  That wasted run is accepted.
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
  `refs/convoy/held/<target>`, whose commit has them as parents, as the remote
  record's did. A stopped green candidate has no worktree, and a later turn of
  its PR takes it over from its commit.
- Kept on the remote, as before: the base branch, claims, heartbeats and gate
  records. Gate records are still reused across hosts (ADR-0069).
- Removed: the remote chain ref, its reads and writes, cancelling another
  host's candidates through it, and the 30-minute lease by which another host
  is judged dead *for the chain*. A host's claims and their lease are
  unchanged, so an issue is still worked by one host.
- A host runner restarted on the same host finds its database, as it finds
  its claims: a turn that finds its candidate there finishes it or builds it
  again, and does not gate a green candidate again where its run is on record
  (ADR-0069).

## The old remote record

An afkconvoy before this one wrote `refs/convoy/candidates/<target>`. This
one ignores it. The tick deletes it, leased at the sha it read, once no turn
of an older afkconvoy on this host holds a place on it: no live process of
this host owns a candidate, barrier, pending turn or round ask on it.
`doctor` says whether one is left (`LegacyChain`), and for each whether a live
turn of this host still holds a place on it. A turn of an older afkconvoy on
another host may write it again; that host's own tick does not remove it
either, and its writes are refused for what they were made on. Nothing here
reads it.

## What a look shows

`doctor`, `plan` and the panel read the chains of the host's database and show
the fields they showed. `landing`, `pending_lands` and the delivery handoff
read them there too.

## The landing crate

`turn::publication` is what one publication updates where the chain is not a
ref of the remote: every member's source head kept, and the target moved.
`turn::updates` is it with the chain's record, for a program of another
repo that still keeps its chain on the remote (`land`, ADR-0060); afkconvoy
does not call it.
