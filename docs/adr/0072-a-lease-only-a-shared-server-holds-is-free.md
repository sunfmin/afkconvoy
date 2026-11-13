---
status: accepted
---

# A lease only a server shared by the host holds is free

Amends ADR-0037 and ADR-0049 (sunfmin/control#822).

A command's lane is held by a kernel lease, a locked slot file whose descriptor the command it runs inherits, so that a dead owner's still-running descendants are not replaced before confirmed cleanup (ADR-0037). But every process under the command inherits it, and what a gate's build starts may outlive the gate. sccache's server is started by the first build to reach it after it went idle; it is handed to pid 1, works in `/` for every worktree of the host, and keeps every descriptor it inherited. With builds reaching it all day it never idles out, and the lease it took with it stays locked long after the gate that took it ended. On one host both places of the fast lane were held so by gates gone for an hour or two, across a host runner restart, and every later fast gate timed out waiting for a place.

## Decision

A lease whose owner ended is held while the gate's own work holds it, and is free once only servers shared by the host do:

- **Its owner** is the process that took it, by its identity (host, pid and start time). While it runs, its lease is held, whatever else holds the file.
- **A server shared by the host** is a process handed to pid 1 that works outside the worktree the lease was taken in — the test that tells a worker's left-behind processes from the host's servers already (the hot-worker sampler's) — or a process under one, as the compilers sccache's server starts are.
- **Free**: once its owner ended, every process holding its file is such a server. It is not counted in the queue, and the next to take a place there takes the file off its path, under the lock of the slots' directory and only if the path still names the file it read, and makes a new one; the server keeps its lock on a file nothing opens. A candidate's dead owner is cleaned up first, as before.

A test, build or gate shell left behind by a killed owner works in its worktree, or under what does: it still holds the lane until it ends. What cannot be looked at — the processes holding the file cannot be listed — is held.

## Considered options

- **Free every lease whose owner ended.** Refused: a gate's tests and builds go on after their owner was killed, and ADR-0037 keeps them from being replaced before they end.
- **Not let the descriptor be inherited.** Refused: the inherited lock is how a dead owner's descendants are seen at all, and how a candidate whose turn died is found and cleaned up.
- **End the server.** Refused: it is the host's, serving every worktree's build; ending it slows every other gate and frees nothing a gate's own work holds.

## Consequences

- A gate that started a shared server frees its lane as it ends, and a host runner restarted finds no holder in it.
- Looking at a dead owner's lease reads the process table and lists what holds the file; a look stands two seconds, as a waiting turn reads the leases every 100 ms.
- A repo's lease, the lock that keeps an ordinary command of a repo from another of it, is not a lane's place and has no owner written in it: a shared server holding one is not freed by this.

The same issue settles a second thing ADR-0049 left open: `gate --fast` and `gate --slow` take their lane before their command starts, as `gate` takes its turn, so the wait for a place uses none of the gate's limit, which times the command alone.
