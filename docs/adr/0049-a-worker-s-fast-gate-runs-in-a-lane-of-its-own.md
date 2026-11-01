---
status: accepted
---

# A worker's fast gate runs in a lane of its own

Amends ADR-0019 and ADR-0037 (sunfmin/control#237).

A worker passes the fast gate before it opens or updates its PR (ADR-0047), and only the fast gate: the slow gate is a landing's, run once per round (ADR-0048). But a fast gate is declared as `afkconvoy exclusive -- ./gate.sh fast`, so it waited in the host's exclusive queue like any heavy command: behind landing turns whose gate runs about 1,000 seconds, and, under ADR-0037's same-repo isolation, behind every other heavy command of its repo in any lane. In one observation 12 of 14 workers were waiting on background work, most of it their own checks queued behind other issues' turns; on a host of 3 worker lanes one lane ran and two stood free while every waiting request of this repo gave `same_repo` as its reason.

## Which commands queue where

The exclusive queue now has three kinds of lane:

- **The worker lanes**, as many as the host's `gate_lanes`: landing turns, with their fast and slow gates; serial barriers; a slow gate a worker runs alone, `afkconvoy gate --slow`; and every other heavy command run through `afkconvoy exclusive`: builds, tests, benchmarks. ADR-0019's queue and ADR-0037's same-repo isolation hold for them as before.
- **A repo's own lane**, one place, for a light gate (ADR-0037), as before.
- **The fast lane**, as many places as the host's `fast_lanes`: a fast gate a worker runs alone, `afkconvoy gate --fast`, and nothing else.

A fast gate is known by what runs it: `gate --fast` tells the `exclusive` of the fast gate's command that it runs the fast gate (`AFKCONVOY_GATE=fast`), and its ticket says so. `exclusive` does not pass that on to the command it runs, so a test the fast gate starts, which itself runs `exclusive`, is an ordinary command.

A fast gate contends only with other fast gates, for the fast lane's places. No slow gate, turn or other command holds a place it needs, running or queued, and it holds none they need. A landing turn's own fast gate is part of the turn and keeps the turn's worker lane: the turn holds that lane from its merge to its landing (ADR-0034), and a candidate's fast gate is the turn's validation, not a worker's check.

## Fast gates are not held apart by repo

Same-repo isolation does not apply between fast gates. Each runs in its own worktree, with its own build directory, so two of one repo build and test side by side as two of different repos do; none waits with `same_repo` as its reason, and none takes the repo lease an ordinary command of its repo holds. What they share is guarded already, and nothing more is guarded for them:

- the compiler cache's server (sccache), which serves concurrent builds by design;
- cargo's registry and git checkouts under `~/.cargo`, which cargo locks itself;
- the remote, to which `gate --fast` writes nothing, as it records nothing.

## The bound is the host's

Fast gates still fill a host: each builds the workspace and runs Flux, clippy and the tests not marked slow. So the fast lane is bounded, by a host setting beside `gate_lanes`: `run --fast-lanes N` says it live across the host, and `default` restores it, 2. Two by default, as the fast gate is a small part of the whole (137 of about 1,000 seconds on an 8-core host), so two of them beside one landing gate add less than a second landing gate would; a host that is larger or smaller says its own, as it says its worker lanes. The bound is held as the worker lanes' is: in the host runner's queue, and by kernel-held slots that outlive a host runner replaced.

## Seen

`afkconvoy queue`, doctor's `host.exclusive` and the panel's command lanes show the fast lane as its own lane, `fast`, with its capacity, used and free places beside the worker lanes', and each request's `kind`: `fast_gate`, `slow_gate`, `turn` or `command`. A fast gate waiting for a place gives `fast_capacity` as its reason.

## Considered options

- **Fast gates in the worker lanes, ahead of slow gates.** Refused: priority never interrupts a running command (ADR-0019), so a fast gate would still wait out a running landing turn of up to an hour, and a full window of turns would hold every worker's check.
- **One fast lane per repo.** Refused: it bounds nothing across a host of many repos, and keeps the same-repo wait this ADR exists to remove.
- **No bound on fast gates.** Refused: five workers of one repo finishing at once would run five workspace builds and Flux runs together, past the load at which admission holds new workers back.
- **Fast gates outside the exclusive queue.** Refused: the queue is where `doctor`, the panel and silence detection see what a worker waits on (ADR-0019), and its slots are what bound a host across its fleets and host-runner restarts.

## Consequences

- A worker learns whether its fast gate passes in about its own run time, not after the turns queued ahead of it; it then opens or updates its PR.
- A host's load may rise by up to `fast_lanes` fast gates beside its worker lanes' commands. Admission and the hot-worker sampler (ADR-0019) still watch it.
- A host runner older than this one schedules a fast gate's ticket as an ordinary heavy command: the fast lane begins once the host runner is replaced by this release.
- `afkconvoy gate --slow` run by a worker stays in the worker lanes, behind the turns: it is what a landing runs, and no worker needs it before its PR.
