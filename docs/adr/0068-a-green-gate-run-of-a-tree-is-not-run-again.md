---
status: accepted
---

# A green gate run of a tree is not run again

Felix, 2026-11-12, deciding Guild's throughput (sunfmin/control#693): 「同意，一直开着」 — a green gate run of a tree is not run again, for the same tree, gate declaration and kind of machine, always on. The gate and landing crates are Guild's as well as the fleet's (ADR-0059, ADR-0060), so the rule is theirs (sunfmin/control#788).

A recorded gate run already names its tree and its command, which is the gate the tree's own commit declares. It now also names the kind of machine it ran on: the host's operating system and processor architecture, `macos-aarch64`. Asked to run the same command on the same committed tree on the same kind of machine, the gate crate's runner (`run::gate`) answers with the green run on record, a reused run, and says so; nothing runs and the record stays as it is. A red run is never on record, so it is never reused. Another kind of machine runs it again, since some tests depend on the machine; so does a worktree that is not exactly its commit, and a record whose machine is not stated.

The landing uses it, in the landing crate and in the fleet's turn: a candidate whose tree is green on record is not gated again, and a round whose tip is green on record of the slow gate is not slow-gated again. That happens when a candidate's predecessor is the target tip and the pull request's head already holds that tip, so their merge is the head's own tree. The candidate's evidence names the run it stands on under `reused`: when it finished, on which host and kind of machine, and how long it took. This narrows ADR-0037's rule that every candidate gates its exact commit. The tree is what the commit lands, and a record is of the tree.

## Considered options

- **A repo's choice, off by default.** Refused: Felix wanted it always on.
- **The host's name as the machine.** Refused: two hosts of one kind pass the same tests, and that sameness is what reuse saves.

## Consequences

- The fleet's own `afkconvoy gate --fast` still records nothing, so a worker's fast run does not yet answer for its candidate. Only a run that is on record does.
- Records written before this name no kind of machine. They are still believed as evidence for landing, but no run reuses them.
