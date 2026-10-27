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
landing candidate may include upstream changes and therefore more tests;
its source commit, test counts and step times must accompany its duration.
