---
status: accepted
---

# A look reads each source once

`doctor` is one look. Each tick of the host runner is its own. On a host of several checkouts the same facts were read again for every checkout, and again for every GitHub client a checkout built: the host's load, memory and disk, the login, the quota note, whether a tracker is public, orca's repo list, and the remote's refs. A repeated network read is the slow part of the look. The fleets were healthy (sunfmin/control#162).

One look reads each of these once and answers every later question in that look from what was read.

A look begins through one interface. A tick and a fleet-instance start each call it once. It resets every observation a look reads once: orca's repo list, whether orca is running, the quota note (including a `rate_limit` body), and each repo's HEAD. The look owns the read-once mechanics: one shared cell per reading and key, a second ask waiting on the first read, forgetting, and seeding a look handed to a test as a value. A source declares beside its read what it reads, the key it reads by, its lifetime — the look or the process — and which of its own writes forget it, so creating or removing a worktree forgets the worktree list with no call at the site that makes it. A further once-per-look reading is declared with that lifetime, so neither call site gains a line. `doctor` does not call it. Host readings stay behind the readings module. The login still stands for the life of the process.

The host's load, its memory and each volume's free disk are read once. Every checkout's admission and its hot-worker limits are computed from that reading.

The login stands for the life of the process: nobody changes `gh`'s login under a running runner. The quota note is observed at most once per look. `doctor` is one look, so once there. Each tick observes again, so the note stays the tick's own instead of whatever response headers arrived earlier in the process. An unreadable note does not fail a read. A look that has already called GitHub reports the note those responses carried and does not ask `rate_limit` for its own sake.

The remote is listed once per checkout when the fleet view opens: the branches, the fleet's refs and the version tags, as one moment, which is the moment the scan already reasons from. A claim's ref, a target, a recorded gate run, a candidate chain and a branch tip in that look are answered from the listing. A write still reads the remote again when it has to be certain of what it just pushed.

Orca's repo list is read once per look, and so is its worktree list. A tracker's public probe is asked once per tracker in that look.

`doctor`'s `host.watch` keeps the watch's state and the last round's report. `doctor --all` says each watch log line once, on that object, and not again in every checkout. Earlier rounds are answered with `--rounds`, and they remain where the watch already stores them. The host kit in a look keeps the identities, commands and research state a finding judges. The research notes stay on `plan` and on `afkconvoy kit`.

`doctor` still writes nothing but the fleet's cache, and makes no startup probe. While a startup probe that checked the kit's current launch commands still stands, `doctor` starts no harness. Otherwise a ready harness is started once; a harness whose first login answer reads logged out is asked once more, and the second answer stands for the look. A finished candidate — landed, red or cancelled — is one record of its phase, commit, issue, pull request, predecessor and waiting state, with no gate excerpt. A candidate that is gating now may still carry its excerpt. After a child exits, a pipe a grandchild still holds does not keep the look waiting. A finding keeps its kind, severity, wording and fix. `waiting_on_human` keeps its answer line.
