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
A tick stopped after its rebuild, which reports what an acting tick would do and leaves nothing written: its startup probe, when it makes one, is the only thing it pushes, and it deletes that.
_Avoid_: dry run, preview

**Startup probe**:
What `run`, `tick` and `plan` ask of a host and a repo before anything else: that every command the fleet needs is installed and logged in, that orca is running and knows the checkout, that the base branch exists and no rule of its protection refuses a push, where the remote takes the fleet's refs, and whether it takes recorded gate runs. A host or a repo the fleet cannot work is refused here, not at its first tick or landing. `run` makes one at every start; one that passed stands for an hour on the host that made it, and `tick` and `plan` make none while it does.
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
The read-only pass at the top of every tick that derives the working set. Any fleet instance rebuilds the same working set from the same GitHub. It lists the remote once and asks GitHub a fixed number of times, however many issues are open.
_Avoid_: refresh (a heartbeat is refreshed), resync

**Frontier**:
The issues a worker can be started on now: open, carrying a flow label, not an epic, unclaimed, with no open blocker, and at a stage a role performs or whose `escalate` a consult is to rule on — or landing, with an open PR to join. In an unbound repo: carrying the ready label, with no open linked PR but one of the fleet's own that a released claim kept, off the landing train — which its worker takes up. While a fleet lock stands: of these, the lock issues only.
_Avoid_: queue, backlog (all the issues a repo has open)

**Idle**:
A fleet instance with nothing to do now: it holds no claim — on an issue, a patrol's run or the proposal of a precedent — nothing is on its landing train, and its frontier is empty. An escalated issue waiting for a human leaves it idle, and so does a patrol not yet due.
_Avoid_: drained, done, finished

**Host sleep**:
A fleet instance putting its host to sleep: every fleet instance on the host has been idle for ten minutes, each has ticked once more and found nothing, no one has used the host's keyboard or mouse for ten minutes, the one that puts it to sleep has worked since the host last woke, and no instance on the host was told not to. A host woken stays awake until some instance on it has worked again.
_Avoid_: sleep (the wait between ticks), hibernate, suspend, shutdown

**Base branch**:
The one branch a repo's fleet work is cut from and lands on.
_Avoid_: trunk, main, default branch (a team's or a role's repo is read at its own)

### Teams and roles

**Role**:
A directory that defines one kind of worker: the rules it always works under, the limits it cannot cross, and the skills it can be asked to perform. It is a whole repo or a subdirectory of one. A worker starts in it, and is given nothing else of its repo.
_Avoid_: agent (as under Worker), persona, profile

**Team**:
A directory that names a set of roles and holds the workflows, patrols and precedents they work by. It is a whole repo or a subdirectory of one, and its roles may be directories of the same repo.
_Avoid_: org, crew, squad

**Team rules**:
The rules every role of a team works under, whatever its own rules add. They say how the team works, and hold in any repo bound to it; what one codebase asks is that repo's to say, and what one kind of worker does is its role's.
_Avoid_: global rules, shared rules

**Bound repo**:
A repo whose config names a team. An unbound repo has no roles and one fixed path for every issue: one worker, then the landing train.
_Avoid_: team mode, managed repo

**Guard**:
The check made before each thing a worker does, refusing what its role or its stage may not do. It stops a worker that errs, not one that means to get past it.
_Avoid_: permission (what a harness's own settings deny), sandbox (what a harness may shut a worker's commands in)

### Workflows

**Workflow**:
The stages one issue passes through and where each verdict leads, ending in the landing train or in nothing more to do.
_Avoid_: pipeline, process (a program that is running)

**Flow label**:
The label that names the workflow an issue follows. A bound repo's issue is the fleet's to work only while it carries one.
_Avoid_: ready label (the unbound repo's), type label

**Intake**:
Putting a flow label on an issue. A human does it on GitHub; a worker does it through the fleet, which records who did and holds the number of such issues still open under a limit.
_Avoid_: enroll, admit, triage

**Stage**:
One step of a workflow: one skill of one role, performed by one new worker — or a decision waited for from a human.
_Avoid_: step (one thing a tick does), phase, state (fleet state and worker state are terms)

**Stage record**:
The comment a stage leaves on its issue: what it produced, its verdict, the versions of the role and team it ran under, and the harness its worker ran on. A human who declares for a stage, rules on it or sends the issue to it leaves one too, which says a human did; so does the fleet, for a stage that runs again because a fleet lock closed. The stage an issue is at is derived from these and stored nowhere.
_Avoid_: handoff note, log (the tick log; what a gate prints), stage label

**Position**:
Where an issue is in its workflow: at a stage a worker is to perform, waiting on a human, with a consult, parked, landing, ended, or escalating. It is derived from the stage records each tick and stored nowhere.
_Avoid_: state (as under Stage), status (the status board; a key of what a command answers), current stage

**Rework**:
A stage running again because a later stage rejected its work or a consult ruled on it. It is counted per issue from the stage records, from where a human last ruled on the issue or sent it to a stage.
_Avoid_: retry, round, attempt (what an issue was worked into before a fresh start)

**Breaker**:
A limit on rework — in total for an issue, or as rejections in a row by one stage — past which the issue escalates.
_Avoid_: retry limit, circuit

**Consult**:
A worker in a more senior role, asked to rule on a stage's `escalate` verdict before a human is. It rules or lets the escalation through; it writes no code.
_Avoid_: review (a stage may be named it), second opinion, handoff

**Ruling**:
What a consult or a human decides on an escalated issue. The stage that asked runs again under it; a human's also hands the issue back to the fleet.
_Avoid_: answer (what a command prints), decision (what a `by: human` stage waits for), override

**Precedent**:
A ruling kept so that later stages follow it instead of escalating. A worker proposes it; a human accepts it.
_Avoid_: rule (a role's rules and the team rules; the command `rule`), policy, guideline

**Proposal**:
One human's ruling put forward as a precedent: a PR on the team's repo that appends one entry to its precedents file. A ruling gets one; a human merging it is what accepts the precedent.
_Avoid_: suggestion, draft, candidate (a key of what `recovery` answers)

**Split**:
Parking an issue on issues its own stage opened. When they have all closed, that stage runs again and judges the whole.
_Avoid_: decompose, epic (an issue under an epic label), fan out

**Patrol**:
A role run on a schedule rather than for an issue. It opens issues and takes them in; it writes no code. Its run takes one slot until its worker declares it over, which the next tick acts on; a worker that never does is ended once it is gone or has stopped for a grace period. Each run has a checkout of its own, removed as the run ends unless it failed.
_Avoid_: cron, job, scheduled worker

**Fleet lock**:
An open issue carrying the lock label, in a bound repo or an unbound one. While one stands the fleet works lock issues only — nothing else is dispatched, no PR of another issue joins the landing train, and workers already running are left alone, what they declare being acted on afterwards — and closing the last one ends it. A stage's `escalate` or failure the lock explains — declared before it closed, and not acted on since — is then not carried out: the stage runs again, which is no rework.
_Avoid_: freeze, pause, kill switch

### Workers

**Harness**:
The program a worker is a session of: it runs a model, with tools, on the work it is handed. Claude Code is one, pi another, Codex the third. A stage names the one its worker runs, or takes the fleet's default.
_Avoid_: agent (as under Worker), coding agent (as under Worker), CLI, provider (whose models a harness runs)

**Worker**:
A session of one harness, started in one role to perform one stage of one issue — or, after its last stage, to join its PR to the landing train — in that issue's worktree, and to report through GitHub. In an unbound repo it has no role and owns its issue from start to landing. It is ended — its terminal closed — by the step that ends what it was started for: its stage, its claim, or its run.
_Avoid_: agent (whoever reads the help; and a worker, to whoever has not met the term), subagent, child (an issue a split opened)

**Worker launch command**:
The shell string every worker of one harness is started with, in a fleet instance: one for each harness its workers run. The host says it, per harness, and a harness it says none for has a stock one; a start checks that of every harness it could be asked for, and starts no worker when one cannot be started. In an unbound repo it is opaque: the fleet never parses it or appends to it. In a bound repo the fleet adds to it what puts the worker in its role.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped, as its harness reports it to the worker backend — or, where the backend has no report of it, as its terminal's last output shows. Stopped never means finished.
_Avoid_: liveness, status (as under Position)

**Verdict**:
What ends a stage, declared on the issue by whoever performed it: `pass`, `reject`, `blocked`, `already-satisfied`, or `escalate`. Opening a PR declares nothing.
_Avoid_: outcome (what a worker leaves on GitHub: its PR on the landing train, or its verdict), result, needs-human

**Nudge**:
The one line the fleet types at a worker that stopped with no outcome, telling it to carry on. Sent once per worker.
_Avoid_: ping, reminder, retry

**Restart**:
Replacing a silent worker with a new one in the same worktree and the same role, with its PR, branch and worktree untouched. Made once, and only for a worker that is joining the landing train.
_Avoid_: retry, re-dispatch

**Continuation**:
Recovering a claim whose worker died from the progress that survived: its worktree if still on this host, else its pushed branch, else a fresh start from the base branch.
_Avoid_: resume, checkpoint restore

**Wake**:
A contentless signal a worker sends to its host's fleet instance once its outcome is on GitHub, ending the sleep between ticks early.
_Avoid_: notification, callback, done signal

### Claims

**Claim**:
The atomic lock marking one issue as owned by one fleet instance. Its creation succeeds for exactly one instance. It takes one slot of the `concurrency` while its worker is at work, and none once a verdict is declared under it: it is still held until that verdict is acted on, and a worker started under it again takes a slot as any start does.
_Avoid_: assignee, assignment, lock (the fleet lock)

**Heartbeat**:
The timestamp a fleet instance publishes for itself while it holds claims, stamped with its host. A claim is live while its owner's heartbeat is within the lease. A `run` told to stop while it holds no claim removes its own; a start removes the dead ones that hold nothing: those of the instances it took from, its own host's past the lease, and anyone's older than a day.
_Avoid_: ping, keepalive

**Orphaned claim**:
A claim of this fleet instance with no PR and no live worker. It is recovered by continuation, never released.
_Avoid_: stuck issue, zombie

**Stale claim**:
Another fleet instance's claim whose heartbeat is past the lease. It is the only claim a fleet takes from another host unattended.
_Avoid_: dead claim, abandoned claim

**Takeover**:
Taking another fleet instance's claims — on issues, on patrols' runs, on proposals — without waiting for the lease. It happens unconditionally at start for claims stamped with this host, and by explicit command for another host's. A claim taken — by a takeover, or as a stale claim — is still the claim it was created as: what was declared under it stands.
_Avoid_: failover, stale reclaim

### Outcomes

**Park**:
Recording a dependency a worker discovered as a blocker on its issue and releasing the claim, so the frontier holds the issue back until the blocker closes.
_Avoid_: defer, snooze, hold (a fleet instance holds its claims)

**Escalate**:
Handing an issue to a human: the reason is commented, with the line that answers it, the flow label is swapped for the escalate label, and the claim is released. Every failure escalates and nothing is retried; an `escalate` verdict escalates unless a consult rules on it first. The issue then waits for a human, with every other open issue only one can move: `plan`, `doctor` and the panel list them, each with its line.
_Avoid_: fail (what a command, a gate or one thing a tick does may), retry, give up (a landing train is given up: Abandon)

**Status board**:
The single comment on a claimed issue that shows a human how far along it is, and what the afkconvoy that works it was built from. It is rendered from fleet state and never read back.
_Avoid_: progress log, worklog

**Panel**:
The web page that shows a human where the whole fleet stands: each workflow as the line of its stages, the issues at work, queued and held back at each, and the landing train. A fleet instance serves it on its own host, made anew from what each tick read; it is never read back.
_Avoid_: dashboard, status page, status board (the comment on one issue), UI

**Tick log**:
The file a fleet instance's `run` keeps on its own host: one line as it starts and one for each tick, cut back to its newest lines at a bound. It is a log and not fleet state: nothing reads it to decide what the fleet does.
_Avoid_: state file, journal, history

**Finding**:
One thing `doctor` says is wrong with the fleet on this host, with how bad it is, what shows it, its fix — a line to run, or a file and the key in it to edit — and the help topic that explains it. `doctor` fixes none itself.
_Avoid_: diagnosis, alert, warning (one of its severities), error (what a command that failed answers with)

### Landing

**Gate**:
The repo's build-and-test command. It is the only machine verification a change must pass before it lands.
_Avoid_: CI (what a push to an ordinary branch starts), checks (what `check` and the startup probe make), local CI

**Recorded gate run**:
The evidence that the gate passed on one committed tree, with how long the run took and on which host, made by the tool that saw the exit code and kept where every host can read it.
_Avoid_: gate cache, CI status

**Sync**:
Bringing a branch up to date by merging into it. Nothing is ever rebased.
_Avoid_: rebase, update branch

**Landing train**:
The one append-only line of merge commits ahead of the base branch that finished PRs join, one merge commit per PR.
_Avoid_: merge queue, merge batch, landing turn

**Join**:
Putting a finished PR on the landing train. In an unbound repo the worker that wrote it does; in a bound repo a new worker does, in the landing role its workflow names, after the last stage passed.
_Avoid_: merge (what git does to two branches, and a human to a proposal's PR), enqueue

**Train worker**:
The one worker per repo that gates the landing train's tip and pushes it to the base branch as a fast-forward. It holds no claim and wrote none of the PRs it lands.
_Avoid_: merger, integrator, batch worker

**Abandon**:
Giving up a landing train whose worker stayed silent, with nothing landed; its PRs join the next train.
_Avoid_: fail (as under Escalate), bisect
