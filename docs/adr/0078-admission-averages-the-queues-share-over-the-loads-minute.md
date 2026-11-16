---
status: accepted, amended by ADR-0084
---

# Admission averages the queue's share over the load's minute

Amends ADR-0050 (sunfmin/control#1069).

On Studio64, admission held new workers back for about half an hour while
the load was almost all the fleets' own gates. ADR-0050 apportions the
one-minute load average by the queue's CPU share in one reading of the
process table. The load lags the work that made it by about a minute, and
that one reading does not: between two of a gate's phases its tree takes
nothing for a moment, and its whole lagging load counted as other load. Nor
did the reading count what a gate makes the host's daemons do: a gate's
tests start dozens of programs a second, and each is checked and each file
written seen by root's daemons, outside the queue's tree.

## The share is averaged as the load is

The host runner keeps the queue's share of the host's CPU, decayed over one
minute as the kernel decays the load average. It notes it every five
seconds while a command of the queue runs, and as it answers what its
fleets share, at most once a second; once none runs, the share decays
towards none, as the load it made does, and is gone once it is below a
ten-thousandth. Admission apportions the load by that average. Where no
host runner keeps one — none runs, it is older, or the queue has not run
lately — admission apportions it by one reading as before.

## The scanners' CPU is the queue's while it runs

While a command of the queue runs, the CPU `fseventsd`, `XProtectService`,
`syspolicyd` and `opendirectoryd` take counts as the queue's. Nothing else on
a host makes them as busy as a gate's tests do; while no command runs, their
CPU is nobody's share of the queue.

## What stays

Load from processes outside the queue throughout still holds: their share
is never the queue's, and averaging it changes nothing. A reading of the
process table that cannot be read, or that holds none of a running ticket's
processes, is noted not at all, and the average stands as it was. Doctor
reports total load, estimated queue load and the remainder as before.
