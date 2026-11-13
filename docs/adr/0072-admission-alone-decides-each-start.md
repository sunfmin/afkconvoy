---
status: accepted
---

# Admission alone decides each start

Amends ADR-0019 and ADR-0050, and supersedes ADR-0025 (sunfmin/control#824).

No number bounds how many workers a checkout's fleet instance holds at work at once: there is no concurrency to set, on the host or in the repo. A tick starts workers while admission admits, from what the host and the accounts can carry now.

## One reading per start

The reading a tick opens with admits its first start. Each start after one is admitted by a reading of its own, taken just before it, so that each start's load and memory are counted before the next, and the workers it started are counted on their account. The first reading that holds ends the tick's starts; what it held stays on the frontier. Workers already running, and the worker a declared verdict starts, are never held back.

## Its readings

Beside the host's load, memory and disk (ADR-0019, ADR-0050):

- **Usage windows.** The account a checkout's next worker starts on — its own harness, or the host kit's first — as the newest records of its workers at work on it say its windows (GLOSSARY: Usage window). Admission starts no worker while a window, at the rate it was used at since it began, would run out before it resets. Each running worker on the account is taken to use its share of that rate, and the new one as much again, so room is reserved for the workers already running. A window used up holds too. A harness that reports no window, or none of whose workers says one, is judged by the load, the memory and the disk alone.
- **The exclusive queue.** Admission starts no worker while the commands in the host's exclusive queue would take longer than 15 minutes to drain. The drain is each lane's commands one after another, as many at once as the lane has places: what is left of each running command and the whole of each waiting one, by the median of that command's last five runs, which the queue keeps. A command no run of which is kept counts for nothing. More workers would only wait behind the gate.

`plan`, `doctor` and the tick's line say which reading held a start back, with its numbers. Held back for one reason at every tick past the host's span calls for the host's watch, whichever reading it is.

## Retiring `--concurrency`

`run --concurrency` is accepted for one release and changes nothing: it is kept with the checkout's entry on the checkout list, and `doctor` reports it as `info`, with `--concurrency default`, which drops it. A repo config's `concurrency` is likewise read and ignored, and `doctor` reports it with the key to delete. After that release the flag is removed. The checkout list, `plan`, `checkouts` and the panel say no concurrency, and the tick's line says the claims held with no bound.
