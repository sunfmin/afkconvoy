---
status: accepted
---

# A resident watch receives independent rounds

Amends ADR-0030, ADR-0035, ADR-0038 and ADR-0039 for sunfmin/control#145.
Their rules that an active watch stops looks, a report ends the harness,
and each later watch starts a new Agent no longer apply. Authority, guards,
the four human decisions, resource episode recovery spans and harness
selection remain unchanged.

The host runner owns one resident interactive Terminal Agent. It keeps
independent watch rounds: each has an ID, brief, episode reservations,
delivery state, report and hour of execution including continuations.
Queueing, Terminal startup, standby and earlier rounds consume no new
round's hour. A report completes only its named round. The same report
can be repeated; a changed report or a late report for another round is
refused. Reports and scheduler transitions share a durable lock.

Looks continue at `watch_every`, 300 seconds by default, without a model.
The host runner atomically persists each observation interval and its
pending queue before launch or input. Conditions retain an episode identity
until observed recovery. A recovered-and-returned condition can queue a new
episode even while the previous one is executing. An active round reserves
only its own episodes; an explicit report covers them afterwards. Human
questions already reported stay covered. Failed unreported work is eligible
again, preserving ADR-0038's correction for sunfmin/control#138.

Before delivery, a fresh look removes recovered conditions. Delivery waits
for the previous round's report and this Terminal's input readiness; it
never types into a busy Agent. A healthy session remains after reporting.
Standby without pending work sends no prompt and runs no model. Idle is not
completion of an unreported round.

The launch journal from ADR-0039 still fences creation and owns exactly one
executor. Its session identity is independent of round IDs. Before every
round submission it persists a UUID request key, exact brief pointer and
send intent. Orca's supported `terminal send --retry-request` replays that
exact mutation on recovery. The harness's session for this directory
proves the turn; the receipt and the Terminal's screen do not. An
accepted send with no session turn stays unproven.
Each round retains its delivery archive after the session advances. Unproven or unsupported delivery remains explicit and paused,
never retried as fresh input. The journal's round intent wins over a stale
progress snapshot after a crash. Creation ambiguity retains ADR-0039's
pause and recovery instructions; no external Orca change is required.

Exit, a proven quota/harness/context failure or an explicit directory reset
ends the owned marked executor before any replacement. Unfinished rounds
retain their look, progress evidence and original execution deadline across
continuations. The operator's other sessions are never adopted or closed.
Doctor, panel and tick logs distinguish resident session state, the current
round, pending episodes and individual reports.

Keeping one Agent per round would discard useful session context and break
the continuously usable Terminal. Storing the queue in conversation alone
would lose delivery ownership on restart. Persistent rounds preserve both
the interactive surface and a deterministic scheduler with durable truth.
