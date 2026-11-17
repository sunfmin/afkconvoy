---
status: accepted
---

# A ready issue no fleet takes is reported across hosts

An idle fleet publishes no heartbeat, so an absent heartbeat cannot establish
that a repo has no fleet. Every host runner's look instead checks the trackers
its checkout list names, including repo-labelled work absent from that list.
A ready issue with no open blocker or claim must have waited strictly more
than an hour since the later of its dispatch label's last application and its
last blocker closing. Only then, or for an existing report, does the look ask
that repo's heartbeat remote. A heartbeat within the lease rules out a report;
a claim leaves the issue to lease takeover. Config and team flow labels are
read from the repo on GitHub; an absent config leaves dispatch labels unknown. Label
times and blocker closure times remain the tracker's facts.

`doctor` stays read-only; the host runner reconciles its evidence. Of a GitHub
tracker (ADR-0091), several hosts may report the same work, so before opening
or closing a report the runner claims a repo-specific ref on the tracker,
`refs/heads/convoy-unworked/<owner>/<repo>`, by an atomic leased push of a
parentless empty-tree record. The record names the host and time; an
interrupted holder can be replaced after the ordinary claim lease. The winner
reads the tracker and remote again under that claim, finds existing reports by
their marker before creating any, and releases only its own claim after
success. An uncertain write leaves the claim for recovery. This extends the
cross-host claim rule of ADR-0028 to a tracker whose affected repo may have no
local checkout. A host's local tracker is written by that host alone, so its
reports need no such claim. Reports carry the affected repo's label and
escalate label, never a dispatch label. Both heartbeat ref layouts are read
without cloning that repo. An unreadable source is a finding and never
establishes that no fleet works it.

The report records its opening host. Only that host raises its finding, on every
look while open, so its watch episode and human notification survive restarts;
ordinary escalation findings exclude these reports to avoid notifying peers.
A later look on any host closes it when the reported issues are claimed or a
live heartbeat appears, naming the host that took the work. Which host works
a repo belongs to the operator: the watch asks that question and never adds
the repo to its checkout list. The report describes the work no fleet has taken,
without guessing whether a host is off, asleep or misconfigured.
