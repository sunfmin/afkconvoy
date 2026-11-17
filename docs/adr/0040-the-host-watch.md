---
status: accepted
---

# The host watch

Every host has one **watch**: the operator on that host, started by the host runner (ADR-0023) so that the fleets' work goes on. The fleets' workers, patrols and named consults do the work and the team's judgment; everything operational on the host is the watch's. It finds what holds the work back, fixes it — whatever that takes — checks that the work goes on, and says what it did. It opens no issue of what it found and hands nothing to a human but money.

## The look runs no model

Every `run --watch-every` seconds, five minutes by default, the host runner takes the look `afkconvoy doctor --all --since <until of its last look>` answers. This is the fleet's own code; a look that holds nothing to act on starts nothing, and no model runs while nothing needs acting on. Looks go on while a watch is busy.

What an operator would act on: an issue waiting on a human (one this host's watch holds, ADR-0103); a finding that holds something up (an `info` finding holds nothing up); a fleet instance not alive or broken; a failed tick; a worker whose health observation is past its time; a hot worker; a resource condition. Each is known by an identity that holds while its **episode** lasts — while each look still finds it — and starts at most one watch round per episode. A condition that recovered and returned is a new episode.

The resource conditions are those of ADR-0019, ADR-0088 and ADR-0107, each a finding in the look rather than an agent of its own. A hot worker is a finding at once, its evidence taken as it turns hot, before its process can disappear. Admission holding back for one reason over the host resource span is a finding. Memory pressure at macOS's warning level or above, held for that span, is a host finding of its own: it makes no worker hot and changes no admission decision. Five minutes recovered ends each resource episode, and resource episodes and what covered them survive a host runner replacement. Host evidence is taken only when a round is about to start.

Memory pressure whose largest holder is another user account — it holds more than every other account and more than this account's processes together, read from the process table — starts no watch while this host's fleets run no worker: the watch keeps to fleet-owned processes and could do nothing. It is an `info` finding naming the account, its uid, its memory and its processes, and `plan` says it as `memory_pressure`. Once a watch has reported a pressure episode held by another account, later episodes with the same largest holder are left with the operator, an `info` finding, until the operator settles that question in the asking session (ADR-0052) or the largest holder changes. Admission still holds workers back under the memory it reads, whoever holds it.

## A resident session receives independent rounds

The host runner keeps one resident interactive agent in a terminal titled “afkconvoy watch”, run in the watch's directory, trusted and with prompts skipped. A human may see it work and type to it. Its work arrives as **rounds**: each has an ID, a brief, the episodes it reserves, its delivery state, its report and one hour, its continuations together. Queueing, terminal startup and standby consume no round's hour. Before delivery a fresh look removes conditions that recovered. A round is delivered as the first message, through the same delivery protocol a worker uses, only once the previous round has reported and the terminal is ready for input; nothing is typed into a busy agent. Standby with nothing pending sends no prompt and runs no model. The pending queue and each observation interval are kept durably, so a restarted host runner picks up where it was; the terminal and its ownership are ADR-0096's.

The brief's last line is the exact `afkconvoy watched --watch <id> --text-file <report.json>` command. A report has six parts — `held_back`, `did`, `verified`, `decided`, `human`, `pending` — and completes only its named round. The same report again is harmless; a changed one, or one for another round, is refused. `doctor`'s `host.watch` and the next brief carry it. Printed output, a terminal gone idle and a harness that exited are never a report or proof of recovery. A healthy session stays after reporting, awaiting the next round.

A round's report keeps its episodes covered. A round that ends unreported — its terminal closed, its hour up, its harness exited, every harness given out — consumes nothing: the next look that still finds the condition queues it again, bounded by the look interval. A question a round reported for a human is not dispatched again. The latest outcome for an item wins.

## Which harness, and when it gives out

A watch runs on the harness this host last started a worker on that took its brief, then the host kit's order, never one paused or with a fault on record in the worker-health book until its probe answers. A watch has no model label: it runs the harness's default. Worker health observes the watch's terminal as it does a worker's. A usage limit, capacity stop, refusal or failed exit ends the owned session and starts the next eligible harness in the same slot, with the same look and the earlier session's terminal evidence, within the round's hour. The fault is recorded at account scope where the harness's words prove the account — so no worker is started there either — and otherwise at worker scope, which pauses only the watch. With no harness left, `doctor` says at critical that nothing watches the host.

## Its directory

The watch runs in the directory `run --watch-dir` names: the operator's own, under its own rules for looking after the host's projects, with nothing added. Where none is named, or `--watch-dir default` is said, the **stock watch** runs in a directory afkconvoy makes in its cache and keeps, under one short instruction carried in the binary. The authority, bounds, triggers, harness choice and record are the same in either directory. A running watch keeps the directory it started under when the setting changes. The operator's other sessions in that directory are never adopted or closed.

## Its authority across the checkout list

What the operator would do at this host's keyboard, for every repo on the host's checkout list, bound or unbound: a fleet unable to start a worker cannot make the change that fixes its own start. For example, and not a list to keep: trust a worktree; end a runaway or orphaned fleet process; restart, nudge, continue or switch a worker; change a host setting; free disk; send heavy work through `afkconvoy exclusive`. After fixing a fleet fault it hands the escalated issue back with its current answer line, wakes its checkout and checks that the next tick dispatches the issue and its worker takes its brief; a label change alone is no evidence.

It answers any escalation with the commands a human uses — rule, hand back, send, close, change blocked-by — from the same answering list a human and a named consult receive. Its answer carries no consult identity and has the same effect as a human's, its rework counts starting over. There is no stock consult: an unbound repo's escalation and every hand-off from a stage with no named consult escalate at once, for the watch's next look (ADR-0005).

A lasting fix in code it makes itself: in a linked worktree of the affected repo named `afkconvoy-watch` (or with a numeric suffix), on a branch of its own from that repo's base branch, with a PR on no issue and no intake or flow label, landed by `afkconvoy land --watch` in that repo's turn. Its join commit names the watch and the host. Where the fix must reach an installed tool, it waits for the landing, has that repo's fleet cut through its declared steps (ADR-0028), fixes what holds the cut back, and checks the fault on the installed version; it never tags by hand, makes a cut record, or kills or restarts the host runner, which moves onto the new release itself. What has not finished is reported as pending, never as verified.

No command per remedy is added: the watch has the commands an operator has and their help.

## Only money is left to a human

Every watch, stock or not, leaves only spending money with a human: it acts on none of it and lists each such decision in `human` with its condition, question and answer line, for the asking session (ADR-0052). Everything else it decides by the recommended answer and acts on — what cannot be undone, taking a secret or private data off the host, changing what a product is for or who it is for — recording each in `decided` with what it chose, why and how to undo it; the round's daybook entry carries it. Before what cannot be undone it first makes what way back it can: a backup, the old copy kept. A step that needs a person's hands — an account, a credential, a login, a click — is listed for the asking session and holds back only the work that needs it. A prohibition that guards money only a human loosens; any other the watch may loosen, recorded in `decided`.

## Its bounds

Those of any unattended worker, held by the guard where it can (ADR-0012), under `AFKCONVOY_WATCH`:

- **Only what the fleets of this host started or own.** A `kill` names a pid the guard reads as a fleet's: started with a worker's `TMPDIR`, or under or left behind by such a process. Never the host runner, the user's programs, other apps or Orca; `pkill` and `killall` are refused.
- **Never delete what cannot be rebuilt**, nor a change no commit holds: model weights, downloads, media, Docker's images, the user's files, `git clean` without `-X`, `git worktree remove --force`, `git reset --hard`, `orca worktree rm --force`.
- **No force push, no push to a base branch, no landing outside a turn.**

What the guard cannot hold, the brief says.

## Considered options

- **One command per remedy**, each a check made before acting. The list is never done, and each new item waits for a release while the work stands.
- **A consult that looks but cannot act, and opens an issue for what it cannot fix.** The issue waits for a human and the work waits with it; the stock consult that did this repeated faults it could not fix.
- **Bound the operator by an allowlist of commands.** The same list at a lower layer. The guard refuses what crosses a bound and leaves the rest to the operator.
- **Run a model on every look.** A model every five minutes for nothing on most looks; `doctor` already says what is wrong.
- **A non-interactive watch whose printed output is the record.** Silent for minutes while it worked, it read as stuck, and could not be reached. The operator is visible and reachable as a worker is, and reports by command.
- **A new agent per round.** Discards useful session context and the continuously usable terminal. Keeping the queue only in the conversation would lose delivery ownership on restart.
- **A patrol.** A patrol is a role on a schedule in one bound repo that writes no code; the watch acts on any repo of the host, only when something needs acting on.

## Consequences

- A watch is one more agent session on the host, outside every fleet's concurrency, and one at a time.
- The worker-health book holds faults a watch proved: an account's usage limit a watch met pauses that harness for workers too.
- The watch's start, each round, its continuations and its end are lines of every tick log on the checkout list and of the host runner's log; `doctor` and the panel show the resident session, the current round, pending episodes and each report.
