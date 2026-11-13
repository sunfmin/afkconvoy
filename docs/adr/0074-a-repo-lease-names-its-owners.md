---
status: accepted
---

# A repo lease names its owners, and is free as a lane's place is

Amends ADR-0072 (sunfmin/control#829).

A command's repo lease — the `repo-*` lock under the cache's `worker-slots` that keeps an ordinary command of a repo from another of it, exclusive for one that is no candidate and shared among candidates' gates — is inherited by the command as its lane's place is. A server shared by the host that the command's build starts, as sccache's server, inherits it too and outlives the command. ADR-0072 frees a lane's place only such a server holds by its owner written in it; a repo lease had no owner written in it, so a shared server holding one kept every later command of its repo outside the fast lane waiting, as `same_repo`, for as long as the server lived.

## Decision

A repo lease names its owners in it, and is free on the terms of ADR-0072:

- **Its owners**: the one that takes it where nothing holds it writes itself alone; a candidate that shares one still held writes itself beside each that took it since it was last free, ended or not. An owner that ended is not dropped while the lease is held: its own work may be what still holds it. One takes and writes it under the lock of the slots' directory.
- **Free**: once every owner ended and every process holding its file is a server shared by the host — handed to pid 1, working outside the worktree of each owner — or under one. The next command of the repo, or the queue asking whether one can be granted, takes the file off its path under the same lock, only if the path still names the file it read and the file still names the owners it judged, and takes a new one.
- **Held**: while an owner runs, while the work of one — a test, a build, a gate shell in its worktree — holds the file, where the holders cannot be listed, and where the file names no owner, as one taken before its owners were written in it.

## Considered options

- **Judge by the holders alone.** Refused: a command run by a process handed to pid 1 outside its worktree — a host runner's own — would be taken for a shared server while it runs.
- **Drop owners that ended as a candidate joins.** Refused: an ended owner's own work in its worktree would then be judged against the other owners' worktrees alone, and taken for a shared server's.

## Consequences

- A command that started a shared server frees its repo as it ends; a candidate's gate that did frees it once every candidate sharing it ended.
- A shared lease held without a break by one candidate after another names each of them until it is free again.
