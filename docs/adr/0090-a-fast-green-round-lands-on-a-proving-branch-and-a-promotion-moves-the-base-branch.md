---
status: accepted
---

# A fast-green round lands on a proving branch, and a promotion moves the base branch

Amends ADR-0037, ADR-0048, ADR-0083 and ADR-0006 (sunfmin/control#1308).

Decided by Felix in the control session on 2026-11-17, about 10:40 JST:

> 大改 fast gate 过了之后就land，但是不要往 main 分支里面。而是往一个新的分支里合并，这个新的分支也有一个固定的 worktree 和一个 worker 在做。它只有在机器闲的时候才去跑 slow gate。跑过了 slow gate 就合到 main 里面。

(Land once the fast gate passes, not onto main but onto a new branch. That branch has a fixed worktree and a worker of its own, which runs the slow gate only while the host is idle. Once the slow gate passes, it goes to main.)

He then chose four points, each the recommended option:

1. An issue closes, and its worker ends, when its PR lands on the new branch. New workers branch from it, so a blocked-by dependent sees its blocker's change.
2. A red slow gate on the new branch is fixed forward by that branch's own worker. Fast-green PRs keep landing on the branch meanwhile; only the move to the base branch waits.
3. The slow gate runs while the host is idle, but at most 1 h after the oldest change not yet on the base branch; past that it runs anyway, queued as any heavy command.
4. Every repo that declares a ```` ```yaml gates ```` block gets it. It lives in the shared landing crate (ADR-0060), so Guild's lander gets it too.

## The decision

The new branch is the **proving branch**; moving the base branch to the proving branch's green tip is a **promotion**. "Integration branch" stays set aside (GLOSSARY, under **Target branch**).

- A repo that declares a ```` ```yaml gates ```` block may name its proving branch, `proving_branch: <branch>`, in its config block. `afkconvoy config` refuses the key where no gates block is declared, and where it names the base branch, saying why. A repo that names none lands exactly as before.
- **Landing**: a task's target branch, unless pinned to another, is the proving branch. A turn lands a round as before, through its pairing tree (ADR-0083), but its root, merged onto the proving branch's tip, is gated by the fast gate only: the fast gate is the whole gate of a landing on the proving branch, with its limit and budget (`Config::fast_as_whole`), and its green run is put on record for the root's tree. The proving branch fast-forwards to it; each PR answers `merged` and its issue closes. A proving tip with no record is certified by the fast gate alone.
- New workers, patrols, watches and fleet-lock repairs are cut from the proving branch and land on it. Nothing lands on the base branch but by promotion.
- **Promotion** (`afkconvoy promote [CHECKOUT]`): the slow gate runs on the proving branch's tip, in a worktree of its own, and is joined with the fast run on record of the same tree into a record of `gate_command` — ADR-0048's join of a round's tip. Where no fast run of that tree is on record, the whole gate runs. Green, the base branch is pushed to that tip by a plain push, which the remote refuses unless it is a fast-forward; a base branch that is not an ancestor of the proving tip is never moved, and the promotion answers `diverged`. Red, the base branch stays where it was, and the answer and the fleet's log say so.
- In this slice, a promotion is started by `afkconvoy promote` and by a tick whenever the proving branch is ahead of the base branch, detached from the tick, one per checkout at a time. Idle scheduling, the 1 h bound and the proving branch's own repair worker come in the issues that follow.
- `doctor` and `plan` show, per repo with a proving branch: its tip, the base tip, how many landed PRs are not yet promoted, and how long the oldest has waited.

## Amendments

- **ADR-0037** lands with no step after the turn, because a second step "would add no verification after a recorded turn". The proving branch adds the slow gate after the turn, so that reason no longer holds for a repo with a proving branch: its turn records only the fast gate, and the promotion is the verification that follows it. A repo without one still lands with no second step.
- **ADR-0048 and ADR-0083** refused "a separate repair worker for a red round". On the proving branch a round is not slow-gated at all, and its PRs' workers have ended once it lands; under point 2, a red slow gate on the proving branch is repaired by the proving branch's own worker. The refusal stands for a round landing on a base branch.
- **ADR-0006**: a repo now has two branches in play. A task's target branch is the proving branch; the base branch moves only by promotion, and is still where versions are cut (ADR-0037: a cut lands nothing, and tags what it gated).

## Considered options

- **Run the slow gate per round, but after the round landed.** Refused: it is the proving branch with one promotion per round, and gives up batching the slow gate across rounds, which is what an idle host buys.
- **Keep landing on the base branch and make the slow gate optional.** Refused: nothing would hold the base branch to the whole gate.

## Consequences

- A fast-green PR lands, and its issue closes, without waiting for the slow gate: a round's critical path is its fast gates.
- The base branch lags the proving branch by what no promotion has passed yet; a red promotion holds the base branch back until a later tip is green.
- A task pinned to the base branch before its repo named a proving branch keeps that target, and lands on the base branch as before.
