Every term afkconvoy's help uses, each with its one definition and the words to avoid in its place. afkconvoy works a repo's ready GitHub-issue backlog unattended, from the command line: it dispatches one worktree-isolated coding agent per issue and lands their finished PRs on the base branch behind a local gate. It is a trimmed Rust port of the `afk-fleet` skill.

## Language

### The fleet

**Host**:
One user account on one machine, and what the fleet keeps for it there: its host runner, its checkout list and host settings, its host kit, its exclusive lanes and its panel. A machine shared by several accounts holds a host of each, each working its own checkouts and seeing nothing of the others' but their processes: `doctor` names the host runners of the other accounts on its machine, read from the process table alone.
_Avoid_: machine (the hardware every host on it shares)

**Fleet**:
One repo's workers under afkconvoy: those its fleet instances start on that repo's checkouts, working the issues of its tracker that carry the repo's label (`repo:<owner>/<repo>`). A repo whose truth is GitHub is the fleet's; one whose truth is Guild's Forgejo is Guild's, and a repo belongs to one of them alone (sunfmin/guild's ADR, sunfmin/control#764).
_Avoid_: Guild (the other agent system of this host's user), convoy

**Fleet instance**:
One repo's fleet in its host's host runner, and everything it owns: the workers it started and the claims it holds, under an id minted when it starts. One host runs at most one per repo.
_Avoid_: launcher, coordinator, node (the Node executable), process (the host runner is the process)

**Host runner**:
A host's one long-running process of the fleet, holding a fleet instance for each checkout on its host's checkout list. `run` starts it in the background where none runs, and adds its checkout to it either way. `stop` takes one checkout off its list, and `stop --all`, or SIGTERM, stops it and keeps the list. Once a newer release is installed where it was started from, it moves onto it by itself between ticks once no exclusive command runs, on the same list, with its workers and work carried over. Its **keeper** starts it again after an unexpected end.
_Avoid_: daemon (a process outside the fleet), server (an MCP server, a model provider's), supervisor

**Keeper**:
The process `run` starts for the host runner, `afkconvoy host-runner` alone: it holds nothing of the host, starts the host runner as its child, in a process group of its own, and starts it again, on the same checkout list, after an unexpected end — a signal ended it, or an error once it held the host — saying so in the host runner's log. A host runner that stops as told, or moves onto a newer release, ends its keeper with it; the newer one's `run` start is a keeper of its own. It works in the cache directory, not in `/` as the host runner does, so what ends every process of one directory does not end both (ADR-0074).
_Avoid_: supervisor, watchdog (the watch is the host's operator), launch agent (launchd's)

**Checkout list**:
The checkouts a host's runner works, kept on that host and never pushed. The host runner converges on it: a checkout added is started, and a host runner started again starts every checkout on it. Each checkout on it may say its own **tracker** and its **harness** there, which a `run` that names them changes, and its fleet instance reads at each tick — a `run` that starts the host runner says the tracker there before it starts it, so that the host runner's start of the checkout from the list goes by it. A checkout that names no harness starts on the host kit's first eligible one.
_Avoid_: registry, workspace list, projects (Codex's config key)

**Concurrency**:
How many workers a fleet instance happens to hold at work at once: what admission allowed, start by start. No number bounds it, on the host or in the repo: nothing sets one, and a repo config names none (ADR-0072): `run --concurrency`, still accepted for one release, changes nothing, and `doctor` says so.
_Avoid_: parallelism, workers (the processes it counts), a concurrency limit, slots to set

**Tracker**:
The repo a fleet reads and writes its issues in: its checkout's own, until the checkout says another on the checkout list of its host (`run --tracker`), never pushed. Everything about an issue is the tracker's — its labels, its comments, its blockers, its escalation, its closing, and every issue the fleet opens — and every PR, branch, worktree, gate run and landing are the repo's. `#<n>` alone is the repo's own issue: the repo names a tracker's `<owner>/<tracker>#<n>`. A worker's commands read the tracker its fleet kept in its worktree as it started it, not the checkout list, which holds no entry of a checkout before its first tick ends. A public one is refused — the checkout's own repo too, where it says none — by every start's startup probe and by `run --tracker`, and a fleet instance whose tracker is made public is broken at its next check, within the hour one that found it private stands: anyone can comment on a public repo's issues, and so write what the fleet reads as its state and what its workers read as their task.
_Avoid_: issue repo, backlog (all the issues a repo has open), project (a GitHub Project)

**First start**:
The start of a checkout not yet on its host's checkout list, whose tracker is settled with the user; on a host with no host settings yet, its operator's directory is asked too. The agent explains the choices, sets up what needs no decision, and asks the user together for what does.
_Avoid_: onboarding, setup wizard

**Repo label**:
`repo:<owner>/<repo>`: what makes an issue of a tracker one of that repo's. A fleet whose tracker is another repo works only the issues there that carry its repo's, and gives it to every issue it opens, takes in or locks there; one carrying two is no fleet's. In a repo that is its own tracker, an issue carrying one is the repo's it names.
_Avoid_: component label, project label

**Broken**:
A fleet instance whose loop panicked or ended in an error — its tracker found public, say — or a start of which ended before its first tick while its checkout is on the checkout list, whoever started it: the host runner, from the list or again, or a `run`, which is told why — its startup probe no longer passing, say. It harms no other and stays on the checkout list, which only `stop` takes a checkout off: the host runner starts it again after a backoff — a minute, twice as long at each break in a row, half an hour at the most — or at once when a `run` of its checkout asks. It is **back** once such a start ended its first tick. A tick that fails is no break: the loop goes on.
_Avoid_: crashed, dead (a heartbeat past the lease), down (a harness that refuses every prompt)

**Host settings**:
What a host says for every fleet instance on it — the limits past which a worker is hot, the shares admission holds back at, afkconvoy's own share of its cores and memory (`--max-share-of-cores`, `--max-share-of-memory`, none by default), the bound on an exclusive command holding others up, the number of worker lanes and of fast lanes, the panel's port, the operator's directory a watch starts in and how often the host runner looks, the command that supplies a TypeSafe key for Jev's worker-health judgments, and an optional shell command that sends the human decisions a watch reports once per condition episode (`--notify-command`, none by default; `default` removes it), and where the **agent setup** is declared (`--agent-instructions`, `--agent-skills`, `--agent-guards`, none by default) — kept beside the checkout list and changed by any `run` that names one, for every fleet instance on the host at once.
_Avoid_: global config, runner flags

**Tick**:
One reconciliation pass: rebuild the working set from fleet state, then perform every transition it calls for, without waiting for the workers it started.
_Avoid_: cycle, poll, batch (batch round: a landing mode)

**Plan**:
A tick stopped after its rebuild, which reports what an acting tick would do and leaves nothing written: its startup probe, when it makes one, is the only thing it pushes, and it deletes that.
_Avoid_: dry run, preview

**Startup probe**:
What `run`, `tick` and `plan` ask of a host and a repo before anything else: that every command the fleet needs is installed and logged in, that the repo’s declared gate needs pass on this host, that orca is running and knows the checkout, that the base branch exists and no rule of its protection refuses a push, that its tracker is not public, where the remote takes the fleet's refs, and whether it takes recorded gate runs. A host or a repo the fleet cannot work is refused here, not at its first tick or landing. `run` makes one at every start; one that passed stands for an hour on the host that made it, while the tracker the checkout says is the one it found private, and `tick` and `plan` make none while it does. A running fleet instance asks again whether its tracker is public as that hour ends, or as it says another.
_Avoid_: bootstrap, preflight, health check

**Ref layout**:
One of the two ways every ref the fleet writes is named: hidden refs (`refs/convoy/*`), or — only where the remote rejects those — ordinary branches (`convoy-*`), on which push-triggered CI fires.
_Avoid_: namespace, fallback mode

**Remote view**:
A repository's one way to read and write the remote's refs. A read names how fresh it must be: as the view opened (the one listing a fleet's view is read from), current for this process (a ref already read is not asked again until this process writes it), or live, for a check another process may have moved or a write that must be certain. Every write — a push or a receive-pack publication — states the refs it writes, and the view drops exactly those; a read begun before a write landed never answers for a ref it wrote. Nothing else pushes or lists the remote.
_Avoid_: remote cache, ref cache

**Fleet state**:
The authoritative record of what is claimed, in flight, landed, parked and escalated. It lives entirely in GitHub — the issues in the fleet's tracker, the PRs, branches and refs in the repo — and no process holds it. A PR whose head is in another repository is no part of it: only a login with write access can push a branch to the repo, so a PR from one of its branches is a collaborator's, and one from a fork is anyone's. The fleet acts as if a fork's PR did not exist — none is landed, taken up, synced, closed, commented on or reported, none holds an issue off.
_Avoid_: run state, memory (what a host holds resident, as under Hot worker)

**Working set**:
A tick's view of the fleet at one moment, derived from fleet state and dropped when the tick ends.
_Avoid_: session state

**Rebuild**:
The read-only pass at the top of every tick that derives the working set. Any fleet instance rebuilds the same working set from the same GitHub. Missing facts fail the rebuild rather than forming an empty working set.
_Avoid_: refresh (a heartbeat is refreshed), resync

**Frontier**:
The issues of the fleet's tracker a worker can be started on now — of a tracker that is another repo, those carrying the repo's repo label and no other: open, carrying a flow label, not an epic, unclaimed, with no open blocker, not waiting on the next cut, and at a stage a role performs or whose `escalate` a consult is to rule on — or landing, with an open PR to land. In an unbound repo: carrying the ready label, with no open linked PR but one of the fleet's own that a released claim kept, not landed — which its worker takes up. While a fleet lock stands: of these, the lock issues only.
_Avoid_: queue (the exclusive queue of a host's heavy commands), backlog (all the issues a repo has open)

**Idle**:
A fleet instance with nothing to do now: it holds no claim — on an issue, a patrol's run or a cut — its frontier is empty. An escalated issue waiting for a human leaves it idle, and so does a patrol not yet due, and an issue waiting on the next cut: it holds no claim and is off the frontier, so the cut it waits for can start (ADR-0056).
_Avoid_: drained, done (what a step or a command did), finished (a PR its worker is through with)

**Host sleep**:
What a fleet no longer does: put its host to sleep once every fleet on it was idle (ADR-0016, superseded by ADR-0050). No fleet puts its host to sleep; a fleet instance still notices its host waking from a sleep something else caused.
_Avoid_: sleep (the wait between ticks)

**Base branch**:
The configured destination of a repo's ordinary tasks, watches and cuts, and the branch from which versions are cut. A task with an explicit target branch leaves it unchanged.
_Avoid_: trunk, main, default branch (a team's or a role's repo is read at its own)

**Target branch**:
The same-repo branch one task is cut from and lands on, pinned before it is claimed: the selected branch, or the configured base branch. Its stages and continuations retain that destination.
_Avoid_: delivery branch (a Jira ticket's broader delivery), integration branch (a team's purpose for a branch)

**Jira delivery**:
One Jira ticket's delivery in one code repo, identified by its full Jira reference, including the site. It owns one delivery branch, one final target branch and one final review PR. Attached tasks contribute through internal task PRs. The same ticket across code repos has a separate delivery in each.
_Avoid_: Jira workflow, task (one contribution to a delivery)

**Delivery branch**:
The branch a Jira delivery gathers its attached tasks on. Each attached task resolves it as its target branch; the repo's configured base stays unchanged.
_Avoid_: task branch (the worker's own source branch)

**Final review PR**:
A Jira delivery's single PR from its delivery branch to its final target, opened as a Draft once there is a meaningful diff. A human owns colleague review and merging it; ordinary fleet task landing never selects it.
_Avoid_: task PR (an internal contribution), final task

**Delivery hand-off**:
The explicit check that a Jira delivery's attached tasks are implemented, that its delivery branch merges cleanly onto its current final target, and that the existing gate passes on that merge without publishing to the final target. A green check updates the one final review PR and marks it ready for a human. The fleet does not merge it.
_Avoid_: auto-land, Jira transition

**Delivery evidence**:
The record of one delivery hand-off: the delivery head, the final-target head, and what it checked of each attached task. A later commit, a moved final target, or a different task set does not match it. A green gate of an individual task does not stand in for it.
_Avoid_: child gate, approval (a reviewer's approval on GitHub)

**Delivery settlement**:
The one observation that a final review PR merged into its declared final target, or closed without merging. Repeating it does not write another record or remove other work. A merge while attached work is incomplete is a discrepancy, not delivery of the whole Jira ticket.
_Avoid_: Jira resolution, done (a Jira ticket marked done)

### Teams and roles

**Role**:
A directory that defines one kind of worker: the rules it always works under, the limits it cannot cross, and the skills it can be asked to perform. It is a whole repo or a subdirectory of one. A worker starts in it, and is given nothing else of its repo.
_Avoid_: agent (as under Worker), persona, profile (as under DSH profile)

**Team**:
A directory that names a set of roles and holds the workflows, patrols and rules they work by. It is a whole repo or a subdirectory of one, and its roles may be directories of the same repo.
_Avoid_: org, crew, squad

**Team rules**:
The rules every role of a team works under, whatever its own rules add. They say how the team works, and hold in any repo bound to it; what one codebase asks is that repo's to say, and what one kind of worker does is its role's.
_Avoid_: global rules, shared rules

**Admission**:
Whether a tick starts another worker, which it alone decides: no number bounds a fleet's workers. A tick starts workers while it admits, one reading per start, so that each start's load and memory are counted before the next. It does not admit while the host is past a share of its load outside the exclusive queue, below its share of memory free, or low on free disk under the checkout; while a **usage window** of the account the checkout's next worker starts on would, at its present rate, with one more worker beside those running on it, run out before it resets — a harness that reports no window is judged by the host alone; or while the commands in the host's exclusive queue would take longer than 15 minutes to drain, by their recent run times (ADR-0072); or, where the host caps afkconvoy's own share of its cores or memory, while afkconvoy's own processes — its host runner, its fleets' workers and what runs in its exclusive queue — already take that share, for a start would take them past it (ADR-0075). The queue's share of the one-minute load is estimated from the current CPU share of its running commands' process trees (ADR-0050). Its readings are the machine's, shared with the host runners of the other accounts on it: their workers load it and fill its memory as this host's do. What it holds back stays on the frontier for the next tick; workers already running and the worker a declared verdict starts are never held back. The shares have defaults taken from the host's cores, memory and disk, and the host can say its own in its host settings. Holding back a checkout's work for one reason — the load, memory, disk, a usage window, the queue or the share — at every tick past a span (ten minutes, or the host's own) calls for the host's watch, once an **episode** of that checkout: the span starts when its work is first held, and the episode ends after five minutes without held work. Holding back for reasons that change starts none. Memory pressure is observed independently: held past the same span, it calls for the host's watch for memory, once an episode ending after five minutes without pressure; it changes no admission decision. It calls for none while another user account holds the most resident memory and this host's fleets hold no worker, nor, once a watch has left pressure from that account with the operator, until they answer it or the largest holder changes: a watch keeps away from another account's programs (ADR-0061).
_Avoid_: throttling, backpressure

**Host user's skills and rules**:
The skills and the rules the host's own user has for a harness, which a session they start by hand loads: a worker of a bound repo is given them beside its role's, unless its team file says `host_user: false`. A role may say it for itself, whatever the team says: `reviewer: { dir: ./roles/reviewer, host_user: false }` in `roles`; a role named by its directory alone takes the team's. Its role's skills win over them, and the brief, its role's rules, the team rules and the repo's own rules win over theirs. DSH alone may load those too when its role chooses the host user's DSH profile.
_Avoid_: global rules, user config, personal skills

**Agent setup**:
The operator's global instructions and skills, the same for every harness the host kit holds, whatever harness a worker starts on. The operator declares it once, in files of their own — an instructions file and a skill list (`<source> <skill>...`, or `<source> *` for every skill a source ships), in sunfmin/dotfiles `claude/CLAUDE.md` and `skills.txt` — and the host settings name where the declaration is (`run --agent-instructions`, `--agent-skills`). The host kit owns where each harness reads them: the instructions file is linked at each held harness's global instructions location, and the skill list installed through `npx skills` for each one's agent into the shared `~/.agents/skills`, whose copies are read-only between syncs. Its third part is the **guards**: small programs of the operator's, each run before the tool calls it applies to (`shell` or `edits`), that can refuse one, declared in a guard list (`run --agent-guards`). The host kit wires each into every held harness's own hook settings (Claude Code `~/.claude/settings.json`, Codex `~/.codex/hooks.json`), merging: only its own entries, known by the command they open with, are replaced, after every other, and every other key and entry is left as it was. A Codex session outside a role, started with a user directory the host keeps rather than `~/.codex`, is given the guards there, in a `hooks.json` holding the guards alone; a worker in a role holds none of them, as its role allows. Writing the host kit gives it to every held harness, a tick to a harness held since it was last given, and `doctor` names each that differs from the declaration. It is what a harness's **host user's skills and rules** then hold. With no declaration nothing is given. What the host kit does of it is a crate of its own, `afkconvoy-host-kit` (ADR-0062). It is given on the fleet's machines only: Guild's instances are not given it, and Guild uses the crate for tmux alone (ADR-0066).
_Avoid_: dotfiles, global config, skills sync

**DSH profile**:
What a DSH worker loads of bundles, plugins, MCP servers, patches, themes and settings: the fleet's own (`dsh_profile: fleet`, the default), or the host user's `dsh-tui` profile (`dsh_profile: host-user`). The team chooses for every role; a role may choose its own, independently of `host_user`, and both keep the guard and the role's limits.
_Avoid_: provider profile, persona

**Bound repo**:
A repo whose config names a team. An unbound repo has no roles and one fixed path for every issue: one worker, then a turn that lands its PR. It has no position and no workflow to draw: `show <issue>` there says where the issue stands on that path — ready, held back and why, claimed and by which fleet instance, landing with the fleet's PR, waiting on a human, or closed — from the facts a tick acts on.
_Avoid_: team mode, managed repo

**Guard**:
The check made before each thing a worker does, refusing what its role or its stage may not do — and, of a watch, what crosses the bounds of any unattended worker. Every worker, bound or unbound, and the watch is refused input to the asking session's Terminal. Only the host runner's marked deliveries and the operator supply that input; the asking session itself is exempt by its Terminal identity. Every worker, bound or unbound, is refused a **heavy command** of its repo run outside `afkconvoy exclusive`. It stops a worker that errs, not one that means to get past it.
_Avoid_: permission (what a harness's own settings deny), sandbox (what a harness may shut a worker's commands in)

### Workflows

**Workflow**:
The stages one issue passes through and where each verdict leads, ending in a landed PR or in nothing more to do.
_Avoid_: pipeline, process (a program that is running)

**Flow label**:
The label that names the workflow an issue follows: `flow:<name>`, unless the workflow's file names another with `label` — the ready label, say, so that an issue opened the usual way is worked. A bound repo's issue is the fleet's to work only while it carries one; there the ready label is one only where a workflow names it.
_Avoid_: type label

**Intake**:
Putting a flow label on an issue. A human does it on GitHub; a worker does it through the fleet, which records who did and holds the number of such issues still open under a limit — of a tracker that is another repo, the repo's issues alone, each of which intake gives the repo label.
_Avoid_: enroll, admit (a tick starting new workers on a host not busy: admission), triage

**Stage**:
One step of a workflow: one skill of one role, performed by one new worker — or a decision waited for from a human.
_Avoid_: step (one thing a tick does), phase (candidate lifecycle), state (fleet state and worker state are terms)

**Stage record**:
The comment a stage leaves on its issue: what it produced, its verdict, the versions of the role and team it ran under, and the harness its worker ran on, with the model where it was started on one. A human who declares for a stage, rules on it or sends the issue to it leaves one too, which says a human did; so does the fleet, for a stage that runs again because a fleet lock closed. The stage an issue is at is derived from these and stored nowhere.
_Avoid_: handoff note, log (the tick log; what a gate prints), stage label

**Work account**:
The one comment the fleet leaves when work on an issue ends, and edits at a later end: one versioned JSON record of its wall clock from the earliest recorded claim, its outcome, the fleet version and host, its workers, their raw token use by model, how a Codex worker's worker goal went, and the gate runs on its own tree, with unknown facts left unknown and human text rendered from the record. Known worker tokens add into an issue total, with unknown workers counted separately; cached input is counted once and reasoning is part of output. A worker does not write it; position, rework and the guard do not read it.
_Avoid_: summary (the read-only summary a command answers), time log, report (a failure a patrol or a cut opens; what a watch round delivers; what doctor says)

**Position**:
Where an issue is in its workflow: at a stage a worker is to perform, waiting on a human, with a consult, parked, landing, ended, or escalating. It is derived from the stage records each tick and stored nowhere.
_Avoid_: state (as under Stage), status (the status board; a key of what a command answers), current stage

**Rework**:
A stage running again because a later stage rejected its work or a consult ruled on it. It is counted per issue from the stage records, from where a human last ruled on the issue or sent it to a stage.
_Avoid_: retry, round (a watch round is an independent task of a resident watch), attempt (what an issue was worked into before a fresh start)

**Breaker**:
A limit on rework — in total for an issue, or as rejections in a row by one stage — past which the issue escalates.
_Avoid_: retry limit, circuit

**Consult**:
A worker in a senior role explicitly named by a team's stage, asked before escalation about the work. It rules, sends, closes, parks or lets the issue through, and never commits, pushes or changes the host; the same hand-off again, its own failure, a fleet fault and any hand-off with no named consult escalate for the host's watch to answer.
_Avoid_: review (a stage may be named it), second opinion, handoff, default consult

**Fleet fault**:
A hand-off caused by the fleet, its harness or its host: a worker start that failed again, a worktree trust gap. It escalates at once, in a bound repo and an unbound one alike, because the fix is on the host; a worker gone silent had its brief, so its hand-off is about the work.
_Avoid_: infrastructure failure, system fault

**Ruling**:
What a consult or a human decides on an issue handed on. The stage runs again under it; a human's also hands the issue back to the fleet and starts the rework counts over, a consult's starts none over.
_Avoid_: answer (what a command prints), decision (what a `by: human` stage waits for), override

**Split**:
Parking an issue on issues its own stage opened. When they have all closed, that stage runs again and judges the whole.
_Avoid_: decompose, epic (an issue under an epic label), fan out

**Patrol**:
A role run on a schedule rather than for an issue. It opens issues and takes them in; it writes no code. It is due once its interval has passed since its last run, and — where its file says `max_runs_a_day` — while fewer than that many of its runs started in the last 24 hours, rolling. Where it says `max_landed_a_day`, it is also held while that many issues it opened were closed as completed in the last rolling 24 hours, until enough completions are 24 hours old to fall below the bound. Its run takes one slot until its worker declares it over, which the next tick acts on; a worker that never does is ended once it is gone or has stopped for a grace period. Each run has a checkout of its own, removed as the run ends unless it failed.
_Avoid_: cron, job, scheduled worker

**Fleet lock**:
An open issue carrying the lock label, in a bound repo or an unbound one — of a tracker that is another repo, and the repo label of the repo whose fleet it holds: no other repo's fleet that works the same tracker. While one stands the fleet works lock issues only — nothing else is dispatched, no PR of another issue lands, and workers already running are left alone, what they declare being acted on afterwards — and closing the last one ends it. A stage's `escalate` or failure the lock explains — declared before it closed, and not acted on since — is then not carried out: the stage runs again, which is no rework.
_Avoid_: freeze, pause (a harness paused on a host, as under Host kit), kill switch

### Workers

**Harness**:
The program a worker is a session of: it runs a model, with tools, on the work it is handed. Claude Code, pi, Codex, DeepSeek Harness (DSH), Kimi Code and Grok are harnesses. A checkout that names none follows the host kit's order, which chooses the first eligible one. A checkout names one on its entry on the checkout list (`run --harness`); its workers start on that harness, and the host kit still orders that harness's models. A role's limits and range constrain eligibility. A role that holds no limits for the named harness starts no worker, and no other harness is started in its place.
_Avoid_: agent (as under Worker), coding agent (as under Worker), CLI (Orca's own command-line program, which is no harness), provider (whose models a harness runs)

**Roster**:
A team's list, for each of its roles, of the harnesses and models that role's workers may run, with what each model is good at. It is committed with the team, written under the team file's `roster`: for each role it names, a `run` listing `{ harness, model, good_at }` as eligibility constraints — a harness named with no model allows any model the host kit holds for it. A `cast` key is accepted and ignored: a start does not read it. A role's **range** on a host is its entry cut to the host kit, in the host kit's order, less each harness the role holds no limits for while it holds limits for another; a role it names no entry for ranges over the host kit, less the same — and so does one whose entry matches nothing in the host kit, which `check`, `plan` and `doctor` name — and an unbound repo, having no team, over the host kit whole. A role whose range is empty, holding limits for no harness the host kit holds, refuses no start: no worker of it is started, its stages wait while it lasts, and every other stage goes on. A stage, a patrol and a config name no harness and no model: what a worker runs is its role's range's to say. A roster is drafted by `afkconvoy draft` from a host kit into the team's working tree — each role's `run` from the host kit, in its order, with `good_at` from the kit's research, and no prompt — and committed by a human; the fleet never changes it. An entry that names a model the host kit holds by no name is named with the nearest names and given one issue in the team’s repo for its own fleet to correct.
_Avoid_: harness config, model table, routing rules

**Host kit**:
The harnesses this host can start and their models, each in order of preference, with each harness's worker launch command and what research says of each model. Writing it lists each harness's models and sends none a prompt. Each model is held by the id its harness lists, with the names its listing shows it by and those research ties to it with a dated source. A harness found and listed is held at once and starts workers on its **default model** until research places its models: a model is placed once research ranks it with a dated source. The watch researches models awaiting research and records sourced placements or settlements; research runs alongside its other work or starts the watch alone at most once a day. A model that fails at work is found by worker health, not by the kit. These orders are this machine's sole priority for every checkout that names no harness. A checkout that names one still takes that harness's worker launch command and model order from the kit. The operator can remove a harness, which keeps it out of every start and routine relisting until `switch --harness` puts it back; merely leaving it out by hand records no such removal. An old `cast` key is ignored and is not written again.

Research states: `research pending` is a newly listed model awaiting dated research; `not researched` is an older release's model with no dated answer, also awaiting research. Neither is started on. A settled model has a dated source and a reason research found it unsuited to workers: it is not placed, is named by no unplaced-model finding and starts no watch. The finding names exactly the models awaiting research, including models held with no dated answer; every one can be placed or settled through `afkconvoy kit --research <file>`. Placements and settlements survive routine relisting; an identity dropped from the kit and later listed anew awaits research again.
_Avoid_: roster (the team's), host config, inventory

**Guild machine**:
A host whose user has Guild's home, `~/.guild`: a machine of Guild, the other agent system of this host's user (sunfmin/control#693), which runs no fleet of its own there but takes tmux, and only tmux, from the host kit crate (ADR-0062, ADR-0066); it is given none of the operator's agent setup. Guild runs each instance in a tmux server of its own, so writing the host kit on a Guild machine with no tmux installs one where a standard account can — `pixi global install tmux`, or Homebrew where it has bottles, installing a pinned pixi checked by its published checksum first where neither is there — and leaves an existing tmux alone; `doctor` names a Guild machine with no tmux, with the line that fixes it.
_Avoid_: Guild host (a host is one account's, as under Host), node

**Default model**:
What a harness runs when a worker is started with no model named: its own login's model. Every worker whose model label is not followed starts on it, including one whose range holds a single model. Codex uses the model in the user's Codex config when the host kit lists that model; with none, Codex's own. A named Codex model the kit does not list, or that the role's range does not hold, refuses the start.
_Avoid_: fallback model, the host kit's first (a model's place in the kit; the first harness keeps the phrase), `default` alone (a model id Claude Code lists)

**Codex effort**:
The reasoning effort a Codex worker takes from the host user's own Codex settings at its start, or Codex's model default where the user says none. It is independent of the model selected from the user's Codex preference, and is no setting of the fleet.
_Avoid_: fleet effort, role effort

**Codex service tier**:
The service tier a Codex worker takes from the host user's own Codex settings at its start when one is explicitly configured, including `default` and `priority`, or Codex's own default where the user says none. It is read afresh for each launch in bound and unbound repos regardless of `host_user`, changes no already-running session, and is no setting of the fleet.
_Avoid_: fleet service tier, role service tier

**Model label**:
An issue's one `model:` label. It names the model a worker of the selected harness starts on when it names exactly one model that harness holds and the role's range holds. The selected harness is the one the checkout names, or the first eligible harness in the host kit's order when the checkout names none. Anything else leaves the worker on that harness's **default model**, and the label stays. A `harness:` label never selects a harness.
_Avoid_: casting (the removed automatic choice, as in "automatic casting is gone"), caster (the removed chooser; help may say there is none), assignment (a GitHub assignee), routing, dispatch (starting a worker)

**Worker selection**:
The one decision of what a worker runs: its harness, its model, whether a model label named that model, and what an unfollowed label is told — or why nothing is safe, as the user is told it. It is made from facts gathered once for the start: the host kit whole, what of it can start now and why the rest cannot, the checkout's named harness, the pauses, the role, the issue's labels and the harness's **default model** where the user's Codex config names one. Dispatch, switch, a patrol, plan and the panel each ask it, and it says how the model goes on the start line of every harness.

**Worker**:
A session of one harness, started in one role to perform one stage of one issue — or, after its last stage, to land its PR in a turn — in that issue's worktree, and to report through GitHub. In an unbound repo it has no role and owns its issue from start to landing. It is ended — its terminal closed — by the step that ends what it was started for: its stage, its claim, or its run.
_Avoid_: agent (whoever reads the help; and a worker, to whoever has not met the term), subagent, child (an issue a split opened)

**Switch**:
Moving a harness, or one of its models and that model's harness, to the first position in this machine's host kit while retaining the other entries' relative order and contents. It reorders the host kit and does not choose the model a worker starts on. A known installed harness it does not hold is added on its stock worker launch command with no model, on its default model, and listed and researched only after the switch answers; no model is sent a prompt. A checkout that names no harness follows these orders. A checkout that names a harness keeps it: switch does not change that name and does not continue that checkout's workers onto the kit's new first. Existing workers of a checkout that names none continue only when explicitly targeted, preserving their scene and claims and starting no replacement until the old writer is confirmed ended.
_Avoid_: harness change (a model label)

**Worker launch command**:
The shell string every worker of one harness is started with, in a fleet instance: one for each harness its workers run. The host kit says it, per harness, kept on the host at `$XDG_CONFIG_HOME/afkconvoy/kit.json` or `$HOME/.config/afkconvoy/kit.json` and written by `afkconvoy kit`; each launch reads its latest command, or discovers and records a known harness's stock command when none is held; a start checks that of every harness it could be asked for, and starts no worker when one cannot be started. A start of a checkout that names a harness checks that harness alone, and does not refuse the checkout because some other harness cannot start. In an unbound repo it is opaque but for what the fleet adds: its harness's model flag where a model label is followed — otherwise no model flag, the harness's default model — and its harness's worktree-trust setup, so every worker starts trusting its worktree and none waits on a folder-trust prompt. Every worker starts under `nice`, with a `TMPDIR` of its worktree's own: a link, `/tmp/afkconvoy-<uid>/<16 hex digits>`, to `.afkconvoy-tmp` in the worktree, short enough for a socket's path however long the worktree's is, and gone with the worktree once nothing started with it runs. An automatic Codex worker reads the user's current model from `~/.codex/config.toml` at launch when no model label is followed, and only when the host kit lists that model. A followed model label is recorded as the issue's model label; a launch record that already says `casting` is still read as that. With no preference it leaves the model unset for Codex's native default. Its effort remains the host user's Codex effort. A worker's private session choice never changes what a later worker reads. In a bound repo the fleet adds to it what puts the worker in its role.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped. It is busy while the turn its harness took is still open, and stopped once that turn has ended or none was recorded. Of the primary sessions recorded for one worktree, the one that started last is that turn. The harness names the session that records that turn. Stopped never means finished.
_Avoid_: liveness, status (as under Position)

**Task refusal**:
A harness's fixed refusal message on a worker's screen: it refuses this issue, by rule, and the worker continues on the next harness of the host kit that can start for its role. A checkout that names a harness does not continue a refused worker onto another harness. The issue remembers the harness at every later stage and start; other issues may still run it. With no harness left for the issue, it escalates at once with the refusal's words as evidence.
_Avoid_: account fault, model fault, failed task

**Capacity stop**:
A worker stopped by its harness's fixed message that its selected model is at capacity. It is transient and belongs to that worker: the harness remains available to other workers.
_Avoid_: quota fault (an account's usage limit), task refusal (a harness refusing this issue)

**Usage window**:
One span of an account's use of its engine — Claude Code's `five_hour` or `seven_day`, Codex's `primary` or `secondary`, Kimi Code's `limit_5h` — named as its engine names it, with how much of it is used and when it resets, as the engine reports them (`Harness::windows`). A **limit event** is the engine's structured record that a request met a limit (`Harness::limit_event`): a usage limit, which holds until its window resets, or the service's throttle, which is not the account's use. The harness crate reads both and predicts neither.
_Avoid_: quota (the screen words of `account_quota`), capacity stop (a model at capacity)

**Session status**:
What a harness's session is doing, as its hook events say it, in the words of the Program Status Protocol (OSC 7501): `idle` (it waits for a prompt, or its session ended), `working` (it works on a prompt), `blocked` with the kind `permission` (it waits for a tool call to be allowed), `question` (for its question to be answered) or `auth` (for its login), `done` (its turn ended, answered) or `error` (its turn ended in an error). With it go one line for a person — the tool and its input, the question, the prompt, the answer or the error — the tool, and whether the event is the call's start or what came back of it, with the error it failed with. An event that changes nothing says so. The harness crate names, per harness, the hook events to register one command at (`Harness::status_events`, `status_hooks`) and what one event's payload says (`hook_status`); a harness whose hooks do not say it reliably — every one but Claude Code and Codex — says none, and a caller reads its session file instead. afkconvoy's own workers use none of it: their **worker state** is read from their session files.
_Avoid_: worker state (busy or stopped, from the session file), state (Orca's own word)

**Step**:
One tool call a session made, with what came back of it, whether the harness marked that an error, and when it came back, as the harness's session file records it (`Harness::steps`): Claude Code's `tool_use` and its `tool_result`, Codex's `function_call` and its output, and each other harness's own. A call nothing has come back of yet is no step, and a child session's are not the session's. The harness crate reads them and judges none: whether a session repeats itself is for the program that reads its steps.
_Avoid_: event (a line of a log, or what a harness records a session's state changed by), action (a step's call alone), turn (a session's span of work)

**Verdict**:
What ends a stage, declared on the issue by whoever performed it: `pass`, `reject`, `blocked`, `already-satisfied`, or `escalate`. Opening a PR declares nothing. A `blocked` verdict names the issues it waits on, or the next cut of its own repo (`blocked_by=cut`, `--blocked-by cut`), when the stage can go on only once a version not installed yet is: the claim is parked under the label `convoy-waits-on-cut` instead of polling for the version, and the issue returns at the stage it left once a cut has installed one (ADR-0056).
_Avoid_: outcome (what a worker leaves on GitHub: its landed PR, or its verdict), result, needs-human

**Silent worker**:
A worker that stopped, declared nothing, and for the grace period showed no sign of life — the end of its turn, a commit, a file touched, being started or nudged — while waiting on no background work of its own: a shell or a monitor it started that its harness shows still running, or a command of its in the exclusive queue. One waiting on such work is not silent until that work ends and it stays still past its nudge, or until it has waited longer than the gate limit.
_Avoid_: hung, idle (a fleet instance with nothing to do)

**Hot worker**:
A worker whose processes — every one started with its `TMPDIR`, and every one under those, but a server shared by the host that it started: one handed to pid 1 that works outside its worktree — take more than their share of the host, or whose worktree has outgrown its build. Its processes: more than half the host's cores (one at the least) held for two minutes, more than a quarter of its memory resident — what runs under its `afkconvoy exclusive` commands, which the exclusive queue already bounds, counting only once the host's memory pressure has held at level 2 or above for two minutes (ADR-0056) —, or more written to disk in ten minutes than a tenth of the disk free (a GiB at the least). Its worktree: what git ignores there past `OUTGROWN_TIMES` its **fresh build** and past `OUTGROWN_FLOOR` — judged once its first green `afkconvoy gate` has recorded that fresh build. The host can say other limits in its host settings (`--hot-cores`, `--hot-memory`, `--hot-written`). `run` samples each worker every ten seconds, with no root, and measures its worktree every minute; one that has stayed under every limit for five minutes — its worktree back under the rule among them — stops being hot, and that ends its **episode**: hot again later is a new one. Memory pressure makes no worker hot: held past the host resource span, it calls for the host's watch.
_Avoid_: hog

**Watch**:
In a repo several hosts work, each human wait is answered by one host's watch. It takes the wait with a claim on that repo's remote; the others name its holder and leave it alone. Its heartbeat keeps the claim live while the wait stands, even without a worker claim. Past the lease another host's next look takes it over; its holder's next look ends the claim once the wait ends. Only its holding host carries the wait into a round, human report entries, sendings through `--notify-command` and the asking session. A sending or question checks ownership again when it goes out; a host that lost the wait drops the queued item and names its holder in the log. Host conditions stay with their own host.
The operator on its host, started by its host runner when a **look** holds something an operator would act on. It runs in the operator's directory where one is named, otherwise as the **stock watch** under the stock instruction. The watch retains one interactive Agent session and Terminal across successive **watch rounds**. A human can see it work and type to it. Reporting completes a round and leaves the healthy session standing by; standing by with no pending work calls no model. Only exit, a proven harness fault, an unusable context or an explicit reset replaces it, after confirming its executor ended. Its Terminal is named “afkconvoy watch” for discovery; the name alone proves no ownership. The host runner keeps confirmed ownership across restart or upgrade and pauses uncertain association without replacing or taking an operator's other sessions. A pane of its own, by its saved pane identity, that Orca restored under a new incarnation is not taken back: its executor is ended, the pane closed, and the next look starts a fresh watch (ADR-0070).
A look is the host runner's own check of every checkout, every five minutes or as its host settings say, and runs no model. `doctor` is one look; each tick of the host runner is its own. One look reads the host once, the login once for the life of the process, the quota note at most once, each repo's HEAD at most once, whether orca is running once, orca's worktree list once, and each checkout's remote once, and answers every later question in that look from what was read. A doctor starts no harness while a startup probe that checked the current launch commands still stands, and otherwise starts each harness at most once. Looks continue during active rounds. Each actionable condition has an **episode**, lasting until an observed recovery, with resource episodes retaining their existing recovery span. New episodes queue while a round runs. Repeated observations of queued, active or reported episodes do not duplicate delivery; reported human decisions stay covered until their episode ends. Recovered conditions leave the queue before delivery. An unreported round leaves unresolved work eligible at the next look. The host runner keeps the queue, episode coverage, look interval, current round and reports durably; session conversation and an open Terminal are no proof of completion.
Its authority spans every repo on the host's checkout list: it fixes the host and hands an issue escalated for a fleet fault back once its cause is gone; makes a lasting fix in a checkout of its own of any listed repo, through that repo's landing turn; and, where the fix must reach an installed tool, has that repo cut a version after landing, lets the host runner move onto it and checks the fault on the new version. It answers escalations with the same commands as a human, whose effect and rework counts its answer has, and a failed **cut**, which no human is asked for. Every watch leaves only spending money with a human, and records it without acting on it. Everything else — what cannot be undone, a secret or private data leaving the host, a change of what a product is for — it decides by the recommended answer, recording what it chose, why and how to undo it in the daybook; a step that needs a person's hands is listed for the asking session and holds nothing else back.
A watch for host load or memory receives the fleet's worker process trees and, where `hostvitals now` answers on the host runner's `PATH`, its whole-host findings followed by its process and App rankings and remaining readings; otherwise it receives the fleet's hottest processes outside those trees. The evidence names its source and says why hostvitals could not answer. hostvitals is optional; hot-worker sampling stays the fleet's own.
_Avoid_: patrol (a role run on a schedule that writes no code), supervisor (the host runner is the process)

**Watch round**:
One bounded task delivered to the host's watch, with its own identity, brief, delivery receipt and explicit `afkconvoy watched` report. It contains the look, its pending episodes and earlier reports on the same work. Only one round executes at a time. Its hour begins with accepted execution, includes harness continuations, and excludes queueing, idle standby and completed rounds. Its report requires what held work back, actions, verification, human questions with answer lines and pending work. A successful exit or idle Agent without that report leaves it unreported. An active round may be nudged; an idle resident session is never nudged for a missing report. A failed harness continues the same round on the next eligible harness with retained progress and the remaining hour. Research alone starts a round at most once daily, and accompanies other work. The host runner selects the last harness to take a worker brief, then the host kit's eligible order. Its durable usage record states the session's tokens from delivery to report, continuations included and standby excluded, in the same `tokens` shape as a work account's workers; missing or unreadable usage is unknown, never zero. `accounts` summarizes these records beside issue work accounts.
_Avoid_: session (the Agent can execute many rounds)

**Asking session**:
The host's second resident interactive Agent and Terminal, beside the watch and in the watch's directory, through which the operator answers the human decisions recorded in watch reports. Its Terminal is named “afkconvoy asks” for discovery; the name alone proves no ownership. Its own pane back under a new incarnation, by its saved pty, tab and leaf, is taken back while its marked executor runs, and otherwise closed and replaced (ADR-0074). It starts when the first question is pending, stands by without a model when none is, and is kept across host runner restarts and upgrades. Only exit, a proven harness fault, an unusable context or an explicit reset replaces it. It does no watch's work: it runs no watch round, takes no look and reports nothing through `afkconvoy watched`. That excludes nothing the operator tells it in the session: a host fix or install, a ruling, a hand-back or a ticket it carries out at once, as the operator acting, within the bounds a watch keeps. The host runner durably keeps one current question and those waiting, each once per condition episode. It delivers one only when input is ready, withdraws it if its condition recovered after delivery, and delivers the next only after the current is reported settled or put aside by the operator. Put-aside questions stay pending. With the operator it clarifies, investigates and consults, then acts with their authority; the operator's quoted words and their time on the issue are the human instruction, never the session's own words. `doctor` and the panel show its state, current question and those waiting.
_Avoid_: watch round (the watch's independent bounded task), notification (a one-way sending with no place to answer)

**Exclusive queue**:
The host runner's FIFO queue for heavy commands across its fleets. Commands run in worker lanes, as many as the host settings allow; independently owned candidate worktrees of one repo may validate together. A light gate takes a turn in its repo's own lane. A worker's fast gate takes the fast lane.
_Avoid_: gate queue

**Heavy command**:
A command of a repo that fills the host by itself — a build or a suite of the whole workspace, a gate run by hand — which a worker runs only through the exclusive queue. A repo declares its heavy commands as patterns of words in its config's `yaml heavy-commands` block; the guard refuses a worker's call that runs one bare, naming the pattern and the line to run instead. What `afkconvoy gate`, `land` and `cut` run already queues, and is never refused.
_Avoid_: heavy step (unnamed), expensive command

**Nudge**:
The one line the fleet types at a stopped worker with no outcome, telling it to carry on. Sent once for silence; a capacity stop may call for another after a longer wait.
_Avoid_: ping, reminder, retry

**Worker goal**:
The goal a harness that keeps one — Codex — is handed a worker's brief as: the brief pointer, and when to mark the goal complete. While it is active the harness begins the worker's next turn by itself each time its model ends one short, a continuation, which is a sign of life. It ends complete, cleared by the fleet once the worker's verdict stands, `blocked` by its model, or at its token budget; a worker whose goal its harness stopped is a stopped worker, nudged as any other.
_Avoid_: thread goal, task goal

**Tell**:
The operator's note to the worker at work on an issue, busy or stopped, with `afkconvoy tell`, or a later issue comment or body edit the fleet tells it automatically each tick. The issue is the words' one home, which every later worker reads. Typed at the worker under this host's claim, with the comment's link or the body's changed lines (the whole new body when needed), they are told only once its harness's session record holds them as a message its model was given as the user's. A note seen only in the harness's input queue is not told. A busy Grok keeps a line typed while it works in that queue until its screen shows the line queued, and tell presses send now so the model is given it. The fleet skips its own marked comments and comments whose write receipt is in this worker's session, retries failed deliveries next tick, and names an update still untold after three consecutive ticks in doctor. With no worker here, nothing is typed. A nudge is the fleet's own line to a stopped worker, telling it to carry on; a tell carries the issue's words for the worker to decide within its scope or open another issue for.
_Avoid_: relay

**Restart**:
Replacing a silent worker with a new one in the same worktree and the same role, with its PR, branch and worktree untouched. Made once, and only for a worker waiting to land its PR.
_Avoid_: retry, re-dispatch

**Continuation**:
Recovering a claim whose worker died from the progress that survived: its worktree if still on this host, else its pushed branch, else a fresh start from its target branch.
_Avoid_: resume, checkpoint restore

**Wake**:
A signal that ends the sleep between ticks of its host's fleet instance early: every command that changes fleet state sends one as it ends — a worker's last among them, once its outcome is on GitHub — naming itself and what it acted on, and carrying no state. `gh` sends none: whoever hands the fleet work sends one with `afkconvoy wake` after the issues they opened with it. The `run` that hears it answers at once, naming the tick it starts or the tick in progress it follows.
_Avoid_: notification (a human decision sent through --notify-command), callback, done signal

### Claims

**Claim**:
The atomic lock marking one issue as owned by one fleet instance. Its creation succeeds for exactly one instance. It takes a slot — counts among the workers at work — while its worker is at work, and none once a verdict is declared under it: it is still held until that verdict is acted on, and a worker started under it again takes a slot as any start does. A landing claim's worker is at work only while it is changing the tree — resolving a conflict, fixing a red gate, or still preparing the branch for the turn. Gone or idle, or with its turn queued, waiting on a predecessor or last answering a wait, it takes no slot: the claim is still held until the PR lands or it ends otherwise, the tick asks for the turn again without starting a harness, and a worker it starts because the turn needs the tree changed takes a slot as any start does.
_Avoid_: assignee, assignment, lock (the fleet lock)

**Heartbeat**:
The timestamp a fleet instance publishes for itself while it holds claims, stamped with its host. A claim is live while its owner's heartbeat is within the lease. A land turn stamps its host the same way on the candidate record while its host holds a candidate, a barrier or a pending turn there; that host's members of the chain are live while either stamp of the host is within the lease, and any other host's next turn takes them over once neither is. A `run` told to stop while it holds no claim removes its own; a start removes the dead ones that hold nothing: those of the instances it took from, its own host's past the lease, and anyone's older than a day.
_Avoid_: ping, keepalive

**Repo nobody works**:
A repo with an open ready issue of its tracker, carrying its repo label, with no open blocker or claim, ready for more than an hour since the later of readiness and its last blocker closing, and no heartbeat within the lease on its remote. Every host serving that tracker looks, but only the report's opening host asks the operator which host should take it; an unreadable remote establishes no such repo.
_Avoid_: host is off (the cause is unknown), abandoned repo

**Orphaned claim**:
A claim of this fleet instance with no PR and no live worker. It is recovered by continuation, never released.
_Avoid_: stuck issue, zombie

**Stale claim**:
Another fleet instance's claim whose heartbeat is past the lease. It is the only claim a fleet takes from another host unattended. When the old host wakes, its next tick closes the worker's terminal and keeps the worktree, pushing nothing of its branch. A worker's verdict, land or gate is refused while another host holds the claim; a new instance on the same host keeps its workers. Commits only the old worktree holds stay there for a human, named by doctor until removed.
_Avoid_: dead claim, abandoned claim

**Takeover**:
Taking another fleet instance's claims — on issues, on patrols' runs, on cuts — without waiting for the lease. It happens unconditionally at start for claims stamped with this host, and by explicit command for another host's. A claim taken — by a takeover, or as a stale claim — is still the claim it was created as: what was declared under it stands.
_Avoid_: failover, stale reclaim

### Outcomes

**Park**:
Recording a dependency a worker discovered as a blocker on its issue and releasing the claim, so the frontier holds the issue back until the blocker closes.
_Avoid_: defer, snooze, hold (a fleet instance holds its claims)

**Escalate**:
Handing an issue on with its reason and answer line, its flow or ready label swapped for the escalate label, and its claim released. The host's watch answers it with the operator's authority, through the same commands as a human; only spending money stays with a human; a step that needs a person's hands is listed for one and holds nothing else back.
_Avoid_: fail (what a command, a gate or one thing a tick does may), retry

**Status board**:
The single comment on a claimed issue that shows a human how far along it is, and what the afkconvoy that works it was built from. It is rendered from fleet state and never read back.
_Avoid_: progress log, worklog

**Panel**:
The one web page that shows a human where every fleet of a host stands: the host itself, what waits for a human across its repos, and for each repo its workflows as the lines of their stages, the issues at work, queued and held back at each, and their landing turns. The host runner serves it, made anew from what each tick read; it is never read back.
_Avoid_: dashboard, status page, status board (the comment on one issue), UI

**Tick log**:
The file a fleet instance keeps on its own host: one line as it starts, a line as each tick starts — with why: its timer, a wake and the command that sent it, the fleet instance's start, or a follow-up — and one as it ends — what it did, and what changed on GitHub or by another host since the tick before where it acted on that — plus the lines of the events that reached the fleet instance: a wake heard during a tick, the host waking, being told to stop. Every command that changes fleet state, whoever runs it, appends a line of its own to the file of its host and repo as it acts — the command, the issue, who ran it, and how it came out or that it failed — whether a fleet instance is there or not. Each line is appended whole, and only the fleet instance cuts the file back: it keeps every line of the last day, and never fewer than 200 lines nor more than 5000. It is a log and not fleet state: nothing reads it to decide what the fleet does.
_Avoid_: state file, journal, history

**Daybook**:
The operator's one place to read, after the fact, what agents did and decided outside any issue — an interactive session's fix, a host install, a ruling made without a ticket. Its home is the Discussions of the host's tracker, in the category `Daybook`: one discussion a day, titled with the host's local date, and one comment an entry, which `afkconvoy daybook` appends in one shape — the repo, the kind of session that wrote it, and what it did, why, how to undo it, its lesson and its links. An entry's **lesson** is one sentence its agent, at the moment, thinks should outlast the day — "the gate on this host needs X" — written without interrupting the work, as raw material for the weekly distillation that carries each lesson to its lasting home; the day's discussion body indexes the day's lessons, derived from its entries, so a span's lessons are listed (`afkconvoy daybook --lessons --since <day>`) without reading every entry. Agents record there and carry on rather than stop to report — the host runner records each round the watch reports, linking the human questions it raised without asking them; the operator reads it when they choose. It is not fleet state: nothing reads it to decide what the fleet does (the watch reads it only for its own distillation), and it stays out of the repo, whose files hold what lasts. Its format and its one-discussion-a-day rule are a crate of their own, `afkconvoy-daybook`, which another program writing the same daybook depends on (ADR-0071). It is neither the **tick log** (a host's file of fleet events) nor the **status board** (one issue's progress).
_Avoid_: journal, diary, changelog, activity log

**Distillation**:
The host's watch's weekly reading of the **daybook** that carries each lesson to its lasting home (ADR-0065). At most once a week — sooner where the operator asks, `afkconvoy daybook --distill` — and only when the span since the last one holds a lesson, or two entries without one, enough for a fault to recur, a look makes it a watch item; the round decides each lesson's home and proposes it through that home's normal path: a repeated mistake first as a mechanism, a ticket to the repo that owns it; one project's decision or term, a ticket for its ADR or GLOSSARY; a cross-project one, the control repo's own ADR or GLOSSARY, changed directly; how to use a tool, a ticket for its skill; the operator's global principles, a draft PR they read and no fleet merges. A lesson its home already holds is `covered`. It keeps nothing of its own — no memory, precedent or proposal list: the proposal is the record. `afkconvoy daybook --distilled <file>` checks that the run carried every lesson of its span exactly once, then writes its one daybook entry mapping each lesson to where it went; the entries are left to age out. The watch keeps only when it last distilled and the last day it covered.
_Avoid_: digest, retrospective, memory (the host's, as memory pressure says it)

**Finding**:
One thing `doctor` says is wrong with the fleet on this host, of one **kind**, with how bad it is, what shows it, its fix — a line to run, or a file and the key in it to edit — the help topic that explains it, and the identity of its condition, made where it is judged and unchanged by its words. `doctor` fixes none itself. The host's watch acts on each finding that is not `info` by its kind and identity: each condition it acts on is one finding.
_Avoid_: diagnosis (a worker health assessment), alert, warning (one of its severities), error (what a command that failed answers with)

### Landing

**Gate**:
The repo's build-and-test command. It is the only machine verification a change must pass before it lands.
_Avoid_: CI (what a push to an ordinary branch starts), checks (what `check` and the startup probe make), local CI

**Stopped orphan**:
A process stopped by a signal, handed to pid 1 because what stopped it is gone, and working in a worker's worktree: nothing is left to send it SIGCONT, and it never leaves on its own. Settling ends it, and what it started, and the tick log names each (ADR-0074).
_Avoid_: zombie (a process ended but not reaped), paused gate (one whose pauser still runs), left-behind process (any process a test left, running or not)

**Gate limit**:
How long the gate may run. A run that reaches it is ended and is red. A repo declares it; nothing learns it from past runs.
_Avoid_: timeout (what a run that reached it did), budget (the warning threshold), baseline (a committed timing observation)

**Gate budget**:
The fixed duration a repo expects its gate to stay within. Recent recorded landings on one host that all exceed it make a gate gone slow; exceeding it never ends a run. The fast gate and the slow gate each have one of their own.
_Avoid_: gate limit (the termination bound), learned baseline

**Recorded gate run**:
The evidence that the gate passed on one committed tree, with how long the run took, on which host and on which kind of machine, and each test's wall time and CPU time where the run's output said them, made by the tool that saw the exit code and kept where every host can read it. A turn believes one from either ref layout while it stands.
_Avoid_: gate cache, CI status

**Kind of machine**:
A host's operating system and processor architecture, `macos-aarch64`. A recorded gate run says the kind it ran on, and answers for a run on that kind alone: some tests depend on the machine.
_Avoid_: host (one machine by its name), platform

**Reused run**:
A recorded gate run that answers when the same gate is asked of the same committed tree on the same kind of machine: nothing runs again, and the answer says so. The gate is the command the tree's own commit declares, so another declaration runs again; a red run is never on record, so never reused. It is always on: a candidate whose tree is green on record, and a round whose tip is, are not gated again, and the candidate's gate evidence names the reused run it stands on.
_Avoid_: cache hit, skipped gate, cached result

**Fast gate**:
The part of the gate a worker passes before it opens or updates its PR: the checks that are quick, and every test not marked slow. A repo declares it beside the slow gate, each with its own command, limit and budget; together they are the gate. A new test is in it until it is marked slow.
_Avoid_: smoke test, pre-check, quick gate

**Fast lane**:
The places of the exclusive queue that only a worker's fast gate takes, as many as the host settings say. No slow gate, turn or other command holds one, and fast gates of one repo run side by side in it, each in its own worktree.
_Avoid_: fast queue, priority lane

**Slow gate**:
The part of the gate the fast gate leaves out: every test marked slow, and any check not in the fast gate. Nothing in the fast gate is run again in it. A landing turn runs it once per round, not per candidate.
_Avoid_: full gate (the gate, both parts), gate gone slow (a gate whose runs exceed its budget)

**Gate gone slow**:
What the gate is when its recent runs for the base branch, on one host, all exceeded the gate budget, or took most of the gate limit where no budget is declared. The fleet opens one issue that says so, labelled `slow-gate`, and no second while that one is open.
_Avoid_: slow gate (the part of the gate holding the slow tests), regression, drift, creep

**Sync**:
Bringing a branch up to date by merging into it. Nothing is ever rebased.
_Avoid_: rebase, update branch

**Turn**:
One command's construction, validation and ordered landing of an open PR's candidate. Red, a conflict, invalidation or a rejected push ends it with nothing landed.
_Avoid_: landing slot, merge queue

**Candidate**:
An immutable merge of a PR's original head onto its assigned predecessor, validated in its own worktree, or by a reused run where its tree is green on record. Once the candidate has stopped, that worktree is removed and the candidate chain keeps the gate evidence. Its predecessor is the certified target tip or the preceding candidate.
_Avoid_: speculative branch, train tip

**Candidate chain**:
The ordered candidates of one repo and target branch, each depending on its predecessor. A failed or invalid candidate cancels its dependent suffix; already landed predecessors remain landed.
_Avoid_: landing train, speculative queue

**Round**:
One run of the slow gate on a candidate chain's tip, once every PR that asked to land is a fast-green candidate, that lands every candidate up to the tip together. Its size is the PRs that are ready, not the host's worker lanes. A PR that asks during it waits for the next. A red round is not split to find whose change failed: the tip's worker fixes it whole, from the round's issues and its failing tests, and the round's other candidates stop, their changes carried by the tip's PR.
_Avoid_: batch round (no candidate gated alone), bisection, merge train, rollup

**Batch round**:
A landing mode in which no candidate is gated alone (ADR-0068): the candidate chain is built as for a round, and its tip runs the fast gate and then the slow gate. Green, every candidate up to it lands as one fast-forward. Red, the chain is bisected — the middle candidate is gated, then the half holding the first red, until the first red candidate is found; it is dropped and its change sent back with the red run, the candidates before it land, and those behind it are rebuilt without it, for the round to go on. A repo is in it where its config says `landing: batch`, or where a round's caller asks for it; otherwise it lands per candidate. afkconvoy's own turns land per candidate; the landing crate's plain runner lands either.
_Avoid_: batch (a tick's), merge train, rollup

**Land**:
Pushing the exact candidate a turn recorded green onto its target branch as a fast-forward after its predecessor has landed, and closing the issue it completes. Issue workers, a workflow's landing worker and watches all land in a turn; watches retain the configured base branch. A cut lands nothing.
_Avoid_: join, enqueue

**Forge**:
Where a program's changes ask to land, as a landing asks it (ADR-0060): the change as it stands now — its source branch, its target, its title — whether it may land, the message of the commit that lands it, and what follows once it landed. afkconvoy's is GitHub's pull requests, with the issue each closes, its open blockers and its pinned target; another program's may be Forgejo's, or its own tasks. The forge never merges: a turn lands the exact candidate the gate passed.
_Avoid_: tracker (where issues are kept), remote (where git refs are kept)

**Repair**:
A change the fleet starts to restore a red target tip or finish retained work from an older release. Its operator worker owns an issue and a worktree, works outside product workflows and human stages, and lands through the same recorded turn as every other change.
_Avoid_: manual repair

**Cut**:
The run that makes a new version of a repo from its base branch: it commits the version on a branch of its own from the base tip, gates it, tags that commit, and publishes and installs it. The base branch never receives that commit, and turns keep landing while a cut runs; the version's home is its tag. Landed work past the newest version tag starts one when the fleet goes idle or the oldest landing reaches the repo's age bound; an untagged repo's declared initial version starts one when idle. A repo may declare cut needs: host checks that must pass before a host claims a cut. A due cut whose check fails is reported by the tick, plan and doctor, leaving another host free to cut. Each cut belongs to one host; a failed cut never holds the next cut back (ADR-0072): its issue, labelled `convoy-cut-failed`, stays open while the next due cut cuts again from the start on the base tip, and a cut that publishes closes it, naming the version; three failures of the same step the same way in a row make it an ordinary ticket of the fleet's, under the ready label, and the next cut waits for a landing. The next cut deletes the branches earlier cuts pushed but the one it takes up, and the checkout a failed one kept. A look that finds the issue starts a watch, which looks for its cause. Only a failure that needs money spent is a human's, under the escalate label (sunfmin/control#801) — a gate that ran red is answered with the tests it names as failing, its output from the first of them on, and the file of that run's own its whole output is kept in, which no later run writes; a gate red only on TIMEOUTs it says it cannot classify on an overloaded host is gated again within the cut's time, and is no failure — while a patch overtaken by a minor landing ends without tagging and gives way to the minor cut. A cut that took every step takes the label `convoy-waits-on-cut` off every open issue carrying it: those waiting on the next cut return to the frontier. A failed or overtaken cut takes it off nothing (ADR-0056).
_Avoid_: release (a claim's last step, and `afkconvoy release`), publish (a heartbeat's), deploy
