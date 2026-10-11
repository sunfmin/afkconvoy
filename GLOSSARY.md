# afkconvoy

A command-line tool that works a repo's ready GitHub-issue backlog unattended: it dispatches one worktree-isolated coding agent per issue and lands their finished PRs on the base branch behind a local gate. It is a trimmed Rust port of the `afk-fleet` skill (`docs/adr/0001`).

## Language

### The fleet

**Fleet instance**:
One `afkconvoy run` process and everything it owns: the workers it started and the claims it holds, under an id minted when it starts. One host runs at most one per repo.
_Avoid_: launcher, coordinator, node

**Tick**:
One reconciliation pass: rebuild the working set from fleet state, then perform every transition it calls for, without waiting for the workers it started.
_Avoid_: cycle, poll, batch

**Plan**:
A tick stopped after its rebuild, which reports what an acting tick would do and leaves nothing written: its startup probe is the only thing it pushes, and it deletes that.
_Avoid_: dry run, preview

**Startup probe**:
What `run`, `tick` and `plan` ask of a repo before anything else: that the base branch exists and no rule of its protection refuses a push, where the remote takes the fleet's refs, and whether it takes recorded gate runs. A repo the fleet cannot work is refused here, not at its first landing.
_Avoid_: bootstrap, preflight, health check

**Ref layout**:
One of the two ways every ref the fleet writes is named: hidden refs (`refs/convoy/*`), or — only where the remote rejects those — ordinary branches (`convoy-*`), on which push-triggered CI fires.
_Avoid_: namespace, fallback mode

**Fleet state**:
The authoritative record of what is claimed, in flight, landed, parked and escalated. It lives entirely in GitHub; no process holds it.
_Avoid_: run state, memory

**Working set**:
A tick's view of the fleet at one moment, derived from fleet state and dropped when the tick ends.
_Avoid_: session state

**Rebuild**:
The read-only pass at the top of every tick that derives the working set. Any fleet instance rebuilds the same working set from the same GitHub.
_Avoid_: refresh, resync

**Frontier**:
The issues that can be dispatched now: open, carrying the ready label, not an epic, unclaimed, with no open linked PR and no open blocker.
_Avoid_: queue, backlog

**Base branch**:
The one branch a repo's fleet work is cut from and lands on.
_Avoid_: trunk, main, default branch

### Workers

**Worker**:
A coding-agent session isolated in one git worktree that owns exactly one issue and reports its outcome through GitHub.
_Avoid_: agent, subagent, child

**Worker launch command**:
The one opaque shell string every worker of a fleet instance is started with. The fleet never parses it or appends to it.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped, as its agent reports it to the worker backend. Stopped never means finished.
_Avoid_: liveness, status

**Verdict**:
What a worker that opens no PR declares instead, on its issue: `already-satisfied`, `blocked`, or `needs-human`.
_Avoid_: outcome, result

**Nudge**:
The one line the fleet types at a worker that stopped with no outcome, telling it to carry on. Sent once per worker.
_Avoid_: ping, reminder, retry

**Restart**:
Replacing a silent worker with a new one in the same worktree, with its PR, branch and worktree untouched. Made once, and only for a worker that is joining the landing train.
_Avoid_: retry, re-dispatch

**Continuation**:
Recovering a claim whose worker died from the progress that survived: its worktree if still on this host, else its pushed branch, else a fresh start from the base branch.
_Avoid_: resume, checkpoint restore

**Wake**:
A contentless signal a worker sends to its host's fleet instance once its outcome is on GitHub, ending the sleep between ticks early.
_Avoid_: notification, callback, done signal

### Claims

**Claim**:
The atomic lock marking one issue as owned by one fleet instance. Its creation succeeds for exactly one instance.
_Avoid_: assignee, assignment, lock

**Heartbeat**:
The timestamp a fleet instance publishes for itself while it holds claims. A claim is live while its owner's heartbeat is within the lease.
_Avoid_: ping, keepalive

**Orphaned claim**:
A claim of this fleet instance with no PR and no live worker. It is recovered by continuation, never released.
_Avoid_: stuck issue, zombie

**Stale claim**:
Another fleet instance's claim whose heartbeat is past the lease. It is the only claim a fleet takes from another host unattended.
_Avoid_: dead claim, abandoned claim

**Takeover**:
Taking another fleet instance's claims without waiting for the lease. It happens unconditionally at start for claims stamped with this host, and by explicit command for another host's.
_Avoid_: failover, stale reclaim

### Outcomes

**Park**:
Recording a dependency a worker discovered as a blocker on its issue and releasing the claim, so the frontier holds the issue back until the blocker closes.
_Avoid_: defer, snooze, hold

**Escalate**:
Handing an issue to a human: the reason is commented, the ready label is swapped for the escalate label, and the claim is released. Every failure escalates; nothing is retried.
_Avoid_: fail, retry, give up

**Status board**:
The single comment on a claimed issue that shows a human how far along it is. It is rendered from fleet state and never read back.
_Avoid_: progress log, worklog

### Landing

**Gate**:
The repo's build-and-test command. It is the only machine verification a change must pass before it lands.
_Avoid_: CI, checks, local CI

**Recorded gate run**:
The evidence that the gate passed on one committed tree, made by the tool that saw the exit code and kept where every host can read it.
_Avoid_: gate cache, CI status

**Sync**:
Bringing a branch up to date by merging into it. Nothing is ever rebased.
_Avoid_: rebase, update branch

**Landing train**:
The one append-only line of merge commits ahead of the base branch that finished PRs join, one merge commit per PR.
_Avoid_: merge queue, merge batch, landing turn

**Join**:
Putting a finished PR on the landing train, done by the worker that wrote it.
_Avoid_: merge, enqueue

**Train worker**:
The one worker per repo that gates the landing train's tip and pushes it to the base branch as a fast-forward. It holds no claim and wrote none of the PRs it lands.
_Avoid_: merger, integrator, batch worker

**Abandon**:
Giving up a landing train whose worker stayed silent, with nothing landed; its PRs join the next train.
_Avoid_: fail, bisect
