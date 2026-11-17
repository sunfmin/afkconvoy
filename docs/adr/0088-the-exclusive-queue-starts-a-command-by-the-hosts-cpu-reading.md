---
status: accepted
---

# The exclusive queue starts a command by the host's CPU reading

The host's exclusive queue (ADR-0019) has no count of places. A queued command starts once the host has room for it, and a running command may hold others up only for a bounded time.

Felix (sunfmin/control#1240): whether a gate starts should be decided from the host's resources right now, by its CPU use, not by waiting in a queue. A fixed count of places was right on no host: one place left an 18-core host mostly idle while turns waited, and a count large enough for an idle host overfills a busy one.

## When a command starts

- **Room is read as admission reads it** (ADR-0107): the share of the host's cores busy by the kernel's CPU ticks over the last minute, sampled each second by the host runner; the load average over its cores where the ticks cannot be read. A command starts while that share is under the **busy share** (90% by default, the one admission holds at; `run --busy-share PERCENT` sets it live for the host) and while free memory is over admission's floor (`--admit-memory`).
- **No overshoot from lag.** A command just started is not in the minute's reading yet. Starts are made one reading at a time, and each counts at once as the CPU its kind of command took in its recent runs, the median of its last five runs' mean share of the host, sampled from the process trees of the queue's running commands, until the reading's span begins after it. A kind with no run kept counts as a quarter of the host.
- **Never starved.** While nothing of the queue runs, the oldest command that may start starts whatever the reading says, so a host loaded by others still lands. A retained ticket, whose command ended, runs nothing.
- **What guards correctness stays**: same-repo isolation for ordinary heavy commands and turns, though a repo's landing candidates run side by side (ADR-0037, ADR-0083); a light gate in its repo's own line, one at a time; cuts first among the waiting; and kernel-held places that outlive a replaced host runner, one per running command. Fast gates run side by side and are held apart from no slow gate, turn or command of their repo.
- A lane still says what a command is (`worker`, `fast`, `slow`, or a repo's) in the log, `queue`, `doctor` and the panel, and which commands it is held apart from.
- A gate's tests are given the host's cores divided over the queue's running commands, itself among them.
- Admission counts the queue's load (ADR-0107): each lane's commands drain as many at once as run in it now, at least one.
- The disk floor (ADR-0082) counts four builds of the queue's own worktrees, the places it had by default, beside the worktrees workers were started in.

`afkconvoy queue`, doctor's `host.exclusive` and the panel show `room`: the reading, the busy share, what the reading does not include yet, and whether a command already started on it. A command waiting on the room gives `host_busy`; `next_reading` where one already started on this reading; `host_memory` where memory is under the floor. `run --gate-lanes`, `--fast-lanes` and `--slow-gate-lanes` are refused, naming `--busy-share`; their environment variables are said to be ignored, and `run` goes on.

The rule lives in the gate crate, which guildd also embeds (ADR-0058). Its `Host` says the reading, the share and what a kind of command takes; a host that says no reading runs one command at a time.

## The bound on holding others up

A benchmark once held the queue for over an hour with eleven gates and landings of several fleets behind it (sunfmin/control#100). A running command is now ended once it has held others up for the host's bound, 1,200 seconds by default. Only time with another command waiting counts: gaps without waiters pause the count, and changing waiters do not reset it. A waiter held by the host's room counts as held by every command of the queue that is running. `run --exclusive-seconds N` (or `AFKCONVOY_EXCLUSIVE_SECONDS`) sets it for every fleet on the host, including a command already running; `default` restores it, and zero ends a command as soon as another waits.

The host runner counts with a monotonic clock at every queue change and asks the command to end at the bound; its place stays held until its descendants are ended and it is reaped. The command exits 124, reserved for this outcome; a command itself exiting 124 is reported as 1. One stderr line tells the worker to run each step needing a quiet host as its own `afkconvoy exclusive`, so gates and turns run between steps; the tick log says which command ended, its runtime and how many waited; `doctor` reports each occurrence of the last day, so a bound too low for an ordinary gate shows. A command running with no host runner is unbounded: there is no queue to hold up.

A cut's own commands are exempt (sunfmin/control#1263). A cut goes ahead of every waiting command, so ending it frees nothing for long and only restarts it; its gate is bounded by its own `gate_seconds`, and the cut by its time limit.

## Considered options

- **Raise the default counts.** Refused: a count right for an idle host overfills one loaded by another account, and no count follows the load as it changes.
- **Start by the load average alone.** Refused: macOS counts threads that want no core in its load average; the ticks say what the cores did.
- **Start every command whose own cost fits under the share.** Not taken: one reading per start and the counted lag keep the next reading honest.
- **A wall-clock limit on a command.** Refused: it would end a long command on an otherwise idle host.
- **Reset the hold count when a waiter leaves.** Refused: changing waiters could then extend a hold forever.
- **A per-repo bound.** Refused: it would let one repo hold every other fleet up.

## Consequences

- On an idle host, turns of different repos, fast gates and slow gates run side by side, as many as its cores carry; on a busy one, one at a time.
- A host that wants cores kept free for another system sets `--busy-share` lower, or `--max-share-of-cores` (ADR-0107).
- A long measurement takes the queue once per step that needs a quiet host.
