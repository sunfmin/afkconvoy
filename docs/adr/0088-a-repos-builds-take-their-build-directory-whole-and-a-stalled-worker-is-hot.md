---
status: accepted
---

# A repo's builds take their build directory whole, and a stalled worker is hot

Amends ADR-0019 (sunfmin/control#1249).

The host's cargo is the flux toolchain's fork, huacnlee/cargo, which builds
every worktree of a repo into one build directory of its own choosing
(`~/.cargo/targets/<workspace>-<hash>`, and `shared-deps` beside it), and
turns on upstream's unstable fine-grain locking by default. Under it a cargo
takes a shared lock on every unit it plans to use and keeps it for its whole
run, and, for a unit it judged dirty, waits for that unit's exclusive lock
with no bound. Two cargos each holding shared a unit the other waits on wait
forever: hold and wait, in no fixed order. On 2026-11-16 five cargos of three
workers waited so for 8 to 20 minutes, at no CPU, with nothing queued, and
nothing in the fleet saw it: a process stuck on a lock takes nothing, so it
is never a hot worker.

## A build takes its build directory whole

Every worker's terminal exports, beside its `TMPDIR`, cargo's opt-out of
fine-grain locking (`__CARGO_TEMPORARY_FINE_GRAIN_LOCKING_OPT_OUT=1`), and the
gate's command is started with it; one constant says it
(`afkconvoy_gate::builds::SERIAL_BUILDS`). With it a build takes its build
directory's one lock, exclusive, and its artifact directory's after it, in
that order: there is nothing to cycle on. A cargo of another program that
keeps fine-grain locking holds the build directory's lock shared, so it and
one of ours exclude each other whole.

What it costs:

- **Disk**: nothing more. The worktrees of a repo still share one build
  directory, and what was built there stands: built with fine-grain
  locking, `afkconvoy-gate` was fresh, all 24 units, at once without it
  (measured on macnoMac-Studio).
- **Build time**: the compiles of one repo's worktrees run one at a time,
  each waiting for the one compiling. Only compiling holds the lock: a
  `cargo test` whose test ran 12 s let another build of its directory finish
  in 0 s meanwhile (measured on macnoMac-Studio). Workers mostly compile
  little at a time on a warm directory; a cold one makes the next worker
  wait for it once.

## Considered

- **A build directory per worktree**, set by the worker's environment as
  `TMPDIR` is. No lock is shared, and builds run side by side, but each
  worktree builds everything once more: the repo's build directory is
  4.2 GiB on macnoMac-Studio, so as much again a worker, which the
  disk rule (ADR-0082) then has to hold, and a cold first build for every
  worker. Taken where serial compiles prove too slow.
- **Waiting for the fork's fix.** A cargo should not keep shared the units it
  no longer needs while it waits for an exclusive one, or should try the
  exclusive one and back off. It is the fork's to fix; the fleet cannot wait
  on it, and a fixed fork changes nothing here but what serial builds cost.

## A stalled worker is hot

A deadlock can still happen where the opt-out does not reach — a build no
worker or gate of the fleet started, a fork that stops reading it, another
tool's locks of the same names. So the hot-worker sampler finds it wherever
it does. It keeps, for each process of each worker, since when it has taken
no CPU and had no children. One idle so for `STALL_HELD`, three minutes, is
asked of `lsof` for the build-directory locks it holds open (cargo's `.lock`,
`.cargo-lock`, `.cargo-build-lock` and `.cargo-artifact-lock`), and each lock
for every process holding it. Where every holder is idle too — for
`STALL_HELD`, or, being no worker's, at no CPU and with no children as `ps`
says it — nothing will release them, and the worker turns hot, why
`stalled`. A build waiting on one that runs waits on a holder that compiles,
so it never turns stalled.

As it does, the fleet names the stall on the worker's issue: the processes
that wait, how many locks, and each holder with its worker and command; the
same goes into its evidence, and the host's watch takes the hot worker as
any other. It ends, as every episode does, five minutes after it is found
no more.
