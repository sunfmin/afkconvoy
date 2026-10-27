# Watch Terminal recovery evidence

Implemented for sunfmin/control#146; the resident-round scheduler in
sunfmin/control#145 can use the launch journal and lookup below without
requiring changes to Orca.

## Real visible Orca

On 27 October 2026, the opt-in test ran against the installed local Orca:

```sh
RUSTUP_TOOLCHAIN=nightly-2026-09-25 cargo test -p afkconvoy --lib \
  watch_terminal::tests::real_orca_recovers_a_visible_terminal_and_preserves_input_while_refusing_ambiguity \
  -- --ignored --nocapture
```

It used the production `prepare`, `launch_line`, `Worktree::start_watch`,
startup acknowledgement, `locate`, `active` and status persistence. The
executor was a harmless interactive shell wrapped in the existing marked
watch session, so the test did not ask a model to operate the real host.

- Watch Terminal: `term_3cdfe006-00c2-4278-aebc-a17994a7e089`.
- Executor: PID `84036`, with exactly one matching process for
  `afkconvoy-watch-session-smoke-83840-0-1` in checked local process readings.
- Same-name unrelated test Terminal:
  `term_34d817c7-eeea-4eb9-a58c-3d18e18d2f4c`.
- The test deliberately discarded the creation receipt, recovered using
  the shell's own acknowledgement, then reconstructed the running session
  from the on-disk journal. All lookups returned the original Terminal.
- Typed `human progress` remained observable as
  `retained input: human progress` after reconstruction.
- With both same-name Terminals present, exact-ID lookup still reused the
  watch. Removing the exact ID made discovery pause on ambiguity; the
  unrelated Terminal remained live.
- The test passed in 7.15 seconds and closed only its two test Terminals.

## Host runner regressions

`a_lost_creation_receipt_recovers_the_acknowledged_terminal_without_another_agent`
passed through the real CLI and host runner with the external Orca stand-in:
creation returned invalid JSON after its effect, yet one Terminal and one
brief were delivered. Deleting the runner's state file and restarting it
recovered the same watch and Terminal with no second creation or delivery.

The watcher suite additionally covers an unknown unique name, same-name
ambiguity, stale handles with stable public identity, uncertain creation and
prompt delivery across restarts, explicit-report recovery, retained directory
rules, model continuation, termination bounds and migration from a runner
that predates the journal. The launch-shell test runs concurrent duplicate
calls and asserts exactly one execution.

All 34 selected host-watch and launch-shell tests passed under nextest in
30.659 seconds. The project gate runs on the merged candidate during the
landing turn; its exact result is recorded by that turn and on the PR.

## Contract for sunfmin/control#145

`watch/terminal.json` in the host cache holds the watch snapshot, guarded
launch command, public Terminal metadata and launch status. A launch reserves
execution with an atomic, retained claim beside its transcript and writes its
own Orca runtime handle before starting the harness. The host runner's lock
serializes launches. Recovery consults the saved exact ID first, then only a
unique proven candidate; ambiguous creation is never resent. Before brief
input the status changes to `sending`; an uncertain acknowledgement pauses
instead of repeating input. `doctor.host.watch.association` exposes the
current association and any problem.

This change retains the existing report/termination and continuation rules.
#145 must separate the resident session identity from round IDs and decide
when a completed round can receive its next brief. It must retain this
creation fence, verify old executors have ended before replacement, and use
durable delivery receipts or pause at uncertain input. Terminal liveness and
Agent idleness still do not prove a round completed.
