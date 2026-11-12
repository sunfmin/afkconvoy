---
status: accepted
---

# Memory pressure another account holds starts no watch

Amends ADR-0029 and ADR-0035 (sunfmin/control#740).

On a 16 GiB host, another macOS account's Colima VM and iOS Simulator held about 20 GiB while this account's fleets held no worker. Memory pressure stayed at level 2, dipped below it for five minutes between rises, and each rise was a new episode that started a new watch. A watch keeps to this account's fleet-owned processes, so each round could only report again what the last had already left with the operator.

A look reads the process table once, as `doctor`'s `host.other_accounts` already does, and sums each account's resident memory. The pressure's largest holder is another user account — a uid from 500 up to 60000, not this one — when it holds more than every other account and more than this account's processes together; otherwise it is this account's. While it is another account's and the samplers of this host's fleets watch no worker, the pressure episode starts no watch: it is an `info` finding, `under memory pressure from another account`, that names the account, its uid, its resident memory and its processes. `plan` says it too, as `memory_pressure`.

While a fleet worker runs, the same pressure still starts a watch: there is something it may act on. Once a watch has reported a pressure episode whose largest holder was another account, that account is left with the operator: a later episode with the same largest holder starts no watch, and is said as the same `info` finding, until the operator settles a question the watch left on a pressure episode in the asking session, or a look finds the largest holder changed — another account, or this account again, its fleet workers among its processes. The record is kept with the resource episodes and survives a host runner replacement.

Admission is unchanged: it still holds new workers back under the memory it reads, whoever holds it. Pressure episodes, their span and their five minutes of recovery are unchanged; only whether one starts a watch is.
