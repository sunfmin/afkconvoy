# Recovery integration measurements — sunfmin/control#151

Measured on 2026-10-27, Mac16,8, 14 logical cores and 48 GiB RAM. The
starting tree was `456b24d1683252d0386a3b79c1ec98cb2c342ea7`.
`8c3b078f` adds fixture timing probes without changing the scenarios. An
archived copy of that tree, with the [confirmation timing probes](151-baseline-probes.patch),
provided the baseline while implementation proceeded in the worker's checkout.
All substantial runs used the host's exclusive queue. Its lane setting was
unchanged; nextest ran six tests at once.

Four stalled-terminal scenarios now enter the same CLI recovery decisions
through `examples/test_termination_clock.rs`. After the initial unsuccessful
confirmation, its monotonic clock advances to the five-second deadline.
Another complete OS-process and Orca-runtime observation still precedes the
refusal to start a replacement. The packaged `afkconvoy` executable installs
no clock and retains its real five-second bound and one-second SIGTERM grace.
The packaging script archives only `afkconvoy`.

Consult and patrol fixtures reuse complete, immutable Git repository seeds.
Publication is an atomic directory rename; losing builders remove their own
scratch directories. Each scenario copies files, objects, refs and index into
its own writable repository. Seeds live beside the test binary, are read-only,
and are keyed by their files, base branch and builder source. The manifest
checks the requested files. Changes to the builder invalidate seeds even when
Cargo retains the executable's filename.

## Matched measurements

Both sides below ran the same 164 existing cases with prebuilt artifacts and
six test threads. These matched measurements preceded the teardown repair;
the final repaired tree is measured again below. The new isolation test was excluded from this comparison
and included in the full gate and stress runs. Both sides created 188 Worlds,
186 repository fixtures, 35 consult bound fixtures and 35 patrol bound
fixtures. Assertions, scenario iterations and input sizes were retained.

| Group | Cases | Before cumulative seconds | After cumulative seconds |
| --- | ---: | ---: | ---: |
| switch | 43 | 188.820 | 157.215 |
| host_watch | 27 | 144.461 | 146.980 |
| consult | 31 | 96.941 | 118.947 |
| host_runner | 31 | 78.860 | 80.694 |
| patrol | 30 | 101.777 | 84.527 |
| existing fixture tests | 2 | 0.290 | 0.291 |
| **All selected cases** | **164** | **611.149** | **588.654** |

Nextest's actual wall time was **110.024 → 98.850 seconds**; the driver measured
110.21 → 99.04 seconds. Cumulative process seconds are not wall time.
The before run's 1-minute load went 20.17 → 14.01; the after run's went
13.69 → 27.10. These are loaded measurements, not isolated-host guarantees.

The before patrol run also contained a 35.993-second case whose browser
produced no screenshot. The unchanged renderer permits up to 150 polls of
200 ms and reported that only markup was checked. This outlier and the load
variation prevent attributing the entire wall-time difference to this change.
The real browser dependency and the existing markup assertions were retained.

An earlier, separate 164/165-case pair took 107.417/108.658 seconds: the after
run improved the four target cases while other work slowed, including consult
cases. That result is retained here rather than replaced with only the more
favourable comparison.

| Original switch test suffix (all names unchanged) | Before seconds | After seconds |
| --- | ---: | ---: |
| an_acknowledged_close_that_leaves_the_terminal_is_no_permission_for_a_second_writer | 6.451 | 2.024 |
| capacity_continuation_never_starts_a_second_writer_before_termination_is_confirmed | 20.056 | 6.134 |
| codex_task_refusal_never_starts_a_second_writer_while_termination_is_unconfirmed | 23.796 | 4.149 |
| explicit_usage_limit_starts_nothing_until_the_failed_codex_agent_is_gone | 18.474 | 3.955 |
| **Cumulative time of these four cases** | **68.777** | **16.262** |

The baseline probes observed 119 confirmation loops after terminal close: 64.458 seconds in their
loops, including 30.704 seconds sleeping. Eleven unconfirmed calls each reached
the five-second bound; together they consumed 56.077 seconds. The controlled
clock removes the actual sleep and repeated identical polls in those fake
terminal scenarios. It retains the initial and deadline observations.

| Fixture operation | Samples per side | Before | After |
| --- | ---: | ---: | ---: |
| Repository setup, median | 186 | 33.927 ms | 7.608 ms |
| Repository setup, cumulative | 186 | 6.493 s | 1.989 s |
| Consult bound setup, median | 35 | 255.846 ms | 211.618 ms |
| Patrol bound setup, median | 35 | 217.260 ms | 170.719 ms |
| World setup, median | 188 | 90.525 ms | 95.362 ms |
| Empty-World teardown | 1 | 7.587 ms | 8.574 ms |

Bound setup includes World and repository setup: those rows must not be added
together. World remotes, clones, fake service state, login environments, caches,
ports and host runners remain private to each scenario. The empty-World
directory cleanup was not changed. Owned-command teardown was repaired
separately below; this sample does not describe active host-runner cleanup.

The final repaired tree at `80718b80` also passed the same 164-case slice:
**90.723 seconds wall** (90.91 driver seconds), **539.343 cumulative seconds**.
Its group totals were switch 132.962, host_watch 145.307, consult 105.199,
host_runner 85.574, patrol 70.013 and existing fixture cases 0.288 seconds.
The 1-minute load was 10.97 → 11.05; artifacts and seeds were warm. Setup
sample counts remained 188/186/35/35. Final repository median was 7.481 ms
(1.784 seconds cumulative); consult/patrol bound medians were
190.482/162.555 ms. These later, lower-load observations are additional
validation, not a controlled attribution of the whole wall-time difference.

The decision to try repository copies followed a 20-sample prototype: median
rebuild 132.145 ms, copy 8.676 ms, with a 1-minute host load of 85.41. Actual
fixture timings above replace that prototype estimate. First-use seed creation
has a cost: an implementation probe measured consult bound setup at 382.598 ms
cold and 170.997 ms warm, versus 275.349 ms before. The first nightly after-group
run started with cold seeds and measured 189 setups at 3.841 seconds total;
the matched warm run measured 186 at 1.989 seconds. The extra three setups
belong to the new isolation scenario.

## Preserved checks

Final nextest inventories contain 921 original CLI names before and 923 after:
zero original names removed or renamed. The clock/seed measurement added
one isolation scenario and the teardown
repair added one lease regression. The repository isolation test is
`fixture::repository_templates_leave_each_world_with_its_own_writable_git_history`.
The four clock-controlled cases retain every original assertion and additionally
compare the complete original claim and brief after the failed continuation.
The consult/patrol scenario bodies retain their original assertions and inputs.

The new fixture scenario starts two independent Worlds, concurrently publishes
a fresh seed, verifies distinct inodes for the index, ref, file and Git object,
commits to one copy, makes its index read-only, and removes that World. It then
checks the other copy and a later copy of the seed, including clean status and
independent commit histories. A unique input forces cold publication on every
stress repetition. Paths include spaces; the complete gate also supplies its
existing deep TMPDIR.

Two deliberate faults were injected separately into both versions:

| Injected fault | Before | After |
| --- | --- | --- |
| Return success without termination confirmation | 4/4 target cases detected it | 4/4 detected it |
| Release the claim before confirmation in switch_issue | 4/4 target cases detected it | 4/4 detected it |

The two faults were repeated on the final teardown-repaired implementation;
both again failed all four cases (nextest exit 100), followed by a normal
four-case pass in 5.380 seconds. The later tracker-only isolation fix does
not alter those cases or their dependencies.

Each mutation run built the CLI and examples, ran the same four cases, and
failed all four as expected. Sources were restored afterward. Artifact-only
fixture reuse was also checked through inventory equality and concurrent
repetition. No property-test case count or input range was reduced.

The separate real-process recovery suite passed all 12 cases in 11.785 seconds.
It retained actual SIGTERM and SIGKILL, escaped children, an orphan in the
worktree, a shared server outside it, and failed kill confirmation. Those tests
use the production executable and real time. Real Git claim/candidate races
remain in the complete gate.

## Fixture teardown and inherited capacity

The host watch found that the otherwise green gate at `1a316aad` exited at
1793076143 while orphan `sh` PID 99314 and `sleep` PID 99331 retained inherited
worker-slot/repo lease descriptors 6 and 5. Both came from the stopped-worker
fixture, under the gate's deep temporary directory, and had started at
2026-10-27 04:40:55Z. The watch verified ownership before terminating them;
capacity was granted normally at 1793076399, 256 seconds after the green exit.
The production descendant lease correctly prevented premature reuse.

`stopped::Queued::drop` killed/reaped only the exclusive CLI parent, leaving
its `sleep 600` command alive. The doctor's queue fixture used the same
parent-only pattern with `sleep 30`. A regression running the actual original
scenario with a private inherited kernel lease failed in 2.10 seconds even
though the scenario itself passed.

Both fixtures now use `FixtureProcess`. It keeps its root unreaped, stops it,
freezes discovered descendants, and ends/reaps the attached tree from the
leaves upward. A stopped parent reserves each child's PID until that child's
exit is observed. It preserves the runner's process group, so nextest can
still clean up on timeout; it neither closes nor filters inherited lease FDs.
An early process-group variant was replaced before final verification because
it would have bypassed nextest's timeout group.

The final regression executes both original scenarios, plus an exact injected
panic after the stopped-worker scenario's original assertions. A rendezvous
lets the parent prove that the actual live `sleep` holds the inherited lease
and remains in the runner's group. After normal return or panic, the kernel
lease must be free and an unrelated process in the same group must still be
alive. Each child uses a deep TMPDIR with spaces. This regression passed in
12.53 seconds; five instrumented owned-command teardowns took 53–129 ms each.
The existing production test that kills an exclusive owner while its orphan
continues holding capacity also passed, retaining its real descriptor checks.

While final repetitions waited, a peer worktree still using the old fixture
independently reproduced the same leak. Its test owner PID 27442 exited, but
`sh` PID 69193 and `sleep` PID 69224, started at 14:31:41 local time, retained
FDs 5/6 in that worktree's stopped-worker fixture. Read-only `ps`/`lsof`
confirmed the exact `sleep 600` command, cwd and lease files. This worker
signalled no peer processes. Both holders later disappeared and the queue
advanced to a legitimate serial barrier; the disappearance mechanism was
not independently observed.

Watch evidence: `watch/1793075575-77415-4144/{lease-holders.txt,blocked-slot.json,
queue-ghost-check.json,queue-after-orphans.json}` in the host's afkconvoy cache.

The first repaired full-gate attempt at `8a8af1dd` released its exclusive
owner PID 45215 normally after exiting red. A subsequent queue snapshot at
epoch 1793081250 contained only the next legitimate owner PID 11066, whose
turn had started at 1793081205. The worker capacity remained one; no ghost
request from that gate remained. The final green retry is checked separately
below.

## Full verification

The pre-teardown-repair fixed-HEAD complete gate at `1a316aad` passed all 1,623 tests, with the
same 15 skips, in **327.40 seconds**: fmt 0 s, Flux 1 s, clippy 2 s, manual 1 s,
build 2 s, tests **320.572 s**. Its target and compiler/proof artifacts were
warm from the preceding checks. The 1-minute load at admission was 31.60 and
was 6.69 at the final observation. The test phase was 353.087 → 320.572 seconds;
the complete gate was 424.61 → 327.40 seconds. The whole-gate reduction includes
build-cache effects and changing load, so it is not a causal estimate for the
clock and fixture changes alone.

The first repaired attempt stopped after 1,030 passing tests and one tracker
failure in 295.89 seconds. Its existing test executed the suggested
`afkconvoy flows` shell command through the host PATH, picking up the newly
installed release instead of this build. The error text did not exist in
this branch. The exact test reproduced the failure in 1.298 seconds; pinning
only that scenario's `afkconvoy` symlink to `CARGO_BIN_EXE_afkconvoy` passed
in 1.336 seconds with every original assertion retained (`80718b80`). No
recovery or shared-fixture code changed for that isolation repair.

The repaired complete gate at `80718b80` exited **0**, passing **1,624
tests** with the same **15 skips** in **329.94 seconds**. Nextest measured
**322.990 seconds**; steps were fmt 0 s, Flux 1 s, clippy 3 s, manual 0 s,
build 2 s and tests 324 s. Build/proof artifacts and immutable seeds were
warm. The 1-minute load was 10.49 at admission and 9.96 after exit.

Its exclusive owner PID 81882 disappeared from the queue after exit. At
epoch 1793082040, the next legitimate command (PID 89075) had
acquired the unchanged single worker lane. Thus green exit was followed by
actual lease release, rather than an orphan continuing to hold capacity.
The queue snapshot is retained as `final-gate-retry-release.json` in the
local measurement directory.

The affected selection passed **20 consecutive repetitions × 70 cases =
1,400 passing cases**, at six-test concurrency, without a failed repetition.
Nextest wall time per repetition was **33.114–38.553 seconds**,
**694.743 seconds** across the 20 runs, excluding queue waits.
Admission 1-minute loads ranged from 10.15 to 25.31. Artifacts/seeds
were warm, while the isolation scenario forced a fresh concurrent seed
publication every time. The private inherited-lease regression and unchanged
production owner-death protection test ran in every repetition. After the
last batch, exclusive owner PID 50078 was absent and the next command had
acquired the lane (queue observation 1793083901).

The baseline complete gate passed 1,622 tests with the existing 15 skips in
424.61 seconds: fmt 1 s, Flux 30 s, clippy 15 s, manual 16 s, build 10 s,
tests 353.087 s. Its target directory was fresh; the host's existing compiler
cache was retained. After measurements used built artifacts and warm immutable
seeds, with the fresh-seed race still exercised by the added scenario.

One intermediate after-gate attempt is excluded from timing comparison: it
stopped at 819/1,623 cases because a diagnostic-document commit moved HEAD
after compilation. `release::the_binary_prints_what_it_was_built_from` correctly
rejected the old binary stamp. The retry holds HEAD fixed; the assertion and
gate were not changed to accommodate that mistake.

The regular gate still runs the whole workspace. Its existing 20-second SLOW
report and 240-second per-test limit remain in place. This work changed neither
the gate's test selection nor host gate lanes.

## Reproduction and limits

Build the test artifacts before timing. Run the existing selected cases with:

```sh
CONVOY_FIXTURE_TIMINGS=1 cargo nextest run -p afkconvoy --test cli \
  --test-threads 6 \
  -E 'test(/^(switch|host_watch|consult|host_runner|patrol|fixture)::/) & !test(=fixture::repository_templates_leave_each_world_with_its_own_writable_git_history)' \
  --status-level pass --final-status-level pass --success-output final
```

Use `afkconvoy exclusive --` for that measurement and for `./gate.sh`.
For the baseline, archive `8c3b078f` into a private directory and apply
`151-baseline-probes.patch` there; the patch was checked against those exact
source files. The fixture probes print their own setup durations; the baseline
patch also forwards the CLI's confirmation-loop counters into test output.
Keep HEAD fixed while the full gate runs.

The final stress selection is every consult and patrol case, the four switch cases
in the table, the repository isolation scenario, both repaired fixture callers,
the lease regression and the existing orphan-capacity protection test: 70 cases,
six at a time, repeated 20 times. The two pre-repair 66-case repetitions are
not counted toward this final verification. The tracker-only PATH isolation
repair leaves these 70 cases and their dependencies unchanged. Whole-suite benchmarking, exhaustive mutation testing and individual
build-unit profiling are outside these measurements. Build costs are reported
by gate step; cold and warm builds must not be compared as a causal runtime
speedup. The renderer outlier and host load remain limitations of the total
wall-time comparisons. The targeted fault cases and per-fixture timings show
where overhead was removed without reducing the safety checks.
