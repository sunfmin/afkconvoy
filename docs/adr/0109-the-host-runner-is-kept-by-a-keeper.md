---
status: accepted
---

# The host runner is kept by a keeper

Amends ADR-0023 (sunfmin/control#820). The host runner was nobody's child
once `run` exited, and nothing started it again. On Studio64 it ended four
times in two days, each time with no stop or panic line. Every fleet on the
host stood still until a person or the control session ran `afkconvoy run`.
The cause was outside this repo: guild's launchd stand-in for its tests
SIGKILLed every process of the host user working in `/`
(sunfmin/control#831), and the host runner works from `/`. Whatever the next
cause is, a host whose fleets stop on one lost process is fragile.

`run` now starts a **keeper** as the host runner: `afkconvoy host-runner`
alone, the same hidden command an older release starts when it moves onto a
newer one. The keeper holds nothing of the host: no lock, no socket, no
panel. It starts `afkconvoy host-runner --kept-by <its pid>` from the same
file, in a process group of its own, and waits for it. It starts the host
runner again after an **unexpected end**: one a signal ended, or one that
ended in an error after holding the host for as long as a start takes. The
first restart comes after a second, each one in a row after twice as long,
and none waits more than half a minute or so. Each is a line of the host
runner's log. A host runner that ends as told (`stop --all`, SIGTERM, a
move onto a newer afkconvoy, another holding the host) ends its keeper too.
So does one that fails as it starts, which the `run` waiting for it reports
as before. SIGTERM to the keeper is passed on. The lock says the keeper's
pid beside the host runner's, so the guard refuses a worker's `kill` of
either, and `doctor` names another account's host runner by the kept pid,
not its keeper's.

The keeper works in the cache directory, not in `/`: whatever ends every
process working in one directory does not end both.

Considered: a LaunchAgent with KeepAlive. launchd would also start the host
runner again after a reboot. But the keeper must not fight the host runner's
own moves onto newer releases, `stop --all` must stay a stop, and a test must
never register a job with the real launchd. A LaunchAgent is also missing on
a host whose user has no GUI session. The keeper needs none of these, and
the tests run it as it runs on a host.

Tests leave the host's own host runner alone in two more ways. A world's
panel takes any free port, never the host's 7070. And the candidates tests'
cleanup ends a gate's process group only while the gate is still in the
group it recorded. Pids wrap within minutes during a gate, and a reused pid
can be in the host runner's group, whose `gh` and `git` children run in it.
