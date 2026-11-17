---
status: accepted, amended by ADR-0032 and ADR-0109
---

# One host runner holds every checkout of its host

A host that works many repos ran one `run` per repo: eleven processes on one laptop, each started from whichever terminal or session was at hand, each inheriting that session's environment, each to be found and restarted by hand after an upgrade, and each asking the others through a socket whose inspection ran and whose `exclusive` command was next. So a host now runs one **host runner**: one process holding a fleet instance for each checkout on the host's **checkout list**.

A fleet instance is still one repo's fleet — its own id, claims, heartbeat, tick log and ticks — and nothing of fleet state on GitHub changes: only what holds it does. ADR-0003 stands as one fleet per host per repo.

## How it is run

- `run [CHECKOUT]` — the current directory where none is named — adds the checkout's root to the checkout list, starts the host runner in the background where none runs, waits for that checkout's startup probe and first tick, answers one JSON object, and exits. A checkout the probe refuses is refused there, to whoever ran it. `stop [CHECKOUT]` takes it off the list, and its fleet instance stops as ADR-0003 says; `stop --all`, or SIGTERM, stops the host runner and keeps the list.
- Every command about one repo takes its checkout the same way: positional where the command has no positional argument of its own, `-C <dir>` where it has. A subdirectory names its checkout; a worker's worktree is refused; a second checkout of a repo already on the list is refused, naming the first.
- The checkout list and the **host settings** — the hot-worker limits, admission's shares, the panel's port — are one file on the host. The host runner converges on it: started again — after an upgrade, a crash, a reboot — it starts every checkout on it. A `run` that names a host setting changes it for the host.
- The host runner starts with the environment of a fresh login shell, never with that of the session that started it.
- A newer afkconvoy that finds an older host runner has it exit and starts itself on the same list.
- The host runner moves onto a newer afkconvoy installed where it was started from by itself, between ticks (ADR-0032): each fleet instance stops between two ticks, and the host runner starts the newer one's on the same list and lets the host go to it. It never moves onto an older release; one that does not start leaves it holding the host, and `doctor` names it.
- A fleet instance whose tick panics, whose loop ends in an error, or whose probe no longer passes as the host runner starts it again harms no other: it stays on the list, shown as broken with why, and is tried again after a backoff. A tick that fails is a line of its tick log, as before, and the loop goes on.
- The host runner keeps a log of its own — checkouts added and removed, fleet instances broken and back, upgrades and moves — beside each repo's tick log, which is unchanged.

## What becomes one

- **One socket**, the host runner's, for every command that speaks to it: adding a checkout, `stop`, `wake`, `exclusive`, `panel --open`, the version check. Each request names its repo. No fleet instance has a socket of its own, nor a repo a wake socket, and none asks another: the exclusive queue is state shared inside the process. No older afkconvoy is spoken to.
- **One panel**, always served by the host runner: the host first (admission, hot workers, the exclusive queue, the host kit), then what waits for a human across every repo with the fleet instances that are broken, then each repo as its panel showed it. `afkconvoy panel` without a host runner prints the same page from one rebuild of each checkout on the list, or of the one checkout named.

## Considered

- **One process that is one fleet instance across repos.** One id and one concurrency for every repo would undo the per-repo claims and takeover of ADR-0003 for nothing.
- **The first `run` becomes the host runner in the foreground.** Every repo would stop with whichever terminal happened to start first.
- **launchd.** Starts at login and restarts on a crash, but is a larger step. A keeper starts the host runner again instead (ADR-0109).
- **The checkout list in memory only.** Every upgrade would have every checkout added again by hand.

## Consequences

- Supersedes, of ADR-0002, that `run` is itself the long-running process: `run` adds a checkout and returns, and the host runner is the process that ticks. A wake is still a signal, sent through the host runner's socket.
- The release step of restarting every `run` becomes nothing beyond the upgrade itself: each host runner moves onto the release by itself between ticks.
- A personal control session that reads every repo needs nothing of afkconvoy but this: the checkout list, and `doctor` for every checkout, as JSON.
