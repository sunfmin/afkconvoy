---
status: accepted, amended by ADR-0036, ADR-0038, ADR-0053 and ADR-0073
---

# Every host has a watch

The fleet's workers, patrols and the consults a team names do the work and the team's judgment. The host's one watch does everything operational on that host. It needs no operator directory to exist: where the host names none, the **stock watch** runs in a directory afkconvoy makes and keeps, under one short instruction carried in the binary. An operator's directory wins wherever one is named, under that directory's own rules, with none of the stock instruction added.

The instruction asks it to find what holds the fleets' work back, fix it with the watch's authority, check that the work goes on and say what it did. Spending money stays with a human (ADR-0073): the watch acts on none of it and lists each such decision, with its question and answer line, in its report.

This amends ADR-0030, *The host runner watches its host* (sunfmin/control#78): no operator directory means the stock watch, rather than no look and no watch. `--watch-dir default` returns to the stock watch. The directory and whether it is stock are shown even before a watch starts. The trigger, look, harness choice, bounds, record and authority are the same in either directory, including sunfmin/control#79's authority across the checkout list. A running watch and its continuations keep the directory and rules they started under when the setting changes.

Keeping operational work in one watch ends the need for each fleet agent to stand in as the operator.

Amended by ADR-0036 (sunfmin/control#96): the stock consult is removed; its hand-offs escalate for the watch to answer with the operator's authority. Named consults remain.
