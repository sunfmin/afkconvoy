---
status: accepted
---

# Later issue words are told to its worker

A worker reads its issue at the start. A later body edit or comment used to
reach only the next worker: sunfmin/control#343 needed an operator to find
the terminal, and #347 added `tell` for that manual delivery. The operator
then avoided edits to claimed issues. sunfmin/control#408 replaces that rule
with delivery at each tick, using tell's session proof (ADR-0042).

The tick uses the body and comments already read in its rebuild. Under this
host's claim, with a worker that took its brief and still has a terminal,
it tells comments newer than that worker's start and changes from the body
captured before its brief. A body change carries the changed lines, or the
whole new body when no meaningful baseline is available. The
worktree keeps the observed body and pending revisions, scoped to that
worker's start; a newer edit cannot erase a pending one. GitHub remains the
words' authoritative home.

A worker started before this delivery mechanism has no body snapshot. Where
GitHub records an edit after its start, its first tick tells the whole new
body and establishes the snapshot. An issue update timestamp alone is not
proof of a body edit: comments and label changes move it too.

Every comment containing a `<!--convoy:` marker is skipped. The login is no
filter: people and workers use the same one. A worker's own comment is
identified by the result link of a comment-writing tool call in its harness
session. A read result or a user's quoted command proves no write.

An accepted terminal send is kept beside the worktree so later ticks poll
for session proof without retyping queued input. A tick waits briefly, then
retries an untold update next tick without failing the pass. An accepted
line its harness has not given the model yet — a harness running a tool
hands over its queued input when the tool ends — is recorded queued, not
failed, until the bound a typed note has to reach the model; past it, each
tick it is still not taken fails (sunfmin/control#525). A refused line, or
a harness at no input, fails at once. Each update
has its own failure identity in the tick log: three consecutive failures
produce a doctor finding naming the issue and its link. A recorded user
message, rather than a terminal receipt or queue entry, proves delivery.

The worker decides each update: carry it out within its issue, or open
another issue through the repo's flow/intake and say so on the current one.
Its brief says this. With no worker here, between stages or under another
host's claim, nothing is typed: the next worker reads the issue at its start.
PR comments and reopening a closed issue remain outside this decision.
