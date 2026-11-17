---
status: accepted
---

# Later issue words are told to its worker

A worker reads its issue at the start. A later body edit or comment reaches it at the next tick, delivered as `tell` delivers, with its session proof (ADR-0042), so an operator may edit a claimed issue or comment on it and need not find the worker's terminal.

The tick uses the body and comments already read in its rebuild. Under this host's claim, with a worker that took its brief and still has a terminal, it tells comments newer than that worker's start and changes from the body captured before its brief. A body change carries the changed lines, or the whole new body when no meaningful baseline is available. The worktree keeps the observed body and pending revisions, scoped to that worker's start; a newer edit cannot erase a pending one. The tracker remains the words' authoritative home. An issue's update time alone is no proof of a body edit: comments and label changes move it too.

Every comment containing a `<!--convoy:` marker is skipped. The login is no filter: people and workers use the same one. A worker's own comment is identified by the result link of a comment-writing tool call in its harness session. A read result or a user's quoted command proves no write.

A recorded user message, not a terminal receipt or a queue entry, proves delivery. An accepted send is kept beside the worktree, so later ticks look for that proof without typing the line again; a line the harness has taken but not yet given the model is queued, not failed, for a bounded wait. A refused line, or a harness at no input, fails at once. Each update fails under its own identity in the tick log, and repeated failures become a doctor finding naming the issue.

The worker decides each update: carry it out within its issue, or open another issue through the repo's flow or intake and say so on the current one. Its brief says this. With no worker here, between stages or under another host's claim, nothing is typed: the next worker reads the issue at its start. PR comments and reopening a closed issue are outside this decision.
