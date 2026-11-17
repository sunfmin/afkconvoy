---
status: accepted
---

# A host runs one fleet per repo, and takes its own host's claims at start

afk-fleet takes a dead fleet's claims only after its lease lapses, or when a human runs a takeover and picks the instance. The common death is local — the process was killed, the quota ran out — and the human restarting it on the same machine is the proof. So a fleet instance that starts takes every claim stamped with its own host, unconditionally, whatever the heartbeat says.

That makes "one fleet per host per repo" a rule: a second `afkconvoy run` on the same host and repo takes the first one's claims while it is still alive. Checking that the other process is really dead was considered and rejected in favour of the simpler rule.

## Consequences

- By default there is no drain: plain `stop` is exiting; the claims stay on the remote, the workers keep working, and the next start on this host takes them back and continues them. An orphaned claim is still recovered by continuation, and never released.
- `stop --release` is the drain an operator names: it releases the instance's claims and ends their workers, keeping any worktree that holds work the remote does not. Its rules are in `afkconvoy stop --help`.
- A heartbeat outlives its fleet instance only while that instance holds claims. The start that takes those claims removes the heartbeat they were held under — it is the same proof — and with it this host's other heartbeats that are past the lease and hold nothing. Another host's is no proof of anything here: it is removed only once it is old and holds nothing.
- A claim is one whatever it is on: an issue, a patrol's run or a cut. A start takes all three, as well as legacy proposal claims, which it retires (ADR-0039). An issue taken is continued; a run taken is ended by the next tick once its worker is gone or has stopped, as any run of the instance's own.
- Another host waits for the lease, or uses the explicit `takeover` command, which is kept for that case only.
