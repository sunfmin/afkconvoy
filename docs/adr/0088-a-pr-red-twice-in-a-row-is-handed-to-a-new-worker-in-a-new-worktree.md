---
status: accepted
---

# A PR whose landing turns go red twice in a row is handed to a new worker in a new worktree

sunfmin/control#1254.

On 2026-11-16, between 11:27 and 12:43 UTC, nothing landed on afkconvoy's
`main`. Of the last 20 `land` turns, 7 ended `gate_red`, 4 conflicted and 1
was `round_red`. #1231 (PR #938) went red at 12:17. Its own worker took
another turn at 12:18 in the same worktree, and it went red again at 12:38.
Each worker retried only its own PR, in the worktree where it had already
failed, and nothing gave the PR a fresh look. Felix, asked who was fixing the
red PRs, said: 「连接的红。应该再起一个新的 work 和一个新的 work tree 来弄啊。」
(https://github.com/sunfmin/control/issues/1254#issuecomment-6916832809): red
twice in a row, start a new worker and a new worktree on it.

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

The row is kept per landing worktree, beside its turn note, in the
worktree's own git dir (`convoy-turn-reds.json`). Every `land --issue` turn
writes it, fleet-asked or the worker's own. Forgetting a turn's wait leaves
it. It is gone with the worktree, so a new worker's row starts empty.

At the second red, the next tick does not hand the answer back to the
worker in that worktree, whatever the worker is doing. With a slot free:

1. Its worker is ended the way a claim's end ends one. Its terminals are
   closed and its process tree is confirmed gone before anything else
   writes. One that cannot be confirmed stopped is given no second writer:
   the step fails, and the tick's errors say so. The worktree is kept, and
   a later tick tries again.
2. What the worktree holds that the PR's branch on the remote does not is
   saved as one binary patch against that branch's tip, in the checkout's
   git dir (`convoy-salvage/<issue>-<seconds>.patch`). That is its
   uncommitted and untracked files that git does not ignore, and its commits
   not pushed. It is the patch the operator made by hand on 2026-11-16
   (`salvage-20261116`), made by the fleet.
3. The worktree is removed. The local branch goes with it, since all it held
   is in the patch.
4. A worker of the issue starts in a new worktree, on the PR's branch as the
   remote has it, merged up to the base tip as it stands. A merge that
   conflicts is left for it to resolve. The claim, the PR and the branch
   stay.

Its brief ends with both red runs. Each names its failing tests (nextest's
`FAIL`, `TIMEOUT` and signal lines) and the path of its whole gate output.
The brief also gives the earlier worker's last verdict text, the end of its
terminal, and the saved patch. It says the worker may change the approach
rather than patch the last attempt.

The tick's line, and so its log, says `handed #<issue>'s PR #<pr> to a new
worker in a new worktree <path> after 2 red turns in a row (failing …)`. The
issue's status board says the same line under **Handed to a new worker**,
for as long as the claim it was made under holds.

## A red turn names its failing tests and its gate output

Every candidate gate, fast or slow, of a pair or a round, now writes its
whole output to a file of its own in the checkout's git dir
(`convoy-gate-<candidate>-<fast|slow>.log`). Only the newest 40 are kept. A
turn that answers `gate_red` adds `failing`, the tests that output names as
failing, and `gate_output`, the file, to its answer. Its tick-log line names
both, as a cut's red gate step already does: `gate_red: gate output <file>;
failing <tests>`. The file comes first, so a line cut at its length cuts the
tests, not the file. The brief of the new worker reads them from the row,
which keeps each red answer's.

## The Breaker is not a way round

A hand-on writes no stage record, no ruling and no claim. Every rework count
of the issue, which is counted from its stage records (ADR-0010), stays what
it was. The new worker's rework counts with the old one's, and the Breaker
escalates at the same limit it did before. A fresh worker is a fresh look,
not a fresh budget.

No breaker counts landing turns. A PR that keeps going red is handed to a
new worker at every second red in a row, and is not escalated for it.
Bounding that is left to a decision of its own.
