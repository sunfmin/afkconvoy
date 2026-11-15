---
status: accepted
---

# A round lands through a pairing tree

Replaces the candidate chain within a round of ADR-0037 and ADR-0048, and the batch round of ADR-0068 (sunfmin/control#1135; decided by Felix, control session 2026-11-15 ~21:20 JST).

Felix: 「我想把这个逻辑改成。并发的寻找和自己不冲突的 PR。两个合在一起。跑快测试。……两两合并这种。所有这种只靠跑快测试。只有到最后合并到 main 的时候。才跑一次整体的测试。当然是这些 PR 都已经改完了的情况下。」 — find, concurrently, the PRs that do not conflict with each other; merge them two by two and run the fast gate; run the whole gate once, only as it all merges onto main.

In a candidate chain each candidate is merged onto the one before it, so a red or invalid candidate cancels every candidate behind it (ADR-0037), green or not. From `refs/convoy/candidates/main`, 2026-11-11..15: afkconvoy had 29 candidates cancelled by another's failure, 20 of them already fast-green; guild 46, 34 fast-green. `predecessor_failed` was 12 and 17 of those: the cancellations a pairing tree saves. `round_red`, 14 and 23, stays: a red round is still not bisected.

## The decision

- **Who is in a round**: every PR that asked to land and whose worker's fast gate is green, taken when the round starts. A PR that asks during a round waits for the next — unless the round's whole gate runs: then it stacks on the round (below).
- **A stacked round** (sunfmin/control#1140; decided by Felix, control session 2026-11-15 ~21:50 JST: 「好，就按你说的加进去」): while a round's root, merged onto the target tip, runs its whole gate, the PRs that ask to land neither wait idle nor interrupt it. They are a round of their own, stacked on it: they pair among themselves as any round's PRs do, and their root, merged onto that root-onto-tip commit, is fast-gated there, concurrently — the **stacked merge**. A PR that asks later joins the stacked round while it can pair it: as a node of the level it pairs, or a level above its root. The running round green and landed, the target tip is exactly that commit: the stacked round is the next round, its fast gate on record for the tree its root has on that tip, so it runs only its slow gate. The running round red, or ended otherwise, the stacked work is discarded and its PRs pair in the next round as if they had just asked; the red is never charged to them. The round in progress is never killed or restarted for a newcomer.
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

### The stacked round

The chain holds it beside the open round (`Chain::stacked`), with what it stands on (`Round::on`): the open round's last candidate, whose whole gate runs, and its commit. A pending turn opens it once that gate has started (`Chain::whole_running`) and no barrier stands, and later turns join it (`Chain::open_round`); a PR that left it does not join it again. Its pairs are pairs of its own round. Once it has its root, the root's last-asked PR's turn merges the root's PRs' original heads onto that commit one after another, in the order they asked, each a merge commit as its candidate of the root will be (`Chain::to_stack`), and gates the last, a candidate beside the chain (`Candidate::pair`, `Paired::stacked`), by the fast gate its commit declares (`Chain::stack_merge`). A join above the root ends a stacked merge not yet started, `stack_joined`; one gating runs on.

The open round closed, the stacked one is settled (`Chain::settle_stacked`): its candidate landed, the stacked round is the open round, on the tip, and its root is assigned onto the tip as any root is. Its last candidate has the stacked merge's tree, so the whole gate's fast part is that run, reused (ADR-0069), and only its slow part runs. That candidate ended otherwise, the stacked round's pairs and stacked merge end `stacked_round_discarded`, and its PRs are pending again, ahead of the rest. A pair or stacked merge that ended before its gate started is stopped by its own turn, which has no process of it to confirm gone (`Chain::stop_unstarted`). A new round does not open while a stacked one waits on what it stands on.

The plain runner stacks the same way: a run that asks while another run's round runs its whole gate pairs its changes, gates their stacked merge and waits, reading the chain, until that round settles or its candidate is wedged; a stacked round discarded takes its changes into a round of their own.

## A red root (sunfmin/control#1137)

A red root is the chain's, as carrying is: the candidate of the round's last-asked PR, the tip whose whole gate ran red, keeps the failure, and every other candidate of the root stops `round_red` by it (ADR-0048); the PR of that tip carries the PRs of those (`Chain::root_carry`), and each is carried by it (`Chain::round_carrier`). Its turn is handed the root's run and the round, as a red round's tip was, and fast-forwards its branch to the tip's commit, which holds every PR of the round; it pushes it and carries at once (`forge::carry`), as of a red pair, so its description closes every issue of the round, and answers `gate_red` with `carrying`. A merge that conflicts — its worker pushed past what it asked to land — is left in its worktree, and the turn after the worker resolved it carries. Each other turn answers `round_red`, naming the carrier `carried_by`. No bisection runs: the fixed round asks again as one PR.

The plain runner (`Lander::land`) says the same: the last-asked change's outcome is `Carrying` the others, its forge told the run and the tip to merge (`Forge::sent_back`), and each other's is `Carried` by it.

## Carrying (sunfmin/control#1136)

Carrying is the pairing tree's (`pairing`), on the round's one record: a pair red on its fast gate takes both its sides out of the round; its later-asked PR leaves it `carrying`, handed the pair's merge and the PRs it carries, and each of those leaves `carried` by it. A level's last-asked node that `git merge-tree` finds conflicting with every other node of the level carries the earliest-asked of them no pair took, one a level, its last-asked PR handed its own node's commit and then that node's; where the two of them conflict with each other only, the later-asked carries the earlier, as of a red pair. The round notes each carrying PR; one asked again while no PR of its root has its candidate rejoins the round at the level pairing then, or a root not yet assigned pairs with it again (`Chain::rejoin`); otherwise it waits for the next.

A carried PR is closed only once the carrying PR's head holds its head, so nothing carried is lost while its worker merges: every turn, before its round, carries each open PR whose head its own head holds and the target tip does not (`forge::carry`), whatever brought it there. A red pair's merge is a fast-forward of the carrying branch, so its turn pushes it and carries at once, and answers `gate_red` with the run; a conflicting merge is left in the worktree, its turn answers `conflict`, and the turn after the worker resolved it carries. The carried PR's turn answers `carried`, naming its carrier. The forge (ADR-0060) closes a change with a comment, reads and rewrites its description, deletes its source while still at the head carried, and tells its program (`Forge::carried`): the carrying description is its own followed by each carried one's under a heading that names it, once, so it closes every issue they closed.

afkconvoy tells each carried issue, by a comment, which PR carries it and on which branch, which it records as one of the issue's own branches. While that PR is open the issue is off the frontier, and a claim on it is settled — its worker ended, its worktree removed, the claim let go — without closing it. The carrying PR's landing closes every issue its description closes. Should it close unlanded, the issue is dispatchable again, and its next worker starts from the carrying branch, which holds its work.

The plain runner (`Lander::land`) carries the same way among the changes it is given: each one's outcome says `Carrying` or `Carried`, and the forge is told what a carrying change merges (`Forge::sent_back`).

## Considered options

- **Kill the running round's slow gate for a newcomer, merge it in and gate fast and slow again** (asked by Felix, 2026-11-15 ~21:50 JST). Refused: it delays the round already running by F+S−R (R its remaining slow gate) to save the newcomer only R, and throws away the slow gate already run (medians from 2026-11-11..15: fast 132 s and slow 344 s for afkconvoy, 136 s and 194 s for guild); arrivals closer together than F+S — guild saw 302 PRs in 3.5 days, in bursts — would restart a round forever; and a red newcomer would make the round's earlier green PRs carried by it. Stacking takes the newcomer's critical path from R+F+S to about R+S, leaves the running round untouched, cannot starve, and costs one extra fast gate.

- **Keep the candidate chain and only gate its tip whole.** The batch round (ADR-0068). Refused: one red change still holds back every change behind it, and finding it costs log2 N whole gates.
- **Land the root's merge commit itself.** Refused: a merge of the root onto the tip carries no commit per PR, and the target's history is where a landing is read from (ADR-0037). The PRs merged onto the tip one after another hold the same changes, and their last commit is what is gated.
- **Bisect a red root.** Refused, as ADR-0048 refused it: each split is a slow gate run, and the round's issues and failing tests say what a fix needs.

## Consequences

- One red change no longer cancels the changes asked after it: its pair alone is red.
- A PR that asks while a round's whole gate runs is fast-gated beside it: when that round lands, the next starts with its fast gate done. Each PR that joins a stacked round above its root costs one more pair and one more stacked merge, fast only.
- A round of N mutually clean PRs runs N−1 pair gates, fast only, about log2 N levels deep and each level's at once, and one whole gate; before, it ran N fast gates one behind the other and one slow gate.
- A pair's fast gate runs on a tree that is not the target's: a change that breaks only with the tip's newer commits is first seen by the root's whole gate.
- `Phase::Batched` and a candidate's `batch` now mean a candidate of a round's root, not gated alone; records of batch rounds still read.
- Guild lands with the same crate, and its `land_with` and `Landing` are gone with the batch round.
- A repo's `landing: batch` (sunfmin/control#1120), which put afkconvoy's own turns in batch rounds, goes with them: every round pairs, and the key is unknown again.
