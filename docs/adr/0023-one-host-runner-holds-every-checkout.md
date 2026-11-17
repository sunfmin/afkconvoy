---
status: accepted
---

# One host runner holds every checkout of its host

A host that works many repos ran one `run` per repo: many processes on one machine, each started from whichever terminal or session was at hand, each inheriting that session's environment, each to be found and restarted by hand after an upgrade, and each asking the others through a socket. So a host runs one **host runner**: one process holding a fleet instance for each checkout on the host's **checkout list**.

A fleet instance is still one repo's fleet — its own id, claims, heartbeat, tick log and ticks. ADR-0003 stands as one fleet per host per repo.

## How it is run

- `run [CHECKOUT]` adds the checkout's root to the checkout list, starts the host runner in the background where none runs, waits for that checkout's startup probe and first tick, answers one JSON object, and exits. A checkout the probe refuses is refused there, to whoever ran it. `stop [CHECKOUT]` takes it off the list; `stop --all`, or SIGTERM, stops the host runner and keeps the list.
- Every command about one repo takes its checkout the same way: positional where the command has no positional argument of its own, `-C <dir>` where it has. A subdirectory names its checkout; a worker's worktree is refused; a second checkout of a repo already on the list is refused, naming the first.
- The checkout list and the **host settings** are kept on the host. The host runner converges on them: started again — after an upgrade, a crash, a reboot — it starts every checkout on the list. A `run` that names a host setting changes it for the host.
- The host runner starts with the environment of a fresh login shell, never with that of the session that started it.
- A fleet instance whose tick panics, whose loop ends in an error, or whose probe no longer passes harms no other: it stays on the list, shown as broken with why, and is tried again after a backoff.
- The host runner keeps a log of its own — checkouts added and removed, fleet instances broken and back, moves and restarts — beside each repo's tick log.

## What becomes one

- **One socket**, the host runner's, for every command that speaks to it: adding a checkout, `stop`, `wake`, `exclusive`, `panel --open`, the version check. Each request names its repo. No fleet instance has a socket of its own, and none asks another: the exclusive queue is state shared inside the process.
- **One panel**, served by the host runner: the host first, then what waits for a human across every repo with the fleet instances that are broken, then each repo. `afkconvoy panel` without a host runner prints the same page from one rebuild of each checkout on the list.

## It moves onto a newer release between ticks

Once the installed file answers a newer release, the host runner moves onto it by itself; an upgrade needs no restart by hand. It does not wait for every fleet to be idle: a busy host almost never is, so installed fixes would never reach the work they were meant to fix. Workers already call the installed binary, and a start already takes this host's claims and continues their workers, their turns and a cut, so waiting protects none of them.

It pauses new ticks and new exclusive commands, lets active ticks and the running exclusive command finish, then stops each fleet instance between ticks and starts the replacement on the same checkout list and host settings. No worker is stopped and no claim released; a queued command rejoins with its original ask time. It never interrupts a tick or an exclusive command to force the move. A wait for that moment is bounded: past it, the old host runner resumes, `doctor` names what held it, and the move is tried again later. It never moves onto an older release. A replacement that fails to start leaves the old runner holding the host. A newer afkconvoy that finds an older host runner has it exit and starts itself on the same list.

## A keeper starts it again

A host whose fleets all stop on one lost process is fragile: the host runner is nobody's child once `run` exits. So `run` starts a **keeper** (`afkconvoy host-runner`), which holds nothing of the host — no lock, no socket, no panel — and starts the host runner (`afkconvoy host-runner --kept-by <its pid>`) from the same file in a process group of its own, and waits for it. After an **unexpected end** — one a signal ended, or one that ended in an error after holding the host for as long as a start takes — the keeper starts it again with a growing, capped backoff, each restart a line of the host runner's log. A host runner that ends as told (`stop --all`, SIGTERM, a move onto a newer release, another holding the host), or that fails as it starts, ends its keeper too. SIGTERM to the keeper is passed on. The lock records the keeper's pid beside the host runner's, so the guard refuses a worker's `kill` of either. The keeper works in the cache directory, not in `/`, so whatever ends every process working in one directory does not end both.

## Considered

- **One process that is one fleet instance across repos.** One id and one concurrency for every repo would undo the per-repo claims and takeover of ADR-0003 for nothing.
- **The first `run` becomes the host runner in the foreground.** Every repo would stop with whichever terminal happened to start first.
- **Move only once every fleet is idle.** A busy host never moves.
- **A LaunchAgent with KeepAlive.** launchd would also start the host runner after a reboot. But it would fight the host runner's own moves onto newer releases, `stop --all` must stay a stop, a test must never register a job with the real launchd, and a LaunchAgent is missing on a host whose user has no GUI session. The keeper needs none of these, and tests run it as it runs on a host.
- **The checkout list in memory only.** Every upgrade would have every checkout added again by hand.

## Consequences

- `run` is not itself the long-running process: it adds a checkout and returns, and the host runner ticks. A wake is a signal sent through the host runner's socket.
- A personal control session that reads every repo needs nothing of afkconvoy but the checkout list, and `doctor` for every checkout, as JSON.
