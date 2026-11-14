---
status: accepted
---

# A gate runs two tests to each core it is given

Amends ADR-0049 and ADR-0075 for this repo's gate (sunfmin/control#965).

`afkconvoy exclusive` says a gate's share of the host as
`NEXTEST_TEST_THREADS`: the cores afkconvoy may use where the host caps its
share (ADR-0075), fewer beside another lane (sunfmin/control#245). nextest
reads that variable over each profile's own `test-threads`, and runs one test
to each core. But a test of the CLI waits on the processes it starts: it keeps
about a third of a core busy, not one.

On Studio64 (16 cores, afkconvoy capped at 50%: 8 cores), from 2026-11-14,
every landing's gate took longer than the 600-second budget: 678 to 803
seconds. Its slow gate ran its tests 8 at a time, 4 beside another lane, where
the slow profile says 12. Run through the exclusive queue, 154 of the slow
tests took 290 CPU seconds at 4, 8 and 16 at a time alike, and 199, 98 and
55 seconds: 16 at a time kept 5.9 cores busy, still under the 8 the cap
allows. The fast gate's 1,494 tests took 71, 36 and 22 seconds at 4, 8 and 16,
in about the same CPU, the slowest of them 4.4 to 5.2 seconds at each count,
under the fast gate's 6-second end (docs/gate-baseline.md).

## Decision

`./gate.sh` reads `NEXTEST_TEST_THREADS` as the cores it is given, and runs
its tests, fast and slow, two to each of them: `--test-threads` twice that
number. Given none, or not a positive whole number, each profile's own count
holds (`.config/nextest.toml`): 12 for the slow gate, 6 for the fast gate.
The check of one TIMEOUT alone keeps its one thread.

The cap holds what afkconvoy's processes use, read of the process table
(ADR-0075), and a gate twice as wide as its cores stays within them. Nothing
of `afkconvoy exclusive` changes: the variable is still the share every repo's
gate is given, and another repo's gate that runs a test to a core reads it as
before.

## Considered options

- **Raise the share `afkconvoy exclusive` gives.** Refused: the share is in
  cores, for builds and for every repo's tests, and how much of a core one
  test takes is the repo's own fact.
- **Raise the cap on Studio64.** Refused: the host said the rest belongs to
  the Guild Hub's own work, which this change still leaves it.
- **Three tests to a core.** Not taken: two keeps the measured use under the
  share with room for the processes' peaks and the system time that grows
  with the threads (161 to 191 system seconds from 8 to 16).

## Consequences

- On Studio64 the slow gate takes about 330 seconds in place of 550 to 635,
  and the fast gate's tests about 22 in place of 73.
- A host that caps nothing and runs the gate alone gives no share, and the
  gate runs as before.
- A test that waits on a process's timing sees about twice the tests beside
  it where the gate is given a share; the 6-second end of the fast gate and
  the 240-second end of one test still name any that cannot keep up.
