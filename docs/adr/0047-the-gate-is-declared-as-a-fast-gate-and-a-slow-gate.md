---
status: accepted
---

# The gate is declared as a fast gate and a slow gate

Amends ADR-0024 (sunfmin/control#236).

On `Offices-MacBook-Pro.local` (8 cores, 16 GiB) the gate takes about 1,000 seconds, and 796 of them are its tests. Whatever a worker changes, it learns whether a check failed only from that whole run. A repo now declares its gate as two parts beside each other, in a block of its config of their own, ```` ```yaml gates ````:

- **The fast gate**: what a worker passes before it opens or updates its PR. In this repo: formatting, Flux, clippy, the manual, the build of every test, and every test not marked slow.
- **The slow gate**: everything the fast gate leaves out. In this repo: every test marked slow, and no check the fast gate makes.

Each says its own command, limit (`seconds`) and warning budget (`budget_seconds`). Together they check exactly what `gate_command` checks, and `gate_command` is still the gate: a landing turn runs it whole, and only its runs are recorded. `afkconvoy gate --fast` and `--slow` run each part alone, under its own limit, record nothing, and say when a run passed its budget. A worker whose repo declares the block is told to pass the fast gate before opening or updating its PR. By hand, `./gate.sh fast`, `./gate.sh slow`, and `./gate.sh` for both, which is the gate.

## Where the line sits

The line is drawn per test, at 2 seconds on `Offices-MacBook-Pro.local`: a test that took 2 seconds or more there is marked slow, by its binary and whole name, in `.config/slow-tests`; every other test is fast. `gate.sh` makes one nextest filter of the list: the slow gate runs the tests it matches, the fast gate the tests it does not, so no test runs in both or in neither. A new test is fast until it is marked.

The fast gate keeps the line honest. It runs its tests under nextest's `fast` profile, which ends a test still running after 6 seconds, three times the line. Three times, not the line itself: a test that takes 1.8 seconds on a quiet host takes more on a host running three gates, and a line enforced at 2 seconds would flip such tests on load alone. A test that now runs under 2 seconds may be taken off the list, with its time.

Amended by sunfmin/control#445: a fast TIMEOUT alone does not establish that a test is slow. When the host's one-minute load is below its logical CPU count, the gate checks each timed-out test alone against the 2-second line, and recommends naming it slow only if that check also times out while the host remains below that load. A check that passes identifies a load fault. If the host is overloaded or its load cannot be read, the gate asks for a retry on a quiet host; it does not classify the test. The original fast gate remains red in each case.

A line per test, not a budget for the whole fast gate, was chosen because a budget says only that the fast gate is slow, never which test to move, and a growing suite would breach it a test at a time with nothing for the author of each to do. A line per test tells the author of a slow test that it is slow, the day it is written.

Flux stays in the fast gate. It checks every function of the core crate, which a change to the core can break as easily as the build, and it took 59 of the fast gate's 137 seconds on this host. In the slow gate a refinement error would surface only at landing, costing a whole landing turn and its queue wait to learn what the worker could have learnt before opening the PR.

The measurement the list was drawn from, and both gates' step times on this host, are in [the gate baseline](../gate-baseline.md).

## Limits and budgets

The two gates are the gate split, not a gate more: their limits add up to no more than `gate_seconds`, and their budgets to no more than `gate_budget_seconds` where it is declared, and a config that says otherwise is refused. Splitting raises neither. The fast gate's limit is wide, to leave a cold build room; its budget is narrow, as the point of it is to be quick. ADR-0024's rule is otherwise unchanged: the gate gone slow is still read off recorded landings of `gate_command` against `gate_budget_seconds`, since only the whole gate is recorded, and the part budgets are what `gate --fast` and `gate --slow` hold a run to.

"Slow gate", which named the condition of a gate whose landings exceed its budget, now names the second part; that condition is the **gate gone slow** in the glossary. Its issue label, `slow-gate`, and its keys in `plan` and `doctor` are unchanged, since they are read by installed versions and by people's filters.

## Considered options

- **The tests split by crate or kind: the core crate's, or those that start no process.** One filter, no list. Refused: a quick test of the CLI is still quick, and several of the core's start a shell; what makes a test cost the fast gate is its time, which is what the line measures.
- **A landing turn that runs the fast gate, then the slow gate.** Each part's limit would bound a landing. Adopted since, as rounds (ADR-0048), once the fleet is upgraded by hand before a repo lands with them. Refused here at first: an installed afkconvoy that does not know the block would land with the slow gate alone, without formatting, Flux or clippy, until it was upgraded; and the record would need two durations. The gate stays one command to the fleet (ADR-0004), and the block can be read by a landing later.
- **A fast gate that records green runs a landing believes.** Refused: a green fast gate has not run the slow tests, so it proves nothing a landing may rely on.
- **The line enforced at 2 seconds.** Refused: a loaded host would flip tests near the line from run to run.

## Consequences

- A worker learns of formatting, Flux, clippy, manual, build and quick-test failures before opening its PR, from a run that skips the slow tests.
- A landing still waits on the whole gate. Making the slow gate faster is sunfmin/control#234; running it once per round of candidates, not per candidate, is ADR-0048 (sunfmin/control#240).
- A slow test added without being marked fails the fast gate past 6 seconds, by name, and is checked alone before it is named slow; one between 2 and 6 seconds passes, and stays in the fast gate until its time is measured again.
- The list is measured on one host. Another host's fast gate may run a little slower or faster; the 6-second end is what holds it.
- An older afkconvoy skips the gates block: it can land and cut the version that reads it, and until then `gate --fast` is unknown to it.
