---
status: accepted, amended by ADR-0035, ADR-0072, ADR-0084 and ADR-0088
---

# A hot worker is inspected, and a heavy command queues

The workers of a host build and test as they work, and several at once — of one fleet, and of every other fleet on the host (ADR-0003) — can take so much CPU, memory or disk that the host slows for everyone, the human at its keyboard among them. Most of the time a worker that takes that much has written something wrong: a test that never ends, a leak, a fixture copied a thousand times, a temporary directory made and never removed. Sometimes the work really is that heavy. The first wants the worker told why; the second wants the workers of the host not to run it at the same time.

So the fleet watches what each worker's processes take, has the host's watch look into a worker that takes too much (ADR-0035), and gives the work that really is heavy one command that queues it on the host.

## What is watched

A **hot worker** is one whose processes — the process tree under its terminal — take more than their share of the host:

- **CPU**: more than N cores, held for two minutes.
- **Memory**: more than X GB resident.
- **Disk written**: more than Y GB written in ten minutes.
- **Disk held**: what git ignores in its worktree more than `OUTGROWN_TIMES` its fresh build and more than `OUTGROWN_FLOOR`: the landing worktree's rule (`landing::outgrown`), applied to a worker's worktree.

The thresholds are shares of the host — of its cores, its memory, its free disk — and the host can say others. CPU, memory and bytes written are sampled every ten seconds by `run`, from each process's own usage (no root needed); what the worktree holds is measured once a tick, for measuring it is slow.

A worker is started with its own `TMPDIR`, so what it writes there is its own and goes when its worktree does. What it writes elsewhere — a shared cache, a download — cannot be told apart, and is not counted.

### A worker's `TMPDIR`

Its `TMPDIR` is a link, `/tmp/afkconvoy-<uid>/<16 hex digits>`, to `.afkconvoy-tmp` in its worktree, a directory git ignores. A worktree is named for its issue's title, so its path is long, and macOS holds a socket's path in 104 bytes: a `TMPDIR` in the worktree left no room for a tool that binds a socket under it. sccache is one: the first build after its server went idle starts one, and waits for it on a socket under its `TMPDIR` (#439). The link is no longer than macOS's own temporary directory for a user, and what is written through it lands in the worktree. Its name is the key of the worktree's whole path, so it is unique on the host, across its fleets, and the same after `run` starts again: the sampler finds a worker by it whenever it samples. One place says it (`worker_tmp`), and every other reads it there.

A process started with it can outlive the worktree. A server shared by the host that the worker happened to start keeps the `TMPDIR` it was started with, and sccache's makes a directory under it for every compile, whoever asks: with that gone, every build of the host fails until the server goes idle, which a busy host never lets it. So a worktree removed by the fleet has its link turned first, in one step, to the fleet's own temporary directory, and a link no worktree holds — turned, or pointing at nothing once its worktree went some other way — is removed once no process started with it runs; one a process still runs with is turned where it points at nothing, and kept. Each start of a worker, and each removal, reclaims them.

### A server a worker started is the host's

A worker's processes are those started with its `TMPDIR`, and every one under them. But a worker may start a server shared by the host, and a process may outlive what started it, and both are then handed to pid 1, started with the worker's `TMPDIR`. They are told apart by where they work. A process handed to pid 1 that works outside the worktree, as a daemon does, is a server shared by the host: it is no worker's, and nothing under it is counted to one through it. sccache's server works in `/`, and each compile it runs for a worker is started with that worker's `TMPDIR`, so that compile is counted to that worker. A process handed to pid 1 that works in the worktree is the worker's, left behind: a test its runner left, still running, is counted to its worker as it was before.

The gate runs of the fleet's own turns are no worker's and are not counted to one.

## A hot worker is a finding

When a worker turns hot, the fleet takes down what shows it, there and then — the process tree with each command line, a five-second `sample` of the hottest process, the resident memory over the last minutes, the processes that wrote the most, the largest directories of its worktree — for the process may be gone before anyone looks. What was taken down, the issue, the worktree and the hot worker's terminal are a finding of the look, which calls for the host's watch (ADR-0035).

### Bounds

- **Once per episode.** A worker is one finding each time it turns hot. It stops being hot once its CPU and memory have stayed under the threshold for five minutes; held disk stops being hot only once the worktree is back under the rule, for files do not go when their process ends.
- **A worker that does not listen is not a failure.** Hot again after the watch looked into it, it is a finding again; it is never escalated for it. What holds the host back from it is the queue and admission below.

## A heavy command queues

`afkconvoy exclusive -- <command>` queues a command in the host's exclusive queue, across all its fleets, and runs it once the host has room for it (ADR-0088). A worker uses it for work it knows to be heavy — told so by the watch, or by its role's rules. A worker waiting in the queue is busy, not silent; its wait counts toward the gate limit.

### Cuts go first among the waiting

A cut's commands and exclusive commands run ahead of waiting worker commands (ADR-0028, sunfmin/control#106), so an arbitrary queue of worker gates cannot consume a busy cut's overall time limit. Cuts keep request order among themselves, as ordinary commands keep theirs. Its cut worktree determines its identity and its ticket preserves it across host runner replacement. Priority never interrupts a running command, never adds a lane, and never bypasses the one-turn-per-repo rule. ADR-0031's contention bound still applies. The gate's own limit starts after the turn is granted.

This amends ADR-0034's FIFO rule for cuts. Doctor and the panel show the same order the queue uses.

## Admission, and priority

- **Admission.** A tick starts no new worker while the host is past a share of its load with its cores more than 90% busy (ADR-0084), below its share of memory free, or low on free disk; each start is admitted by a reading of its own (ADR-0072). The issue stays on the frontier for the next tick. Workers already running are left alone.
- **Priority.** Every worker is started under `nice`. It runs as fast as before on a host with nothing else to do, and gives way to the human and to the fleet's own gate when the host is busy.

## Admission holding back calls for the host's watch

Admission only holds back: tick after tick, nothing looks into why. And what holds the host may be no one worker's — the sum of several worktrees, none outgrowing its fresh build, or worktrees no worker holds any more, which nothing samples (#324). So when admission has held back for one reason — load, memory or disk — at every tick past a span (ten minutes, or the host's own), it is a finding of the **host** for the host's watch (ADR-0035), given admission's reading and what holds it — for disk, every worktree of the host's fleets with its issue, its stage and whether a worker holds it, the caches and the largest directories of the whole volume under the home folder outside them (`du -x`, within five minutes); for load or memory, the process tree of each worker and of each turn, and the hottest processes outside them.

It is once per episode; the episode ends once admission has admitted for five minutes.

## Considered

- **A nudge-like line from the fleet**, typed at the hot worker with the numbers. Cheap, but the worker is in the middle of its work, its context full, and inclined to think its own code fine. A fresh look with the evidence and the means to profile tells apart work that is wrong, work that is that heavy, and load that is not the worker's.
- **Stopping other workers** (`SIGSTOP`) while a heavy one runs. A test stopped times out and fails for no fault of its own, cargo's build lock stays held, and a harness may count the worker stopped. Queuing what has not started is safe; pausing what has is not.
- **Background QoS** (`taskpolicy -b`). On Apple Silicon it keeps a process to the efficiency cores even on an idle host: every worker slower all the time. `nice` gives way only when something else wants the CPU.
- **A cap on build jobs** (`CARGO_BUILD_JOBS`) per worker. It slows a worker that is alone on the host as much as one that is not.
- **Once per worker, as the nudge.** A nudge is for a worker that stopped: one that did not wake to one line will not wake to ten (ADR-0005). A hot worker is at work, and may make one mistake and then another.
- **A short directory of its own, outside the worktree, for a worker's `TMPDIR`.** What a worker leaves there would not show in its worktree, where the host's watch measures it and reclaims it.
- **Telling a server by its terminal or its session.** A daemon leaves its terminal and starts a session of its own. But Claude Code runs each command a worker gives it the same way, so a test it left has neither, and macOS's `ps` says no session.
- **Telling a server by its name.** sccache's is one of many: Gradle's, Bazel's and others' are started the same way. Where it works says the same of each, and no list has to be kept.

## Consequences

- `run` keeps a sampler beside its ticks.
- `doctor` and the panel show the hot workers and the queue of `exclusive`.
- The tick log has a line as a worker turns hot, and as an `exclusive` command waits and runs.
- A worker's `TMPDIR` is short enough for a socket's path, and a server a worker started keeps its `TMPDIR` until it ends.
- A server a worker started is taken for the host's, and so is a process the worker left that works outside its worktree.

Tracked as #308–#313.

## Amended by ADR-0034 and ADR-0037

A turn holds one command from its merge through its gate to its landing on the base branch, one turn of a repo at a time.

## Amended by ADR-0049

A worker's fast gate, run alone by `afkconvoy gate --fast`, queues in a lane of its own, the fast lane: it is held apart from no slow gate, turn or other heavy command. Every other `exclusive` command queues as above.
