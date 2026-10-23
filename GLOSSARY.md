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
_Avoid_: run state, memory (what a host holds resident, as under Hot worker)

**Working set**:
A tick's view of the fleet at one moment, derived from fleet state and dropped when the tick ends.
_Avoid_: session state

**Rebuild**:
The read-only pass at the top of every tick that derives the working set. Any fleet instance rebuilds the same working set from the same GitHub. It lists the remote once and asks GitHub a fixed number of times, however many issues are open.
_Avoid_: refresh (a heartbeat is refreshed), resync

**Frontier**:
The issues a worker can be started on now: open, carrying a flow label, not an epic, unclaimed, with no open blocker, and at a stage a role performs or whose `escalate` a consult is to rule on — or landing, with an open PR to join. In an unbound repo: carrying the ready label, with no open linked PR but one of the fleet's own that a released claim kept, off the landing train — which its worker takes up. While a fleet lock stands: of these, the lock issues only.
_Avoid_: queue (the exclusive queue of a host's heavy commands), backlog (all the issues a repo has open)

**Idle**:
A fleet instance with nothing to do now: it holds no claim — on an issue, a patrol's run or the proposal of a precedent — nothing is on its landing train, and its frontier is empty. An escalated issue waiting for a human leaves it idle, and so does a patrol not yet due.
_Avoid_: drained, done (what a step or a command did), finished (a PR its worker is through with)

**Host sleep**:
A fleet instance putting its host to sleep: every fleet instance on the host has been idle for ten minutes, each has ticked once more and found nothing, no one has used the host's keyboard or mouse for ten minutes, the one that puts it to sleep has worked since the host last woke, and no instance on the host was told not to. A host woken stays awake until some instance on it has worked again.
_Avoid_: sleep (the wait between ticks), hibernate, suspend, shutdown

**Base branch**:
The one branch a repo's fleet work is cut from and lands on.
_Avoid_: trunk, main, default branch (a team's or a role's repo is read at its own)

### Teams and roles

**Role**:
A directory that defines one kind of worker: the rules it always works under, the limits it cannot cross, and the skills it can be asked to perform. It is a whole repo or a subdirectory of one. A worker starts in it, and is given nothing else of its repo.
_Avoid_: agent (as under Worker), persona, profile (as under DSH profile)

**Team**:
A directory that names a set of roles and holds the workflows, patrols and precedents they work by. It is a whole repo or a subdirectory of one, and its roles may be directories of the same repo.
_Avoid_: org, crew, squad

**Team rules**:
The rules every role of a team works under, whatever its own rules add. They say how the team works, and hold in any repo bound to it; what one codebase asks is that repo's to say, and what one kind of worker does is its role's.
_Avoid_: global rules, shared rules

**Admission**:
Whether a tick starts new workers on its host: not while the host is past a share of its load, under memory pressure, or low on free disk under the checkout. What it holds back stays on the frontier for the next tick; workers already running, the landing train and the worker a declared verdict starts are never held back. The shares have defaults taken from the host's cores, memory and disk, and the host can say its own. Holding back for one reason — the load, memory or disk — at every tick past a span (ten minutes, or the host's own) starts an inspection of the host, once an **episode**: the episode ends once admission has admitted for five minutes. Holding back for reasons that change starts none.
_Avoid_: throttling, backpressure

**Host user's skills and rules**:
The skills and the rules the host's own user has for a harness, which a session they start by hand loads: a worker of a bound repo is given them beside its role's, unless its team file says `host_user: false`. A role may say it for itself, whatever the team says: `reviewer: { dir: ./roles/reviewer, host_user: false }` in `roles`; a role named by its directory alone takes the team's. Its role's skills win over them, and the brief, its role's rules, the team rules and the repo's own rules win over theirs. DSH alone may load those too when its role chooses the host user's DSH profile.
_Avoid_: global rules, user config, personal skills

**DSH profile**:
What a DSH worker loads of bundles, plugins, MCP servers, patches, themes and settings: the fleet's own (`dsh_profile: fleet`, the default), or the host user's `dsh-tui` profile (`dsh_profile: host-user`). The team chooses for every role; a role may choose its own, independently of `host_user`, and both keep the guard and the role's limits.
_Avoid_: provider profile, persona

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
The label that names the workflow an issue follows: `flow:<name>`, unless the workflow's file names another with `label` — the ready label, say, so that an issue opened the usual way is worked. A bound repo's issue is the fleet's to work only while it carries one; there the ready label is one only where a workflow names it.
_Avoid_: type label

**Intake**:
Putting a flow label on an issue. A human does it on GitHub; a worker does it through the fleet, which records who did and holds the number of such issues still open under a limit.
_Avoid_: enroll, admit (a tick starting new workers on a host not busy: admission), triage

**Stage**:
One step of a workflow: one skill of one role, performed by one new worker — or a decision waited for from a human.
_Avoid_: step (one thing a tick does), phase, state (fleet state and worker state are terms)

**Stage record**:
The comment a stage leaves on its issue: what it produced, its verdict, the versions of the role and team it ran under, and the harness its worker ran on, with the model where it was started on one. A human who declares for a stage, rules on it or sends the issue to it leaves one too, which says a human did; so does the fleet, for a stage that runs again because a fleet lock closed. The stage an issue is at is derived from these and stored nowhere.
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
A worker in a more senior role, asked before a human is whenever its stage would hand the issue to a human — an `escalate` verdict, a failure, a tripped breaker, a workflow changed under the issue. It stands where that human stands: told what the human would be, and free to run what a human answering runs. It rules, sends the issue to a stage, closes it, or lets it through; it commits, pushes and changes the host never. The same hand-off again, and its own failure, go to a human. Where a stage names none, and in an unbound repo, the **stock consult** stands in: afkconvoy's own, in no role of a team, whose one instruction is to decide as the human would. A stage that says `escalate: { to: human }`, or an unbound repo whose config says `escalate_to: human`, has no consult.
_Avoid_: review (a stage may be named it), second opinion, handoff, default consult

**Ruling**:
What a consult or a human decides on an issue handed on. The stage runs again under it; a human's also hands the issue back to the fleet and starts the rework counts over, a consult's starts none over.
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
A role run on a schedule rather than for an issue. It opens issues and takes them in; it writes no code. It is due once its interval has passed since its last run, and — where its file says `max_runs_a_day` — while fewer than that many of its runs started in the last 24 hours, rolling. Where it says `max_landed_a_day`, it is also held while that many issues it opened were closed as completed in the last rolling 24 hours, until enough completions are 24 hours old to fall below the bound. Its run takes one slot until its worker declares it over, which the next tick acts on; a worker that never does is ended once it is gone or has stopped for a grace period. Each run has a checkout of its own, removed as the run ends unless it failed.
_Avoid_: cron, job, scheduled worker

**Fleet lock**:
An open issue carrying the lock label, in a bound repo or an unbound one. While one stands the fleet works lock issues only — nothing else is dispatched, no PR of another issue joins the landing train, and workers already running are left alone, what they declare being acted on afterwards — and closing the last one ends it. A stage's `escalate` or failure the lock explains — declared before it closed, and not acted on since — is then not carried out: the stage runs again, which is no rework.
_Avoid_: freeze, pause, kill switch

### Workers

**Harness**:
The program a worker is a session of: it runs a model, with tools, on the work it is handed. Claude Code is one, pi another, Codex the third, and DeepSeek Harness (DSH) the fourth. An issue's casting names the one its workers run, within their role's range; the host kit names the stock consult's. A host where a switch chose one starts every worker of the repo on that one.
_Avoid_: agent (as under Worker), coding agent (as under Worker), CLI, provider (whose models a harness runs)

**Roster**:
A team's list, for each of its roles, of the harnesses and models that role's workers may run, in order of preference, with what each model is good at and how an issue is to be cast among them. It is committed with the team; a role's range on a host is the part of it the host kit holds, and an unbound repo, having no team, ranges over the host kit whole.
_Avoid_: harness config, model table, routing rules

**Host kit**:
The harnesses and models one host can start now — each harness's worker launch command, each model it was seen to answer with, in order of preference, and the harness and model of its stock consult. `afkconvoy kit` asks each harness for its models, or the human names them, sends each a one-line prompt, and keeps those that answered, with how long they took and when; the human sets the order. Its **first** is its first harness on that harness's first model, which the stock consult runs where the host kit names none for it. Kept on that host and never pushed; a harness paused on the host is left out of it until it works again.
_Avoid_: roster (the team's), host config, inventory

**Casting**:
The harness and model an issue's workers run, named by the issue's `harness:` and `model:` labels — and the act of choosing them, by a human or by a caster. A stage whose range holds the casting runs it, and one whose range does not runs its range's first; an issue none of whose stages has a choice to make is not cast. In an unbound repo the host kit is the range: its worker runs the casting where the host kit holds it, and the host kit's first where the issue has none; one it does not hold is not dispatched, and `plan` says which label and what the host kit holds. A harness named with no model starts its command as written. A label a human put on is never recast.
_Avoid_: assignment (a GitHub assignee), routing, dispatch (starting a worker)

**Caster**:
A worker in no role that casts, in one session, every open issue of the repo's that carries a flow label, has a choice to make and no casting, by the roster and the host kit.
_Avoid_: assigner, router, dispatcher

**Worker**:
A session of one harness, started in one role to perform one stage of one issue — or, after its last stage, to join its PR to the landing train — in that issue's worktree, and to report through GitHub. In an unbound repo it has no role and owns its issue from start to landing. It is ended — its terminal closed — by the step that ends what it was started for: its stage, its claim, or its run.
_Avoid_: agent (whoever reads the help; and a worker, to whoever has not met the term), subagent, child (an issue a split opened)

**Switch**:
Moving running work onto another harness in one command: the host's choice of harness for a repo, kept on that host and never pushed — whoever else works the repo runs the harness they have — and each worker named ended and one of that harness started in its place, in the same worktree, on the same branch, at the same stage, briefed to continue what is there. Every worker the host starts for the repo afterwards is of the chosen harness, whatever an issue's casting names, until a switch chooses again or the choice is cleared — `switch --clear` forgets it — and the casting stands again: every worker started afterwards, and each the clear names, runs what its range and casting say. The claim, the branch and the PR are untouched, the switch is recorded on the PR, and it is no restart, rework or nudge.
_Avoid_: harness change (what a new casting makes)

**Worker launch command**:
The shell string every worker of one harness is started with, in a fleet instance: one for each harness its workers run. The host kit says it, per harness, kept on the host at `$XDG_CONFIG_HOME/afkconvoy/kit.json` or `$HOME/.config/afkconvoy/kit.json` and edited with `afkconvoy kit`; a harness absent from it is refused; a start checks that of every harness it could be asked for, and starts no worker when one cannot be started. In an unbound repo it is opaque: the fleet never parses it, appends to it nothing but its harness's model flag where the worker runs a model — its issue's casting, the host kit's first, or the stock consult's — and only starts it — as every worker — under `nice`, with a `TMPDIR` of its worktree's own. In a bound repo the fleet adds to it what puts the worker in its role.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped, as its harness reports it to the worker backend — or, where the backend has no report of it, as its terminal's last output shows. Stopped never means finished.
_Avoid_: liveness, status (as under Position)

**Verdict**:
What ends a stage, declared on the issue by whoever performed it: `pass`, `reject`, `blocked`, `already-satisfied`, or `escalate`. Opening a PR declares nothing.
_Avoid_: outcome (what a worker leaves on GitHub: its PR on the landing train, or its verdict), result, needs-human

**Silent worker**:
A worker that stopped, declared nothing, and for the grace period showed no sign of life — its terminal's output, a commit, a file touched, being started or nudged — while waiting on no background work of its own: a shell or a monitor it started that its harness shows still running, or a command of its in the exclusive queue. One waiting on such work is not silent until that work ends and it stays still past its nudge, or until it has waited longer than the gate limit.
_Avoid_: hung, idle (a fleet instance with nothing to do)

**Hot worker**:
A worker whose processes — every one started with its `TMPDIR`, and every one under those — take more than their share of the host, or whose worktree has outgrown its build. Its processes: more than half the host's cores (one at the least) held for two minutes, more than a quarter of its memory resident, the most of any worker while the host is under memory pressure, or more written to disk in ten minutes than a tenth of the disk free (a GiB at the least). Its worktree: what git ignores there past `OUTGROWN_TIMES` its **fresh build** and past `OUTGROWN_FLOOR` — the train worktree's rule, judged once its first green `afkconvoy gate` has recorded that fresh build. The host can say other limits (`--hot-cores`, `--hot-memory`, `--hot-written`). `run` samples each worker every ten seconds, with no root, and measures its worktree every minute; one that has stayed under every limit for five minutes — its worktree back under the rule among them — stops being hot, and that ends its **episode**: hot again later is a new one. The landing train's gate runs are the fleet's own and no worker's.
_Avoid_: hog

**Inspection**:
A worker in the role of the consult of a hot worker's stage — the stock consult where the stage names none, at no stage, and in an unbound repo — started in a checkout of its own once per episode, and given what was taken down as that worker turned hot (its process tree, a `sample` of its hottest process, its resident memory over the last minutes, what its processes wrote, the largest directories of its worktree), the issue, the worktree and the hot worker's terminal. It finds the work wrong, that heavy, or what takes so much not the worker's, and says so with `afkconvoy inspected`; the fleet types the first two at the hot worker and leaves what it found as a comment on the issue that is no stage record. One runs on a host at a time, across its fleets, takes no slot of the `concurrency`, and has 15 minutes. It is held as a consult and hands nothing on; nobody inspects where the stage or the repo says `human`, and a worker hot again after its inspection is never escalated for it. An inspection of the **host** is started by admission instead, on no worker: the stock consult, given what was taken down as it started — admission's reading and shares; for disk, each of the host's worktrees of every fleet with its issue, its stage, whether a worker holds it and what it holds, the landing train's, the fleet's caches and the largest directories of the whole volume under the home folder outside them, found within five minutes; for load or memory, the process trees of each worker and of the train and the hottest processes outside them. It finds what holds it `reclaimable` — something under the home folder can be rebuilt, which its `afkconvoy inspected` frees once it has checked each path: what git ignores in a git checkout no process uses and no worker holds, never a file git tracks or a change no commit holds, a closed issue's worktree holding no change, a tool's cache; what cannot be rebuilt or is expensive to fetch again — model weights, downloads, media, Docker's images, a user's files — it names, and nothing deletes — `fleet` — the fleet's own shape outgrows the host, for which the fleet opens one issue with the escalate label, and none other while it is open — or `not-its`; the log, the panel and `doctor` show it beside admission's reading. Of either kind, one runs on a host at a time, and none where another fleet of the host inspected it in the episode.
_Avoid_: investigation, diagnosis

**Exclusive queue**:
The queue of the commands run through `afkconvoy exclusive` on one host, shared by every fleet on it whatever its repo: one runs at a time, in the order they asked, and the rest wait. A command whose process dies leaves it. A worker with a command in it is busy, not silent, for no longer than the gate limit.
_Avoid_: mutex, semaphore, heavy lock (the fleet lock is another thing)

**Nudge**:
The one line the fleet types at a silent worker with no outcome, telling it to carry on. Sent once per worker.
_Avoid_: ping, reminder, retry

**Restart**:
Replacing a silent worker with a new one in the same worktree and the same role, with its PR, branch and worktree untouched. Made once, and only for a worker that is joining the landing train.
_Avoid_: retry, re-dispatch

**Continuation**:
Recovering a claim whose worker died from the progress that survived: its worktree if still on this host, else its pushed branch, else a fresh start from the base branch.
_Avoid_: resume, checkpoint restore

**Wake**:
A signal that ends the sleep between ticks of its host's fleet instance early: every command that changes fleet state sends one as it ends — a worker's last among them, once its outcome is on GitHub — naming itself and what it acted on, and carrying no state. The `run` that hears it answers at once, naming the tick it starts or the tick in progress it follows.
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
Handing an issue to a human: the reason is commented, with the line that answers it, the flow label is swapped for the escalate label, and the claim is released. Every hand-off — a failure, a tripped breaker, an `escalate` verdict, a changed workflow — goes first to the consult where the stage names one, and escalates when the consult lets it through or where none is named; nothing is retried. The issue then waits for a human, with every other open issue only one can move: `plan`, `doctor` and the panel list them, each with its line.
_Avoid_: fail (what a command, a gate or one thing a tick does may), retry, give up (a landing train is given up: Abandon)

**Status board**:
The single comment on a claimed issue that shows a human how far along it is, and what the afkconvoy that works it was built from. It is rendered from fleet state and never read back.
_Avoid_: progress log, worklog

**Panel**:
The web page that shows a human where the whole fleet stands: each workflow as the line of its stages, the issues at work, queued and held back at each, and the landing train. A fleet instance serves it on its own host, made anew from what each tick read; it is never read back.
_Avoid_: dashboard, status page, status board (the comment on one issue), UI

**Tick log**:
The file a fleet instance's `run` keeps on its own host: one line as it starts, a line as each tick starts — with why: its timer, a wake and the command that sent it, the `run`'s start, or a follow-up — and one as it ends — what it did, and what changed on GitHub or by another host since the tick before where it acted on that — plus the lines of the events that reached the `run`: a wake heard during a tick, a check for host sleep, the host waking, being told to stop. Every command that changes fleet state, whoever runs it, appends a line of its own to the file of its host and repo as it acts — the command, the issue, who ran it, and how it came out or that it failed — whether a `run` is there or not. `run` prints each of its own lines as it is written. Each line is appended whole, and only `run` cuts the file back: it keeps every line of the last day, and never fewer than 200 lines nor more than 5000. It is a log and not fleet state: nothing reads it to decide what the fleet does.
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
