---
status: accepted
---

# Admission holds a repo whose candidate chain cannot keep up

Amends ADR-0072 (sunfmin/control#914).

On 2026-11-14 the guild fleet on Studio64 had 24 workers at work on one
repo: admission decides each start by what the host and its accounts can
carry, and the host had room by its own measure. In three hours its `land`
turns answered 20 `speculative_conflict`, 5 `conflict`, 2 `gate_red` and 9
`merged`, and nothing landed in the last of them. Each conflict is a merge
again and a gate again — a full Rust build — so the load stood at 24–31 on
16 cores, and the Guild Hub on the same Mac was refused for CPU. More
workers on one repo than its candidate chain can take make each landing
slower, not faster.

## The candidate chain is a reading

Beside the host's readings, admission reads each checkout's candidate chain:
the `land` turns this host's tick log of the repo has in the last hour,
each by its `outcome` word. `conflict` and `speculative_conflict` are
conflicts; `merged` is a landing; nothing else counts. While the conflicts
outnumber the landings, and are three or more, admission starts no new
worker of that repo. One or two conflicts are the ordinary cost of parallel
work. It holds until the chain drains — the conflicts age out of the hour,
or the landings catch up — and the workers already running go on and land.
Another repo on the host is judged by its own chain.

The log is the reading because it is already there: every `land` writes its
outcome to it (`acting`), and a host's workers are what its admission
starts. A chain's turns on another host are that host's to count. A log
that cannot be read holds nothing back, as any reading does.

The hold is said as every reading is — the tick's line, `plan` and
`doctor`: `landing contention, N conflicts in the last hour against M
landing(s)` — and `doctor` reports the repo as a warning. It calls for no
watch: nothing of the host is to mend, and the chain drains on its own.

## Apart from what is in flight

A tick starts the frontier's issues whose named files overlap none the
claimed issues name before those that overlap, each side lowest number
first, after what was begun. A file is named in backticks, as the issues
here name them. A PR's diff is not read: that is a call per PR at every
tick, and the issue that a PR closes names its files already. An issue that
names none overlaps nothing.

No number bounds a fleet still (ADR-0072): the chain's own answers decide
when a repo has enough workers, and that changes with the work.
