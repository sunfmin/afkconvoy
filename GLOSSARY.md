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
The issues a worker can be started on now: open, carrying a flow label, not an epic, unclaimed, with no open blocker, and at a stage a role performs. In an unbound repo: carrying the ready label, with no open linked PR.
_Avoid_: queue, backlog

**Base branch**:
The one branch a repo's fleet work is cut from and lands on.
_Avoid_: trunk, main, default branch

### Teams and roles

**Role**:
A repo that defines one kind of worker: the rules it always works under, the limits it cannot cross, and the skills it can be asked to perform. A worker starts in it.
_Avoid_: agent, persona, profile

**Team**:
A repo that names a set of roles and holds the workflows, patrols and precedents they work by.
_Avoid_: org, crew, squad

**Bound repo**:
A repo whose config names a team. An unbound repo has no roles and one fixed path for every issue: one worker, then the landing train.
_Avoid_: team mode, managed repo

**Guard**:
The check made before each thing a worker does, refusing what its role or its stage may not do. It stops a worker that errs, not one that means to get past it.
_Avoid_: permission, sandbox

### Workflows

**Workflow**:
The stages one issue passes through and where each verdict leads, ending in the landing train or in nothing more to do.
_Avoid_: pipeline, process

**Flow label**:
The label that names the workflow an issue follows. A bound repo's issue is the fleet's to work only while it carries one.
_Avoid_: ready label (the unbound repo's), type label

**Intake**:
Putting a flow label on an issue. A human does it on GitHub; a worker does it through the fleet, which records who did and holds the number of such issues still open under a limit.
_Avoid_: enroll, admit, triage

**Stage**:
One step of a workflow: one skill of one role, performed by one new worker — or a decision waited for from a human.
_Avoid_: step, phase, state

**Stage record**:
The comment a stage leaves on its issue: what it produced, its verdict, and the versions of the role and team it ran under. The stage an issue is at is derived from these and stored nowhere.
_Avoid_: handoff note, log, stage label

**Rework**:
A stage running again because a later stage rejected its work or a consult ruled on it. It is counted per issue from the stage records.
_Avoid_: retry, round, attempt

**Breaker**:
A limit on rework — in total for an issue, or as rejections in a row by one stage — past which the issue escalates.
_Avoid_: retry limit, circuit

**Consult**:
A worker in a more senior role, asked to rule on a stage's `escalate` verdict before a human is. It rules or lets the escalation through; it writes no code.
_Avoid_: review, second opinion, handoff

**Ruling**:
What a consult or a human decides on an escalated issue. The stage that asked runs again under it.
_Avoid_: answer, decision, override

**Precedent**:
A ruling kept so that later stages follow it instead of escalating. A worker proposes it; a human accepts it.
_Avoid_: rule, policy, guideline

**Split**:
Parking an issue on issues its own stage opened. When they have all closed, that stage runs again and judges the whole.
_Avoid_: decompose, epic, fan out

**Patrol**:
A role run on a schedule rather than for an issue. It opens issues and takes them in; it writes no code.
_Avoid_: cron, job, scheduled worker

**Fleet lock**:
An open issue carrying the lock label. While one stands the fleet works lock issues only, and closing the last one ends it.
_Avoid_: freeze, pause, kill switch

### Workers

**Worker**:
A coding-agent session started in one role to perform one stage of one issue, in that issue's worktree, and to report through GitHub. In an unbound repo it has no role and owns its issue from start to landing.
_Avoid_: agent, subagent, child

**Worker launch command**:
The one shell string every worker of a fleet instance is started with. In an unbound repo it is opaque: the fleet never parses it or appends to it. In a bound repo the fleet adds to it what puts the worker in its role.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped, as its agent reports it to the worker backend. Stopped never means finished.
_Avoid_: liveness, status

**Verdict**:
What ends a stage, declared on the issue by whoever performed it: `pass`, `reject`, `blocked`, `already-satisfied`, or `escalate`. Opening a PR declares nothing.
_Avoid_: outcome, result, needs-human

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
Handing an issue to a human: the reason is commented, the flow label is swapped for the escalate label, and the claim is released. Every failure escalates and nothing is retried; an `escalate` verdict escalates unless a consult rules on it first.
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
