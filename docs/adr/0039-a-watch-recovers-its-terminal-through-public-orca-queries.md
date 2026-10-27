---
status: accepted
---

# A watch recovers its Terminal through public Orca queries

Amended by ADR-0040 (sunfmin/control#145): the watch retains a resident
interactive session across independent rounds; looks continue while it is
busy, and reporting leaves the healthy harness standing by. The older rules
below that stop looks or end a reported harness are superseded.

Amends ADR-0038 (sunfmin/control#146). Orca is external software. afkconvoy
uses its existing Terminal CLI and its own durable launch journal; it needs
no upstream creation key, custom Orca build or private database access.

The fixed title `afkconvoy watch` is a discovery hint. Normal checks first
use the saved exact handle and verify its saved public pane identity. After
a missing or stale handle, only one candidate proven by the saved identity
or the launch shell's acknowledgement can be associated. A title alone
cannot prove ownership, uniqueness or idempotence. Unknown or ambiguous
sessions remain open, and creation pauses with recovery instructions in
`doctor` and help.

The journal precedes creation and retains the watch, guarded command,
transcript and delivery status independently of the host runner's progress
file. Before executing the harness, the launch shell atomically claims its
session and writes its `ORCA_TERMINAL_HANDLE` acknowledgement. The claim
survives exit: repeating the same launch cannot execute another harness.
Without the handle the shell starts no harness. A failed creation reply is
reconciled against that acknowledgement and the public Terminal list,
never retried blindly or treated as grounds to try another harness. A
previously confirmed missing Terminal permits recovery only after its
marked executor has ended. Continuations retain their existing termination
check before any replacement starts.

The journal also records intent before delivering the brief. A crash or
uncertain reply during delivery pauses instead of duplicating the prompt.
An operator can inspect the retained session and submit its explicit,
complete `watched` report, recording pending work. Renaming unrelated
same-name sessions lets unambiguous discovery proceed. The command, model,
directory rules, guard and process ownership bounds stay intact. Liveness,
idleness and printed output are never a report.

This decision preserves ADR-0038's ending of a reported harness and
continuation rules. Resident multi-round scheduling, continuous looks and
busy-round deduplication belong to sunfmin/control#145; a repeated lookup or
host runner replacement alone starts no new Agent.
