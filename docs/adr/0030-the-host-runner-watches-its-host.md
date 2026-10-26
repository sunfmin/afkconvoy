---
status: accepted
---

# The host runner watches its host

Amended by ADR-0038 (sunfmin/control#118): watches run interactively, report by command, and retain their terminal after the harness ends. The non-interactive launch and printed-output record below no longer apply.

Amended by ADR-0033 (sunfmin/control#94): every host has a watch. Where no operator directory is named, the stock watch runs in afkconvoy's own directory. The statements below that no directory means no look or watch no longer hold.

An inspection starts only on a hot worker, or on admission holding back or memory pressure (ADR-0019, ADR-0026, ADR-0029). Everything else that holds a fleet's work back waits for a human. On afkconvoy 0.5.19, 25 to 26 October 2026, that was all of what held it back: every unbound repo's Kimi worker stopped at Kimi's "Trust this folder?" and its claim was escalated to a human (sunfmin/control#75, #76, #77, #77 being the fix, which could not start either); two workers sat on Codex's usage limit for about an hour and a half until the operator came back (sunfmin/control#74); and the only thing watching the host was an agent session a human had left running, checking `doctor --all --since` every five minutes by hand, which sat on a permission prompt from 13:58Z to 23:15Z.

So the host runner watches its host itself. It is the one long-running process on the host (ADR-0023), and it already knows how to look: `afkconvoy doctor --all --since <until>` (sunfmin/control#48) is the operator's check-in.

## The look runs no model

Every five minutes — a host setting, `run --watch-every` — the host runner takes the look `doctor --all --since <until of its last look>` answers, and keeps `until` itself. This is the fleet's own code. A look that holds nothing to act on starts nothing, and no model runs while nothing needs acting on.

What an operator would act on: an issue waiting on a human; a finding that holds something up (an `info` finding holds nothing up by its definition); a fleet instance that is not alive, or is broken; a failed tick, or a step failed since; a worker whose health observation is past its time. Each is known by an identity that holds while its **episode** lasts — the episode lasts while each look still holds it — and each starts at most one watch per episode. That no harness can start a watch is left out: it is a finding for the operator, which a watch cannot act on.

## A watch is the operator, in the operator's directory

A look that holds something new starts one **watch**: an agent session started in the directory a host setting names, `run --watch-dir` — the operator's own, holding the rules for looking after this host's projects. With none named there is no look and no watch.

It is given its brief as a file, beside its transcript in the host's cache: what the look answered, whole; why it was started; and what each earlier watch on the same items did. It has the authority and the bounds of an inspection (ADR-0026), which its brief says, and an hour.

It runs on the harness this host last started a worker on that took its brief — the fleet records each delivery — then the host kit's order, never one paused and never one with a fault on record in the worker-health book (sunfmin/control#74), whatever its scope. It is started so that nothing it runs can wait on a prompt: its directory trusted as a worker's worktree is, in its harness's non-interactive mode (`--print`, `exec`, `--prompt`), and its terminal exits with the harness. What it printed is kept in its transcript, and is what it did.

The watch is known by its own terminal, and only that terminal is ever closed: the operator's directory holds the operator's own sessions too — the stalled one above was there.

## A watch whose harness gives out moves on

The harness may stop the watch partway: its usage limit reached, its account refused, the harness failing. The host runner reads that from the harness's own words in the transcript (Codex's usage limit, as the observation of a worker's health reads it), or from the harness exiting with a failure. It then closes that session's terminal, records the fault — at account scope where the words prove the account, so that no worker is started there either; at worker scope for a failed exit, which pauses nothing but the watch — and continues the watch on the next harness that can start it, the way a worker is continued (sunfmin/control#19, #74). The new session gets the same look, and what the first one printed. It does not wait for the watch's hour. A harness skipped for a fault is eligible again once the hourly probe of the worker-health book finds it answering. If no harness can start or continue the watch, `doctor` finds so, at critical: nothing watches the host.

## Never a second watch

While a watch runs no look is taken, so none can start another: what a look would have found meanwhile is in the first look after the watch ends, whose `--since` is the until before it. The watch, its terminal, its transcript and the episodes watched are kept in the host's cache, not in the process: a host runner that starts again — after moving onto a release a watch cut, say — first finds the watch still running, and keeps it.

## The operator for every checkout (sunfmin/control#79)

The watch's authority spans every repo on its host's checkout list, bound or unbound. A fleet unable to start a worker cannot make the change that fixes its own start: on 26 October 2026 the Kimi worktree trust gap held afkconvoy's fix, sunfmin/control#77, back along with every affected repo's work. The watch can fix that host and make the lasting change itself.

It may trust a worktree, restart, nudge, continue or switch a worker and change a host setting. After fixing a fleet fault it hands the escalated issue back with the current line `doctor` gives, using that repo's tracker, wakes it after a `gh` change and checks the next tick dispatches or continues it and the worker takes its brief. An escalation asking a human for a decision stays untouched: the watch lists the question and its answer line in its record and supplies no ruling of its own.

A code change is made in a linked worktree of the affected repo, on a branch of the watch's own from its configured base branch. Its checkout is named `afkconvoy-watch` (with the usual numeric suffix allowed), distinct from `afkconvoy-inspection`: a later inspection must never take the watch's checkout for one a previous inspection left. The watch commits, pushes, opens the PR against that repo's base branch, gates it with `gate --train` and joins with `land --inspection`, on no issue and with no intake or flow label. This reuses the inspection's join attribution and all its train behavior — including locks and rejoining an abandoned train — rather than adding a second no-issue landing protocol. `inspected` still belongs only to inspections; the watch's transcript names its repo and PR.

When code must reach an installed tool, the watch follows through: wait for landing, wake that repo's fleet and check its due cut, fix what holds back its next idle tick, and check publication and installation. It may continue a mechanically failed cut with its report's `cut --from` line after fixing its cause. It neither manufactures a cut record nor tags outside the declared steps (ADR-0028). It lets afkconvoy's host runner move onto the installed version itself (ADR-0023), reads what holds that move back and checks the fault and the fleets' work on the new version. It never kills or restarts the host runner. A human decision, missing cut steps or an unfinished cut or move is recorded as pending within its existing hour, never reported as verified.

No new remedy command or retry loop is added: the operator has the existing commands and their help. The brief and top-level help say its whole authority and require evidence of recovery, not just a label change, joined PR or installed version.

## The record

The watch's start, each thing it did, how it continued and its end are lines of every tick log on the host's checkout list, and of the host runner's log. `doctor` answers the watch running and what the last one did, under `host.watch`.

## Considered options

- **A patrol.** A patrol is a role run on a schedule, in a bound repo, that writes no code. The watch acts as the operator does, on any repo of the host, bound or not, and starts only when something needs acting on. **Patrol** names something else.
- **Widen the inspection's triggers.** An inspection is started in a checkout of the repo whose fleet called for it, in the stage's consult role, and is held by the guard to that repo's worktrees. What a watch acts on spans every fleet of the host, and the operator's directory holds the rules for looking after all of them.
- **Run a model on every look, to decide whether to act.** That is the stalled session above, every five minutes, for nothing on most looks. `doctor` already says what is wrong, with the fix; the watch needs a model only once something is.
- **Keep looking while a watch runs, and queue what a look finds.** A look's interval would then be consumed while nothing could act on it, and a queue would be one more thing to keep across a restart. Not looking keeps the interval for the first look after.
- **Read what the watch did from a command it calls** (`afkconvoy inspected --did`, as an inspection does). An inspection's record is that. A watch runs in the harness's non-interactive mode, whose answer is printed: the transcript is the record without a new command, and keeps what a session whose harness gave out had printed so far.

## Consequences

- A host with a watch directory set watches itself, and says so in `afkconvoy --help` and `doctor`. One without one is as before.
- A watch is one more agent session on the host, outside every fleet's concurrency, at most one at a time and an hour each.
- A watch writes into the operator's directory only what its session does there, and the worker `TMPDIR` link a worker's worktree is given.
- The worker-health book holds faults a watch proved too: an account's usage limit a watch met pauses that harness for workers until its probe answers, as a worker's would.
