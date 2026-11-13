---
status: accepted, amended by ADR-0072
---

# Admission discounts the exclusive queue's load

Amends ADR-0019 and ADR-0049 (sunfmin/control#451).

The queue bounds its heavy commands, but their load can keep admission from starting the workers whose heavy work would itself wait for a lane. Admission now apportions the host's one-minute load average by the current CPU share of the process trees rooted at running queue tickets. It compares the remainder with the same load threshold as before. Waiting tickets contribute no tree. Memory and disk remain whole-host readings.

The kernel does not provide a per-tree load average: current CPU share is an estimate, not a measurement of each tree's contribution over the minute. A command that has just stopped, blocked, or started may be attributed too little or too much. When the queue or process table cannot be read, admission subtracts nothing; doctor says the queue share is unknown. Doctor reports total load, estimated queue load and the remainder used for admission. Other accounts' queues are outside this host's queue and count in the remainder.
