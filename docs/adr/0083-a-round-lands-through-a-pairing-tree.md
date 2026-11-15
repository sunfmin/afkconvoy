---
status: accepted
---

# A round lands through a pairing tree

Replaces the candidate chain within a round of ADR-0037 and ADR-0048, and the batch round of ADR-0068 (sunfmin/control#1135; decided by Felix, control session 2026-11-15 ~21:20 JST).

Felix: 「我想把这个逻辑改成。并发的寻找和自己不冲突的 PR。两个合在一起。跑快测试。……两两合并这种。所有这种只靠跑快测试。只有到最后合并到 main 的时候。才跑一次整体的测试。当然是这些 PR 都已经改完了的情况下。」 — find, concurrently, the PRs that do not conflict with each other; merge them two by two and run the fast gate; run the whole gate once, only as it all merges onto main.

In a candidate chain each candidate is merged onto the one before it, so a red or invalid candidate cancels every candidate behind it (ADR-0037), green or not. From `refs/convoy/candidates/main`, 2026-11-11..15: afkconvoy had 29 candidates cancelled by another's failure, 20 of them already fast-green; guild 46, 34 fast-green. `predecessor_failed` was 12 and 17 of those: the cancellations a pairing tree saves. `round_red`, 14 and 23, stays: a red round is still not bisected.

## The decision

- **Who is in a round**: every PR that asked to land and whose worker's fast gate is green, taken when the round starts. A PR that asks during a round waits for the next.
- **The pairing tree** replaces the candidate chain within a round. In the order the PRs asked to land, each pairs with the first later one it merges with cleanly (`git merge-tree`, textual conflicts only), and each **pair**'s merge is fast-gated alone. Pairs pair the same way, level by level, fast gate only. A node left without a partner at a level goes up unpaired.
- **The root** is merged onto the target tip and gated whole, fast and slow, once. Green: the target branch fast-forwards to it; every PR in it lands, and each issue closes.
- **Carrying**: a pair red on its fast gate → the later-asked PR's worker merges the partner's PR into its own, fixes it, and passes its fast gate. A PR that conflicts with every possible partner merges the earliest-asked one it conflicts with into its own, resolves, and tries pairing again, one at a time. The **carried PR** is closed, commented "carried by #…", its branch deleted, its worktree removed and its worker ended; the **carrying PR**'s description is rewritten as the changes together, with `Closes` for every issue. The carried issue stays open until the carrying PR lands, so it can be dispatched again if that PR never lands. A carrying PR re-enters the current round if its whole gate has not started, else the next.
- **A red root** is not bisected: ADR-0048's reason stands. The worker of the round's last-asked PR fast-forwards the root-onto-tip merge into its branch, fixes it on the spot, and lands the whole round as one carrying PR; every other PR of the round is carried as above.
- **Removed**: the candidate chain within a round, its suffix cancellation, and the batch round with its bisection (ADR-0068). **Kept**: the local gate only, no rebase, the forge never merges, reused runs (ADR-0069), and a cut lands nothing.
- **Both programs**: it lives in the landing crate, so afkconvoy's turn and Guild's lander use the one implementation (ADR-0060).

## How it is built

The rules are the landing crate's (`pairing`), on the one record the remote holds per target (`Chain`): the round, its PRs in asking order and why any left it (`Chain::round`), and its pairs, each a candidate of its own beside the chain (`Candidate::pair`). A pair's candidate is a merge commit of its two nodes, the earlier first, of the later-asked PR, whose turn runs its gate; the pairs of one level gate at once, each in its own worktree and lane.

The root lands as the round's PRs merged onto the target tip one after another, in the order they asked: one merge commit of each PR's original head on the chain, so the target's history says each PR landed, as every landing before said it. None of them gates alone; the last runs the whole gate once, its fast part answered by a green run on record of its tree where there is one (ADR-0069) — where the PRs stood on the tip, the root's pair has that tree — and its slow part after. That last candidate is the round's tip, its PR the round's last-asked; red, it keeps the failure and the others stop `round_red`, as a red round's did (ADR-0048). One atomic publication fast-forwards the target through them.

A pair's merge, or a level's, is never landed: it is the tree a pair's fast gate runs on. A green pair whose tree equals the root's onto the tip — its PRs held the tip — makes the root's fast gate a reused run.

afkconvoy's turns each run their own PR's part: a turn reserves its pending place with what it asks to land; any turn opens a round of every pending turn once no round is open, no barrier waits and no gate of the chain runs; a turn pairs a level when it is the one to see it unpaired; gates each pair it is the later-asked PR of; assigns its own candidate of the root once every PR asked before it has; runs the whole gate when its candidate is the tip; and settles its own issue once its candidate landed. A turn that ends takes its PR out of the round, and a pair it would gate is cancelled. Barriers — cuts, watches, fleet-lock repairs — still land alone through a candidate of their own (ADR-0037).

The plain runner (`Lander::land`) runs a round over all its changes in one process, gates each level's pairs on threads of its own, and takes the changes that left a round for the next into the next.

## This slice (sunfmin/control#1135)

Carrying is the next ticket. Until it lands, a pair red on its fast gate hands the red run back to its later-asked PR's worker, which answers `gate_red`; the other side goes up unpaired, and the rest of the round lands. A PR that merges with no partner at its level, or whose pair said nothing of its tree, leaves the round for the next, as a conflicting PR waited before. A red root is handled as a red round was (ADR-0048): its tip's worker is handed the round, and the others answer `round_red`.

## Considered options

- **Keep the candidate chain and only gate its tip whole.** The batch round (ADR-0068). Refused: one red change still holds back every change behind it, and finding it costs log2 N whole gates.
- **Land the root's merge commit itself.** Refused: a merge of the root onto the tip carries no commit per PR, and the target's history is where a landing is read from (ADR-0037). The PRs merged onto the tip one after another hold the same changes, and their last commit is what is gated.
- **Bisect a red root.** Refused, as ADR-0048 refused it: each split is a slow gate run, and the round's issues and failing tests say what a fix needs.

## Consequences

- One red change no longer cancels the changes asked after it: its pair alone is red.
- A round of N mutually clean PRs runs N−1 pair gates, fast only, about log2 N levels deep and each level's at once, and one whole gate; before, it ran N fast gates one behind the other and one slow gate.
- A pair's fast gate runs on a tree that is not the target's: a change that breaks only with the tip's newer commits is first seen by the root's whole gate.
- `Phase::Batched` and a candidate's `batch` now mean a candidate of a round's root, not gated alone; records of batch rounds still read.
- Guild lands with the same crate, and its `land_with` and `Landing` are gone with the batch round.
- A repo's `landing: batch` (sunfmin/control#1120), which put afkconvoy's own turns in batch rounds, goes with them: every round pairs, and the key is unknown again.
