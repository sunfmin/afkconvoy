---
status: accepted
---

# A gate turn merges, gates and joins on the train's green tip

A worker used to merge the train before waiting twenty to forty-five minutes for its gate. The train then gated the combined tree again: of thirteen observed landings, ten needed its own gate. One red tip also made many workers repeat the same failure. A turn now holds one command through merge, gate, recording and join, with the PR open before it asks. Red or a merge conflict frees the turn and joins nothing; conflicts stay in progress for the worker to resolve and commit.

This amends ADR-0004: one PR at a time gets a turn again, as the way into the landing train, never as a second way to land. “Landing turn” stays avoided. It amends ADR-0019 and replaces the train priority from sunfmin/control#104: the host has independent worker and train lanes, each FIFO across repos. `run --gate-lanes N` and `run --train-lanes N` (`AFKCONVOY_GATE_LANES` and `AFKCONVOY_TRAIN_LANES`) set their counts for every fleet on the host, 1 each by default; `default` restores 1. A count is a positive whole number and never a repo config key. Queue grants read the settings live, without restarting a fleet instance. Whatever the counts, one repo holds at most one turn at a time and at most one train gate; an earlier second request of a running repo waits while another repo may use an available lane. At the defaults a train gate can run beside one worker command, never waits behind one and never interrupts one. Light gates take turns in a line of their own repo. The contention bound of ADR-0031 covers the complete turn and counts waiters only in its lane.

A turn takes the green tip as it stands when the turn starts. A missing record is established in a separate checkout in the train lane; a red tip holds worker turns until the train worker repairs it. After acquiring its lane, a turn reads the tip and its record again. Its join is accepted only if the new merge commit's tree is the tested tree, so a join from another host cannot silently add untested work. The train lands on that record without another gate. Cuts, inspections and rejoined PRs follow the same rule.

Existing gate commands that wrap the build in `afkconvoy exclusive` inherit the owning command's lease. They do not ask for another place behind themselves. Config and check refuse gate commands that call `afkconvoy gate` or `land`, with the build-and-test command to set instead. A standalone gate before workflow review joins nothing; only the workflow's landing worker takes the joining turn.

## Host measurement

On FelixMBP14.local on 2026-10-26, commit `199483c91e4e585b97b8549d2b56eb540ad10658` ran the full afkconvoy gate in 334 seconds alone. With a train gate beside it, the worker gate took 416 seconds and the train gate took 483 seconds. All three runs were green and recorded; each passed 1,638 tests, with 10 skipped.

The host had one worker lane and one train lane. The train started one second before the worker; both were observed running in their respective lanes. An outer reservation excluded other heavy commands for each measurement. The two checkouts used separate target directories, with dependency artifacts copied before the runs; the train checkout rebuilt its workspace artifacts. The worker's extra 82 seconds is one observed comparison, not a repeated benchmark. It shows the cost of overlap on this host while confirming that the train gate starts without waiting for the worker lane.

## Replaced by ADR-0035

A turn now lands its gated merge commit directly on the base branch. The landing train, its worker and its lane have been removed; ADR-0035 describes certification, repair and migration of an older line.
