---
status: accepted
---

# The gate crate may be depended on by another program

Amends ADR-0019, ADR-0024, ADR-0047, ADR-0049 and ADR-0057 (sunfmin/control#695).

How afkconvoy gates and lands — no CI, a fast gate and a slow gate (ADR-0047), a worker's fast gate in the fast lane (ADR-0049), heavy commands only through the host's exclusive queue (ADR-0019, ADR-0057), a run's evidence recorded on the remote (ADR-0024) — is wanted by another program of this host's user (Guild, sunfmin/control#693), which uses none of afkconvoy's team, role or workflow. It took long to get right, and a second copy would drift from the first: the gate has one home.

That home is its own crate, `crates/gate` (`afkconvoy-gate`), which `afkconvoy-core` depends on and re-exports, and the cli uses:

- `declared`: the gate as a repo declares it — the fast gate and the slow gate, each with its command, gate limit and gate budget, read off their block; the heavy commands, read off theirs; whether a gate command runs through the queue.
- `gate`: a tree, a gate command, a run of it and its green run on record, from either ref layout; a run judged green, red or unsettled from its output, a test ended at its bound (`TIMEOUT`) among it.
- `slow_gate`: a gate gone slow, judged from the recorded runs of the landings.
- `layout` and `record`: the two ref layouts records are kept under, and the one encoding of a record.
- `exclusive` and `shell`: the queue's tickets, lanes — worker, fast, slow and a light gate's repo lane — FIFO order within a lane and waiting time; and the shell reader by which a heavy command run bare is told, and a ticket's command is named. The command that enters the queue is the embedder's to name (`shell::QueueCommand`, sunfmin/control#749): a simple command running it is already queued, and a refusal gives the line to run behind it — afkconvoy's own is `afkconvoy exclusive`, Guild's daemon names its own.
- `holder`: the queue as one process of a host holds it, which that process embeds: afkconvoy's host runner today, another program's daemon later. What only the embedder knows — the time, the host's lanes and bound, tickets held outside the queue, what must happen before a ticket leaves, what is shown of a ticket — it says through `Host`. How an asker reaches the holder (the runner's socket, for afkconvoy) stays the embedder's.
- `run`: what a gate run writes on the remote, takes away, and reads back, through `Records`, which afkconvoy implements with its own remote (pushes tried again on the remote's own faults, bounded in time) and the crate with plain git (`Git`), with which `run::gate` runs a gate command for a program that has no runner of its own. A recorded run also times each test its output timed — nextest's wall time, and the CPU time a line `gate: cpu [ …s] <test>` says — on lines of its commit's message after the subject, which a reader of subjects alone never sees; `run::recorded_with_tests` reads them back (sunfmin/control#752).

Another program depends on it as a git dependency pinned to a revision of this repo; it is not published.

Its API stays free of fleet concepts: no team, role, stage, issue, claim, patrol, intake, tracker or GitHub type appears in it, and it does not depend on `afkconvoy-core`. What a fleet notes beside a fact of the gate is passed in neutrally: a ticket carries a `note` of its holder's type, which no rule of the queue reads and whose fields are the ticket's own on the wire (afkconvoy's `Note`: the issue of the worker, a landing turn's candidate); a record carries the `more` fields its program writes (afkconvoy's: the issue whose tree it was). The issue a slow gate is reported in, the red run kept for a work account and the comment a red run leaves on a PR stay in the core.

The crate keeps its Flux refinements (ADR-0007). `./verify.sh` checks it as the core's dependency, and the gate holds it, as the core and the harness crate, to no function taken on trust.

## Considered options

- **Let the other program depend on afkconvoy-core.** Refused: the queue's ticket named the issue of a worker and a candidate's validation, the gate's record named the issue, a slow gate the issue that reports it; every change to the fleet's types would reach the other program.
- **Copy the gate and queue modules into the other program.** Refused: the rules would have two homes, and drift.
- **Move the whole effect half too** — afkconvoy's gate runner with its cancellation, candidate process groups and retried pushes, and the runner socket. Not now: those are tied to afkconvoy's candidates, claims and host settings. The crate holds the rule of what a run records, and a plain runner beside it; the landing that uses them follows in sunfmin/control#696.
- **Publish the crate.** Not needed: a git dependency pinned by revision is reproducible, and nothing outside this host's user depends on it.

## Consequences

- A rule of the gate or of the queue is spelled only in the gate crate: afkconvoy-core re-exports it, and the cli calls it.
- A change to the crate's API is a change to another program's dependency: it stays neutral, and a fleet fact it needs is noted beside it or mapped by the core, not named in the crate.
- A test of the crate from outside embeds a holder, runs a repo's fast gate in the fast lane on a scratch repo and reads the recorded run back — its tree, duration and host — from another worktree; heavy commands wait their turn in the order they asked; and the crate depends on nothing of the fleet, nor names a fleet concept in a public item.
- Which tests are slow, and the line a fast gate's test is ended at, stay the repo's own gate script's and nextest's (`./gate.sh`, `.config/slow-tests`, `.config/nextest.toml`); the crate reads what the gate's output says of them.
- Candidates, rounds and landing followed in sunfmin/control#696 (ADR-0060), on the gate crate.
