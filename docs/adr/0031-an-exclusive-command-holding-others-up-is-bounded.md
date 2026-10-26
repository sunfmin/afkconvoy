---
status: accepted
---

# An exclusive command holding others up is bounded

A benchmark held the exclusive queue for over an hour, with eleven commands behind it, including gates and the landing train of several fleets (sunfmin/control#100). A running command is now ended after it has held others up for a host setting's bound. Time with nobody waiting does not count; gaps without waiters pause the count, and changing waiters does not reset it. A long measurement takes the queue once per step needing a quiet host, so gates and the landing train can run between steps.

The default is 1,200 seconds. On FelixMBP14.local, the retained tick logs read on 26 October 2026 contained 203 exclusive gate runs: the longest was 605 seconds (`bizkernel-c2f3f0f3f8e2c073.jsonl`, epoch 1792926595, exit 100). The repo's earlier measurements in `docs/agents/afkconvoy.md` report a longest gate of 734 seconds, which is also below this default. Of five retained `packaging/release.sh build` runs, the longest was 40 seconds (`inspectvideo-1c9d77a78f0f4c3a.jsonl`, epoch 1792922456, exit 0); the afkconvoy 0.5.20 cut log also shows its two release builds taking 20.15 and 18.98 seconds. These are running times, excluding queue waits. The default leaves headroom above the longest measured gate and release build without allowing the twelve six-minute benchmark steps to become one hold.

`run --exclusive-seconds N` (or `AFKCONVOY_EXCLUSIVE_SECONDS`) changes the host setting for every fleet, including a command already running. `default` restores it, and zero ends a command as soon as another waits. It is shown beside the other host settings. The host runner counts contention with a monotonic clock at every queue change, and asks the running command to end at the bound; its ticket stays held until cleanup ends its descendants and reaps the command. Commands keep their inherited process group so a gate's own timeout still ends the whole gate. A command running without a host runner remains unbounded, as there is no exclusive queue to hold up (ADR-0023).

Exit 124 is reserved for this outcome, distinct from the command's ordinary status, the queue's 125 and a command that cannot start's 127. A command itself exiting 124 is reported as 1, keeping the bound distinguishable. One stderr line tells the worker to split the work, and one tick-log line says which command ended, its runtime and how many waited. `doctor` reports each occurrence retained from the last day, so a bound too low for an ordinary gate shows.

A wall-clock limit would end a long command even on an otherwise idle host. Resetting the count when a waiter leaves would let changing waiters extend a hold forever. A repo setting would let one repo hold every other fleet up. None fits the host's shared queue.
