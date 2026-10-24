---
status: accepted
---

# The gate limit is declared, and a slow gate is an issue

The gate was given an hour by a flag nobody set, and a green run kept no time. So when one function of the core took Flux nine minutes instead of fifteen seconds, every run was green and nothing said so: the gate could grow to an hour before anything refused it.

A repo declares its gate limit in its config, beside the gate command, and that is the only place the number is written. A run that reaches it is ended and is red, as a run that reached the flag's hour was. A recorded gate run says how long the run took and on which host. When the recent landings on the base branch, on one host, all took most of the limit, the gate is slow: the fleet opens one issue that says so and gives it to the fleet to work, and opens no second while that one is open.

The gate stays the repo's command and nothing else to the fleet (ADR-0004): the fleet times the whole command and does not know its steps. Which test or which function is slow is for the gate to print.

## Considered options

- **A limit learned from past runs: red when a run is much slower than the last ones.** No number to choose. Refused: workers gate side by side on hosts of different speeds, so a run is often slower than the last for no reason in the tree, and a limit that follows the runs follows them up — each small slowing passes and becomes what the next is measured against.
- **Only the declared limit, and no issue.** Less to build. Refused: a gate that slows a little with every PR is paid for by the PR that happens to reach the limit, whose worker did not cause it and is stopped by it.
- **The time written on the commit that lands.** One `git log` would show it. Refused: that commit exists before the gate runs, so the fleet would land a commit other than the one it gated. A recorded gate run is already a commit of the tested tree.
- **A patrol that looks for a slow gate.** Patrols are how work reaches the fleet. Refused: whether the gate is slow is a comparison of numbers, decided the same way every time, and an unbound repo runs no patrol.
- **Steps the gate reports to the fleet.** The fleet could then name the slow step. Refused: every repo's gate would have to speak a form the fleet reads.

## Consequences

- The fleet opens an issue of its own for the fleet to work. When this was decided, the only issue it opened was for a human, when a patrol's run failed.
- A worker given a slow gate can end it by deleting tests or by raising the limit. The issue says neither is a fix. Raising the limit is a change to a committed file, seen in a PR like any other.
- The limit is one number for every host. A repo sets it for its slowest host with its workers running side by side, so it stops a gate that has become several times slower, not one a fifth slower.
- A red run leaves no record, so the run that reached the limit leaves no time. A landing answered by a run on record adds no new time. Runs of another gate command are not counted, and after a slow-gate issue is closed only the landings since are.
- A repo that declares no limit has the hour it had, and `doctor` says so.
