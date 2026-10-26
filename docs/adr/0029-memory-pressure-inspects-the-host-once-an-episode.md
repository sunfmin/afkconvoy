---
status: accepted, operator lifecycle superseded by ADR-0035
---

# Memory pressure inspects the host once an episode

On 25 October 2026, memory pressure from a process outside every fleet made each fleet's largest worker hot, though each held only 170–620 MiB. As the largest changed, inspections ran back to back and found nothing they could fix. Admission still admitted: the kernel reported 43% memory available, above its 10% floor (sunfmin/control#49).

Memory pressure is observed on the host. It makes no worker hot; a worker turns hot for memory only past its own resident-memory limit, a quarter of the host's memory or its `--hot-memory`. Pressure at macOS's warning level or above, held for the host inspection span (ten minutes or `--admit-inspect-after`), calls for one host inspection for memory. Its episode ends after five minutes without pressure, independently of admission's episode. Admission's available-memory reading and its decision to hold or admit stay as they are.

The host inspection receives the pressure level, admission's reading and shares, the process trees of each worker and the landing train, and the hottest processes outside them. Its brief asks it to check that pressure has gone; admission may already admit. It shares every inspection bound in ADR-0026, including one at a time across the host's fleets and no second inspection where another fleet already inspected memory in the episode. Changing which worker holds the most starts nothing.

An inspection waiting its turn must still have a current episode when it starts. The inspector reads the hot workers again after ending a previous inspection and rechecks the chosen episode before launch. A host inspection rereads its condition too: cooled episodes start nothing.

This supersedes ADR-0019's memory-pressure clause in “What is watched”. Its worker memory limit, admission and exclusive queue stand; ADR-0026's authority and bounds stand.
