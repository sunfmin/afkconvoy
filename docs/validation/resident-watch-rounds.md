# Resident watch round evidence

Verified on 27 October 2026 for sunfmin/control#145 with the installed local
Orca and real interactive Codex 0.164.0. The test used the production guarded
launch, creation acknowledgement, Terminal lookup, keyed round delivery,
explicit `watched` command, report reconciliation and persistent session
reuse. Its two bounded tasks only submitted their own complete reports.
The normal host-runner scheduler is separately exercised by the CLI tests.

| Identity | Both rounds |
| --- | --- |
| Terminal | `term_b938756b-09b0-4fe1-88b7-7e27cac73f20` |
| Resident session | `4c53933b-6510-46d0-812f-b5c759051592` |
| Marked executor PID | `75740` |
| Orca process incarnation | `21169a87-b6e5-4c1b-bfb1-b3b81369a314` |

| Round | Request | Report |
| --- | --- | --- |
| `real-first-71313-0` | `89d89c91-d036-4c84-94ae-dde91c5618b0` | `smoke round 1 only` |
| `real-second-71313-33` | `986d9949-d3f3-403c-aa56-6ce37f100dc6` | `smoke round 2 only` |

The first receipt reported accepted input with `observation: unsupported`.
As with existing worker delivery, the watch retained the receipt and its
own Terminal's visible `Working` evidence before accepting execution. The
second receipt included `input_accepted` and `turn_started`. Both named the
same process incarnation. After the first report, durable state supplied
the resident identity and public lookup returned the same Terminal for the
second task. Both explicit reports remained independent, and the marked
executor stayed alive. Input readiness was confirmed after each report.
The Terminal remained visible and writable; no chat surface was used.

[Captured evidence](resident-watch-rounds.json) retains both receipts,
execution evidence, reports, pane observations and the final screen. The test
passed in 43.93 seconds. Cleanup then terminated and closed only this test
session; the worker's original Terminal and all operator sessions remained.

## Reproduce the opt-in test

Run from a worktree registered with a real local Orca, with a configured
Codex login and host kit. The unit-test cache uses a temporary directory;
`TMPDIR` puts that directory inside this worktree. Copying the kit protects
the invoking host's kit from command discovery writes.

```sh
mkdir -p .afkconvoy-tmp/resident-real/tmp .afkconvoy-tmp/resident-real/config/afkconvoy
cp "${XDG_CONFIG_HOME:-$HOME/.config}/afkconvoy/kit.json" .afkconvoy-tmp/resident-real/config/afkconvoy/kit.json
cargo build -p afkconvoy
TMPDIR="$PWD/.afkconvoy-tmp/resident-real/tmp" \
XDG_CONFIG_HOME="$PWD/.afkconvoy-tmp/resident-real/config" \
cargo test -p afkconvoy --lib \
  real_orca_resident_agent_reports_two_rounds_in_one_terminal -- --ignored --nocapture
```

The test writes `.afkconvoy-tmp/real-resident-evidence.json` before removing
its temporary state and closing its own Terminal.

## Scheduler regressions

The focused watch suite passed all 35 tests in 45.505 seconds before the
additional receipt archive and provider fallback checks. It covers real CLI
and host-runner scheduling with external-tool stand-ins: queueing during a
busy round; duplicate, recovered and returning episodes; busy input after
reporting; restart recovery; uncertain creation; exact request replay after
a lost response; report identity and idempotence; an aged idle resident
receiving a fresh hour; unreported exit retries; harness continuation; and
the existing resource, ownership and launch-shell creation fences.

The project gate runs on the exact merged candidate in the landing turn.
Its authoritative result is the recorded turn attached to the PR.
