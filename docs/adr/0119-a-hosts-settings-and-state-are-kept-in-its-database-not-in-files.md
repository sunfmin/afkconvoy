---
status: accepted
---

# A host's settings and state are kept in its database, not in files

Decided by Felix in the control session on 2026-11-17, about 11:35 JST (sunfmin/control#1328, now local/macnomac-studio#11):

> 本地的那些配置项也都放到 SQLite 数据库里面。不放到文件里了。

(afkconvoy's host-local settings and state go into an SQLite database, not into files.)

ADR-0117 made `host.db`, in the data directory, the home of the host's state, and moved the checkout list, the host settings, worker health, the account book and the watch's and the asking session's state into it. Files were still the home of the host kit (`kit.json` in the config directory, with `kit.lock` and `kit.refresh`), of the GitHub quota observations (`github-quota/`), and of what the gate and cut needs' checks left (`gate-needs/`, `cut-needs/`, `gate-needs-install/`, `cut-needs-install/`, `cut-held/`). A hand edit of one of those files, a lock beside it and a temporary file renamed over it were three homes for one fact and three ways to lose a write.

## Decision

- **Every host setting and every piece of host state is a table of `host.db`.** The host kit and when it was last written and a refresh last tried (`kit`, `kit_refresh`); each login's quota observations, credential identities and the readings its waiting processes share (`github_quota`, `github_identities`, `shared_readings`); each need's kept stderr and install record (`need_stderr`, `need_installs`). Every read-modify-write is one immediate transaction: the transaction replaces `kit.lock`, `kit.refresh`'s lock and `observations.lock`.
- **The schema has a version, and each change is a migration** (`PRAGMA user_version`, one entry of `host_db::MIGRATIONS` each). The first open of a newer release migrates forward once, in one transaction; an older release that finds a newer schema refuses, saying so.
- **Each old file is taken in once** by the first process of this release that reads its store, in the transaction that records its digest, and renamed `<name>.migrated` beside where it was — ADR-0117's convention, kept so one suffix means one thing — once no host runner of another build holds the host: a free host, a runner of a newer release, or one started from this very executable (its lock says it, as `exe`). A runner of the same version from another executable is another build: a dev build's tests, whose in-process library then still read the real config directory, moved the host's real `kit.json` aside under the installed runner of the same version and stopped every fleet of the host (2026-11-17). The test world now gives its process its own config directory, as it gives its cache and data directories, and a unit test has one of its own. Renamed back, an older release reads it again. A second start takes in nothing. The quota observations are not taken in: each expires within the hour, and losing one costs one rejected request; their directory is moved aside the same way.
- **A human changes them through commands.** Host settings through `run --<setting>`; the checkout list through `run` and `stop`; the host kit through `kit`, `kit --command <harness>=<command>` (what a hand edit of a harness's command did), `kit --remove`, `kit --research` and `switch`. A need's stderr is printed by `doctor --stderr <name>`, the name every refusal, finding and `plan` gives it.
- **The database is its user's alone** (mode 0600, and so its journal and its backups): it now keeps the needs' stderr, which may hold what only the host's user may read.

## What stays a file, and why

The rule is: the database holds settings and state; the kernel holds only liveness; documents stay documents.

- **Kernel locks that say a process still holds something.** The worker slots (`worker-slots/`, ADR-0088): a gate's descendants inherit the lock, so a replaced host runner counts the place held until the last of them exits — no row can know that. Worker health's observation lease (`worker-health.lock`), held across work that runs processes and asks Orca, which no transaction may span (ADR-0117). A shared reading's lock (`shared-readings/<host>/<login>/`), held while GitHub is read so the other waiters wait on that one read (sunfmin/control#1111); what it read is a row. A crash releases each of them; a lease row would need an expiry that is either too short for a slow gate or too long after a crash. The issue asked that no `.lock` file be left; these three are left on purpose, and guard nothing but liveness.
- **Logs.** The cut's red gate output (`cut-failed/`), the host runner's log and the tick logs: whole outputs a human or a watch opens as files.
- **What the issue names as out of scope**: the repo's own declaration, worktrees, candidate worktrees and build caches, the host runner's socket. And `agent-setup.json`, the record of what the agent setup gave each harness, beside the host kit, which no issue names yet.

## Considered options

- **Emulate every lock with a lease row (pid and expiry) and a single flight by a claim row and polling.** Refused: it loses the inherited-descriptor liveness ADR-0088 rests on, brings back the waiting #1111 removed, and needs a crash expiry that is wrong one way or the other.
- **Land the host kit alone and leave the rest for later issues.** Refused: the quota notes and the needs' records are plain state, and the owner's decision is about all of them.
- **Keep the suffix the issue named, `.imported`.** Not taken: ADR-0117 already renames what it took in to `.migrated`, and two suffixes for one act would be two names for one thing.

## Consequences

- `kit.json` is no longer read; a hand edit of it after the move changes nothing. `kit --command` is the edit.
- `plan`'s `need_installs[].stderr`, a refusal's and a `cut-need` finding's `stderr` are names to give `doctor --stderr`, not paths.
- Tests read and write these through the store (`host_db::keep`, `kept`) or the database, never through the old files.
