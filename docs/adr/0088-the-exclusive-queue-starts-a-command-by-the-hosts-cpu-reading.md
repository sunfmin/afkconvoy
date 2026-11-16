---
status: accepted
---

# The exclusive queue starts a command by the host's CPU reading

Amends ADR-0019, ADR-0034 and ADR-0049, and the slow lanes of ADR-0048
(sunfmin/control#1240).

Felix, control session 2026-11-16 ~19:20 JST: 「现在 land 的时候，合并的时候，之前跑测试那个阶段。都是要排队的。我觉得改成不要排队，要看当前系统资源来决定是否调用。看当前系统的 CPU 的使用量。来动态确定。」 — at landing, the gate runs before a merge all wait in a queue; instead, whether one starts should be decided from the host's resources right now, dynamically, by its CPU use.

Seen on macnoMac-Studio, 18 cores, 2026-11-16 19:19: the one worker lane held a turn, and a guild turn and a guild command waited with `worker_capacity` as their reason, with most of the host's cores idle. A fixed count of places was right on no host: one place left 18 cores mostly idle, and a count large enough for an idle host overfills a busy one.

## Decision

The exclusive queue has no count of places. `gate_lanes` (the worker lanes, ADR-0034), `fast_lanes` (ADR-0049) and `slow_gate_lanes` (ADR-0048) are gone. A queued command starts once the host has room for it.

- **Room is read as admission reads it** (ADR-0084): the share of the host's cores busy by the kernel's CPU ticks over the last minute, the host runner sampling them each second; the load average over its cores where the ticks cannot be read. A command starts while that share is under the **busy share** — 90% by default, the one admission holds at, which `run --busy-share PERCENT` sets live for the host — and while the host's memory free is over admission's floor (`--admit-memory`).
- **No overshoot from lag.** A command just started is not in the minute's reading yet. Starts are made one reading at a time, and each counts at once as the CPU its kind of command took in its recent runs — the median of its last five runs' mean share of the host, which the host runner samples every five seconds from the process trees of the queue's running commands — until the reading's span begins after it. A kind no run of which is kept counts a quarter of the host. This is admission's one reading per start (ADR-0072), applied to the queue.
- **Never starved.** While nothing of the queue runs, the oldest command that may start starts whatever the reading says — a host loaded by others, Studio64's simulators (ADR-0085), still lands. A retained ticket, whose command ended, runs nothing.
- **Unchanged**, for they guard correctness, not load: one turn of a repo at a time and same-repo isolation (ADR-0037); a light gate in its repo's own line, one at a time; cuts first among the waiting; the bound on a command holding others up (ADR-0031), which now counts a waiter held by the host's room as held by every command of the queue running; and kernel-held places that outlive a replaced host runner, which now say a command runs rather than fill a count: each command takes a place of its own. Fast gates still run side by side, and are held apart from no slow gate, turn or command of their repo.
- A lane still says what a command is — `worker`, `fast`, `slow`, or a repo's — in the log, `queue`, doctor and the panel, and which commands it is held apart from.
- Admission still counts the queue's load (ADR-0050) and holds while the queue would take more than 15 minutes to drain (ADR-0072): each lane's commands drain as many at once as run in it now, at least one.
- A gate's tests beside other commands of the queue are given the host's cores over the commands running, itself among them (sunfmin/control#245), in place of over the lanes.
- The disk floor (ADR-0082) counts four builds of the queue's own worktrees, the places it had by default, beside the worktrees a worker was started in.

The rule lives in the gate crate (ADR-0059), which guildd also embeds (Guild's `[exclusive]` table in `host.toml`). Its `Host` says the reading (`reading`), the busy share (`share`) and what a kind of command takes (`takes`) in place of `lanes`; a host that says no reading runs one command at a time, as never starved. Guild's queue follows the same rule from the same code once it says its reading.

## Seen

`afkconvoy queue`, doctor's `host.exclusive` and the panel show `room` — the reading (`busy`, `by` ticks or load, its span, and memory under its floor), the share, what the reading does not include yet (`counted`) and whether a command already started on it (`spent`) — in place of capacity, used and free. A command waiting on the room gives `host_busy`, naming how busy the cores count and the share; `next_reading` where one already started on this reading; `host_memory` where memory is under the floor. `worker_capacity` and `fast_capacity` are gone.

## Retiring the counts

`run --gate-lanes`, `--fast-lanes` and `--slow-gate-lanes` are refused, naming `--busy-share`. `AFKCONVOY_GATE_LANES`, `AFKCONVOY_FAST_LANES` and `AFKCONVOY_SLOW_GATE_LANES` are said to be ignored on stderr, and a `run` goes on: a fleet whose environment still sets one keeps working. Host settings that still name a count read as before, and the next write drops it.

## Considered options

- **Raise the default counts.** Refused: a count right for an idle 18-core host overfills one loaded by another account, and no count follows the load as it changes.
- **Start by the load average alone.** Refused: macOS counts in its load average threads that want no core (ADR-0084); the ticks say what the cores did.
- **Start every command whose own cost fits under the share.** Not taken: the issue's rule is a start while the cores are under the share; one reading per start and the counted lag keep the next reading honest.

## Consequences

- On an idle host, landing turns of different repos, fast gates and slow gates run side by side, as many as its cores carry; on a busy one, one at a time.
- A host runner older than this one schedules by its counts until it is replaced; a newer `queue` reading an older runner's answer shows no reading.
- A host that set its counts to fill its cores now fills them by the reading; one that set a count to keep cores free for another system says `--busy-share` lower, or `--max-share-of-cores` (ADR-0075).
