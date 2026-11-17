---
status: accepted
---

# A round lands through a pairing tree

Where a repo declares a fast gate and a slow gate (ADR-0047), its PRs land in **rounds**. Each PR is fast-gated; the slow gate runs once per round, on the whole round merged onto the target tip. A repo that declares no gates is whole-gated per candidate, as ADR-0037 says.

Felix (sunfmin/control#1135, #1239): find, concurrently, the PRs that do not conflict with each other; merge together everything that does not conflict, run the fast gate on it; run the whole gate once, only as it all merges onto main.

Before this, a round's PRs were a candidate chain, each merged onto the one before, so one red or invalid candidate cancelled every candidate behind it, green or not. The pairing tree removes that: a red change takes out only the group it was merged with.

## Who is in a round

Every PR that asked to land and whose worker's fast gate is green, taken when the round starts, in the order they asked. A turn that asks puts itself on the chain as **pending** and comes off when its round takes it, or it ends. A round opens once no round is open, no barrier waits and no gate of the chain runs. A PR that asks during a round waits for the next, unless the round's whole gate is running: then it joins the stacked round (below). A round holds only the PRs whose turns ran on one host (ADR-0117). How many gates run at once is the exclusive queue's (ADR-0088), not the round's: a round is as big as the PRs that are ready.

## The pairing tree

At each level, in asking order, the first node not yet taken starts a **group**, and every later node that merges cleanly with what the group holds so far (`git merge-tree`, textual conflicts only) is merged into it, the earlier-asked first, each onto the merge before. A node no later node merges with goes up alone. Each group's merge is fast-gated alone, in its own worktree, by its last-asked PR's turn; the groups of a level gate at once. Levels repeat until one node is left, the **root**. A level where nothing merges and nothing is carried makes its first node the root, and its other PRs wait for the next round. A level of N mutually clean PRs is one group with one fast gate.

A group's merge is never landed: it is only the tree its fast gate runs on. Its fast gate runs on a tree that is not the target's, so a change that breaks only with the tip's newer commits is first seen by the whole gate.

## The root lands

The root lands as the round's PRs merged onto the target tip one after another, in asking order: one merge commit of each PR's original head, so the target's history says each PR landed. None of them gates alone. The last, the round's **tip**, runs the whole gate once: its fast part is a reused run (ADR-0037) where a group's tree equals the tip's, which it does where the PRs stood on the target tip; its slow part runs after. The fast and slow runs on the tip's tree are joined into one record of `gate_command` on that tree, so the landed tip needs no certification run. Green, one atomic publication fast-forwards the target to the tip and verifies every source head; each PR answers `merged` and its issue closes. A root candidate merged onto the round's candidates ahead that conflicts answers `speculative_conflict`, its conflict left in the worker's worktree.

On a proving branch the root is fast-gated only, and a promotion runs the slow gate later (ADR-0090).

## Carrying

A PR whose change cannot land with others' carries them instead of waiting.

- **A red group**: every PR in it leaves the round. Its last-asked PR is the **carrying PR**: its branch is fast-forwarded to the group's merge and it is handed the red run; it fixes it, passes its fast gate and asks again. Every other PR of the group is **carried** by it.
- **A node that conflicts with every other node of its level**, each pair asked alone: the last-asked such node carries the earliest-asked one no group took, one a level; its worker merges that node into its own and resolves the conflict.
- **A red root** is not bisected. The round's tip keeps the failure; every other candidate of the root stops `round_red`. The tip's PR fast-forwards its branch to the tip's commit, which holds every PR of the round, and carries the whole round: it answers `gate_red` with `carrying`, and each other turn answers `round_red`, naming `carried_by`. The fixed round asks again as one PR.

A carried PR is closed only once the carrying PR's head holds its head, so nothing is lost while its worker merges: every turn, before its round, carries each open PR whose head its own head holds and the target tip does not. The carried PR is closed with a comment naming its carrier, its branch deleted, its worker ended and its worktree removed. The carrying PR's description becomes its own followed by each carried one's, so it closes every issue they closed; just before it lands it is written again to carry any PR the issue comments name that it no longer says it carries. afkconvoy comments on each carried issue which PR and branch carry it; while that PR is open the issue is off the frontier, and its claim is settled without closing it. The carrying PR's landing closes every issue its description closes and every open issue whose latest carrier comment names it. Should it close unlanded, the issue is dispatchable again and its next worker starts from the carrying branch. `doctor` names an open issue whose carrier landed.

A carrying PR that asks again while no PR of its root has a candidate rejoins the current round, at the level pairing then; otherwise it waits for the next.

## The stacked round

While a round's whole gate runs, the PRs that ask neither wait idle nor interrupt it (sunfmin/control#1140). They form a **stacked round**: they group among themselves as any round's PRs do, and their root's PRs are merged one after another onto the running round's tip commit and fast-gated there, the **stacked merge**. A PR that asks later joins it as a node of the level it groups, or a level above its root; a stacked merge not yet started then ends `stack_joined`, and one gating runs on. The running round green and landed, the target tip is exactly that commit: the stacked round becomes the open round, its fast part on record, so only its slow gate runs. The running round red or ended otherwise, the stacked round is discarded (`stacked_round_discarded`) and its PRs are pending again, ahead of the rest, charged nothing. The running round is never killed or restarted for a newcomer.

## A gate that says nothing

A candidate's gate, fast or slow, red only on TIMEOUTs the gate says it cannot classify because the host is overloaded (`gate::judge`) is no red: the candidate leaves the chain `gate_unsettled`, recorded neither green nor red, and its turn answers `gate_unsettled`, naming the TIMEOUTs and the load; the fleet asks again. What was merged onto it goes with it, `predecessor_unsettled`. An unsettled slow gate fails no round: its other candidates stay fast-green, and the last of them runs it again. A FAIL, a classified TIMEOUT, or a run that reached the gate limit is red.

## Both programs

The rules live in the landing crate (`pairing::groups`, on the chain's one record), so afkconvoy's turns and the plain runner `Lander::land` that Guild uses follow the one implementation (ADR-0058). In afkconvoy each turn runs its own PR's part: it groups a level when it is the one to see it ungrouped, gates each group it is the last-asked PR of, assigns its root candidate once every PR asked before it has, runs the whole gate when its candidate is the tip, and settles its own issue once landed. A turn that ends takes its PR out of the round, and a group it would gate is cancelled. The plain runner runs a round over all its changes in one process, gates each level's groups on threads, and tells its forge which change is `Carrying` and which `Carried`.

The code and its records keep the earlier name: a group's candidate is `Candidate::pair`, its record `Paired` (`first`, `second`, `partner`, and `later` for the nodes after the second); a record without `later` reads as a group of two. `Phase::Batched` means a candidate of a round's root, not gated alone. A repo's `landing:` key is refused.

## Considered options

- **Keep the candidate chain and gate only its tip whole** (the batch round). Refused: one red change still holds back every change behind it, and finding it costs log2 N whole gates.
- **Bisect a red round.** Refused: each split is a slow gate run, and the round's issues and failing tests already say what a fix needs.
- **A separate repair worker for a red round.** Refused: the tip's worker already holds the round's tip, its worktree and the failing run; a new worker would start from nothing. On a proving branch a red slow gate is repaired by that branch's own worker (ADR-0090), since the round's workers have ended.
- **Hold a red round's other PRs until the fix lands.** Refused: their turns would wait unbounded on another worker. Their changes are already in the carrying PR.
- **A slow gate per candidate.** Refused: it runs the slow tests once per PR, the cost rounds exist to remove.
- **Land the root's merge commit itself.** Refused: it carries no commit per PR, and the target's history is where a landing is read from.
- **Kill a running round's slow gate for a newcomer and gate again.** Refused: it delays the running round by more than it saves the newcomer, throws away the slow gate already run, would restart forever under bursty arrivals, and a red newcomer would make earlier green PRs its carried. Stacking leaves the running round untouched and costs one extra fast gate.

## Consequences

- One red change sends every PR of its group back to one worker, which carries them all; it cancels nothing outside its group.
- A red root costs every PR of the round its landing until the tip's worker fixes it.
- A worker's PR may carry other PRs' changes, through a red group, a conflict, or a red root. Landing it lands them.
- `doctor` and the panel say, for each candidate, the round whose slow gate it waits on, or none.
