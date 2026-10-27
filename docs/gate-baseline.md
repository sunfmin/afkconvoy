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

## Rationale and interpretation

Ten minutes gives about 58% over the review's 378.67 seconds before a run counts
as slow. Five such recorded landings on one host must confirm the condition:
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
