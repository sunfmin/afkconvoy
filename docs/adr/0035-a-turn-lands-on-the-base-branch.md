---
status: accepted
---

# A turn lands its PR on the base branch

A turn makes the PR's merge commit on the current base tip, gates that commit, records green, and pushes that same commit to the base branch as a fast-forward. This replaces the landing train of ADR-0004 and the joining step of ADR-0034. The train added no verification after a recorded turn, but needed another worker and a separate lane; the measurements in ADR-0034 show the cost of running those gates together.

Red, a conflict, or a rejected push ends the turn without landing. A conflict stays in the worker's worktree. A rejected push requires another turn, which reads the new base tip. Only worker lanes remain, with one turn per repo however many lanes the host allows.

An unrecorded base tip is gated alone in a separate checkout, using its own gate declaration. Ordinary turns wait outside the worker lanes while that tip is red. The fleet opens a dispatchable fleet-lock issue for its repair, under an exclusive claim on opening it. An operator worker makes the repair outside product workflows, so even a workflow that begins with a human stage cannot hold it back. Only the fleet-lock repair issue for that exact red tip may take a turn on it, and it must still gate and record its complete merge commit before landing. The fleet's repair label and ready label make it dispatchable through the ordinary issue and claim protocol, with no human stage. The guard reserves the repair label to the fleet. A standing lock raised by an older release also remains eligible to land its repair without waiting for a green tip.

An older train line is migration input only. A green tip can fast-forward the base with its existing record; otherwise each PR takes a turn. No new line is written, and the old line is removed only when its work has been accounted for. Inspections and cuts use the same turn as issue workers.

## Evidence before removing the train

On FelixMBP14.local, 0.5.21 is the first released version holding sunfmin/control#110. The host runner log records installation at 2026-10-26 11:55:51 UTC and its start on 0.5.21 at 11:56:40 UTC (pid 63495). The tick log read was `~/.cache/afkconvoy/runs/afkconvoy-6f54123d7451f999.jsonl`.

The initial afkconvoy-repo read through 11:58:18 UTC found no train-lane gates or repairs there. A host-wide audit of every `runs/*.jsonl` from that runner start through 2026-10-26 12:57:08 UTC found exactly these train-lane gates:

| Repo / issue | Started UTC | Why it ran | Result |
| --- | --- | --- | --- |
| hostvitals / control#55 | 11:56:44 | `gate the train's tip`: the tip had no green record; the request survived the host-runner upgrade | Green, exit 0 after 58 seconds; #55 then took its worker turn at 12:09:02 |
| bizkernel / control#109 | 11:57:48 | `gate the train's tip`: no green record for `a265af37f9b9e650b4ee77d7497dec6c9d28adf3` | Red, exit 1 after 153 seconds; the repair then waited for the green tip its own fix needed to produce |

There were **no train-worker repair runs** in that window: no train `land` returned `gate_red`, and no train worker made a repair. The watch instead fixed the lock-turn deadlock in [PR 539](https://github.com/sunfmin/afkconvoy/pull/539); its evidence identifies the bizkernel failure as a Flux solver timeout in `abi::Manifest::read`. That repair protocol is retained here, including standing locks raised by an older release. A repair's turn must not wait for the green base tip it exists to restore.

The replacement covers both observed missing-record certifications in a separate checkout, in worker lanes. It covers the red-tip case with the operator repair and its own turn. Unrecorded legacy history, including old worker fixes with no surviving issue PR, is preserved through recovery PRs. Earlier afkconvoy train landings at 11:44:22 and 11:51:08 UTC ran on 0.5.20; they are not observations of #110.

Legacy recovery is resumable through issue-linked Orca workspaces. Conflicts and red gates retain those workspaces for a worker; retries reuse them. Migration returns on a red base instead of blocking the tick that must dispatch its repair. A final recovery PR preserves the original line's history, including a closed or rewritten PR head and commits an older train worker made.
