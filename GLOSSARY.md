Every term afkconvoy's help uses, each with its one definition and the words to avoid in its place. afkconvoy works a repo's ready GitHub-issue backlog unattended, from the command line: it dispatches one worktree-isolated coding agent per issue and lands their finished PRs on the base branch behind a local gate. It is a trimmed Rust port of the `afk-fleet` skill (`docs/adr/0001`).

## Language

### The fleet

**Fleet instance**:
One repo's fleet in its host's host runner, and everything it owns: the workers it started and the claims it holds, under an id minted when it starts. One host runs at most one per repo.
_Avoid_: launcher, coordinator, node, process (the host runner is the process)

**Host runner**:
A host's one long-running process of the fleet, holding a fleet instance for each checkout on its host's checkout list. `run` starts it in the background where none runs, and adds its checkout to it either way. `stop` takes one checkout off its list, and `stop --all`, or SIGTERM, stops it and keeps the list. Once a newer release is installed where it was started from, it moves onto it by itself between ticks once no exclusive command runs, on the same list, with its workers and work carried over (docs/adr/0032).
_Avoid_: daemon (a process outside the fleet), server (an MCP server, a model provider's), supervisor

**Checkout list**:
The checkouts a host's runner works, kept on that host and never pushed. The host runner converges on it: a checkout added is started, and a host runner started again starts every checkout on it. Each checkout on it may say its own **concurrency** and its **tracker** there, which a `run` that names them changes, and its fleet instance reads at each tick — a `run` that starts the host runner says the tracker there before it starts it, so that the host runner's start of the checkout from the list goes by it.
_Avoid_: registry, workspace list, projects (Codex's config key)

**Concurrency**:
How many claims a fleet instance holds at work at once on its host. It is its checkout's, said on the checkout list of the host that runs it, and 3 until that host says otherwise: what a host can carry — its cores, its memory, its logins — is no fact of the repo, so a repo config names none (`docs/adr/0025`).
_Avoid_: parallelism, workers (the processes it bounds), the repo's concurrency

**Tracker**:
The repo a fleet reads and writes its issues in: its checkout's own, until the checkout says another on the checkout list of its host (`run --tracker`), never pushed. Everything about an issue is the tracker's — its labels, its comments, its blockers, its escalation, its closing, and every issue the fleet opens — and every PR, branch, worktree, gate run and the landing train are the repo's. `#<n>` alone is the repo's own issue: the repo names a tracker's `<owner>/<tracker>#<n>` (`docs/adr/0027`). A public one is refused — the checkout's own repo too, where it says none — by every start's startup probe and by `run --tracker`, and a fleet instance whose tracker is made public is broken at its next check, within the hour one that found it private stands: anyone can comment on a public repo's issues, and so write what the fleet reads as its state and what its workers read as their task.
_Avoid_: issue repo, backlog (all the issues a repo has open), project (a GitHub Project)

**First start**:
The start of a checkout not yet on its host's checkout list, whose tracker and concurrency are settled with the user; on a host with no host settings yet, its host sleep is settled too. The agent explains the choices, sets up what needs no decision, and asks the user together for what does.
_Avoid_: onboarding, setup wizard

**Repo label**:
`repo:<owner>/<repo>`: what makes an issue of a tracker one of that repo's. A fleet whose tracker is another repo works only the issues there that carry its repo's, and gives it to every issue it opens, takes in or locks there; one carrying two is no fleet's. In a repo that is its own tracker, an issue carrying one is the repo's it names.
_Avoid_: component label, project label

**Broken**:
A fleet instance whose loop panicked or ended in an error — its tracker found public, say — or a start of which ended before its first tick while its checkout is on the checkout list, whoever started it: the host runner, from the list or again, or a `run`, which is told why — its startup probe no longer passing, say. It harms no other and stays on the checkout list, which only `stop` takes a checkout off: the host runner starts it again after a backoff — a minute, twice as long at each break in a row, half an hour at the most — or at once when a `run` of its checkout asks. It is **back** once such a start ended its first tick. A tick that fails is no break: the loop goes on.
_Avoid_: crashed, dead (a heartbeat past the lease), down (a harness that refuses every prompt)

**Host settings**:
What a host says for every fleet instance on it — whether it may be put to sleep and its sleep hours, the limits past which a worker is hot, the shares admission holds back at, the bound on an exclusive command holding others up, the numbers of worker and train lanes, the panel's port, the operator's directory a watch starts in and how often the host runner looks — kept beside the checkout list and changed by any `run` that names one, for every fleet instance on the host at once.
_Avoid_: global config, runner flags

**Tick**:
One reconciliation pass: rebuild the working set from fleet state, then perform every transition it calls for, without waiting for the workers it started.
_Avoid_: cycle, poll, batch

**Plan**:
A tick stopped after its rebuild, which reports what an acting tick would do and leaves nothing written: its startup probe, when it makes one, is the only thing it pushes, and it deletes that.
_Avoid_: dry run, preview

**Startup probe**:
What `run`, `tick` and `plan` ask of a host and a repo before anything else: that every command the fleet needs is installed and logged in, that orca is running and knows the checkout, that the base branch exists and no rule of its protection refuses a push, that its tracker is not public, where the remote takes the fleet's refs, and whether it takes recorded gate runs. A host or a repo the fleet cannot work is refused here, not at its first tick or landing. `run` makes one at every start; one that passed stands for an hour on the host that made it, while the tracker the checkout says is the one it found private, and `tick` and `plan` make none while it does. A running fleet instance asks again whether its tracker is public as that hour ends, or as it says another.
_Avoid_: bootstrap, preflight, health check

**Ref layout**:
One of the two ways every ref the fleet writes is named: hidden refs (`refs/convoy/*`), or — only where the remote rejects those — ordinary branches (`convoy-*`), on which push-triggered CI fires.
_Avoid_: namespace, fallback mode

**Fleet state**:
The authoritative record of what is claimed, in flight, landed, parked and escalated. It lives entirely in GitHub — the issues in the fleet's tracker, the PRs, branches and refs in the repo — and no process holds it. A PR whose head is in another repository is no part of it: only a login with write access can push a branch to the repo, so a PR from one of its branches is a collaborator's, and one from a fork is anyone's. The fleet acts as if a fork's PR did not exist — none is joined, taken up, synced, closed, commented on or reported, none holds an issue off, and none on a team's repo is taken for a proposal.
_Avoid_: run state, memory (what a host holds resident, as under Hot worker)

**Working set**:
A tick's view of the fleet at one moment, derived from fleet state and dropped when the tick ends.
_Avoid_: session state

**Rebuild**:
The read-only pass at the top of every tick that derives the working set. Any fleet instance rebuilds the same working set from the same GitHub. It lists the remote once and asks GitHub a fixed number of times, however many issues are open, and however many repos' issues its tracker holds.
_Avoid_: refresh (a heartbeat is refreshed), resync

**Frontier**:
The issues of the fleet's tracker a worker can be started on now — of a tracker that is another repo, those carrying the repo's repo label and no other: open, carrying a flow label, not an epic, unclaimed, with no open blocker, and at a stage a role performs or whose `escalate` a consult is to rule on — or landing, with an open PR to join. In an unbound repo: carrying the ready label, with no open linked PR but one of the fleet's own that a released claim kept, off the landing train — which its worker takes up. While a fleet lock stands: of these, the lock issues only.
_Avoid_: queue (the exclusive queue of a host's heavy commands), backlog (all the issues a repo has open)

**Idle**:
A fleet instance with nothing to do now: it holds no claim — on an issue, a patrol's run, the proposal of a precedent or a cut — nothing is on its landing train, and its frontier is empty. An escalated issue waiting for a human leaves it idle, and so does a patrol not yet due.
_Avoid_: drained, done (what a step or a command did), finished (a PR its worker is through with)

**Host sleep**:
A fleet instance putting its host to sleep: the host's local time is within its sleep hours, every fleet instance on the host has been idle for ten minutes, as its host runner holds how each stands, each has ticked once more and found nothing, no one has used the host's keyboard or mouse for ten minutes, the one that puts it to sleep has worked since the host last woke, and the host settings do not keep it awake. A host woken stays awake until some instance on it has worked again. The fleet never wakes a host: one put to sleep within its sleep hours sleeps on past their end.
_Avoid_: sleep (the wait between ticks), hibernate, suspend, shutdown

**Sleep hours**:
The hours of a host's local time its fleets may put it to sleep in: from 00:00 to 06:00 unless its host settings say others (`--host-sleep-hours 22:00-06:00`), the start included and the end not, past midnight where the end comes first, and any hour with `00:00-24:00`. A check for host sleep outside them keeps the host awake, says so, and is followed by the next as they start.
_Avoid_: quiet hours, night mode

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
Whether a tick starts new workers on its host: not while the host is past a share of its load, below its share of memory free, or low on free disk under the checkout. What it holds back stays on the frontier for the next tick; workers already running, the landing train and the worker a declared verdict starts are never held back. The shares have defaults taken from the host's cores, memory and disk, and the host can say its own in its host settings. Holding back for one reason — the load, memory or disk — at every tick past a span (ten minutes, or the host's own) starts an inspection of the host, once an **episode**: the episode ends once admission has admitted for five minutes. Holding back for reasons that change starts none. Memory pressure is observed independently: held past the same span, it starts an inspection of the host for memory, once an episode ending after five minutes without pressure; it changes no admission decision.
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
The check made before each thing a worker does, refusing what its role or its stage may not do — and, of an inspection, what crosses the bounds of any unattended worker. It stops a worker that errs, not one that means to get past it.
_Avoid_: permission (what a harness's own settings deny), sandbox (what a harness may shut a worker's commands in)

### Workflows

**Workflow**:
The stages one issue passes through and where each verdict leads, ending in the landing train or in nothing more to do.
_Avoid_: pipeline, process (a program that is running)

**Flow label**:
The label that names the workflow an issue follows: `flow:<name>`, unless the workflow's file names another with `label` — the ready label, say, so that an issue opened the usual way is worked. A bound repo's issue is the fleet's to work only while it carries one; there the ready label is one only where a workflow names it.
_Avoid_: type label

**Intake**:
Putting a flow label on an issue. A human does it on GitHub; a worker does it through the fleet, which records who did and holds the number of such issues still open under a limit — of a tracker that is another repo, the repo's issues alone, each of which intake gives the repo label.
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
A worker in a senior role explicitly named by a team's stage, asked before escalation about the work. It rules, sends, closes, parks or lets the issue through, and never commits, pushes or changes the host; the same hand-off again, its own failure, a fleet fault and any hand-off with no named consult escalate for the host's watch to answer.
_Avoid_: review (a stage may be named it), second opinion, handoff, default consult

**Fleet fault**:
A hand-off caused by the fleet, its harness or its host: a worker start that failed again, a worktree trust gap, or a landing train given up twice. It escalates at once, in a bound repo and an unbound one alike, because the fix is on the host; a worker gone silent had its brief, so its hand-off is about the work.
_Avoid_: infrastructure failure, system fault

**Ruling**:
What a consult or a human decides on an issue handed on. The stage runs again under it; a human's also hands the issue back to the fleet and starts the rework counts over, a consult's starts none over.
_Avoid_: answer (what a command prints), decision (what a `by: human` stage waits for), override

**Precedent**:
A ruling kept so that later stages follow it instead of escalating. A worker proposes it; a human accepts it.
_Avoid_: rule (a role's rules and the team rules; the command `rule`), policy, guideline

**Proposal**:
One human's ruling put forward as a precedent: a PR on the team's repo that appends one entry to its precedents file. A ruling gets one; a human merging it is what accepts the precedent.
_Avoid_: suggestion, draft (a roster `afkconvoy draft` writes), candidate (a key of what `recovery` answers)

**Split**:
Parking an issue on issues its own stage opened. When they have all closed, that stage runs again and judges the whole.
_Avoid_: decompose, epic (an issue under an epic label), fan out

**Patrol**:
A role run on a schedule rather than for an issue. It opens issues and takes them in; it writes no code. It is due once its interval has passed since its last run, and — where its file says `max_runs_a_day` — while fewer than that many of its runs started in the last 24 hours, rolling. Where it says `max_landed_a_day`, it is also held while that many issues it opened were closed as completed in the last rolling 24 hours, until enough completions are 24 hours old to fall below the bound. Its run takes one slot until its worker declares it over, which the next tick acts on; a worker that never does is ended once it is gone or has stopped for a grace period. Each run has a checkout of its own, removed as the run ends unless it failed.
_Avoid_: cron, job, scheduled worker

**Fleet lock**:
An open issue carrying the lock label, in a bound repo or an unbound one — of a tracker that is another repo, and the repo label of the repo whose fleet it holds: no other repo's fleet that works the same tracker. While one stands the fleet works lock issues only — nothing else is dispatched, no PR of another issue joins the landing train, and workers already running are left alone, what they declare being acted on afterwards — and closing the last one ends it. A stage's `escalate` or failure the lock explains — declared before it closed, and not acted on since — is then not carried out: the stage runs again, which is no rework.
_Avoid_: freeze, pause (a harness paused on a host, as under Host kit), kill switch

### Workers

**Harness**:
The program a worker is a session of: it runs a model, with tools, on the work it is handed. Claude Code, pi, Codex, DeepSeek Harness (DSH) and Kimi Code are harnesses. The host kit's order chooses the first eligible one; a role's limits and range constrain eligibility.
_Avoid_: agent (as under Worker), coding agent (as under Worker), CLI, provider (whose models a harness runs)

**Roster**:
A team's list, for each of its roles, of the harnesses and models that role's workers may run, with what each model is good at and how an issue is to be cast among them. It is committed with the team, written under the team file's `roster`: for each role it names, a `run` listing `{ harness, model, good_at }` as eligibility constraints — a harness named with no model allows any model the host kit holds for it — and a `cast` saying how to cast among them. A role's **range** on a host is its entry cut to the host kit, in the host kit's order, less each harness the role holds no limits for while it holds limits for another; a role it names no entry for ranges over the host kit, less the same — and so does one whose entry matches nothing in the host kit, which `check`, `plan` and `doctor` name — and an unbound repo, having no team, over the host kit whole. A role whose range is empty, holding limits for no harness the host kit holds, refuses no start: no worker of it is started, its stages wait while it lasts, and every other stage goes on. A stage, a patrol and a config name no harness and no model: what a worker runs is its role's range's to say. A roster is drafted by `afkconvoy draft` from a host kit into the team's working tree — every role, every harness and model in the host kit's order, what the host kit's first model says each is good at and how to cast — and committed by a human; the fleet never changes it. An entry that names a model the host kit holds by no name is named with the nearest names and given one issue in the team’s repo for its own fleet to correct.
_Avoid_: harness config, model table, routing rules

**Host kit**:
The harnesses this host can start and their models, each in order of preference, with each harness's worker launch command and what research says of each model. Writing it lists each harness's models and sends none a prompt. Each model is held by the id its harness lists, with the names its listing shows it by and those research ties to it with a dated source. A harness found and listed is held at once and starts workers on its **default model** until research places its models: a model is placed once research ranks it with a dated source, and one `research pending` or `not researched` is never started on. A model that fails at work is found by worker health, not by the kit. These orders are this machine's sole priority for every checkout; the kit also holds whether automatic model casting is enabled, off until the user enables it.
_Avoid_: roster (the team's), host config, inventory

**Default model**:
What a harness's own login runs when a worker is started with no model flag, which the fleet cannot name. A harness the host kit holds with no model placed — none listed yet, or research pending — starts its workers on it, Codex too.
_Avoid_: fallback model, `default` alone (a model id Claude Code lists)

**Codex effort**:
The reasoning effort a Codex worker takes from the host user's own Codex settings at its start, or Codex's model default where the user says none. It is independent of the fleet's model selection and is no setting of the fleet.
_Avoid_: fleet effort, role effort

**Casting**:
An issue's model labels, put on by its caster or a human, selecting a matching model only within the harness chosen by kit order and role eligibility; legacy harness labels never choose a harness. Labels survive switches and faults, and a model label applies again whenever its harness is selected and holds it.
_Avoid_: assignment (a GitHub assignee), routing, dispatch (starting a worker)

**Caster**:
A worker in no role that chooses models for ready issues, only on a host whose kit enables automatic casting and only where a stage's selected harness has several models and no matching model label. It adds model labels while preserving earlier labels and human casting; an issue waits at most ten minutes before dispatch on its range's first model.
_Avoid_: assigner, router, dispatcher

**Worker**:
A session of one harness, started in one role to perform one stage of one issue — or, after its last stage, to join its PR to the landing train — in that issue's worktree, and to report through GitHub. In an unbound repo it has no role and owns its issue from start to landing. It is ended — its terminal closed — by the step that ends what it was started for: its stage, its claim, or its run.
_Avoid_: agent (whoever reads the help; and a worker, to whoever has not met the term), subagent, child (an issue a split opened)

**Switch**:
Moving a harness, or one of its models and that model's harness, to the first position in this machine's host kit while retaining the other entries' relative order and contents. A known installed harness it does not hold is added on its stock worker launch command with no model, on its default model, and listed and researched only after the switch answers; no model is sent a prompt. Every checkout follows these orders; existing workers continue only when explicitly targeted, preserving their scene and claims and starting no replacement until the old writer is confirmed ended.
_Avoid_: harness change (what a new casting makes)

**Worker launch command**:
The shell string every worker of one harness is started with, in a fleet instance: one for each harness its workers run. The host kit says it, per harness, kept on the host at `$XDG_CONFIG_HOME/afkconvoy/kit.json` or `$HOME/.config/afkconvoy/kit.json` and written by `afkconvoy kit`; each launch reads its latest command, or discovers and records a known harness's stock command when none is held; a start checks that of every harness it could be asked for, and starts no worker when one cannot be started. In an unbound repo it is opaque but for what the fleet adds: its harness's model flag where the worker runs a model — its issue's casting, the host kit's first, or the stock inspector's — and its harness's worktree-trust setup, so every worker starts trusting its worktree and none waits on a folder-trust prompt. Every worker starts under `nice`, with a `TMPDIR` of its worktree's own: a link, `/tmp/afkconvoy-<uid>/<16 hex digits>`, to `.afkconvoy-tmp` in the worktree, short enough for a socket's path however long the worktree's is, and gone with the worktree once nothing started with it runs. A Codex worker's model is always the fleet's selection; its effort is the host user's Codex effort. A session's saved choice belongs to that session alone and chooses nothing for a later worker. In a bound repo the fleet adds to it what puts the worker in its role.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped, as its harness reports it to the worker backend — or, where the backend has no report of it, as its terminal's last output shows. Stopped never means finished.
_Avoid_: liveness, status (as under Position)

**Task refusal**:
A harness's fixed refusal message on a worker's screen: it refuses this issue, by rule, and the worker continues on the next harness of the host kit that can start for its role. The issue remembers the harness at every later stage and start; other issues may still run it. With no harness left for the issue, it escalates at once with the refusal's words as evidence.
_Avoid_: account fault, model fault, failed task

**Capacity stop**:
A worker stopped by its harness's fixed message that its selected model is at capacity. It is transient and belongs to that worker: the harness remains available to other workers.
_Avoid_: quota fault (an account's usage limit), task refusal (a harness refusing this issue)

**Verdict**:
What ends a stage, declared on the issue by whoever performed it: `pass`, `reject`, `blocked`, `already-satisfied`, or `escalate`. Opening a PR declares nothing.
_Avoid_: outcome (what a worker leaves on GitHub: its PR on the landing train, or its verdict), result, needs-human

**Silent worker**:
A worker that stopped, declared nothing, and for the grace period showed no sign of life — its terminal's output, a commit, a file touched, being started or nudged — while waiting on no background work of its own: a shell or a monitor it started that its harness shows still running, or a command of its in the exclusive queue. One waiting on such work is not silent until that work ends and it stays still past its nudge, or until it has waited longer than the gate limit.
_Avoid_: hung, idle (a fleet instance with nothing to do)

**Hot worker**:
A worker whose processes — every one started with its `TMPDIR`, and every one under those, but a server shared by the host that it started: one handed to pid 1 that works outside its worktree — take more than their share of the host, or whose worktree has outgrown its build. Its processes: more than half the host's cores (one at the least) held for two minutes, more than a quarter of its memory resident, or more written to disk in ten minutes than a tenth of the disk free (a GiB at the least). Its worktree: what git ignores there past `OUTGROWN_TIMES` its **fresh build** and past `OUTGROWN_FLOOR` — the train worktree's rule, judged once its first green `afkconvoy gate` has recorded that fresh build. The host can say other limits in its host settings (`--hot-cores`, `--hot-memory`, `--hot-written`). `run` samples each worker every ten seconds, with no root, and measures its worktree every minute; one that has stayed under every limit for five minutes — its worktree back under the rule among them — stops being hot, and that ends its **episode**: hot again later is a new one. Memory pressure makes no worker hot: held past the host inspection span, it is inspected on the host. The landing train's gate runs are the fleet's own and no worker's.
_Avoid_: hog

**Inspection**:
The operator on its host, started so that the fleet's work goes on: a worker in the role of the consult of a hot worker's stage — the stock inspector where the stage names none, at no stage, and in an unbound repo — started in a checkout of its own once per episode, and given what was taken down as that worker turned hot (its process tree, a `sample` of its hottest process, its resident memory over the last minutes, what its processes wrote, the largest directories of its worktree), the issue, the worktree and the hot worker's terminal. An inspection of the **host** is started on no worker, once admission holds for one reason past its span or memory pressure lasts past that span: the stock inspector, given what was taken down as it started — what called for it, including the memory pressure level where pressure did, and admission's reading and shares; for disk, each of the host's worktrees of every fleet with its issue, its stage, whether a worker holds it and what it holds, the landing train's, the fleet's caches and the largest directories of the whole volume under the home folder outside them, found within five minutes; for load or memory, the process trees of each worker and of the train and the hottest processes outside them. Either finds what holds the work back and fixes it — whatever the operator at this host's keyboard would do: end a runaway or orphaned process, restart, nudge or switch a worker, have a stopped gate run again, change a host setting, free disk, send heavy work through `afkconvoy exclusive`, change the repo itself through a branch of its checkout, the gate and the landing train, on no issue (`afkconvoy land --inspection`) — checks that the work goes on, and says with `afkconvoy inspected` what held it back, what it could not fix, and what it did. Its role chooses who inspects, not what it may do: it is no consult, and commits, pushes and changes the host. The guard holds it to the bounds of any unattended worker: only what the fleets of this host started or own, never the host runner nor the user's own; never deleting what cannot be rebuilt — model weights, downloads, media, Docker's images, a user's files, a change no commit holds; no force push, no push to the base branch, no merge outside the landing train. In an unbound repo, a base branch proved red the same way gated alone in a clean checkout is the one exception to opening no issue: it finds the open issue for the fix or opens one with the ready label, and raises the fleet lock on it. It opens no other issue and hands nothing to a human: each thing it did, and each PR it put on the landing train, is a line of the log, which `doctor` lists among its events; the panel and `doctor` show a host's beside admission's reading, and a hot worker's is left as a comment on its issue that is no stage record. What it could not fix, the next episode inspects again. An episode that cooled while waiting starts no inspection. Of either kind, one runs on a host at a time, across its fleets, takes no slot of the concurrency, and has an hour; none where another fleet of the host inspected the host in the episode, and nobody inspects where the stage says `to: human`. A worker hot again after its inspection is never escalated for it.
_Avoid_: investigation, diagnosis (a worker health assessment)

**Stock inspector**:
Afkconvoy's fallback inspection worker, used where no team role inspects. It has the inspection's authority and bounds and never answers an issue as a consult.
_Avoid_: stock consult (removed)

**Watch**:
The operator on its host, started by its host runner when a **look** at the host holds something an operator would act on, in the operator's own directory where the host names one, otherwise as the **stock watch** in afkconvoy's own directory. The stock watch stands in where no operator directory is named: its one short instruction is to find what holds the fleets' work back, fix it with the watch's authority, check that the work goes on and say what it did. An operator's directory supplies its own rules, with none of the stock instruction added. A look is `afkconvoy doctor --all --since <until of the last look>`, every five minutes or as the host settings say (`--watch-every`): the fleet's own code, which runs no model. What starts a watch: an issue waiting on a human, a finding that holds something up (no `info`), a fleet instance not alive or broken, a failed tick or a step failed since, a worker whose health observation is past its time. Each starts at most one watch in its **episode**, which lasts while each look still holds it. A watch is one agent session, given its brief as a file — the look, why it started and what earlier watches on the same items did — on the harness this host last started a worker on that took its brief, then the host kit's order, never one paused or with a fault on record; started trusted and in its harness's non-interactive mode, so that nothing it runs waits on a prompt. What it printed is kept as what it did. A watch whose harness gives out — its own words for a usage limit, or an exit that failed — is ended and continued on the next harness that can start it, with the same look and what it did so far, one session at a time; the fault is recorded, and the harness is skipped until its probe answers. One watch runs on a host at a time: no look is taken while it runs, and a host runner started again keeps the one it finds running. An hour bounds it, its continuations together, and it has the authority and the bounds of an inspection. Its start, its end and each thing it did are lines of every tick log of the host; `doctor` says the one running and what the last one did, and a finding where none could start (`docs/adr/0030`).
Its authority spans every repo on the host's checkout list: it fixes the host and hands an issue escalated for a fleet fault back once its cause is gone; makes a lasting fix in a checkout of its own of any listed repo, through that repo's gate and landing train; and, where the fix must reach an installed tool, has that repo cut a version after landing, lets the host runner move onto it and checks the fault on the new version. It answers escalations with the same commands as a human, whose effect and rework counts its answer has. Every watch leaves spending money or needing an account, a credential or a person's hands; what cannot be undone; taking a secret or private data off the host; and changing what a product is for or who it is for with a human, and records each without acting on it.
_Avoid_: patrol (a role run on a schedule that writes no code), supervisor (the host runner is the process)

**Exclusive queue**:
The lines of commands waiting to run on a host: worker commands and train gates in independent lanes, each in request order, with the number of each said in the host settings (one each until the host says otherwise). A repo holds at most one turn and one train gate at a time; a light gate takes its turns in its repo's own line; a running command is bounded by time holding others in its lane up, and a command whose process dies frees its place.
_Avoid_: mutex, semaphore, heavy lock (the fleet lock is another thing)

**Nudge**:
The one line the fleet types at a stopped worker with no outcome, telling it to carry on. Sent once for silence; a capacity stop may call for another after a longer wait.
_Avoid_: ping, reminder, retry

**Restart**:
Replacing a silent worker with a new one in the same worktree and the same role, with its PR, branch and worktree untouched. Made once, and only for a worker that is joining the landing train.
_Avoid_: retry, re-dispatch

**Continuation**:
Recovering a claim whose worker died from the progress that survived: its worktree if still on this host, else its pushed branch, else a fresh start from the base branch.
_Avoid_: resume, checkpoint restore

**Wake**:
A signal that ends the sleep between ticks of its host's fleet instance early: every command that changes fleet state sends one as it ends — a worker's last among them, once its outcome is on GitHub — naming itself and what it acted on, and carrying no state. `gh` sends none: whoever hands the fleet work sends one with `afkconvoy wake` after the issues they opened with it. The `run` that hears it answers at once, naming the tick it starts or the tick in progress it follows.
_Avoid_: notification, callback, done signal

### Claims

**Claim**:
The atomic lock marking one issue as owned by one fleet instance. Its creation succeeds for exactly one instance. It takes one slot of the concurrency while its worker is at work, and none once a verdict is declared under it: it is still held until that verdict is acted on, and a worker started under it again takes a slot as any start does.
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
Handing an issue on with its reason and answer line, its flow or ready label swapped for the escalate label, and its claim released. The host's watch answers it with the operator's authority, through the same commands as a human; only money, an account, a credential or a person's hands; what cannot be undone; a secret or private data leaving the host; and a change of what a product is for or who it is for stay with a human.
_Avoid_: fail (what a command, a gate or one thing a tick does may), retry, give up (a landing train is given up: Abandon)

**Status board**:
The single comment on a claimed issue that shows a human how far along it is, and what the afkconvoy that works it was built from. It is rendered from fleet state and never read back.
_Avoid_: progress log, worklog

**Panel**:
The one web page that shows a human where every fleet of a host stands: the host itself, what waits for a human across its repos, and for each repo its workflows as the lines of their stages, the issues at work, queued and held back at each, and the landing train. The host runner serves it, made anew from what each tick read; it is never read back.
_Avoid_: dashboard, status page, status board (the comment on one issue), UI

**Tick log**:
The file a fleet instance keeps on its own host: one line as it starts, a line as each tick starts — with why: its timer, a wake and the command that sent it, the fleet instance's start, or a follow-up — and one as it ends — what it did, and what changed on GitHub or by another host since the tick before where it acted on that — plus the lines of the events that reached the fleet instance: a wake heard during a tick, a check for host sleep, the host waking, being told to stop. Every command that changes fleet state, whoever runs it, appends a line of its own to the file of its host and repo as it acts — the command, the issue, who ran it, and how it came out or that it failed — whether a fleet instance is there or not. Each line is appended whole, and only the fleet instance cuts the file back: it keeps every line of the last day, and never fewer than 200 lines nor more than 5000. It is a log and not fleet state: nothing reads it to decide what the fleet does.
_Avoid_: state file, journal, history

**Finding**:
One thing `doctor` says is wrong with the fleet on this host, with how bad it is, what shows it, its fix — a line to run, or a file and the key in it to edit — and the help topic that explains it. `doctor` fixes none itself.
_Avoid_: diagnosis (a worker health assessment), alert, warning (one of its severities), error (what a command that failed answers with)

### Landing

**Gate**:
The repo's build-and-test command. It is the only machine verification a change must pass before it lands.
_Avoid_: CI (what a push to an ordinary branch starts), checks (what `check` and the startup probe make), local CI

**Gate limit**:
How long the gate may run. A run that reaches it is ended and is red. A repo declares it; nothing learns it from past runs.
_Avoid_: timeout (what a run that reached it did), budget, baseline

**Recorded gate run**:
The evidence that the gate passed on one committed tree, with how long the run took and on which host, made by the tool that saw the exit code and kept where every host can read it.
_Avoid_: gate cache, CI status

**Slow gate**:
What the gate is when its recent runs for the base branch, on one host, all took most of the gate limit. The fleet opens one issue that says so, and no second while that one is open.
_Avoid_: regression, drift, creep

**Sync**:
Bringing a branch up to date by merging into it. Nothing is ever rebased.
_Avoid_: rebase, update branch

**Landing train**:
The one append-only line of merge commits ahead of the base branch that finished PRs join, one per PR, each naming its issue, inspection or cut. A PR joins in a turn on the train's green tip, and the train lands on the last joiner's recorded gate run; only a tip without a green record needs the train lane's gate.
_Avoid_: merge queue, merge batch, landing turn

**Join**:
Putting a finished PR on the landing train in a turn that merges the green tip, gates and records the resulting tree before joining. An unbound worker joins its own PR, a bound workflow's landing worker joins after the last stage passes, and inspections and cuts join their own changes the same way.
_Avoid_: merge (what git does to two branches, and a human to a proposal's PR), enqueue

**Turn**:
One command's uninterrupted merge, gate and join of an open PR, starting on the train's green tip as it stands when the turn starts and ending only after the join, or at red or a conflict. The command holds it, so nothing waits on the worker's agent between the merge and the join.
_Avoid_: landing turn, landing slot

**Train worker**:
The one worker per repo that gates the landing train's tip and pushes it to the base branch as a fast-forward, on its host kit's first eligible harness. It holds no claim and wrote none of the PRs it lands.
_Avoid_: merger, integrator, batch worker

**Abandon**:
Giving up a landing train whose worker stayed silent, with nothing landed; its PRs join the next train.
_Avoid_: fail (as under Escalate), bisect

**Cut**:
The run that makes a new version of a repo from its base branch: it sets the version, lands it, tags it, and publishes and installs it. An idle fleet starts one when landed work is past the newest version tag, or when the repo declares its initial version and the remote has no version tag; each cut belongs to one host, and a failed cut holds later cuts back until its failure is answered.
_Avoid_: release (a claim's last step, and `afkconvoy release`), publish (a heartbeat's), deploy
