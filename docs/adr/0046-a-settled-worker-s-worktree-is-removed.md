---
status: accepted
---

# A settled worker's worktree is removed, unless it holds what the remote does not

Settling a claim removes that worker's worktree. The removal does not trust Orca's answer alone: the gate leaves read-only copies there, so removal first makes writable only what it has to delete, and a directory still there after a reported success is removed, or finished by a later tick.

The worker the fleet started for the issue is the fleet's own session and keeps nothing: settling ends it — its terminals closed and its process tree confirmed gone — before the removal. A worker still working keeps the worktree, and so does any other process in it but a stopped orphan, which settling ends (ADR-0019): the worktree stays, and the claim with it, until a later tick sees that process gone.

A worktree that holds a commit the remote does not is kept, and the claim is released. A commit is on the remote when the base branch, its own branch or the branch of the PR that carries it (ADR-0083) holds it, or when the base branch already holds its whole change — a carrier or a round landed it under commits of its own. The settle notes in the worktree's git directory that it was kept, and why; each later tick looks again, while no claim holds its issue, and removes it once it holds nothing the remote does not. `doctor` names each kept one under `kept_at_settle`, with `kept_because`. Dirt the gate left, with every commit already on the remote, keeps nothing. Settling removes only the worker's worktree, not an issue's, watch's or cut's; a stopped candidate has its own removal (ADR-0037).

A worktree's files are not deleted on the thread that removes it: a Rust worktree's `target/` is gigabytes, and deleting it would hold the host runner's tick. The removal moves the worktree into a trash directory beside it, on the same filesystem, so the path is free at once, and a detached child under `nice` deletes the trash. What a host that stopped mid-delete leaves in a trash is deleted when the host runner next starts.
