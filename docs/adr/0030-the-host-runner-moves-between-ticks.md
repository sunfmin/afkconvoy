---
status: accepted
---

# The host runner moves onto a newer release between ticks

Amends ADR-0023's rule that a host runner moves only once all its fleets are idle (sunfmin/control#88). A busy host almost never reaches that point, so installed fixes could not reach the work they were meant to fix. Workers already call the installed binary, and a start already takes this host's claims and continues their workers, the landing train and a cut: waiting for idle protects none of them.

Once the installed file answers a newer release, the host runner pauses new ticks and new exclusive commands. Active ticks finish, including their inspection work and log lines, and the running exclusive command ends. It then stops each fleet instance between ticks and starts the replacement on the same checkout list and host settings. A queued command does not hold it back: it rejoins with its original ask time. No worker is stopped, no claim released, and nothing is removed from the landing train. An inspection keeps its subject, role and original start time beside its checkout; the replacement resumes monitoring the same terminal and holds the host's inspection turn for it. The tick log's `moved` line names the claims and workers, train PRs, cut and inspection carried over.

The wait for that moment is bounded at ten minutes. If a tick, a fleet instance still starting or a running exclusive command holds it longer, the old host runner resumes ticks and queue starts, and `doctor` gives a finding naming what held it in `host.installed`. It tries again once the installed file changes. It never interrupts a tick or an exclusive command to force the move. A replacement that fails to start leaves the old runner holding the host again, with the existing finding and retry rule. Host sleep still requires idle fleets, as ADR-0016 says.
