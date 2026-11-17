---
status: accepted
---

# A worker's exclusive memory counts only under pressure

Amends ADR-0019 (sunfmin/control#565).

A worker's heavy work runs under `afkconvoy exclusive`, as its brief asks, and the exclusive queue already bounds it. A cold build of a large workspace there still held more than the resident-memory limit, so the worker turned hot and started a watch round with nothing to do while the host's memory pressure stayed at level 1.

The memory limit now counts a worker's resident memory outside its exclusive commands: the process of each running queue ticket, and every process under it, is left out. Once the host's memory pressure (`kern.memorystatus_vm_pressure_level`) has held at level 2 or above for two minutes, sample after sample in the worker's samples, as CPU must hold, everything counts again. Where the pressure cannot be read, as on Linux, it counts as low; where the queue cannot be read, nothing is left out. The episode still records the worker's whole resident memory, and CPU and disk limits are unchanged.

A single reading was not enough (sunfmin/control#587): a cold build under `exclusive` on a 16 GiB host filled it for moments, the pressure read 2, then 1, then 1, and that one reading turned two workers hot with nothing a watch could do. The span counts from the start of the first sample at level 2; a lower reading starts it over. The episode names the pressure readings that counted its exclusive memory — their levels, how many and over how long — so a watch can tell a pressed host from a blip. Memory over the limit outside `exclusive` still turns a worker hot on its first sample.
