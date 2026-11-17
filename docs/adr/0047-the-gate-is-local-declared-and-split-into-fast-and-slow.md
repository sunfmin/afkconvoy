---
status: accepted
---

# The gate is local, declared, and split into a fast gate and a slow gate

## The gate is the repo's own command, run locally

A repo's gate is its `gate_command`, run on the host. It is the only machine check: GitHub checks are never read, and an unbound repo has no review stage (a bound repo's workflow may have one, ADR-0008). A repo whose CI cannot be reproduced locally cannot use afkconvoy.

The landing train lands by pushing a commit the gate passed straight onto the base branch. So a start reads the base branch's protection and refuses a branch that requires status checks or reviews, restricts who may push, or is locked, naming the rule; a protection it cannot read is a warning, not a refusal.

To the fleet the gate is one command and nothing else: the fleet times the whole command and does not know its steps. Which step, test or function grew is for the gate to print, and this repo's `gate.sh` says each step's seconds as it ends.

## The gate limit is declared

A repo declares its gate limit, `gate_seconds`, in its config beside the gate command, the one place the number is written. A run that reaches it is ended and is red. A repo that declares none has an hour, and `doctor` says so. The limit is one number for every host: a repo sets it for its slowest host with workers running side by side, so it stops a gate grown several times slower, not one a fifth slower.

A recorded green gate run says how long the run took and on which host. When the last five recorded landings on the base branch on one host each took longer than the gate's warning budget, the **gate has gone slow**: the fleet opens one issue labelled `slow-gate` in its tracker, ready for the fleet to work, and opens no second while it is open. After it closes, only the landings made since count. Only the command's wall time counts, not the queue wait. The warning budget is `gate_budget_seconds` where the repo declares it, a fixed number below `gate_seconds`, grounded in a committed observation; where it declares none, 80% of the limit. The budget warns and the limit ends a run, so a gate can warn early without ending a cold build or a run on a contended host.

A worker given a slow gate can end it by deleting tests or raising the limit; the issue says neither is a fix. Raising the limit is a change to a committed file, seen in a PR like any other.

## The gate is split into a fast gate and a slow gate

A repo may declare its gate as two parts in a ```` ```yaml gates ```` block, each with its own command, limit (`seconds`) and warning budget (`budget_seconds`):

- **The fast gate**: what a worker passes before it opens or updates its PR. In this repo: formatting, Flux, clippy, the build of every test, and every test not marked slow.
- **The slow gate**: everything the fast gate leaves out. In this repo: every test marked slow.

Together they check exactly what `gate_command` checks. They are the gate split, not a gate more: their limits add up to no more than `gate_seconds`, and their budgets to no more than `gate_budget_seconds` where it is declared; a config that says otherwise is refused. `afkconvoy gate --fast` and `--slow` run one part alone under its own limit, say when a run passed its budget, and record nothing: a green fast gate has not run the slow tests and proves nothing a landing may rely on. A landing runs the fast gate per candidate and the slow gate per round, and records the whole gate only from both parts green on one tree (ADR-0083). The gate gone slow is still read off records of the whole gate. By hand, `./gate.sh fast`, `./gate.sh slow`, and `./gate.sh` for both.

Flux stays in the fast gate. A change to the core can break a refinement as easily as the build, and in the slow gate that error would surface only at landing, costing a whole landing and its queue wait to learn what the worker could have learnt before opening the PR.

### The line is drawn per test

A test that took 2 seconds or more on the host the line was drawn on is marked slow, by its binary and whole name, in `.config/slow-tests`. `gate.sh` makes one nextest filter of the list: the slow gate runs the tests it matches and the fast gate the rest, so no test runs in both or neither. A new test is fast until it is marked. A fast-gate test that starts more processes than `.config/process-bound` allows, counted as afkconvoy's own code starts them, also belongs in the slow gate, and fails the fast gate by name.

The fast gate runs under nextest's `fast` profile, which ends a test at 6 seconds, three times the line, and names one that passed past 2 seconds as SLOW. The line is not enforced at 2 seconds itself, because a loaded host would flip tests near it from run to run. A TIMEOUT or SLOW alone does not show a test is slow: while the host's one-minute load is below its logical CPU count, the gate checks each such test alone against the 2-second line.

- One that passes alone was a load fault. A TIMEOUT still leaves the fast gate red.
- One still past the line that the change under test added or modified fails the gate, naming `.config/slow-tests`.
- One still past the line that the change did not touch does not fail it. The gate prints it on a line of its own and names it in the one open issue labelled `slow-test` in the fleet's tracker, ready for the fleet to work; a second sighting adds a comment to that issue. The gate does this itself, as the repo's own script: the line is a fact of one test run, not of the recorded gate runs the fleet reads. Where the tracker cannot be reached, the printed line is all there is, and the gate is not red for it. "The change" is the diff from where the branch left the base branch, or for a candidate or round, from the merge's first parent; where the diff cannot be read, every test counts as touched.
- On a host loaded past its cores, or whose load cannot be read, no test is classified: each is flagged as unclassified, and a retry on a quiet host is asked for.

A line per test, not a budget for the whole fast gate, was chosen because a budget says only that the fast gate is slow, never which test to move, and a growing suite would breach it a test at a time with nothing for the author of each to do.

### A wall-clock bound says nothing of the host's load

A host is often loaded from outside the fleet, so a test's wall-clock bound must not turn load into red. Every such bound in the tests is one of two kinds:

- **A wait for an event** (a file appears, a line is printed, a transition is recorded). Its bound only catches a hang, and every test crate takes it from one place: `PATIENCE` in the `afkconvoy-patience` crate, within the limit for one test. A test waits through `until`, `until_true` or `deadline()`, and is held only as long as the event takes.
- **An assertion that something does not block.** The test proves the order, not a number of seconds: it holds the slow side until the fast side has answered, or reads the cause the program states.

A lower bound, and a short check that nothing happens within a moment, may stay: load only makes them pass more easily.

When the fast tests run red only on TIMEOUTs on a loaded host, `gate.sh` runs them again under the `fast-loaded` profile, whose limit for one test is the slow gate's, which only a hang reaches. Green there, the gate is green and each earlier TIMEOUT is flagged as unclassified; red there, the gate is red. A run with a FAIL, or with TIMEOUTs on a quiet host, is not run again.

## A worker's fast gate is its own kind in the exclusive queue

A fast gate run by `afkconvoy gate --fast` is a `fast_gate` in the host's exclusive queue, shown as the lane `fast`. `gate --fast` tells the command's `exclusive` so through `AFKCONVOY_GATE=fast`, which `exclusive` does not pass on, so a test the fast gate starts that itself runs `exclusive` is an ordinary command. A fast gate contends only with the host's room (ADR-0088): no slow gate, turn or other command holds a place it needs, and it holds none they need. Same-repo isolation (ADR-0037) does not apply between fast gates: each runs in its own worktree and build directory, and what they share is guarded already (the compiler cache's server, cargo's own locks, a remote they write nothing to). A landing's own fast gate of a candidate is part of the landing and stays a turn.

## A gate runs two tests to each core it is given

`afkconvoy exclusive` gives a gate its share of the host's cores as `NEXTEST_TEST_THREADS`. A test of the CLI mostly waits on the processes it starts, so `gate.sh` reads that number as cores and runs two tests to each, fast or slow. Given none, each nextest profile's own count holds. The check of one test alone keeps one thread. How much of a core a test takes is the repo's own fact, so the share `exclusive` gives every repo stays in cores.

## Considered options

- **A limit learned from past runs.** Refused: workers gate side by side on hosts of different speeds, and a limit that follows the runs follows them up. The warning budget is fixed for the same reason.
- **Only the declared limit, and no slow-gate issue.** Refused: a gate that slows a little with every PR is paid for by the PR that happens to reach the limit, whose worker did not cause it.
- **The gate's time written on the commit that lands.** Refused: that commit exists before the gate runs.
- **A patrol that looks for a slow gate.** Refused: whether the gate is slow is a comparison of numbers, and an unbound repo runs no patrol.
- **Steps the gate reports to the fleet.** Refused: every repo's gate would have to speak a form the fleet reads.
- **Tests split by crate or kind.** Refused: what makes a test cost the fast gate is its time, which the line measures.
- **Fast gates in the worker lanes, ahead of slow gates; or outside the exclusive queue.** Refused: priority never interrupts a running command, so a fast gate would wait out a landing; and the queue is where `doctor`, the panel and silence detection see what a worker waits on.
- **Raising the share `exclusive` gives, or a host's cap, to widen a gate.** Refused: the share is in cores, for builds and every repo's tests alike.

## Consequences

- A worker learns of formatting, Flux, clippy, build and quick-test failures before opening its PR, in about its own run time.
- A slow test added without being marked fails the fast gate past 6 seconds, by name, and is checked alone before it is named slow; one between 2 and 6 seconds passes and stays fast until measured again.
- A red run leaves no record, so the run that reached the limit leaves no time.

`gate.sh` opens and comments on the `slow-test` issue with `afkconvoy issue`, in the checkout's tracker: the host's local one unless the checkout names a GitHub repo (ADR-0091). Where the tracker cannot be reached, or the installed afkconvoy has no `issue`, the gate's printed line is all there is, and the gate is not red for it.
