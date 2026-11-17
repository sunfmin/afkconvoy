---
status: accepted
---

# A Codex worker works under a Codex goal

Some Codex models end a turn after a first slice of their brief; the fleet's nudge recovers one such turn, and the model may stop again. Codex has its own mechanism: a session goal, which it continues by itself after each turn its model ends while the goal is active, under a goal prompt that keeps the whole objective. So every Codex worker, of every role, is handed its brief as `/goal <objective>` instead of the brief pointer alone. The objective is the brief pointer and one sentence saying to mark the goal complete once the brief's outcome stands: the task stays in the brief, in one home. Other harnesses are unchanged.

A worker whose verdict stands while Codex still continues its goal is never seen stopped, so the tick would never carry the verdict out. The tick therefore reads its comments while it is busy — but only once its goal is active and Codex has continued it since its brief, so a worker at its first turn costs no tracker read — and types `/goal clear` when a verdict stands. A bound worker's declared stage already closes its terminal.

A waiting worker — parked, queued, on a shell of its own — whose goal stays active is continued round at cost. The fleet bounds it with a token budget on the launch line, not the operator's Codex configuration. Pausing the goal for each kind of wait was rejected: Codex's own goal tools refuse `paused` without a user's request, and each wait would need its own pause and resume.

A goal Codex marked `blocked` or `budget_limited` continues no more: worker health judges such a worker stopped short of its task, and it is nudged, then handed to a human, as any worker. The nudge and escalation stay the fallback for every harness.
