---
status: accepted
---

# A fast-green round lands on a proving branch, and a promotion moves the base branch

A PR waits for the slow gate before it lands, so a round's critical path is its slow gate, and the host runs that slow gate whether it is idle or busy. The operator decided that a PR lands once its fast gate passes, not onto the base branch but onto a branch of its own, and that the slow gate runs there, batched across rounds, before the base branch moves.

That branch is the **proving branch**; moving the base branch to the proving branch's green tip is a **promotion**. "Integration branch" stays set aside (GLOSSARY, under **Target branch**).

## The decision

- A repo that declares a fast and a slow gate may name its proving branch, `proving_branch: <branch>`, in its config block. `afkconvoy config` refuses the key where no fast and slow gate is declared, and where it names the base branch, saying why. A repo that names none lands exactly as before.
- **Landing**: a task's target branch, unless pinned to another, is the proving branch. A turn lands a round as before, through its pairing tree (ADR-0083), but its root, merged onto the proving branch's tip, is gated by the fast gate only: the fast gate is the whole gate of a landing on the proving branch, with its limit, and its green run is put on record for the root's tree. The proving branch fast-forwards to it; each PR answers `merged` and its issue closes, and its worker ends. A proving tip with no record is certified by the fast gate alone.
- New workers, patrols, watches and fleet-lock repairs are cut from the proving branch and land on it, so a blocked-by dependent sees its blocker's change. Nothing lands on the base branch but by promotion, and versions are still cut on the base branch.
- **Promotion** (`afkconvoy promote [CHECKOUT]`): the slow gate runs on the proving branch's tip, in a worktree of its own, and is joined with the fast run on record of the same tree into a record of the whole gate, as a round's tip is joined (ADR-0083). Where no fast run of that tree is on record, the whole gate runs. Green, the base branch is pushed to that tip by a plain push, which the remote refuses unless it is a fast-forward; a base branch that is not an ancestor of the proving tip is never moved, and the promotion answers `diverged`. Red, the base branch stays where it was, and the answer and the fleet's log say so; fast-green PRs keep landing on the proving branch meanwhile.
- A promotion is started by `afkconvoy promote`, and by a tick where the proving branch is ahead of the base branch, detached from the tick, one per checkout at a time. A tick starts one only while the host is **idle**, as the exclusive queue and admission read it (ADR-0088, ADR-0107): no command of any repo waits or runs in the exclusive queue, and the host's CPU reading is under the busy share. Once the oldest change the proving branch holds past the base branch — the first PR landed past it — has waited the **promotion bound**, it starts anyway, its slow gate queued as any heavy command. The bound is 1 h, the host setting `afkconvoy run --promotion-seconds N` changes it, and `--promotion-seconds default` changes it back. A promotion started on an idle host is not ended when a worker command then waits; the exclusive bound applies to its gate as to any command. The tick's line, `plan` and `doctor` say why a due promotion has not started (`host busy: N worker commands waiting, M running in the exclusive queue`, or its cores at or over the busy share), and, past the bound, that it starts anyway.
- `doctor` and `plan` show, per repo with a proving branch: its tip, the base tip, how many landed PRs are not yet promoted, and how long the oldest has waited.

Decided but not built yet: a red promotion is fixed forward by a worker of the proving branch's own. Today a red promotion is not repaired: the base branch waits until a promotion of a later tip is green.

## Considered options

- **Run the slow gate per round, but after the round landed.** Refused: it is the proving branch with one promotion per round, and gives up batching the slow gate across rounds, which is what an idle host buys.
- **Keep landing on the base branch and make the slow gate optional.** Refused: nothing would hold the base branch to the whole gate.

## Consequences

- A fast-green PR lands, and its issue closes, without waiting for the slow gate: a round's critical path is its fast gates.
- The base branch lags the proving branch by what no promotion has passed yet; a red promotion holds the base branch back until a later tip is green.
- A task pinned to the base branch keeps that target, and lands on the base branch as before.
