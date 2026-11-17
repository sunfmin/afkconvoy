---
status: accepted
---

# The host reading is a crate of its own

Amends ADR-0059 and ADR-0088 (sunfmin/control#1259).

Since ADR-0088 a holder of the exclusive queue says its host's reading (`Host::reading`), and a holder that says none runs one command at a time. guildd embeds the gate crate's holder (sunfmin/control#1243) and must read its host as afkconvoy does, from the same code: a second reader of the kernel's CPU ticks would drift from the first. The reader was in the cli crate, which guildd cannot depend on.

## Decision

How busy a host is has its own crate, `crates/host-reading` (`afkconvoy-host-reading`), beside the gate crate, on the same terms (ADR-0059): another program depends on it by a git dependency pinned to a revision, and its API names no fleet concept. It depends on the gate crate, for the `exclusive::Reading` it says, and on libc; never on `afkconvoy-core`. It holds:

- `cpu`: the kernel's CPU ticks (`host_statistics` on macOS, `/proc/stat` elsewhere), the busy share between two readings (`busy`, which admission reads), and a `Minute` of them, sampled at most each second, as the queue's holder keeps them; with the test world's stand-in, `AFKCONVOY_FAKE_CPU_BUSY`.
- `load`, `memory_free` and `cores`: the 1-minute load average, the share of memory free, the host's logical cores. Admission reads its host by them too.
- `reading(minute, now, floor)`: the ticks over the last minute, or the load over the cores where those cannot be read, and the memory free where it is under `floor`. The floor is the caller's: afkconvoy's from its host settings (`--admit-memory`), Guild's from its own.

The cli's host runner reads its host through `reading`; there is one reader of the kernel's ticks in afkconvoy.

## Considered options

- **In the gate crate, beside `exclusive::Reading`.** Refused: the gate crate is checked by Flux with no function taken on trust (ADR-0059), and the reader is foreign calls to the kernel and its processes — what a refinement says nothing of.
- **Leave it in the cli, and have Guild read its own host.** Refused: two readers of the same kernel numbers, which a fix to one would leave the other without.

## Consequences

- A change to how a host is read is made once, in this crate, and reaches afkconvoy's admission, its queue and guildd's queue.
- The crate runs `sysctl` by name, as `PATH` finds it, so a test world's stand-in `sysctl` still says the load and memory it was given.
