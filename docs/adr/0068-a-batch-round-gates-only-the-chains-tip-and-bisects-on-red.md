---
status: accepted
---

# A batch round gates only the chain's tip and bisects on red

Amends ADR-0048 and ADR-0060 (sunfmin/control#789; decided in sunfmin/control#693, Felix, 2026-11-12).

A goal of Guild's is split into ten tasks, and each lands through a round: each candidate runs the fast gate alone, and the round's tip runs the slow gate (ADR-0048). What Felix asked of a goal is its lead time, from its split to its release; he named the remedy: "都 merge 到一个分支之后再跑再修" — merge them all onto one branch, then run, then fix.

The **batch round** is a second landing mode. Its chain is built as a round's, each change merged onto the candidate before it, but no candidate is gated alone: the chain's tip runs the fast gate and then the slow gate, as one gate.

- **Green:** its tree passed the whole gate, the two parts' records joined as a round's are (ADR-0048), and every candidate up to it lands as one fast-forward.
- **Red:** the chain is bisected. A candidate's gate says what it says of every change up to it, as its tree holds them: green, none of them is red; red, the first red change is it or one before it. Between the last candidate known green and the first known red, the middle one is gated, until the two are next to each other; the red one is the first red candidate. It is dropped: red, with the red run its own gate found, and its change sent back with that run (`Forge::sent_back`). The candidates before it passed the whole gate at the last green one and land. The changes behind it are cancelled `rebuilt`, assigned again without it, and the round goes on. A first red candidate among N is found in at most ceil(log2 N)+1 gates: the tip's, and the bisection's.

A gate the bisection runs never decides the candidate it runs at: a red run at the middle candidate may be a change before it. Only the first red candidate is red; every one it holds back is rebuilt, not failed.

Either way, what lands has passed the whole gate at the tip it lands as: a batch round lands the same commits a round does, a fast-forward through each candidate, and nothing that its last candidate's whole gate did not pass.

## Which landing a round is in

The per-candidate round stays the default (`Landing::PerCandidate`). A repo is in batch mode where its config says `landing: batch`: the program that lands reads it, with its gate, through `Declared::landing`. A round's caller may ask for either for one round, `Lander::land_with`, with no commit to the repo: Guild's foreman switches a goal's repo to batch rounds while the goal runs, which is Guild's side of it.

The mode is the landing crate's (ADR-0060): the rules in `chain` (`Chain::batch_step`, `start_batched`, `drop_red`, and a candidate's `batch` and `holds_red`), and the plain runner (`Lander`) that runs them. A candidate of a batch round is `batched` until a gate runs at it, and holds no gate process.

afkconvoy's own runner of a turn lands in batch rounds too, where its repo's config says `landing: batch` at the target's tip (sunfmin/control#1120): Guild's fleet of 2026-11-15 held 28 PRs, each mergeable alone, and its turns answered 38 `speculative_conflict`s to 3 `merged`. A turn there assigns its PR a batch candidate on the chain, gated alone by nobody, and holds no worker lane while it waits; a barrier — a cut, a watch, a certification, a repair — drains the chain and lands per candidate as before, and a turn whose landing differs from the valid candidates on the chain answers `waiting_for_capacity` until they are gone. A merge onto the chain that conflicts answers `speculative_conflict` as before.

Its turns are one per PR, and a batch round's bisection needs one runner over the whole round. **That runner is the chain**: `Chain::batch_step` says the round's one next step, and the turn of the candidate it is at runs it — the gate at it, in a worker lane; its drop, where it is the first red candidate, which answers `gate_red` to its worker with the red run its own turn found, as every gate at a candidate is its own turn's; and its landing, with every candidate before it, each of whose turns answers `merged`. Every other turn of the round waits on the chain. A turn whose candidate is cancelled `rebuilt` assigns its PR again on what is left, without the red one. No turn runs a step at another's candidate, so a host runner never holds a round, no turn of the round waits on another to come back, and a turn that dies leaves its candidate to the lease as any other: a waiting batch candidate holds no gate process, but its host is its owner (`Chain::owners`), and a dead owner invalidates it `owner_died` with its suffix. The host runner was refused for it: it is one more process to hold the round's state across restarts, which the chain already holds. The tip's turn was refused: the tip moves as turns join and as a red one is dropped, and a turn whose own candidate landed would have to stay to run another's.

A green candidate lands, with every candidate before it, before the candidates merged onto it since its gate began are gated: in a fleet turns join a round while its gate runs, and gating the new tip first would hold a green prefix back for every gate a stream of turns sets off. With a red known, nothing lands until the first red candidate is dropped, as above.

## Considered options

- **Keep ADR-0048's refusal to bisect.** It stands for the per-candidate round: there each candidate is already fast-green, and its tip's worker fixes a red round from its issues. A batch round has run no gate at any candidate but the tip, so a red tip says nothing of which change broke it; the round's other changes would wait on a fix to a change not theirs. Bisecting costs log2 N gates, each a full one, against a whole round held back.
- **Gate the tip's fast gate first and bisect on it alone.** Refused: the fast gate passing is no green, and a red slow gate would need a second bisection. A gate at a candidate is the whole gate, fast then slow, its slow part skipped where the fast part is red.
- **Land the green prefix as soon as a bisection gate finds it.** Not now: the prefix lands once the first red is found, with the candidates before it; landing earlier would change the base under the bisection.
- **Mark a red middle candidate red.** Refused: it holds every change before it, and the first red may be one of them.

## Consequences

- A green batch round of N candidates runs one fast gate and one slow gate, not N fast gates and one slow gate.
- A red one runs at most ceil(log2 N)+1 gates to find its first red candidate, and one more at the rebuilt round's tip; a second red change is found by the rebuilt round as the first was.
- A change sent back from a batch round was gated only with the changes before it: its red run may be theirs together with it, which its program's worker reads from the run.
- A run unsettled by an overloaded host (ADR-0048) is unsettled in a batch round too: the candidate and what was merged onto it leave the round, and their changes ask again.
