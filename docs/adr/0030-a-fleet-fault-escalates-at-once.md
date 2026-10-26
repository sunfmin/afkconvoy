---
status: accepted
---

# A fleet fault escalates at once

ADR-0018 sends every hand-off to a consult before a human. A consult cannot
change the host, and starts on the same host and harness as the worker it
would answer for. Asking it about a fault there repeats the fault and delays
the fix. So a **fleet fault** skips the consult and escalates at once, in a
bound repo and an unbound one alike.

Exactly three hand-offs are fleet faults: `its worker start failed again`,
`its harness's worktree trust gap`, and `its landing train was given up twice`.
Either the worker never took its brief or the fleet's landing machinery
stopped. The fix is on the host: trusting a worktree, switching a harness,
waiting out a usage limit, or mending a build server. The escalation comment
and `doctor`'s `waiting_on_human` entry name the fleet fault and say that the
fix is on the host. Its escalation marker records which fault it was, so a
later tick leaves it with the human instead of asking a consult.

Every other hand-off still goes to its consult first, as ADR-0018 says: an
`escalate` verdict, a tripped breaker, a `blocked` that will never clear, an
`already-satisfied` over a branch that holds work, a commit a stage may not
leave, a changed workflow, and a worker gone silent. A silent worker had its
brief, so the work may be the cause.

Nothing new retries. The operator fixes the host and hands the issue back.
Another failed start escalates at once again. This amends ADR-0018's
"every hand-off" rule; its consult bounds and the explicit human choices
stay as they are.
