---
status: accepted
---

# A turn lands its PR on the base branch

A turn makes the PR's merge commit on the current base tip, gates that commit, records green, and pushes that same commit to the base branch as a fast-forward. This replaces the landing train of ADR-0004 and the joining step of ADR-0034. The train added no verification after a recorded turn, but needed another worker and a separate lane; the measurements in ADR-0034 show the cost of running those gates together.

Red, a conflict, or a rejected push ends the turn without landing. A conflict stays in the worker's worktree. A rejected push requires another turn, which reads the new base tip. Only worker lanes remain, with one turn per repo however many lanes the host allows.

An unrecorded base tip is gated alone in a separate checkout, using its own gate declaration. Ordinary turns wait outside the worker lanes while that tip is red. The fleet opens a dispatchable fleet-lock issue for its repair, under an exclusive claim on opening it. An operator worker makes the repair outside product workflows, so even a workflow that begins with a human stage cannot hold it back. Only the fleet-lock repair issue for that exact red tip may take a turn on it, and it must still gate and record its complete merge commit before landing. The fleet's repair label and ready label make it dispatchable through the ordinary issue and claim protocol, with no human stage. The guard reserves the repair label to the fleet.

An older train line is migration input only. A green tip can fast-forward the base with its existing record; otherwise each PR takes a turn. No new line is written, and the old line is removed only when its work has been accounted for. Inspections and cuts use the same turn as issue workers.

## Evidence before removing the train

On FelixMBP14.local, 0.5.21 is the first released version holding sunfmin/control#110. The host runner log records installation at 2026-10-26 11:55:51 UTC and its start on 0.5.21 at 11:56:40 UTC (pid 63495). The tick log read was `~/.cache/afkconvoy/runs/afkconvoy-6f54123d7451f999.jsonl`.

Through 11:58:18 UTC, that version has run **zero train-lane gates and zero train-worker repairs**. Its first worker turns (#80 and #95) were waiting in the worker lane. Earlier train landings at 11:44:22 and 11:51:08 UTC ran on 0.5.20, before the runner moved; they are not observations of #110. This is a short observation window, not evidence that repair cannot be needed. The missing-record check and the fleet-lock repair above cover both reasons the train could still need a gate: an outside change of its tip, or an outside red base tip.

Legacy recovery is resumable through issue-linked Orca workspaces. Conflicts and red gates retain those workspaces for a worker; retries reuse them. Migration returns on a red base instead of blocking the tick that must dispatch its repair. A final recovery PR preserves the original line's history, including a closed or rewritten PR head and commits an older train worker made.
