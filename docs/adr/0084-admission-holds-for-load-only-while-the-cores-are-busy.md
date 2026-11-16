---
status: accepted
---

# Admission holds for load only while the cores are busy

Amends ADR-0019, ADR-0050 and ADR-0078 (sunfmin/control#1168).

On Studio64, admission held new workers back for load with most of the
host's cores idle: "the load 104.54 is past 32.00, 200% of its 16 cores".
A minute later the load average was 214 while `top` said 33% of the CPU
idle. macOS counts in its load average more than the threads that want a
core: another account's ~700 mostly idle simulator processes kept it near
100 by themselves. The load average alone said the host was busy when its
cores were not.

## What admission measures

Admission reads two things of the host's CPU: the one-minute load average,
apportioned by the queue's share as ADR-0050 and ADR-0078 say, and how busy
its cores are — the share of all their time not idle, from the kernel's CPU
ticks (`host_statistics`' `HOST_CPU_LOAD_INFO` on macOS, `/proc/stat`
elsewhere) between two readings. A process says it over the time since its
own last reading, up to a minute, the load average's span; one with no
reading that recent reads twice, a quarter of a second apart.

The load holds only while both are past their thresholds: the load past its
share of the cores (`--admit-load`, 200% by default), and the cores more
than 90% busy. Load that cores mostly idle carry is waiting on something
other than a core, and another worker does not make it wait longer. Cores
busy under a load within its share hold nothing either: the queue's own
gates may take every core (ADR-0050), and afkconvoy's own share of them is
bounded by `--max-share-of-cores` (ADR-0075).

Where the ticks cannot be read, the load alone holds, as before.

## What is said

`plan` and `doctor` say the cores' busy share in `admission.reading`
(`cpu_busy_percent`) beside the load, and its threshold in
`admission.thresholds`; a hold for load names both figures it used, or says
that how busy the cores are could not be read.
