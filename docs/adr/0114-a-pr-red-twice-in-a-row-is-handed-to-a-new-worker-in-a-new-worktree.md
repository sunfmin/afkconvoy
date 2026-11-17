---
status: accepted
---

# A PR whose landing turns go red twice in a row is handed to a new worker in a new worktree

A worker whose landing turn goes red retries its own PR, in the worktree where
it already failed. Nothing gives the PR a fresh look, and a PR can go red
again and again while nothing lands. The operator decided: red twice in a
row, start a new worker in a new worktree on it.

## Two red turns in a row hand the PR on

A PR's landing turns are red twice in a row when two of them end `gate_red`
with no green or conflicting turn between them. Red is `gate_red` with its
gate red. A round's slow gate red at its tip is `gate_red` for the tip's PR,
the one whose worker fixes the round, and `round_red` for each PR it
carries, which counts for none of them. A `gate_red` whose gate passed and
only went unrecorded judged the change green, and starts the row over. A
conflict, `conflict` or `speculative_conflict`, starts the row over too,
since it leaves the worker a merge to make. Every other ending, from
`waited_past_bound` to a turn that failed, is no judgement on the change,
and leaves the row as it was.

The row is kept per landing worktree, in the worktree's own git dir. Every
`land --issue` turn writes it, fleet-asked or the worker's own. It is gone
with the worktree, so a new worker's row starts empty.

At the second red, the next tick does not hand the answer back to the
worker in that worktree, whatever the worker is doing. With a slot free:

1. Its worker is ended the way a claim's end ends one, and its process tree
   is confirmed gone before anything else writes. One that cannot be
   confirmed stopped is given no second writer: the step fails, the tick's
   errors say so, the worktree is kept, and a later tick tries again.
2. What the worktree holds that the PR's branch on the remote does not — its
   uncommitted and untracked files that git does not ignore, and its commits
   not pushed — is saved as one binary patch against that branch's tip, in
   the checkout's git dir.
3. The worktree is removed, and its local branch with it, since all it held
   is in the patch.
4. A worker of the issue starts in a new worktree, on the PR's branch as the
   remote has it, merged up to the base tip as it stands. A merge that
   conflicts is left for it to resolve. The claim, the PR and the branch
   stay.

Its brief ends with both red runs, each naming its failing tests and where
its whole gate output is kept, the earlier worker's last verdict text, the
end of its terminal, and the saved patch. It says the worker may change the
approach rather than patch the last attempt. The tick's log and the issue's
status board say the PR was handed to a new worker, for as long as the claim
it was made under holds.

## The Breaker is not a way round

A hand-on writes no stage record, no ruling and no claim. Every rework count
of the issue, which is counted from its stage records (ADR-0010), stays what
it was. The new worker's rework counts with the old one's, and the Breaker
escalates at the same limit it did before. A fresh worker is a fresh look,
not a fresh budget.

No breaker counts landing turns. A PR that keeps going red is handed to a
new worker at every second red in a row, and is not escalated for it.
Bounding that is left to a decision of its own.
