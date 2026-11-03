---
status: accepted
---

# A worker's exclusive memory counts only under pressure

Amends ADR-0019 (sunfmin/control#565).

A worker's heavy work runs under `afkconvoy exclusive`, as its brief asks, and the exclusive queue already bounds it. A cold build of a large workspace there still held more than the resident-memory limit, so the worker turned hot and started a watch round with nothing to do while the host's memory pressure stayed at level 1.

The memory limit now counts a worker's resident memory outside its exclusive commands: the process of each running queue ticket, and every process under it, is left out. While the host's memory pressure (`kern.memorystatus_vm_pressure_level`) is at level 2 or above, everything counts again. Where the pressure cannot be read, as on Linux, it counts as low; where the queue cannot be read, nothing is left out. The episode still records the worker's whole resident memory, and CPU and disk limits are unchanged.
