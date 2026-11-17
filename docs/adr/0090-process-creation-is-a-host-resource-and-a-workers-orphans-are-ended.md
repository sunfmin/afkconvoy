---
status: accepted
---

# Process creation is a host resource, and a worker's orphans are ended

Amends ADR-0019 and ADR-0074 (sunfmin/control#1319).

On a host of 18 cores, 1,518 processes a second were created and 39% of the CPU was in the kernel, 0.1% idle. The processes `top` could see added up to about 3 cores; the rest went to short-lived processes and to the daemons each exec wakes (syspolicyd, trustd, XprotectService, tccd, notifyd, launchd). The exclusive queue read the host as busy and every gate waited. About half of it was five processes a guild test had left in a worker's worktree, handed to launchd, one a `while …; do sleep 0.01; done` loop. Nothing in the fleet saw them: a hot worker was judged by CPU, memory, disk written and stalls, and these used little of each.

## Decision

- **The host's rate is read.** The kernel hands out process ids one after another; the id of a process started now is the counter, and two readings' difference is the processes created between them (`afkconvoy_host_reading::spawn`). `doctor` (`host.admission.reading.process_creation_per_second`) and the panel's admission line say it beside the CPU reading.
- **A worker's rate is its share of it.** A process that lives ten milliseconds is on `ps`'s list of one sample in a few, so the sampler counts what created it: each sample, the host's creation, and the share of the processes created since the sample before and still here that are the worker's (its tree, and the orphans of its commands). Read over the last two minutes' samples, the share times the host's creation is the worker's processes a second.
- **Past a bound, held two minutes, a worker is hot.** The bound is the host setting `--hot-spawns` (default: 25 a second for each core, 200 at the least). Its issue is told the rate and the commands that made the processes, as for a stall.
- **An orphan is ended.** A process working in a worker's worktree that was handed to pid 1, outside an `afkconvoy exclusive` command and not a server shared by the host (one that works outside the worktree, ADR-0072), is named once it has been so for `ORPHAN_AFTER` (five minutes), on the worker's issue and in the tick log, and ended with its processes: SIGTERM and SIGCONT, then SIGKILL at the next sample. It is the stopped orphan of ADR-0074, whether or not it is stopped.

## Considered options

- **Count the processes `ps` lists new since the sample before.** Refused: what lives ten milliseconds is rarely on the list, and the loop that costs the most is the one it misses.
- **Add the rate to the exclusive queue's reading.** Refused: that reading is the gate crate's, which another program embeds, and the queue starts a command by the CPU alone; the rate is shown beside it and judges workers.
