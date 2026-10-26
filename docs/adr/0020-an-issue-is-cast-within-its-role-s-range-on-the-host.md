---
status: accepted
---

# An issue is cast within its role's range on the host

ADR-0013 had a stage name its harness and model, and the config name a default. Which harness and model suit a piece of work depends on the issue — how hard it is and what kind of work it is — more than on the stage it is at, and a stage cannot know that when the team is written. And which harnesses and models can run at all is a fact of each host: what is installed, logged in, and answering.

So the choice is made per issue, inside bounds two places set:

- A team's **roster** lists, for each role, the harnesses and models its workers may run, with what each model is good at and how to cast among them. It is committed with the team, as ADR-0013 wanted which harness does the work to be.
- A host's **host kit** lists the harnesses and models it can start now — each harness's worker launch command and listed models — and the harness and model of the stock consult. It is written with no human: `afkconvoy kit`, or `run` on a host without one, finds each harness installed in the shell a worker gets and has it list its models without prompting them. A harness is held at once on its default model; research then places listed models with dated sources, while models `research pending` or left `not researched` are not started on. The watch researches unplaced models through its web tools and places sourced evidence with `afkconvoy kit --research <file>`, which checks the whole file before writing and refuses one placing nothing. Models omitted stay as held. Unplaced models are a doctor finding, joining other watch work or starting one alone at most once a day (ADR-0030, sunfmin/control#97). Nobody is asked, and nothing is confirmed: one way writes the host kit, and `run` never refuses for want of one. It is never pushed. Once a day a tick lists again, adding new ids for research and dropping ids no longer listed; one fleet of the host does it, under a lock beside the file, and the others read the file. A harness that stops working is paused on the host and left out until worker health's hourly probe finds it working again. ADR-0030 records the listed ids, the default model and these research states.
- A role's **range** on a host is its roster entry cut to the host kit, in the kit's harness and model order, less each harness the role holds no limits for while it holds limits for another: no worker of the role runs a harness it holds no limits for, and a host kit may hold everything its host can start. `check` and `plan` name each harness a range leaves out, and why. An unbound repo has no team and ranges over the host kit whole.

An issue's **casting** is its `model:` labels, the model in the harness's own name or any of its other names. Kit order and role eligibility choose the harness first. Only a model label matching that harness's eligible models is followed; labels belonging to other harnesses remain intact and apply again when those harnesses are selected. Legacy `harness:` labels are retained but never choose a harness. Human labels are never recast or removed. When two labels match different models of the selected range, neither selects one: its first model runs, and diagnostics explain the ambiguity.

## Opt-in model casting and model priority (sunfmin/control#21)

The kit holds one host-local `cast` setting, false when absent. `afkconvoy kit --cast on|off` changes it without probes; no repository setting or second copy exists. Rebuilds, daily refresh, command discovery and runner restarts retain it, including a setting changed while research was running. With casting off, no tick starts a caster or holds an issue for one; `cast` explains that it is off and sends no prompt. Workers still follow human model labels, otherwise the selected harness's first eligible model.

With casting on, a choice means that some stage's kit-selected eligible harness holds more than one model in that role's range and the issue has no matching model label of that harness. A harness with zero or one model offers no choice, whatever later harnesses hold. One caster asks in one session about all such ready issues, with only the selected harnesses' models, their `good_at`, kit order and roster `cast` guidance. It validates the answers against each issue's offered ranges and again against current priority and labels, then adds only `model:` labels naming ids, with a comment. It never removes labels or writes a `harness:` label. A fallback to another harness can therefore add its model beside an earlier caster label; switching back reuses that earlier label without another session. Any human casting label keeps the issue out of automatic recasting.

A tick holds an issue for a caster for at most ten minutes; after that it runs its range's first model. That wait belongs to the available candidate set, so a changed priority or recovered candidate can open a new model choice. A live caster still prevents a second concurrent caster. Plan, doctor and panel share each issue's casting explanation, whether a model label was followed, skipped earlier models, and the recorded model of its running worker, which may differ from the next start after a switch.

`switch --model NAME` resolves any of a model's usual names across the kit. A match must identify exactly one harness and one model; ambiguity lists candidates, and no match lists nearest names, with no mutation. Promotion moves that model first within its harness and that harness first within the kit, retaining every entry and other relative order. Repeating the promotion is idempotent. No separate selected-model record exists. Rebuild, refresh and new-model research preserve established model order and append discoveries after it; mutation reconciliation also preserves a switch made while research ran. A role's roster constrains which models may run, never their order. Paused or unavailable models are skipped within the same harness; recovery restores eligibility without reordering. A switch alone leaves healthy workers running; `--all` explicitly continues them on the new selection.

## Host priority and fault continuation (control #19)

Supersedes the earlier repository-local switch choice and the preference ordering above. Harness priority has one home: the order of the host kit. `switch --harness X` moves an existing entry first, preserving its command, models and research and the relative order of the rest. A known installed harness not held is added first on its stock command with no model, so its workers start on the login's default without waiting for listing or research; a targeted background refresh after the answer hands its listing to research without changing the order or probing a model. An added harness that cannot start stays visible to `doctor`, with the fault also in the switch's answer. It runs without a checkout or instance. There is no separate chosen-harness record, repository mapping, clear operation or saved previous order. Old git-directory choices are neither read nor migrated. Kit writes reconcile with the latest order under the same mutation lock as promotion, so research already in flight cannot overwrite a switch. Research may rank new models, never established model or harness priority.

Every issue, stage, consult, patrol, landing, caster and inspection selects the first eligible harness in kit order. Role support and limits remain constraints. Ticket casting, roster preference and stock-consult preference cannot select a later harness; matching model labels only apply within the selected harness. Startability faults are skipped and explained, not passed off as a successful preferred launch. Team and ticket data are unchanged.

No running worker is interrupted by changing priority alone. `--all` continues workers across the host's checkout list, collecting failures while processing the rest. Continuation keeps the scene and breaker counts. Before another writer starts, terminal closure, runtime reports and the old process tree are checked; uncertainty leaves no second writer.

For the train worker, a new PR joining is a new selection boundary (sunfmin/control#84). It is told only while live, without an agent failure, and on the host kit's first eligible harness; otherwise it is ended and replaced there with the train's brief. This includes a worker at work on an earlier harness after a switch. `switch --all` continues the train worker immediately and lists it despite its having no claim. A continuous worker-health hold is bounded by the recovery window even when evidence changes or diagnosis cannot answer: past it the train worker is replaced and the tick log says why. Every replacement keeps the train, its worktree and its repairs, and confirms the old writer ended before starting another.

Worker health is evidence, not priority: observations and scoped faults live in the host cache, shared by its fleets and retained across runner restarts. Structured quota/authentication/disable, model and session errors retain their scope. Each launch keeps a non-secret command identity in its worktree; shared faults match that identity so replacing a command or account profile while an old worker runs cannot disable the replacement. A legacy worker with no recorded identity can only establish worker-local evidence. A transient rate limit waits for its retry time and has two retries before session fallback. Codex's explicit `Usage limit reached … Increase your limits to continue using codex` is an account quota fault by rule; other screens alone go first to another available kit harness for a tool-free diagnosis. The diagnosis returns a checked structured judgment with quoted evidence and missing information; it cannot perform recovery itself. Unknown, malformed or timed-out judgments preserve the scene. Silence never certifies an account fault.

Observation begins after a worker stops reporting progress. Two observations span at least 300 seconds; known background work and declared verdicts are excluded. Diagnosis has 30 seconds per candidate, three attempts per unchanged episode, and at least 300 seconds between attempts. Diagnostic failures try subsequent eligible harnesses and record which harness answered what. Account quota without a reset continues at once; a reset within the one-hour recovery window may be waited for. Repeated waits on unchanged evidence do not restart that window: after one hour the fleet acts on the fault class or escalates the issue with its evidence. Candidate account/model recovery probes run hourly, and only successful probes restore availability. A recovered preference participates in new starts without moving healthy successors back. Plan, doctor, panel and the tick log expose evidence, diagnosis source, scope, next check, action and actual successor.

## Task refusal is remembered by the issue (sunfmin/control#101)

A harness's fixed task-refusal words decide by rule, beside the explicit usage
limit, before observation or diagnosis. Codex's refusal seen on hostvitals'
sunfmin/control#98 is the first rule; another harness gets one only with its
observed screen in a test. A refusal is of the task, never the account or model:
the harness remains healthy for other issues. The worker continues through the
same confirmed-termination path, keeping its worktree, branch, stage, claim and
PR. The issue records each refusing harness on GitHub, and every subsequent
start, across stages and hosts, excludes it before kit order chooses a harness.
Role permissions are still resolved against the whole kit, so this exclusion
cannot broaden a role's range. Doctor and show expose the harness and its words.
With no eligible harness left for this issue, it escalates immediately with the
refusal evidence. No consult is started on the exhausted kit. This amends the
observation and one-hour recovery window above for explicit task refusals only.

## Capacity stops wait briefly, then continue (sunfmin/control#108)

Codex's fixed `Selected model is at capacity. Please try a different model.`
screen, reported on sunfmin/control#104 on 2026-10-26, classes a capacity stop
by rule, before observation or diagnosis. Other harnesses acquire a rule only
with an observed screen in a test. This amends the 300-second observation
rule above for capacity stops, beside usage limits and task refusals.

The first stop waits 60 seconds before nudging the same session. A second
stop in the episode waits 120 seconds before another nudge. A third continues
through the existing confirmed-termination path to the next eligible harness
in host kit order, keeping the worktree, branch, stage, claim and PR. One
unchanged screen during a wait counts once, and each successful nudge must
have a receipt proving a turn started before another stop counts. The episode
survives brief working turns and host-runner restarts, and ends after 300
seconds observed healthy or when the session or harness changes. Changing
terminal output or git progress alone does not erase its bound.

Capacity is worker scope. It faults no harness or model for other workers,
and adds no task refusal to the issue. The tick log records the capacity rule,
each wait, each nudge and continuation; doctor retains the number of capacity
stops per harness on the host, including after a worker's episode ends.

Immediate continuation would abandon capacity that passes in a minute;
unbounded nudges would keep a slot indefinitely. Two increasing waits give
transient capacity time to pass without paying for diagnosis or spending
another five minutes merely observing a fixed message.

## Considered options


- **Keep a stage's harness and model as an override.** Two places would decide which harness runs, and a wrong one would be looked for in both. A role that must run on one harness is given a range of one.
- **The roster on each host.** Which harness does the work changes the result, so it belongs with the team on every host, not with whichever host started the worker; what a host adds is only what it has.
- **A caster's judgement at each start, kept nowhere.** Not reviewable, paid again at every stage, and different from one start to the next. A label is on the issue, where a human sees and changes it.
- **One casting label per role.** Exact, but twice the labels and harder to read; in practice one step chooses and the rest are held to one harness.

## Consequences

- Supersedes ADR-0013's first paragraph: a stage, a patrol and the config name no harness or model, and `check` and `run` refuse a team file or a config that still does, saying what to delete and what the roster says instead. ADR-0013's other half stands: a role writes its limits once per harness, and a role's range leaves out a harness it has no limits for. A range left empty refuses no start: no worker of that role runs without its limits, so its stages wait while it lasts, and every other stage goes on. A roster entry that matches nothing in the host kit is not followed: the role ranges as if the roster named none for it. `check` still refuses a roster entry that names such a harness: those are the team's own words.
- `--worker-command` is gone: a harness's worker launch command is in the host kit.
- In an unbound repo the fleet now appends a harness's model flag to the worker launch command when an issue's casting names a model; with none, it uses the selected harness's first eligible model, or the command as written when it holds no model.
- A team's roster is drafted by a command, from the host kit, into the team's working tree, and committed by a human; the fleet never changes it.
- ADR-0008 left out a stage choosing a later stage's model; the caster chooses for an issue's stages before any starts, which is that, and is now in.
