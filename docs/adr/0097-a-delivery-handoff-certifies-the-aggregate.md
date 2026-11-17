---
status: accepted
---

# A delivery handoff certifies the aggregate

A Jira delivery (ADR 0041) gathers attached tasks onto one delivery branch and
opens one final review PR. Each task's landing already gates that task. Those
green runs do not say the combined delivery is ready for a human, and they
must not publish the delivery branch onto the final target.

`afkconvoy delivery --handoff` is the explicit check. It reads every attached
task and the actual results: an open task, a task closed as not planned, an
abandoned task, a task closed without a landing, an unresolved claim, retained
work, an open internal PR, a failed or cancelled landing, or an open blocker
keeps the delivery unready and names the reason. An empty attached set is not
ready. It merges the delivery head onto the current final-target head in the
object store only. A conflict leaves both branches where they are. A clean
merge is gated with the repo's existing gate, in a temporary worktree, and
that merge commit is not pushed to the final target.

A green result records one evidence snapshot: the delivery head, the
final-target head, the attached task results, and the gate tree and command.
It updates the same final PR's body with the Jira reference, that summary and
the verification evidence, then marks the PR ready. It never opens a second
final PR and never dismisses reviews. Fleet ticks, ordinary landing, recovery
and watches still exclude that PR, including after it is marked ready.

A later task, a new delivery commit, or a moved final target does not match
the snapshot. Readiness is whatever `gh pr ready` and `gh pr ready --undo`
say, rechecked against the snapshot and a gate record that still stands for
the current command on that tree. Publishing readiness reads the heads and
the task set again and withdraws when they moved. A previously ready snapshot
does not certify later work.

When the final PR has merged or closed, handoff records that observation
once. Repeating it does not write another record and does not remove other
work. Completion is that recorded merge onto the declared final target while
every attached task is implemented. A merge while attached work is incomplete,
or a merged flag whose commit is not on the final target, is a discrepancy.
Closing the PR without merging reports that it needs action. A later live
state that differs from the recorded observation is reported and not rewritten.
