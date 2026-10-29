---
status: accepted
---

# A round of candidates is slow-gated once at its tip

Amends ADR-0037 and ADR-0047 (sunfmin/control#240, and sunfmin/control#238, which asked for the same landing).

ADR-0047 split the gate in two and kept a landing on the whole of it: each candidate ran `gate_command`, about 1,300 seconds on `Offices-MacBook-Pro.local`, of which the slow gate's tests were 1,157. Three candidates validating together ran those tests three times, each on a tree that held the one before it.

Where a candidate's commit declares the ```` ```yaml gates ```` block, a turn now gates the candidate with the **fast gate** alone. A fast-green candidate does not land: it waits for a round.

A **round** is one run of the slow gate, on the commit of the candidate chain's tip, once no candidate of the chain is still building or fast-gating. The tip's turn runs it, in the worktree its fast gate ran in, under the slow gate's limit. A candidate assigned while it runs is built on the tip and waits for the next round. The tip's tree holds every candidate of the round, so its run is a run on each of their changes together.

## Green

The fast gate's record and the slow gate's record on the tip's tree are joined into a record of `gate_command` on that tree: the two parts together check what the whole gate checks (ADR-0047), on the same tree. The joined run is as old as its older part and took both parts' seconds. The target tip the round lands therefore needs no certification run, and the gate gone slow (ADR-0024) reads its time as it read a run of the whole gate.

One atomic publication then lands every candidate up to the tip: the target branch fast-forwards from the first candidate's predecessor to the tip's commit, the chain record marks each of them landed, and each candidate's source head is verified unchanged in the same transaction. Each of their turns answers `merged` and closes its own issue. A candidate whose commit declares no gates is whole-gated as before; one ahead of a round lands by its own turn, and a whole-gated candidate behind fast-green ones lands them with it, as its tree passed the whole gate.

## Red

A red round is not split to find whose change failed. Bisecting runs the slow gate again on each half, and a run is twenty minutes on this host; the issues of the round say what each change is for, and the failing tests say what broke, which is what a fix needs.

The tip's turn answers `gate_red`, with `gate` slow and `round`, the round's PRs and issues. It merges the tip's commit into the worker's branch, a fast-forward, as the tip is the worker's own head merged onto the round: the worker's branch then holds every change of the round. The worker fixes the round from those issues and the failing tests, commits, and asks for a turn again; that candidate carries the whole round and lands it.

Every other candidate of the round is cancelled by the tip, with cause `round_red`, and its turn answers a new ending, `round_red`: stop. Its change is in the tip's PR, and its merge commit is in that PR's history, so its issue is settled from the base history once that PR lands, as a landing cut short of settling is. Candidates built on the tip are cancelled by their failed predecessor, as before.

## Conflicts

A merge onto the candidate ahead that conflicts used to answer `speculative_conflict`, telling the worker to wait for that candidate to land. Now the turn merges that candidate into the worker's branch and leaves the conflict in the worker's worktree, as a conflict with the target tip is left; `speculative_conflict` answers with `files` and `worktree`, and the worker resolves, commits and asks again. Its branch then carries the candidate ahead, which lands with it or before it. Were that candidate cancelled and rebuilt from a changed head, the worker's branch would still carry the old head; that is the price of not waiting.

## Considered options

- **Bisect a red round.** Refused: each split costs a slow gate run, and the worker learns nothing a reading of the issues and the failing tests would not tell it.
- **A separate repair worker for a red round.** Refused: the tip's worker already holds the round's tip, its worktree and the failing run; a new worker would start from nothing. sunfmin/control#238 asked for one, as the fleet starts for a red base tip; the tip's worker is given what that worker would be: every issue of the round and the failing output, with no bisection.
- **Hold a red round's other candidates until the tip's fix lands.** Refused: their turns would wait unbounded on another worker, and their window places would stay taken. Their changes are already in the tip's PR.
- **A slow gate per candidate, run after its fast gate.** The landing of ADR-0047's considered option. Refused: it runs the slow tests once per candidate, the cost this ADR exists to remove.

## Consequences

- A chain of three fast-green candidates runs the slow gate once, not three times.
- `doctor` and the panel say, for each candidate, the round whose slow gate it waits on (`round`, the tip slow-gating now), or none: it waits for the next round.
- An afkconvoy older than this one lands a repo declaring its gates with the whole gate per candidate, as before; ADR-0047's worry, an older version landing on the slow gate alone, does not arise, and the fleet is upgraded by hand before a repo lands with rounds.
- A red round costs every candidate in it its landing until the tip's worker fixes it; a change that broke the slow gate holds up the changes fast-gated beside it.
- A worker's PR may carry other candidates' changes, through a resolved conflict or a red round. Landing it lands them, and their issues are settled from the base history.
