---
status: accepted
---

# A hot worker is inspected, and a heavy command queues

The workers of a host build and test as they work, and several at once — of one fleet, and of every other fleet on the host (ADR-0003) — can take so much CPU, memory or disk that the host slows for everyone, the human at its keyboard among them. Most of the time a worker that takes that much has written something wrong: a test that never ends, a leak, a fixture copied a thousand times, a temporary directory made and never removed. Sometimes the work really is that heavy. The first wants the worker told why; the second wants the workers of the host not to run it at the same time.

So the fleet watches what each worker's processes take, has its stage's consult look into a worker that takes too much, and gives the work that really is heavy one command that runs it alone on the host.

## What is watched

A **hot worker** is one whose processes — the process tree under its terminal — take more than their share of the host:

- **CPU**: more than N cores, held for two minutes.
- **Memory**: more than X GB resident, or the host under memory pressure while it holds the most.
- **Disk written**: more than Y GB written in ten minutes.
- **Disk held**: what git ignores in its worktree more than `OUTGROWN_TIMES` its fresh build and more than `OUTGROWN_FLOOR`: the train worktree's rule (`train::outgrown`), applied to a worker's worktree.

The thresholds are shares of the host — of its cores, its memory, its free disk — and the host can say others. CPU, memory and bytes written are sampled every ten seconds by `run`, from each process's own usage (no root needed); what the worktree holds is measured once a tick, for measuring it is slow.

A worker is started with its own `TMPDIR`, a directory git ignores in its worktree, so what it writes there is counted as its own and goes when its worktree does. What it writes elsewhere — a shared cache, a download — cannot be told apart, and is not counted.

The fleet's own gate runs on the landing train are no worker's and are not counted to one.

## A hot worker is inspected

When a worker turns hot, the fleet takes down what shows it, there and then — the process tree with each command line, a five-second `sample` of the hottest process, the resident memory over the last minutes, the processes that wrote the most, the largest directories of its worktree — for the process may be gone before anyone looks. It then starts an **inspection**: a worker in the role of the stage's consult — the stock consult where the stage names none, and in an unbound repo — given what was taken down, the issue, the worktree and the hot worker's terminal, and asked what is taking so much and why.

It finds one of three things:

- **The work is wrong.** It types at the hot worker what is wrong and where, and asks it to fix that before going on.
- **The work is that heavy.** It types at the hot worker that it is, and that the heavy command goes through `afkconvoy exclusive` from now on.
- **It is not the worker's.** A daemon outside its tree, another fleet's gate. It types nothing at the worker.

Whatever it finds, it leaves a comment on the issue saying what and why, so later stages and a human see it. Like any consult it commits, pushes and changes the host never; unlike one it hands nothing on — the issue stays at its stage and the hot worker keeps working.

### Bounds

- **Once per episode.** A worker is inspected once each time it turns hot. It stops being hot once its CPU and memory have stayed under the threshold for five minutes; held disk stops being hot only once the worktree is back under the rule, for files do not go when their process ends.
- **One inspection on the host at a time**, across its fleets, found as host sleep finds them (ADR-0016). Another hot worker waits for it.
- **It takes no slot** of the `concurrency`: what it runs is a model elsewhere and a `sample`. It has a time limit, and what it found by then is what is typed.
- **A worker that does not listen is not a failure.** Hot again after its inspection, it is inspected again; it is never escalated for it. What holds the host back from it is the queue and admission below.

## A heavy command runs alone

`afkconvoy exclusive -- <command>` runs a command once no other `exclusive` command runs on the host, across all its fleets, and queues it until then. A worker uses it for work it knows to be heavy — told so by an inspection, or by its role's rules. A worker waiting in the queue is busy, not silent; its wait counts toward the gate limit.

## Admission, and priority

- **Admission.** A tick starts no new worker while the host is past a share of its load, under memory pressure, or low on free disk. The issue stays on the frontier for the next tick. Workers already running are left alone.
- **Priority.** Every worker is started under `nice`. It runs as fast as before on a host with nothing else to do, and gives way to the human and to the fleet's own gate when the host is busy.

## Considered

- **A nudge-like line from the fleet**, typed at the hot worker with the numbers. Cheap, but the worker is in the middle of its work, its context full, and inclined to think its own code fine. A fresh worker with the evidence and the means to profile tells the three cases apart.
- **Stopping other workers** (`SIGSTOP`) while a heavy one runs. A test stopped times out and fails for no fault of its own, cargo's build lock stays held, and a harness may count the worker stopped. Queuing what has not started is safe; pausing what has is not.
- **Background QoS** (`taskpolicy -b`). On Apple Silicon it keeps a process to the efficiency cores even on an idle host: every worker slower all the time. `nice` gives way only when something else wants the CPU.
- **A cap on build jobs** (`CARGO_BUILD_JOBS`) per worker. It slows a worker that is alone on the host as much as one that is not.
- **Once per worker, as the nudge.** A nudge is for a worker that stopped: one that did not wake to one line will not wake to ten (ADR-0005). A hot worker is at work, and may make one mistake and then another.
- **A new kind of role for inspections.** The consult already stands for the team's senior judgement, read-only, on the stage's harness; a second setting would say the same thing twice.

## Consequences

- `run` keeps a sampler beside its ticks, and the host's fleets answer, through their sockets, which of them runs an inspection and which an `exclusive` command.
- An inspection's comment is no stage record: the stage an issue is at is not derived from it (ADR-0010).
- `doctor` and the panel show the hot workers, the inspection running and the queue of `exclusive`.
- The tick log has a line as a worker turns hot, as its inspection starts and what it found, and as an `exclusive` command waits and runs.

## Open

- The defaults for N, X and Y, and for the shares that hold admission back.

Tracked as #308–#313.
