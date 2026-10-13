# A host runs one fleet per repo, and takes its own host's claims at start

afk-fleet takes a dead fleet's claims only after its lease lapses, or when a human runs a takeover and picks the instance. The common death is local — the process was killed, the quota ran out — and the human restarting it on the same machine is the proof. So a fleet instance that starts takes every claim stamped with its own host, unconditionally, whatever the heartbeat says.

That makes "one fleet per host per repo" a rule: a second `afkconvoy run` on the same host and repo takes the first one's claims while it is still alive. Checking that the other process is really dead was considered and rejected in favour of the simpler rule.

## Consequences

- There is no drain. Stopping is exiting; the claims stay on the remote and the next start on this host takes them back and continues them.
- A claim is one whatever it is on: an issue, a patrol's run, the proposal of a precedent. A start takes all three. An issue taken is continued; a run taken is ended by the next tick once its worker is gone or has stopped, as any run of the instance's own.
- Another host waits for the lease, or uses the explicit `takeover` command, which is kept for that case only.
