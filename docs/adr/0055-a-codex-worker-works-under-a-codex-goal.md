---
status: accepted
---

# A Codex worker works under a Codex goal

For sunfmin/control#544. Some Codex models end a turn after a first slice of their brief; the fleet's nudge recovers one such turn, and the model may stop again. Codex has its own mechanism: a session goal, which it continues by itself after each turn its model ends while the goal is active, under a goal prompt that keeps the whole objective. So every Codex worker, of every role, is handed its brief as `/goal <objective>` instead of the brief pointer alone. The objective is the brief pointer and one sentence saying to mark the goal complete once the brief's outcome stands: the task stays in the brief, in one home. Other harnesses are unchanged.

Measured with codex-cli 0.166.0 on macs-Mac-Mini.local, 2026-11-03: `/goal <objective>` sets the goal and starts a turn the session records (`task_started`, `turn_trigger: goal`), so it is the brief's delivery; its setting is recorded as `thread_goal_updated` with the objective, which identifies a role session as the worker's. Codex began the next goal turn 11 ms after the last ended. The goal's status after its setting is in no session record: it is in `goals_1.sqlite` of the session's Codex home, read with `sqlite3`. `/goal clear` typed mid-turn lets that turn finish and starts no other. `goals.max_goal_token_budget` on the launch line becomes the budget of a goal set with none; past it Codex stops the turn and marks the goal `budget_limited`. A goal's tokens are uncached input and output.

A worker whose verdict stands while Codex still continues its goal is never seen stopped, so the tick would never carry the verdict out. The tick therefore reads its comments while it is busy — but only once its goal is active and Codex has continued it since its brief, so a worker at its first turn costs no GitHub read — and types `/goal clear` when a verdict stands. A bound worker's declared stage already closes its terminal. A PR's landing removes the worktree.

A waiting worker — parked, queued, on a shell of its own — whose goal stays active is continued round at cost. The fleet bounds it with a token budget on the launch line, not the operator's Codex configuration: 1,000,000, five times the uncached input and output a Codex worker used on this fleet's issues in the week to 2026-11-03. Pausing the goal for each kind of wait was rejected: Codex's own goal tools refuse `paused` without a user's request, and each wait would need its own pause and resume.

A goal Codex marked `blocked` or `budget_limited` continues no more: worker health decides such a worker short of its task without asking Jev, and it is nudged, then escalated, as any worker. The nudge and escalation stay the fallback for every harness.
