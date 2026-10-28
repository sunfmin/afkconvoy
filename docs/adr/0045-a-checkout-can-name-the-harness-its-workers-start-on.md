---
status: accepted
---

# A checkout can name the harness its workers start on

ADR-0020 gave harness priority one home, the order of the host kit, and said there is no chosen-harness record, repository mapping or clear operation. That still holds for a checkout that names no harness. It does not hold for a checkout that names one.

A checkout names at most one harness, on this host, on its entry on the checkout list, beside its concurrency and its tracker. `run <checkout> --harness <name>` records it for a checkout already running and does not start that fleet instance again. `run <checkout> --harness default` clears it. A word that is no harness, or a harness the host kit does not hold, is an error and writes nothing. The repo config still names no harness. The host kit stays the only home of worker launch commands, models and research, and it still orders the models of the harness a checkout names.

Workers of a checkout that names a harness — its issues, stages, consults, patrols, landings and its train worker — start on that harness when the role's range includes it. A `model:` label selects only a model of that harness. With casting on, a caster chooses only among that harness's models. A role that holds no limits for the named harness starts no worker of that role, and no other harness of its range is started in its place. A role that holds no limits at all stays right for every harness. An account fault, a model fault that leaves the harness with no placed model, or a worker launch command that cannot start does not start a worker of this checkout on the next harness of the host kit. A start checks the named harness. It does not refuse the checkout because some other harness of a role's range cannot start.

On the tick that first sees a named harness, a worker of that checkout already at work on another harness is continued onto the named one the way `switch --all` continues a worker: the scene and the claim stay, and no second writer starts until the old one is confirmed ended. A worker already on the named harness is left. Clearing the name continues a worker that is not on the host kit's first eligible harness, once. While the checkout names none, a later tick does not move a healthy worker back onto the host kit's first: a recovered preference still participates in new starts without moving healthy successors back.

`switch` still reorders the host kit. It does not change a checkout's named harness, and it does not continue a worker of a checkout that names one onto the kit's new first.

The host's watch is not a checkout's worker. It still starts on the last harness that took a worker's brief, then the host kit's order, and never by a checkout's named harness.

`checkouts`, `plan` and `doctor` say the harness in force for each checkout, and whether the checkout names it or the host kit does.
