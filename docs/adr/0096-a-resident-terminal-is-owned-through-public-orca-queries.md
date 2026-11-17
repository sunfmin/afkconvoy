---
status: accepted
---

# A resident terminal is owned through public Orca queries

Orca is external software. afkconvoy owns the resident terminals of its host runner — the watch's, “afkconvoy watch” (ADR-0040), and the asking session's, “afkconvoy asks” (ADR-0052) — through Orca's public terminal CLI and a durable launch journal of its own. It needs no upstream creation key, custom Orca build or private database access.

## Ownership is proven, never inferred from a title

A terminal's fixed title is a discovery hint. Normal checks use the saved exact handle and verify its saved public pane identity, its `ptyId` and `incarnationId`. After a missing or stale handle, a listed terminal is associated only when that saved identity matches, or when the launch shell's own acknowledgement names its handle. A title alone proves no ownership, uniqueness or idempotence. A same-name terminal with no saved identity, a changed identity, or several candidates stay open, and creation pauses with recovery instructions in `doctor` and help; renaming the unrelated sessions lets discovery proceed.

The journal precedes creation and keeps the session, its guarded command and its delivery state apart from the host runner's progress file. Before running the harness, the launch shell atomically claims its session and writes its `ORCA_TERMINAL_HANDLE` acknowledgement; without that handle it starts no harness, and the claim outlives the harness, so repeating the launch never runs a second one. A failed creation reply is reconciled against the acknowledgement and the public terminal list, never retried blindly or taken as grounds to try another harness. A confirmed terminal found missing is replaced only after its marked executor has ended. The operator's other sessions are never adopted or closed.

## Delivery is proven by the session's turn

Before each submission the journal saves a UUID request key, the exact text pointer and the intent to send. Recovery replays that exact submission with Orca's `terminal send --retry-request`. The harness's own session for the directory proves the turn; Orca's receipt and the terminal's screen do not. A send that cannot be proven or replayed pauses rather than being sent again as fresh input; neither another harness nor a restarted runner bypasses that pause. The journal's intent wins over a stale progress snapshot after a crash.

## A re-incarnated pane

When Orca restarts or restores a pane, or the session inside restarts in the same pty, Orca lists the same `ptyId` under a new handle and a new `incarnationId`. The pane is ours, re-incarnated; without a rule for it the session stays unavailable for good.

- **The watch's pane is ended and replaced.** A row whose saved `ptyId` matches and whose `incarnationId` differs: the runner ends the session's marked executor wherever it survived, closes the pane — which would otherwise block the next creation by its title — and records the launch ended. Nothing is resent into it. The next look starts a fresh watch for the work pending. A new incarnation proves neither that the harness survived nor what its screen and input hold, and a saved request key binds the old incarnation; a round is bounded work a fresh watch carries on.
- **The asking pane is taken back.** A row whose saved `ptyId`, `tabId` and `leafId` all match and whose `incarnationId` differs — Claude Code's `/restart` after an update, say. Where the marked executor still runs, the session survived: the journal takes the new handle and identity and the session goes on there. Nothing already delivered is sent again; an unresolved send counts as delivered where the session took a turn since the question was offered, and is otherwise offered afresh once input is ready, its old request key dropped. Where the executor is gone, the pane is closed as the watch's is, and a fresh asking session asks the current question again. The same pty in another tab or leaf is not the asking session's.

Neither is a repeated creation: the old creation was confirmed and its executor has ended, or is still the session, before anything starts.
