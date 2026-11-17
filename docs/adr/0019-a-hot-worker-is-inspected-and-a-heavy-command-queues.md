---
status: accepted
---

# A hot worker is inspected, a heavy command queues, and a worker's orphans are ended

The workers of a host build and test as they work, and several at once — of one fleet, and of every other fleet on the host (ADR-0003) — can take so much of the host that it slows for everyone, the human at its keyboard among them. Most of the time a worker that takes that much has written something wrong: a test that never ends, a leak, a fixture copied a thousand times, a loop that starts a process every few milliseconds. Sometimes the work really is that heavy. The first wants the worker told why; the second wants the workers of the host not to run it at the same time.

So the fleet watches what each worker's processes take, has the host's watch look into a worker that takes too much (ADR-0040), gives the work that really is heavy one command that queues it on the host, and ends what a worker's commands leave behind.

## What is a worker's

A worker is started with its own `TMPDIR`, a short link to a directory git ignores in its worktree (`worker_tmp`): what it writes there is its own and goes when its worktree does, and its name is the key of the worktree's path, so the sampler finds a worker by it. A worker's processes are those started with its `TMPDIR`, and every one under them. Before the fleet removes a worktree it turns the link to the fleet's own temporary directory, so a host server started with it keeps working; a link no worktree holds is removed once no process started with it runs.

A worker may start a server shared by the host, and a process may outlive what started it; both are then handed to pid 1, still with the worker's `TMPDIR`. They are told apart by where they work. A process handed to pid 1 that works outside the worktree, as a daemon does, is a **server shared by the host**: it is no worker's, and nothing under it is counted to one through it — though a compile sccache's server runs for a worker, started with that worker's `TMPDIR`, is that worker's. A process handed to pid 1 that works in the worktree is the worker's, left behind. The same test frees a queue lease only such a server holds (ADR-0106).

The gate runs of the fleet's own turns are no worker's and are not counted to one.

## What makes a worker hot

A **hot worker** is one whose processes take more than their share of the host, by any one of:

- **CPU**: more than a share of the host's cores, held for two minutes.
- **Memory**: more resident than a share of the host's memory. What runs under its `exclusive` commands is left out — the queue bounds that already — until the host's memory pressure (`kern.memorystatus_vm_pressure_level`) has held at level 2 or above, sample after sample, as long as CPU must hold; then everything counts. One high reading between low ones is a cold build filling the host for a moment, and a watch could do nothing about it. Where the pressure cannot be read, as on Linux, it counts as low; where the queue cannot be read, nothing is left out.
- **Disk written**: more than a share of the disk free over its floor (ADR-0082), within ten minutes, so it is hot before the disk reaches the floor, not after it is full.
- **Disk held**: what git ignores in its worktree has outgrown its fresh build (`landing::outgrown`, the landing worktree's rule).
- **Process creation**: more processes a second than the host setting `--hot-spawns`, held for two minutes. Process creation is paid in the kernel and in the daemons each exec wakes, outside any worker's tree, so a worker can make a host busy while its own CPU, memory and disk stay small. A process that lives ten milliseconds is rarely on `ps`'s list, so the worker's rate is not counted from the list: the kernel hands out process ids one after another, two readings' difference is the host's creation (`afkconvoy_host_reading::spawn`), and a worker's rate is its share of the new processes still here (its tree and its commands' orphans) times that.
- **Stalled**: a process of it has taken no CPU and had no children for `STALL_HELD`, holding build-directory locks open (`lsof`) whose every holder is idle too. Nothing will release them: a deadlock takes nothing, so no other criterion sees it. A build waiting on one that compiles waits on a holder at work, and never stalls. The serial build of ADR-0082 prevents the cargo deadlock; this finds one wherever the opt-out does not reach.

The limits are shares of the host and the host can say others; their values live in `crates/core/src/hot.rs`. CPU, memory, bytes written and process creation are sampled every ten seconds by `run`, from each process's own usage (no root needed); what the worktree holds is measured once a tick, for measuring it is slow. What a worker writes outside its worktree — a shared cache, a download — cannot be told apart and is not counted. `doctor` and the panel show the host's process creation beside its CPU reading.

## A hot worker is a finding

When a worker turns hot, the fleet takes down what shows it, there and then — the process tree with each command line, a five-second `sample` of the hottest process, the resident memory over the last minutes and the pressure readings that counted its exclusive memory, the processes that wrote or created the most, the largest directories of its worktree, and for a stall the waiting processes with each lock's holders — for the process may be gone before anyone looks. For a stall or process creation the worker's issue is told too. What was taken down, the issue, the worktree and the worker's terminal are a finding of the look, which calls for the host's watch (ADR-0040).

- **Once per episode.** A worker is one finding each time it turns hot. It stops being hot once it has stayed under every limit for five minutes; held disk only once the worktree is back under the rule, for files do not go when their process ends.
- **A worker that does not listen is not a failure.** Hot again after the watch looked into it, it is a finding again; it is never escalated for it. What holds the host back from it is the queue and admission (ADR-0107).

## A heavy command queues

`afkconvoy exclusive -- <command>` queues a command in the host's exclusive queue, across all its fleets, and runs it once the host has room for it (ADR-0088), in its lane (ADR-0047). A worker uses it for work it knows to be heavy — told so by the watch, or by its role's rules. A worker waiting in the queue is busy, not silent.

## Priority

Every worker is started under `nice`. It runs as fast as before on a host with nothing else to do, and gives way to the human and to the fleet's own gates when the host is busy.

## A worker's orphans are ended

An **orphan** is a process working in a worker's worktree that was handed to pid 1, outside an `exclusive` command and not a server shared by the host. Once it has been so for `ORPHAN_AFTER` (five minutes), it is named on the worker's issue and in the tick log, and ended with every process under it: SIGTERM and SIGCONT, then SIGKILL at the next sample. A test that left a `sleep` loop behind costs the host until something ends it, and the worker that started it will not.

A **stopped orphan** is ended sooner, by settling. afkconvoy freezes a gate with SIGSTOP before it kills it, so no process of its tree can fork or be reaped while the tree is found (`exclusive` at its bound, a candidate's cancellation, ADR-0037). When the process doing that dies between the stop and the kill, what it stopped stays stopped for good, and a SIGTERM to it stays pending. Settling waits on any process still in a worktree (ADR-0046), so before it looks, it ends each stopped orphan there and every process under it — SIGTERM, SIGCONT, SIGKILL a second later — and logs `ended stopped orphan pid …`. One still there after five seconds is waited on as any other. A running orphan still makes settling wait: it may be finishing a write. A process a human stopped in a worker's worktree whose terminal then went is ended too: settling is removing that worktree.

The tests do their part: each test world, as it ends, resumes and kills every process still working under its directory, and the gate fails a run whose tests leave a process in `target/` or in the tests' `TMPDIR`.

## Considered

- **A nudge-like line from the fleet**, typed at the hot worker with the numbers. Cheap, but the worker is in the middle of its work, its context full, and inclined to think its own code fine. A fresh look with the evidence and the means to profile tells apart work that is wrong, work that is that heavy, and load that is not the worker's.
- **Stopping other workers** (`SIGSTOP`) while a heavy one runs. A test stopped times out and fails for no fault of its own, cargo's build lock stays held, and a harness may count the worker stopped. Queuing what has not started is safe; pausing what has is not.
- **Background QoS** (`taskpolicy -b`). On Apple Silicon it keeps a process to the efficiency cores even on an idle host: every worker slower all the time. `nice` gives way only when something else wants the CPU.
- **A cap on build jobs** (`CARGO_BUILD_JOBS`) per worker. It slows a worker that is alone on the host as much as one that is not.
- **Once per worker, as the nudge.** A nudge is for a worker that stopped: one that did not wake to one line will not wake to ten (ADR-0005). A hot worker is at work, and may make one mistake and then another.
- **Counting the processes `ps` lists new since the sample before.** What lives ten milliseconds is rarely on the list, and the loop that costs the most is the one it misses.
- **Adding the creation rate to the exclusive queue's reading.** That reading is the gate crate's, which another program embeds (ADR-0058), and the queue starts a command by CPU alone; the rate is shown beside it and judges workers.
- **Telling a server by its terminal, session or name.** A test Claude Code ran has neither terminal nor session either, macOS's `ps` says no session, and sccache's server is one of many started the same way. Where a process works says the same of each, and no list has to be kept.
- **Ending every orphan at once, or only through the pauser's exit.** A running orphan may still be finishing a write, so it gets its five minutes; and a pauser killed with SIGKILL runs nothing on its way out.

## Consequences

- `run` keeps a sampler beside its ticks.
- `doctor` and the panel show the hot workers and the queue of `exclusive`.
- The tick log has a line as a worker turns hot, as an orphan is ended, and as an `exclusive` command waits and runs.
