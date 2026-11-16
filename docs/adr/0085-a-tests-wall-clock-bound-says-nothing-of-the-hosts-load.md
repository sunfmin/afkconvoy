---
status: accepted
---

# A test's wall-clock bound says nothing of the host's load

Amends ADR-0047 (sunfmin/control#1183).

On Studio64 another user's iOS simulators keep the load average at 100–280
on 16 cores, and the slow gate went red on tests whose wall-clock bounds were
tight. The cut of v0.5.98 failed on a transition given 15 seconds, which
took 7–13 seconds alone on the same commit (sunfmin/control#1182). Its next
attempt failed in the fast gate: seven tests reached the fast profile's 6
seconds at a load of 139. The gate could not classify them on a loaded host,
and a host that is rarely quiet held every cut back.

## Two kinds of bound, and no third

Every wall-clock bound in the tests is one of two kinds:

- **A wait for an event**: a file appears, a line is printed, a transition
  is recorded. Its bound only catches a hang. Every test crate takes it from
  one place, the `afkconvoy-patience` crate's `PATIENCE`, 180 seconds,
  within the limit for one test of 240. A test waits through `until`,
  `until_true` or `deadline()`, and is held only as long as the event takes.
- **An assertion that something does not block**: a command answers before a
  slow part of its work finishes, or a wake opens a tick before its sleep
  ends. The test proves the order, not a number of seconds. It holds the slow
  side (a stand-in that waits on a file, a process that never ends by itself)
  until the fast side has answered, or it reads the cause the program states.
  A tick's start line names what opened it, and an answer names the bound
  that ended its wait.

A lower bound (something waited at least its time) stays, since load only
makes it longer. So does a short check that nothing happens within a moment:
load only makes that pass more easily.

## The fast gate on a loaded host

The fast profile's 6 seconds draw the line between the two gates (ADR-0047),
and say nothing of a hang. When the fast tests run red only on TIMEOUTs and
the host is loaded past its cores, `./gate.sh` runs them again under the
`fast-loaded` profile. Its limit for one test is the slow gate's 240
seconds. Green there, the gate is green, and each earlier TIMEOUT is flagged
as unclassified, as a SLOW test on a loaded host already is. Red there, the
gate is red. A run with a FAIL, or with TIMEOUTs on a quiet host, is not run
again: the quiet check against the 2-second line still decides those.
