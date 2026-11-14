# Gate duration baseline

The fixed warning budget is declared in `docs/agents/afkconvoy.md`; the gate
limit there bounds termination separately. Neither is learned from recent runs.
The warning uses the existing last-five-recorded-landings rule on one host,
and opens no second slow-gate issue while one is open. The baseline's machine
class is a 14-core, 48-GiB Apple Silicon Mac, not a promise for every host.

## Review observation

sunfmin/control#150 supplied this observation on
`217aaff78747b9eb714043e2b456ccba9c6bdff5`, 2026-10-27
09:38:40–09:44:59 Asia/Shanghai, through the host's exclusive queue.
Load at start was 41.33 / 46.04 / 40.90. Cache state and queue duration were
not established in that review. This is a loaded observation, not a quiet or
controlled cold-versus-warm benchmark.

| Step | Wall seconds |
| --- | ---: |
| fmt | 1 |
| Flux | 29 |
| Clippy | 15 |
| manual | 13 |
| test build | 9 |
| test execution, including setup and cleanup | 312 |
| Total command, finer timer | 378.67 |

The full gate passed 1,602 tests and skipped 14 under the existing configuration.
The whole-second step timer and the finer command timer have different resolution.

## Current-revision observation

Frozen source: `456b24d1683252d0386a3b79c1ec98cb2c342ea7` (workspace version
0.5.26), taken during this implementation on 2026-10-27. The baseline was a
separate detached worktree inside the worker's ignored temporary directory, so
edits to the implementation could not alter the source being measured.

- Host: `FelixMBP14.local`, `Mac16,8`, 14 logical cores, 51,539,607,552 bytes
  (48 GiB) RAM, Apple Silicon macOS.
- Requested: 02:49:45 UTC; granted: 02:57:24 UTC. Queue wait: **459 seconds**,
  excluded from all step times and the **441.47-second** command wall time.
- Starting load: **57.05 / 44.35 / 35.86**. This is a loaded observation through
  the existing one-place host queue, not a quiet-machine benchmark. The host
  lane settings were not changed.
- Cache conditions: no `target/` in the fresh worktree; existing dependency
  cache and shared sccache retained. The host's Cargo config selected sccache
  and six build jobs. Startup sccache counters were cumulative host counters,
  not hits attributable to this gate. No cold compiler-cache claim is made.
- Complete gate: **1,622 passed, 15 skipped**, exit 0. The test-phase timer
  includes initialization and cleanup, not just assertions. The skips were
  already configured in the frozen source.

| Step | Wall seconds |
| --- | ---: |
| fmt | 1 |
| Flux | 32 |
| Clippy | 15 |
| manual | 15 |
| test build | 10 |
| test execution, including setup and cleanup | 368 |
| Total command, finer timer | 441.47 |

Nextest measured 367.466 seconds for its test run. These are the complete slow
records it printed, including binary, full name and individual elapsed time:

```text
SLOW [  20.601s] afkconvoy::cli switch::capacity_continuation_never_starts_a_second_writer_before_termination_is_confirmed
SLOW [  24.090s] afkconvoy::cli switch::codex_task_refusal_never_starts_a_second_writer_while_termination_is_unconfirmed
```

Those times overlap other tests; their sum is not the test-phase wall time.
Flux checked all 2,503 core functions with zero trusted or ignored. Its slowest
functions were `stage_record::history` 2.94s, `team::workflow_file` 1.44s,
`tick::hand_over` 649.91ms, `team::load` 599.00ms and `tick::end_claims` 561.80ms.
The [captured output](gate-baseline/2026-10-27-before.txt) preserves the raw
step, Flux and slow-test records. The updated candidate's complete landing
gate and comparison are retained in [PR 570](https://github.com/sunfmin/afkconvoy/pull/570).

The current source took 62.80 seconds more than the supplied review's older
source. Versions, test counts, cache conditions and load differ: this is not a
controlled estimate of a code change's cost. The ten-minute budget leaves
158.53 seconds (about **36%**) above this fresh-artifact, loaded observation,
as well as the larger margin above the earlier six-minute observation.

## Rationale and interpretation

Ten minutes gives about 36% over the current 441.47-second observation and
58% over the review's 378.67 seconds before a run counts as slow.
Five such recorded landings on one host must confirm the condition:
one cold compile or a temporary burst of load cannot alone open an issue.
The termination limit remains the declared hour. A warning can therefore
surface minute-scale slowing while a normal cold build or contended run still
finishes and retains its evidence; it never makes a green gate red. Sustained
host contention may also warn, deliberately, and must be investigated with the
step and host evidence rather than assumed to be a code regression. Slower
machine classes need a deliberate review of this declaration. No host's
records are combined with another's.

The gate still runs fmt, full-core Flux with no exemptions, Clippy, manual
verification, test compilation and the complete nextest suite. Nextest still
runs six tests at once, reports tests from 20 seconds, and ends a test at 240
seconds. No assertions, generated cases, input sizes or existing skips change.

## Reading the evidence

`gate.sh` ends with `gate: the seconds each step took:`. Its Flux output names
the slowest core functions. Nextest's final slow-test output keeps each binary,
full test name and seconds. Keep those lines together with the source commit,
host, start/end timestamps, load, whether a target directory existed and whether
sccache was active. A present target directory alone does not prove a warm
build. Queue waiting is measured from request to grant, separately from the
command's wall timer. Adding parallel test durations does not measure wall time.

`plan` and `doctor` retain the host, budget and each of the five commits with
its command duration. The tick's slow-gate step and log line name the same
threshold and evidence; the issue points back to the committed baseline and
gate output. No second performance-monitoring store is introduced.

## Scope and limits of the measurement

This work adds the missing budget guard, not a test-speed optimization. It
keeps every existing test, assertion, generated case count, input size and
validation step. Targeted regressions use controlled recorded durations and a
single real foreground tick, rather than waiting for the budget to expire.
They establish the normal and exact-budget cases, warning and persisted log
attribution, host separation, deduplication, invalid and duplicate settings,
and older-reader compatibility.

A quiet-machine run, a flushed compiler-cache run, individual fixture
setup/cleanup profiling and ranked build-unit profiling are not covered. The
existing gate's phase times suffice to distinguish Flux, Clippy, manual,
test build and the test run; its test timer includes fixtures and cleanup.
Fixture speed work belongs to the separate approved issues. No speedup,
cache-isolated comparison or arbitrary-host guarantee is claimed. The final
landing candidate may include upstream changes with different test counts;
its source commit, test counts and step times must accompany its duration.

## The fast gate and the slow gate (sunfmin/control#236)

The gate is split per test at 2 seconds (ADR-0047). Both measurements below
were made on `Offices-MacBook-Pro.local`, an 8-core, 16-GiB Apple Silicon Mac,
on 2026-10-28, with the host's fleets stopped by the operator, `target/`
present from earlier builds, and the run outside the exclusive queue.

### Per-test times, which drew the line

Source `d606bd3412c87c8d763ae30ab0b101ac7f901ed3`. The whole suite ran under
the gate's own conditions — nextest's default profile, six tests at a time,
the gate's deep `TMPDIR` — with `--message-format libtest-json-plus`, which
gives each test's own `exec_time`. Tests ran 13:30:29–13:52:29 UTC. Load
averages were 16.05 / 35.18 / 46.11 at the start, about 95 midway, from the
suite's own processes, and 30.96 / 59.16 / 62.68 at the end: these are times
under the load the suite puts on this host, as a gate's tests run.

| Time of one test | Tests |
| --- | ---: |
| under 1 s | 979 |
| 1–2 s | 79 |
| 2–3 s | 95 |
| 3–6 s | 210 |
| 6–20 s | 427 |
| 20 s or more | 69 |

1,859 tests ran, 16 of them ignored by existing configuration. The 801 at
2 seconds or more, 7,678 test-seconds together, are named in
`.config/slow-tests`: 796 of the CLI's tests binary, 3 of its library's and
2 of the core library's (the shell-quoting property tests). The 1,058 under
2 seconds took 196 test-seconds together. `cargo nextest list` then listed
1,042 tests for the fast gate's filter and 801 for the slow gate's: 1,843,
every test it lists, each in one.

### Both gates' step times

`./gate.sh`, both gates, on `bf1271c27c15d26eaf99d3d172d23cb444683f03`,
13:56:40–14:18:14 UTC; load averages 27.34 / 37.26 / 51.65 at the start and
26.83 / 49.99 / 52.75 at the end. Exit 0.

| Gate | Step | Wall seconds |
| --- | --- | ---: |
| fast | fmt | 2 |
| fast | Flux | 59 |
| fast | Clippy | 22 |
| fast | manual | 15 |
| fast | test build | 8 |
| fast | fast tests: 1,042 passed, 6 past 2 s, none ended | 31 |
| **fast** | **total** | **137** |
| slow | slow tests: 801 passed, 49 past 20 s | 1,157 |
| **slow** | **total** | **1,157** |
| both | command, `time -p` | 1,293.66 |

Flux checked all 2,784 core functions with none trusted or ignored, 42.72 s
by its own summary. The fast gate's six tests past 2 seconds took 2.2–3.1 s,
under its 6-second end: they are the near-line tests the threefold margin
leaves alone. The slowest slow test took 60.24 s
(`switch::ambiguous_normal_work_and_failed_judgments_never_authorize_a_switch`).

The fast gate's budget, 180 s, is these 137 s with about a third more for
load. The slow gate's, 420 s, is what remains of the whole gate's 600 s; the
slow gate already takes nearly three times that on this host, as the whole
gate's landings already exceed its budget there (sunfmin/control#234).

`afkconvoy gate --fast` then ran the fast gate alone on the same tree, warm:
green in 37 s (fmt 2, Flux 0 with nothing to recheck, Clippy 5, manual 3,
build 7, fast tests 21). With `walk::running_again_after_a_park_counts_as_no_rework`
(24 s) taken off the list, `./gate.sh fast` ended it as TIMEOUT at 6.007 s,
was red, and said to name it in `.config/slow-tests`.

## What the slow gate spends its time on (sunfmin/control#234)

Measured on `Offices-MacBook-Pro.local` (8 cores, 16 GiB) on 2026-10-29,
through the host's exclusive queue, with other fleets' workers running
beside it outside that queue: these are loaded observations, and their load
averages are given with them.

### Where a test's time goes

Every slow test is a test of the CLI, and the CLI spends a tick waiting on
the processes it starts. Sampling running ticks (`sample`) found them in
`poll` and `wait4` nearly always, not in afkconvoy's own code. A traced
`park::a_blocker_nothing_will_resolve_escalates`, whose eight cases each run
two ticks and a scan, started per case about 95 `git`, 34 `gh`, 19 `orca`
and 3 `ps`; `switch::ambiguous_normal_work_and_failed_judgments_never_authorize_a_switch`
started 816 `git` in all.

`git` on this host's PATH is `/usr/bin/git`, Apple's shim, which asks
`xcrun` for the developer directory every time it runs and then starts the
real git: 16 ms a call where the git it starts takes 7 ms
(`git check-ref-format`, 100 calls). A world now puts the directory of that
git (`xcrun --find git`) after its stand-ins on its PATH, and the harness's
own git calls use it too; where the git on PATH is not the shim nothing
changes. The same 31 slow tests (`park::` and `switch::a…`), run twice each
way, interleaved, six at a time:

| | Wall seconds | CPU seconds (user + sys) |
| --- | ---: | ---: |
| through the shim | 56.0, 57.5 | 183.7, 188.4 |
| the real git | 42.0, 39.5 | 125.9, 120.8 |

### Tests at a time

Six at a time, those 31 tests kept about three of the eight cores busy
(121 CPU seconds in 40). 79 slow tests (`park::`, `switch::`, `candidates::`),
with the real git:

| Tests at a time | Wall seconds | Load at the end |
| ---: | ---: | --- |
| 6 | 167.0, 138.2 | 79.63, 77.01 |
| 12 | 117.2, 115.0 | 114.63, 92.49 |
| 16 | red at 63.5 | 95.98 |

At sixteen, `candidates::a_gate_timeout_invalidates_and_terminates_its_running_successor`
waited out its 30 seconds for a candidate's transition. The slow gate runs
its tests under its own nextest profile, `slow`, twelve at a time; the fast
gate and the default profile keep six.

### The slow gate

`./gate.sh slow` on `c807c01278c5e2557f58b22edfb21af8598b6b8f` (the real
git), twelve at a time, 17:35:44–17:46:50 UTC; load averages 28.53 / 28.45 /
23.13 at the start and 116.00 / 112.66 / 77.37 at the end. Exit 0.

| Step | Wall seconds |
| --- | ---: |
| slow tests: 804 passed, 76 past 20 s | 666 |
| command, `time -p` | 665.41 |

That is 1,157 seconds before (above), on the same host. It is still past
the slow gate's budget of 420 seconds there: the time left is the processes
each test starts, which this change does not remove.

The same tree six at a time, started straight after at load 116, ended red
after 289 seconds on
`host_runner::stop_all_stops_every_fleet_instance_then_the_host_runner_and_keeps_the_list`,
which found the host runner's pid still alive just after `stop` answered;
it passed in the twelve-at-a-time run, and no git or thread count is in it.

## The manual step is gone (sunfmin/control#243)

From sunfmin/control#243 the gate has no `manual` step: `docs/manual.md`
and the example that wrote and checked it are deleted, and the help is the
manual. The 13 to 15 seconds the step took in the runs above are no longer
in the fast gate.

## What the gate spent its time on in its slow landings (sunfmin/control#263)

`Offices-MBP.lan` (8 cores, 16 GiB) recorded five landings past the 600 s
budget: 873, 678, 675, 636 and 601 seconds. Two of them left their whole
output, through the exclusive queue, on 2026-10-29:

| Landing | Fast gate | Slow gate | Recorded |
| --- | --- | ---: | ---: |
| `6eb61105` | fmt 2, Flux 41, Clippy 16, manual 19, build 32, fast tests 80: 190 s, beside the round's other candidate's fast gate (194 s) | 482 s, 815 passed, 36 past 20 s | 675 s |
| `28b8b0a6` | fmt 1, Flux 29, Clippy 15, build 32, fast tests 80: 157 s | 479 s, 815 passed, 34 past 20 s | 636 s |

The slow gate is three quarters of it. Its slowest test took 41 s
(`switch::ambiguous_normal_work_and_failed_judgments_never_authorize_a_switch`),
so no test holds its end back: it is the sum of its tests' times over
twelve. The fast gate's tests took 80 s there and 20–23 s at six at a time
in a quiet queue: the 60 s between is what ran beside the landing.

### Where a slow test's time goes

Every eighth of the slow tests (81, `.config/slow-tests` lines 3, 13, 23, …)
were run with each `git`, `gh`, `orca` and `ps` the CLI started counted
and timed by a wrapper first on PATH. They started 14,752 of them: git
9,674, gh 2,886, orca 1,861, ps 242, about 180 a test. Sampling a tick
found it in `poll` and `wait4` nearly always: the CLI's own work is
small, and a test's time is the processes it starts, one after another.
Of the CPU those took (each one's own user and system time, in a run
whose wrapper failed 12 host-runner tests: 69 tests), git took 123 of
180 s: `push` 36 s (702 calls, 51 ms each), `ls-remote` 31 s (1,552
calls, 20 ms each). Each push to the world's remote starts six processes
more (`sh`, `receive-pack`, `pack-objects`, `unpack-objects`, `rev-list`,
`for-each-ref`), each listing two. 270 of the 702 pushes are the startup
probe a world's first `tick`, `plan` or `run` makes, as on a host that
never probed.

The calls repeated, word for word, in one process and directory:
`check-ref-format refs/heads/main` 775 of 912 — a tick checks its base
branch's name for every issue it reads —, `remote get-url origin` 429 of
713 and `rev-parse --absolute-git-dir` 248 of 626. Only the first is a
fact of the call alone; the others come from a checkout looked up again
(`Repo::at`), whose origin a long-running host runner must not take as
fixed. From this change git is asked of a ref name once in a process
(`Repo::ref_name_ok`): the same 81 tests started 13,936 processes, 143
`check-ref-format` among them.

### Before and after

The same 81 tests, the build before the change and after it in turns,
through the exclusive queue, at load averages 45–55 from other work on the
host:

| | Nextest seconds | User + system CPU seconds |
| --- | ---: | ---: |
| before | 49.9, 49.5 | 252.5, 253.6 |
| after | 49.1, 49.3 | 235.6, 236.6 |

Seven percent less CPU, the same time: at that load the tests waited on
the processes' starts, not the CPU. Under a landing's own load the slow
gate fills the host (load 116 at its end, above), and there less CPU is
less time. What would take more off is starting fewer processes in a
tick — the probe of a fresh world, the remote listings a write must be
certain of (ADR 0043) — and is the fleet's behaviour, not the gate's.

## A waiting turn polled the remote beside the gate (sunfmin/control#309)

`Offices-MBP.lan` recorded five landings past the 600 s budget: 799, 731,
972, 1,847 and 780 seconds. No log of them was kept. The whole gate on their
newest, `80e4a145867eca6d9112980c089264dab6d8ab68`, run through the
exclusive queue on 2026-10-30 at 16:25 UTC, started at load 10.38 and was
green in 643 s: fmt 2, Flux 42, Clippy 19, build 8, fast tests 19, slow
tests 553. The slow gate is still six sevenths of it.

### Where a slow test's time goes now

Every eighth slow test (102, `.config/slow-tests` lines 3, 11, 19, …), traced
as in sunfmin/control#263, started 24,902 processes: git 15,068, gh 4,518,
orca 2,988, ps 513. The commands repeated word for word in one process
and directory were 5,000 of them. The largest:

| Command | Calls | Repeated in one process |
| --- | ---: | ---: |
| `ls-remote origin refs/convoy/candidates/main refs/heads/main` | 576 | 558 |
| `show -s --format=%B <sha>` (a candidate record) | 572 | 374 |
| `ls-remote origin refs/heads/issue-N` | 342 | 313 |
| `ps axeww -o pid=,ppid=,rss=,time=,command=` | 467 | 241 |

One `land` process asked for the candidate chain live 80 times. A turn
waiting on the chain — for its round, its predecessor, or another turn of its
PR — read it every 100 ms. Each read was two live listings of the chain,
one of each valid candidate's branch, and its record's message. A
candidate's gate asked whether it was cancelled at every 50 ms look at
the gate, and that ask is the same read. On a real remote each listing is a
connection to GitHub: 0.8–1.1 s and 0.14 CPU seconds here. So a turn running
its gate, and every turn waiting on it, kept connecting to GitHub for as
long as the gate ran, beside it.

From this change a waiting turn waits twice as long after each read that
found the chain unchanged, from 100 ms up to a second, and from 100 ms again
once it changes. A candidate's gate asks once a second. A record's message
is read once per sha in a process (`Repo::message`), as subjects already are.

### Before and after

The two builds, `80e4a145` and this change, in turns through the exclusive
queue, twelve at a time, on 2026-10-30 from 16:45 UTC, at load averages
45–94 from other work on the host:

| Tests | Build | Nextest seconds | User + system CPU seconds |
| --- | --- | ---: | ---: |
| the 102 above | before | 77.6, 77.3 | 413, 401 |
| | after | 75.0, 76.0 | 390, 389 |
| the 68 slow tests of `candidates`, `turns`, `lander`, `cutting`, `slow_gate` | before | 94.6, 94.7 | 577, 580 |
| | after | 86.6, 87.3 | 528, 531 |

Three percent off the sample, eight off the landing tests, in time and in
CPU alike. A landing's own gate gains more than its tests: there the reads
were connections to GitHub, made the whole time the gate ran. This was not
measured on a real landing.

What is left is the processes a tick starts, each repeated read a smaller
share than these: the repeated `ps` listing (151 ms each under the gate's
load, 30 ms on an idle host), an issue's comments read again in one tick
(478 of 726), `remote get-url origin` (601 of 1,040) and
`rev-parse --absolute-git-dir` (521 of 1,096).

## Processes a tick started for facts it already had (sunfmin/control#311)

`Offices-MBP.lan` recorded five landings past the 600 s budget: 753, 716,
947, 761 and 737 seconds. No log of them was kept. On `5e10346f` the fast
gate took 149 s through the exclusive queue at load 24 (fmt 1, Flux 43,
Clippy 4, build 11, fast tests 90) and the slow gate 534 s: the slow gate
is still three quarters of it. The fast tests took 19 s at load 4 straight
after: their 90 s were the work beside them.

### Where a slow test's time goes now

Every eighth slow test (103, `.config/slow-tests` lines 3, 11, 19, …),
traced as in sunfmin/control#263, started 18,584 processes in 106 s: git
11,076, gh 4,155, orca 2,892, ps 461. The run took 209 s of user CPU and
291 s of system CPU: starting processes, not the CLI's own work. With the
workspace's crates built at `opt-level` 1 the sample was 2% faster (70.2
and 69.4 s, against 71.3 and 70.9 s), and its cold build took 126 s: that
is not where the time goes. Each process's call stack, written by a hook
in `process::output`, named the callers:

| Command | Calls | Caller |
| --- | ---: | --- |
| `ps axeww -o pid=,ppid=,rss=,time=,command=` | 404, 42 s | half from `orca::finish_failed_removals`, at every tick |
| `rev-parse --absolute-git-dir` | 805 | `Worktree::note`, for each worktree a tick looks at |
| `sysctl -n hw.memsize` | 497 | `hot::memory`, 178 of them again in one process |
| `hash-object -t tree /dev/null` | 334 | `Repo::record_commit`, once per `Repo` value |
| `orca terminal list` | 656 | 140 of them `Worktree::start` asking again after a close that found none |

`ps axeww` writes every process's environment: 105 ms a call under the
suite's own load. `finish_failed_removals` listed them all before it looked
for a worktree whose removal had failed, and nearly every tick has none.

From this change it lists them only on finding one. A worktree's own git
dir is read from its `.git` file, and git is asked only where there is no
such file (`Repo::git_dir`). The host's memory is asked once a process. The
empty tree's id is kept with the repository's remote reads, which every
`Repo` of it in a process shares. A start whose close found no terminal
lists none again.

### Before and after

The sample above, `5e10346f` and this change in turns, through the
exclusive queue, at load averages 10–112 from this and other work on the
host:

| Build | Nextest seconds | User + system CPU seconds |
| --- | ---: | ---: |
| before | 76.2, 75.5, 76.1, 75.6 | 380, 368, 368, 368 |
| `finish_failed_removals` only | 72.4, 72.1 | 348, 348 |
| this change | 71.9, 71.8 | 345, 344 |

The whole slow gate's tests (818), on 2026-10-31, `5e10346f` straight before
this change, twelve at a time, at load averages 52–84:

| Build | Nextest seconds | Sum of the tests' own seconds | User + system CPU seconds |
| --- | ---: | ---: | ---: |
| before | 533.7 | 6,337 | 2,952 |
| this change | 496.5 | 5,895 | 2,737 |

Seven percent off the slow gate, in time and in CPU alike. That puts the
whole gate near the budget, not under it, on a host with other fleets'
work beside it.

What is left is still processes a command starts. Each command that runs
`gh` asks `gh config get user` and `gh api rate_limit` first (870 calls in
400 of the sample's 588 CLI processes). The admission readings start
`sysctl` and `df`: 1,800 calls, but those are the stand-ins a world sets its
load and disk through. A dispatch reads its issue's comments four times
(`recovery::own_heads`, `dispatch::start`, `note_start`, `board::post`).
`remote get-url origin` is asked again in one start (`probe::Asked::now`,
`tools::look`, `checkout_list::repo_of`: 763 calls in 290 processes),
where a long-running host runner must not take the origin as fixed. A run
of the `candidates::`, `lander::` and `turns::` slow tests at load 90–105
failed 2 of 5 times on `5e10346f`, a cancelled candidate's `land` answering
a missing-file error (sunfmin/control#312).

## The fast gate's tests ran one at a time (sunfmin/control#319)

`Offices-MBP.lan` recorded five landings past the 600 s budget: 685, 723,
683, 717 and 685 seconds. Three left their whole output, each started at
load 2–9:

| Landing | fmt | Flux | Clippy | build | fast tests | slow tests |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `2d5b4883` | 2 | 29 | 15 | 33 | 88 | 515 |
| `01d7bd6d` | 1 | 29 | 16 | 32 | 89 | 513 |
| `bfaae764` | 1 | 29 | 15 | 33 | 90 | 513 |

The 1,098 fast tests took 88–90 s there, and 20.0 s run alone in the queue
at load 11 on 2026-10-31. afkconvoy 0.5.50, which ran those landings, gave
every fast gate its share of the host as `NEXTEST_TEST_THREADS`: 8 cores over
3 worker, 2 fast and 1 slow lane is 1. Each log says the variable was set
(`ignoring --test-threads because --no-run is specified`). A worker's
`gate --fast` on 0.5.50, alone in the queue, ran its fast tests in 88 s too.
sunfmin/control#318 (afkconvoy#690) gives the share only beside another
lane's command. It is on `main` and in no release yet: once the fleet cuts
one, a landing alone in the queue runs its fast tests on every core, about
70 s less than above.

### Where a slow test's time goes now

Every eighth slow test (103), traced as in sunfmin/control#311, with each
process's resource usage written as it exited: of 370 CPU seconds, the CLI's
processes took 60 of their own and 258 in the processes they started; 139
ticks took 1.9 s and 0.78 CPU seconds each. Sixteen tests at a time took as
long as twelve (60.4 and 60.9 s against 60.7 and 60.3 s): the tests are
bound by the CPU spent starting processes, and only fewer or cheaper ones
help.

- Each tick asked its login shell `type -- 'agent'` and `agent --help`
  twice: once for the fleet's view of what it can start
  (`Fleet::runnable`), once for the agent that starts it. 787 of the
  sample's 2,186 login-shell questions were asked again in the process that
  had asked them. From this change a question is asked once a look, in the
  directory it is asked in (`launch::ANSWERED`, docs/adr/0043).
- Each tick listed `refs/convoy/delivery/*` (`delivery::reconcile`) and
  `refs/convoy/base/red/*` (`repair::tend`) on the remote again, right after
  its own listing of `refs/convoy/*`: 2 of the 5.6 `ls-remote`s a tick made.
  Both now read the tick's listing. A glob read at `Fresh::Process` still
  asks the remote: `candidates::abandoned` runs in the host runner, and must
  see chains other processes wrote.
- The stand-in `sysctl` read its files with `cat`: a process more for each
  of the load and memory readings a tick makes. It reads them with the
  shell's `read`: 5.4 against 3.5 ms of CPU a call.

### Before and after

`bfaae764` and this change, in turns through the exclusive queue, twelve at
a time, at load averages 47–77 from this and other work:

| Tests | Build | Nextest seconds | User + system CPU seconds |
| --- | --- | ---: | ---: |
| the 103 above | before | 62.5, 61.6, 61.3 | 352, 340, 338 |
| | after | 58.5, 59.1, 58.8 | 324, 324, 324 |
| all 819 slow tests | before | 504.6 | 2,783 |
| | after | 478.8 | 2,641 |

Five percent off the slow gate, in time and CPU alike. With the fast tests
on every core, the landings above would take about 590 seconds.

What is left is still processes: a fresh world's startup probe (three
pushes), a claim and its heartbeat pushed apart, `remote get-url origin`
asked again in one process (333 of 632; a unit test of `repo_of` changes a
checkout's remote within one process), and an issue's comments read again in
one process (523 of 903, cheap with the stand-in `gh`).

## The gate ran a test to each core it was given (sunfmin/control#965)

`Studio64` (16 cores, 64 GiB; its host settings cap afkconvoy at 50% of its
cores, 8, from 2026-11-14, ADR-0075) recorded five landings past the 600 s
budget: 803, 756, 678, 729 and 693 seconds. The last, `c46574b2`, left its
output: fast gate fmt 1, Flux 54–59, Clippy 14–26, build 25–32, fast tests
73–74; slow gate 635 s. Its slow gates' records took 548 to 671 s; the slow
gate of a landing before the cap, 442 s.

`afkconvoy exclusive` gives a gate its share of the host as
`NEXTEST_TEST_THREADS`, in cores: the 8 the cap allows when it runs alone, 4
(16 cores over 4 lanes) beside another lane. nextest takes that over each
profile's own count, so the slow gate ran 8 or 4 tests at a time, not 12.

### Tests at a time

Through the exclusive queue on 2026-11-14, at load averages 6–12 at each
start, every sixth slow test (154), twice each way, with `/usr/bin/time`:

| Tests at a time | Wall seconds | User + system CPU seconds | Cores busy |
| ---: | ---: | ---: | ---: |
| 4 | 199.1, 188.6 | 296, 277 | 1.5 |
| 8 | 98.0, 96.1 | 291, 289 | 3.0 |
| 16 | 55.1, 54.4 | 325, 323 | 5.9 |

The CPU is about the same at every count: a test waits on the processes it starts, about a
third of a core busy. The fast gate's tests (1,494) under its own profile, at
load 7–9 at the start of each:

| Tests at a time | Wall seconds | User + system CPU seconds | Slowest test |
| ---: | ---: | ---: | ---: |
| 4 | 78.7 | 159 | 4.95 s |
| 8 | 36.4 | 155 | 4.96 s |
| 16 | 22.0 | 173 | 5.16 s |

From this change `./gate.sh` runs two tests to each core it is given
(ADR-0077), and says how many as its tests start.

### After

`26a13a43`, this change, through the exclusive queue on 2026-11-14:

| Gate | Steps | Seconds |
| --- | --- | ---: |
| fast (`afkconvoy gate --fast`, beside another lane, load 24 at its start) | fmt 2, Flux 65, Clippy 16, build 6, fast tests 49 | 138 |
| slow (load 13 at its start): 923 passed, 23 past 20 s | slow tests 323 | 323 |

The slow gate's 323 seconds kept 6.6 cores busy (2,135 CPU seconds), under
the 8 the cap allows. Its slowest test took 66.9 s
(`host_watch::a_hung_human_notification_times_out_and_retries_without_holding_back_looks_or_rounds`).
A landing's gate on Studio64 now comes to about 460 seconds, against 678 to
803.
