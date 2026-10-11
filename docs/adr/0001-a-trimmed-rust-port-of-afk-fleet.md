# afkconvoy is a trimmed Rust port of afk-fleet

afkconvoy reimplements [`sunfmin/afk-fleet`](https://github.com/sunfmin/afk-fleet) — a Claude Code skill backed by Python scripts — as a standalone Rust command-line tool installed with Homebrew. It is not a line-for-line port: each afk-fleet feature was kept, changed or cut on purpose, and afk-fleet's own ADRs remain the rationale for whatever is kept as it was.

## Kept as in afk-fleet

- Cooperative multi-fleet: atomic claim refs, a per-instance heartbeat and lease, unattended reclaim of stale claims (afk-fleet ADR-0003, 0044).
- orca as the only worker backend (ADR-0005, 0021).
- The opaque worker launch command, with custom-provider detection, alias candidates and the resolve check — for Claude only (ADR-0010).
- The landing train, its train worker, red-train repair and abandon (ADR-0048).
- Recorded gate runs (ADR-0030).
- Continuation in three tiers, with the branches the fleet cut recorded on the issue (ADR-0011, 0043).
- The silent-worker ladder: nudge, then escalate; for a worker joining the train, nudge, one restart, nudge, then escalate (ADR-0018, 0035).
- Park, with blocker standings and the cycle check (ADR-0022).
- The status board (ADR-0006).
- Fast-forwarding the fleet's own checkout after a landing (ADR-0037).
- The bootstrap probes: claim-namespace fallback, branch protection, gate-record writability and sweep.
- The explicit `takeover` command, for another host's fleet.

## Changed

- The tool runs its own loop and asks no one anything (ADR-0002).
- A host runs one fleet per repo and takes its own host's claims at start (ADR-0003).
- The gate is local and every PR lands on the train (ADR-0004).
- A failure escalates; nothing is retried (ADR-0005).
- `giving-up` and `needs-decision` are one verdict, `needs-human`.
- `already-satisfied` is confirmed in code: a branch with no commit past the base and a clean worktree closes the issue, anything else escalates.
- Every wake-up runs a full tick; only the busy and idle sleep intervals remain of the pacing.
- A wake is a subcommand that signals the host's `run` process, not a line typed into a terminal.
- The base branch is a config key (ADR-0006).
- Config stays a yaml block in a markdown file under `docs/agents/`, with six keys: `base_branch`, `ready_label`, `epic_labels`, `concurrency`, `gate_command`, `escalate_label`.

## Cut

- The LLM launcher, the opaque cycle state and all four judgments.
- The qoderclicn runtime and runtime detection.
- `gate.ci: required`: reading and waiting for GitHub checks.
- The adversarial verify, and with it the landing turn, the merge queue and giving a turn up.
- The retry ladder and the `afk-attempt/*` labels.
- The fingerprint gate.
- The drain.
- The base branch kept on the remote and confirmed at every launch.

## Commands

`run` (the loop), `plan`, `tick`, `takeover`, `wake`; the worker's own `gate` and `land`; and each transition as a subcommand for debugging by hand.
