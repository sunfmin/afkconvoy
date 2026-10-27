---
status: accepted
---

# An inspection becomes the host's watch

Amended by ADR-0040 (sunfmin/control#145): the watch retains a resident
interactive session across independent rounds; looks continue while it is
busy, and reporting leaves the healthy harness standing by. The older rules
below that stop looks or end a reported harness are superseded.

Supersedes ADR-0026. Amends ADR-0019 and ADR-0029: their resource limits,
readings, evidence and episode boundaries remain; each condition calls for
one host watch through a finding in the look. There is no separate inspection
agent, brief, claim, turn, time limit or `inspected` command.

The watch already operates for every checkout on the host (ADR-0030,
sunfmin/control#79), and every host has one (ADR-0033). Two operators with the
same job duplicated authority, guards and reporting while the watch still had
to handle faults outside the inspection's triggers.

A hot worker is a finding immediately. Admission holding back for one reason
for ten minutes, or the host's own span, is a finding. Memory pressure held
for that span is a separate host finding, once an episode; it changes no
admission decision and makes no worker hot. Five minutes recovered ends each
resource episode, as before. The condition, its duration, the worker or host
reading and the evidence belong to the look given to the watch. Host evidence
is taken only when a watch is about to start; worker evidence is still taken
as the worker turns hot, before its process can disappear.

Ticks and resource sampling continue while a watch runs. No look is taken
and no second operator starts. The first look after it ends sees any new
conditions still present. Each resource episode has a stable identity;
changing readings and brief recovery do not start a second watch. Resource
episodes and watch coverage survive a host runner replacement. Conditions
that recovered while waiting start nothing.

The watch keeps its one hour, continuations together, its harness selection,
its directory's rules and its authority across the checkout list. Its printed
transcript is the record. Doctor, the panel and the tick log call it a watch.
Its guard uses `AFKCONVOY_WATCH`, with the bounds formerly applied to the
operator: only fleet-owned processes, no host runner termination, no deletion
of irreplaceable files or uncommitted work, no force push or base branch push.

`land --watch` joins a watch's change from its own linked worktree on no issue.
Its join commit names the watch and host. Fleet locks, gate turns, abandonment
and rejoining work as before. Existing join commits attributed to an inspection
are still read as watch joins, so an upgrade leaves no PR behind on a train.
