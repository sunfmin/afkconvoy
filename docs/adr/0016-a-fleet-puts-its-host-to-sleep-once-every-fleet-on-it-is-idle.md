---
status: superseded by ADR-0050
---

# A fleet puts its host to sleep once every fleet on it is idle

A host left to work a backlog unattended stays awake long after the work is done. So `run` puts its host to sleep — a host sleep — by default, and `run --no-host-sleep` keeps it awake. It does so only within the host's **sleep hours** — from 00:00 to 06:00 of its local time, unless it says others — and when every fleet instance on the host has been idle for ten minutes, each has just ticked once more and still found nothing, nobody has touched the host's keyboard or mouse for ten minutes, and the instance that puts it to sleep has worked — held a claim, or had something on its landing train — since the host last woke. Only macOS is supported: the sleep is `pmset sleepnow`, and `run` refuses to start elsewhere unless it is told `--no-host-sleep`.

The fleet instances of a host find one another through a socket each listens on in a directory of the cache, named for its instance. Asked, one answers with its repo, its version, whether it was told `--no-host-sleep`, since when it has been idle and when it last worked; asked to tick first, it ticks and then answers. A socket nobody listens on is what a dead instance left, and is removed.

## Considered

- **Turned on with a flag.** A fleet is run to be left alone; the operator who wants the host kept awake is the one who knows it, and says so once.
- **A host-level command beside `run`**, watching every fleet. One more process to keep running, for a decision every `run` can make on its own.
- **A file per instance, rewritten each tick, read by the others.** Simpler, but what it says is as old as the last tick — up to the idle wait between ticks — and whether its writer is alive must be found some other way. A socket answers now, and one nobody listens on is a dead instance.
- **Disarming the instance that put the host to sleep.** Another instance would put the host back to sleep the moment it woke. That an instance worked since the host last woke is read from the host itself, needs nothing kept, and covers a host woken from a sleep a human chose.
- **Trusting each instance's last tick.** An issue filed since would wait until the host is woken by hand. Every instance ticks once more first.
- **Sleeping at any hour.** A host left idle in the afternoon was put to sleep, and was then out of reach — from a phone, among others — until someone woke it by hand. Only within the sleep hours is nobody expected to reach it.
- **Waking the host as the sleep hours end.** The fleet would have to schedule a wake it cannot keep on every host; a host put to sleep at 05:50 sleeps until something else wakes it.

## Consequences

- An instance that does not answer within two seconds, or does not answer within two minutes once asked to tick, is not idle: the host is not put to sleep this time.
- Any instance told `--no-host-sleep` keeps its host awake for every instance on it.
- An instance of a version without the socket is not seen, and does not keep its host awake.
- An instance that has been idle ten minutes checks then, whether or not a tick is due, and at any hour: a check outside the sleep hours keeps the host awake and says so, naming them, and the next is as they start. Within them, the conditions are as above: a host whose fleets have been idle since 23:00 is put to sleep at 00:00.
- The sleep hours are a host setting (ADR-0023): `run --host-sleep-hours 22:00-06:00` changes them for every fleet instance on the host, and `--host-sleep-hours default` changes them back. Hours that span midnight work, `00:00-24:00` is any hour, and hours that cover nothing are refused.
- A host woken by hand is not put back to sleep by the fleet until some instance on it has worked again.
- `run` says at its start that it will put its host to sleep, and within which sleep hours, and says on stdout and in the tick log when it does. `doctor` says what kept the host awake the last time it was checked — outside the sleep hours among them.
