---
status: accepted
---

# A fleet never puts its host to sleep

Supersedes ADR-0016, and of ADR-0033 its host-sleep question.

ADR-0016 had a fleet put its host to sleep (`pmset sleepnow`) once every fleet instance on it had been idle ten minutes within the host's sleep hours, unless the host settings kept it awake. It did so by default: a new host put itself to sleep until someone said `--no-host-sleep`. But a fleet sees only its own work. A host that also serves a person, a watch, another session or a download was put to sleep by a tool that could not see them, and a host asleep is out of reach until something else wakes it. The operator decided to take host sleep out of afkconvoy, so that no fleet can put a host to sleep by mistake.

## Decision

- No fleet instance puts its host to sleep. afkconvoy runs no `pmset`, reads neither `kern.waketime` nor the keyboard's and mouse's idle time, and has no sleep hours.
- `run` takes none of `--no-host-sleep`, `--host-sleep` and `--host-sleep-hours`, nor their variables: a line that names one is refused as it refuses any flag it does not know. Off macOS, `run` starts with nothing said of host sleep.
- A first start asks nothing of host sleep: the question ADR-0033 added is gone, and with it the refusal of a host with no host settings. A checkout off the checkout list is still refused until its tracker is said.
- Host settings that say `no_host_sleep` or `host_sleep_hours` keep them, as they keep any key another afkconvoy says: an older afkconvoy that reads the same file still finds the host kept awake. Nothing in this one reads them.
- How each fleet instance stands is still held by its host runner (ADR-0023), for a move onto a newer release to read (ADR-0032): starting, in a tick, at work or quiet. What only host sleep read of it — since when it was idle, when it last worked, whether it kept the host awake — is gone, and so is waking the other fleet instances to tick once more before a check.
- The tick log has no `host sleep:` line, `doctor` answers no `fleet.host_sleep` and no `host_sleep` event, and the panel no host-sleep paragraph. A line an older afkconvoy wrote of one is read past, as any line no longer known is. A fleet instance still notices its host waking from a sleep, whatever put it to sleep: `host woke:`, the `host_wake` event.

## Considered options

- **Host sleep off by default, kept as an option.** It would keep the whole mechanism — the checks, the reads of the host, the instances waking one another, the settings and the first-start question — for a choice no host here makes, and an option that can put a host to sleep can still be turned on by mistake.

## Consequences

- An idle host stays awake until the operating system's own settings, or a person, put it to sleep.
- Until a host runner moves onto this version, the older one it runs still puts its host to sleep where its host settings let it: `run --no-host-sleep`, said to that older version, keeps the host awake meanwhile.
