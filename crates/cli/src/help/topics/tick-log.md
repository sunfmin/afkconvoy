What every command that acts does as it ends, whoever runs it: the line it adds to the tick log, and the fleet instance it wakes. A command's own `--help` says, under "What it logs", that it is one.

## The line it logs

As it acts it appends one line to the tick log of this host for this repo — the command, what it acted on, who ran it (a worker or the operator) and how it came out, or that it failed — whether a `run` is there or not:

```
park #46 (operator): blocked by #99
verdict #121 (worker): FAILED: issue #121 is closed
```

A worker's `gate` and `exclusive` lines name its issue, in a bound repo or an unbound one. A line names no issue when its command acts on none and is run by no issue's worker: a watch or a human, for example.

## The fleet instance it wakes

Then it wakes this host's fleet instance of this repo, asking the host runner on its socket, naming its checkout: the fleet instance logs `woken by park #46` and starts its next tick, or follows the one in progress with it. It waits {const:WAKE_ANSWER_SECONDS} seconds at most for the answer, which says whether it woke one, as "woke". With no host runner, or none holding a fleet instance of this repo, it does nothing more.

Both are soft: a command acts the same whether either can be done, and with no fleet instance to wake its line is in the log all the same.
