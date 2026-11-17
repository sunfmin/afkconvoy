---
status: accepted, amended by ADR-0035 and ADR-0061
---

# Memory pressure inspects the host once an episode

On 25 October 2026, memory pressure from a process outside every fleet made each fleet's largest worker hot, though each held only 170–620 MiB. As the largest changed, inspections ran back to back and found nothing they could fix. Admission still admitted: the kernel reported 43% memory available, above its 10% floor (sunfmin/control#49).

Memory pressure is observed on the host. It makes no worker hot; a worker turns hot for memory only past its own resident-memory limit, a quarter of the host's memory or its `--hot-memory`. Pressure at macOS's warning level or above, held for the host resource span (ten minutes or `--admit-inspect-after`), calls for the host's watch for memory, once an episode (ADR-0035). Its episode ends after five minutes without pressure, independently of admission's episode. Admission's available-memory reading and its decision to hold or admit stay as they are.

The watch is given the pressure level, admission's reading and shares, the process trees of each worker and each turn, and the hottest processes outside them. Changing which worker holds the most starts nothing.
