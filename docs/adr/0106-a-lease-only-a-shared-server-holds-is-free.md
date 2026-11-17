---
status: accepted
---

# A lease only a server shared by the host holds is free, and a repo lease names its owners

A command of the exclusive queue holds its lane's place, and its repo, by kernel leases: locked files whose descriptors the command inherits, so that a dead owner's still-running descendants are not replaced before confirmed cleanup (ADR-0037). But every process under the command inherits them, and what a build starts may outlive it. sccache's server is started by the first build to reach it after it went idle; it is handed to pid 1, works in `/` for every worktree of the host, keeps every descriptor it inherited, and with builds reaching it all day never idles out. Judged by its holders alone, a lease it took along stays held for as long as it lives, and every later command waiting for that lane or that repo times out.

## Decision

A lease whose owners ended is held while their own work holds it, and is free once only servers shared by the host do.

- **A lane's place** names its owner, the process that took it, by its identity (host, pid and start time).
- **A repo lease** — the `repo-*` lock under the cache's `worker-slots` that keeps an ordinary command of a repo from another of it, exclusive for one that is no candidate and shared among candidates' gates — names its owners: the one that takes it where nothing holds it writes itself alone; a candidate that shares one still held writes itself beside each that took it since it was last free, ended or not. An owner that ended is not dropped while the lease is held: its own work may be what still holds it. Owners are written under the lock of the slots' directory.
- **A server shared by the host** is a process handed to pid 1 that works outside the worktree of each owner — the hot-worker sampler's test (ADR-0019) — or a process under one, as the compilers sccache's server starts are.
- **Free**: once every owner ended and every process holding the file is such a server. It is not counted, and the next to take it — or the queue asking whether a command can be granted — takes the file off its path, under the lock of the slots' directory and only if the path still names the file it read and the file still names the owners it judged, and makes a new one; the server keeps its lock on a file nothing opens. A candidate's dead owner is cleaned up first, as before.
- **Held**: while an owner runs; while its work — a test, a build, a gate shell in its worktree — holds the file; where the holders cannot be listed; and where the file names no owner.

A gate that started a shared server frees its lane and its repo as it ends; a candidate's gate that did frees its repo once every candidate sharing it ended. A host runner restarted finds no holder in either.

`gate --fast` and `gate --slow` take their lane before their command starts, as `gate` takes its turn, so the wait for a place uses none of the gate's limit, which times the command alone.

## Considered options

- **Free every lease whose owner ended.** Refused: a gate's tests and builds go on after their owner was killed, and ADR-0037 keeps them from being replaced before they end.
- **Judge by the holders alone.** Refused: a command run by a process handed to pid 1 outside its worktree — a host runner's own — would be taken for a shared server while it runs.
- **Drop owners that ended as a candidate joins.** Refused: an ended owner's own work in its worktree would then be judged against the other owners' worktrees alone, and taken for a shared server's.
- **Not let the descriptor be inherited.** Refused: the inherited lock is how a dead owner's descendants are seen at all, and how a candidate whose turn died is found and cleaned up.
- **End the server.** Refused: it is the host's, serving every worktree's build; ending it slows every other gate and frees nothing a gate's own work holds.
