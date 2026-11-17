---
status: accepted
---

# Admission alone decides each start

No number bounds how many workers a checkout's fleet instance holds at work at once: there is no concurrency to set, on the host or in the repo. A tick starts workers while admission admits, from what the host, its accounts and the repo's candidate chain can carry now. `run --concurrency` and a repo config's `concurrency` are still accepted and ignored, and `doctor` reports each with how to drop it.

## One reading per start

The reading a tick opens with admits its first start. Each start after one is admitted by a reading of its own, taken just before it, so that each start's load and memory are counted before the next, and the workers it started are counted on their account. The first reading that holds ends the tick's starts; what it held stays on the frontier for the next tick. Workers already running, and the worker a declared verdict starts, are never held back.

## The host's CPU

Admission reads two things of the host's CPU, and holds for load only while both are past their thresholds:

- **How busy its cores are**: the share of all their time not idle, from the kernel's CPU ticks (`host_statistics`' `HOST_CPU_LOAD_INFO` on macOS, `/proc/stat` elsewhere) between two readings — over the time since the process's last reading, up to a minute, or two readings taken for the purpose. Held only past `CPU_BUSY`.
- **The one-minute load average**, less the exclusive queue's share of it, past the host's share of its cores (`--admit-load`). The queue bounds its own heavy commands, and their load must not keep admission from starting the workers whose heavy work would itself wait for a lane. The kernel gives no load average per process tree, so the host runner keeps the queue's share of the host's CPU — the process trees of running queue tickets, and while one runs the CPU of the scanners a gate's tests wake (`fseventsd`, `XProtectService`, `syspolicyd`, `opendirectoryd`) — decayed over one minute as the kernel decays the load average, so a gate's lagging load is still the queue's between its phases. Where no host runner keeps one, admission apportions by one reading of the process table; where the queue or the process table cannot be read, it subtracts nothing. Load from outside the queue always counts, other accounts' queues among it.

Load that mostly idle cores carry is waiting on something other than a core — macOS counts in its load average threads that want none — and another worker does not make it wait longer. Busy cores under a load within its share hold nothing either: the queue's own gates may take every core. Where the ticks cannot be read, the load alone holds.

## Its other readings

- **Memory**: below the host's share of its memory free.
- **Disk**: under the disk floor (ADR-0082).
- **afkconvoy's own share.** A host may say the share of its cores and of its memory afkconvoy may use itself (`--max-share-of-cores`, `--max-share-of-memory`, each a whole percent; none by default). A Hub host runs another system beside the fleet, and the shares above are of the whole host. afkconvoy's own is its host runner and keeper and everything under them, every process its fleets started, and the running work of its exclusive queue, a cut's build among it. Admission starts no worker while they take as many cores as the cap allows, or hold as much memory: a start adds to them. Every command of the exclusive queue is given the cores the cap allows, at least one, as `CARGO_BUILD_JOBS` and `NEXTEST_TEST_THREADS` unless its environment already says them; a host that caps nothing gives a build every core, for a build held to fewer is only slower. Where a repo's builds need more than the cap leaves, its fleet moves to a host with room (`afkconvoy run --help`, MOVING A FLEET TO ANOTHER HOST).
- **Usage windows.** The accounts a checkout's next worker could start on, as the newest records of its workers say their windows (GLOSSARY: Usage window; ADR-0086). Admission starts no worker while a window, at the rate it was used at since it began, would run out before it resets, with each running worker taken to use its share of that rate and the new one as much again. A harness that reports no window is judged by the host's readings alone.
- **The exclusive queue.** Admission starts no worker while the host's exclusive queue would take longer than `QUEUE_BOUND` to drain: each lane's commands one after another, as many at once as the lane has places, by the median of each command's last five runs, which the queue keeps. More workers would only wait behind the gate.
- **The repo's candidate chain.** More workers on one repo than its candidate chain can land make each landing slower, not faster: each conflict is a merge again and a gate again. Admission reads the `land` turns this host's tick log has of the repo in the last hour, by their `outcome`: `conflict` and `speculative_conflict` are conflicts, `merged` a landing. While the conflicts outnumber the landings, and are three or more, admission starts no new worker of that repo; one or two are the ordinary cost of parallel work. It holds until the chain drains, and `doctor` reports the repo as a warning. A chain's turns on another host are that host's to count.

A reading that cannot be taken holds nothing back.

## Apart from what is in flight

A tick starts the frontier's issues whose named files overlap none the claimed issues name before those that overlap, each side lowest number first, after what was begun. A file is named in backticks, as the issues here name them; an issue that names none overlaps nothing. A PR's diff is not read: that is a call per PR at every tick, and the issue a PR closes names its files already.

## What is said

`plan`, `doctor` and the tick's line say which reading held a start back, with its numbers — for load, both figures it used, or that how busy the cores are could not be read; `doctor` reports total load, the queue's estimated share and the remainder. Held back for one reason at every tick past the host's span is a finding of the host for the host's watch (ADR-0040), given admission's reading and what holds it: for disk, every worktree of the host's fleets with its issue, its stage and whether a worker holds it, and the largest directories of the volume outside them; for load or memory, the process tree of each worker and turn, and the hottest processes outside them. It is once per episode, which ends once admission has admitted for five minutes. The candidate chain's hold calls for no watch: nothing of the host is to mend, and the chain drains on its own.

## Considered

- **A concurrency per checkout.** Any number is wrong as the work, the host and the other fleets on it change; the readings say what the host can carry now.
- **Holding on the load average alone.** macOS's load average counts idle processes of other accounts, and held new workers back with most cores idle.
