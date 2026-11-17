---
status: accepted
---

# afkconvoy is a trimmed Rust port of afk-fleet, and runs its own loop

afkconvoy reimplements [`sunfmin/afk-fleet`](https://github.com/sunfmin/afk-fleet) — a Claude Code skill backed by Python scripts — as a standalone Rust command-line tool installed with Homebrew. It is not a line-for-line port: each afk-fleet feature was kept, changed or cut on purpose, and afk-fleet's own ADRs remain the rationale for whatever is kept as it was (cooperative multi-fleet claims with a per-instance heartbeat and lease, orca as the worker backend, recorded gate runs, park with blocker standings, the status board, the bootstrap probes, an explicit `takeover`).

## Its own namespace

Everything afkconvoy writes on the remote carries its own prefix: refs under `refs/convoy/*` and comment markers as `<!--convoy:…-->`, where afk-fleet uses `refs/afk/*` and `<!--afk:…-->`. The two tools share no ref and no marker, so neither reads the other's claims as its own, and afkconvoy does not see a claim afk-fleet holds.

## It runs its own loop, and asks no model to judge

In afk-fleet the loop is an LLM session that calls `afk cycle`, carries an opaque state between calls, and answers four kinds of judgment. Each of those went with something afkconvoy cut, or became a rule code applies. With nothing left to judge, the loop is a plain long-running process: tick, sleep, tick. No state crosses a process boundary, and a wake is `afkconvoy wake`, which reaches the host's runner (ADR-0023), not a line typed at a session.

Anything that would need judgment has no one in the loop to ask: it must be a rule, or an issue — a failure escalates and nothing is retried (ADR-0005). Models work issues; they never decide what the fleet does next.

Cut from afk-fleet on purpose: the LLM launcher and its cycle state; reading and waiting for GitHub checks (the gate is local, ADR-0047); the adversarial verify and the merge queue (a PR lands in a turn, ADR-0037); the retry ladder and its `afk-attempt/*` labels; the fingerprint gate; the drain; the qoderclicn runtime and runtime detection.

## The ref layout, and when it moves

A remote that refuses hidden refs keeps the fleet's refs as `convoy-*` branches instead (`refs/heads/convoy-*`). A startup probe settles which layout the remote takes, and the checkout notes it for every later command. A checkout that has noted no layout takes hidden refs, or branches where the remote rejects those.

A noted layout moves only when two startup probes that actually run, one after the other, each see the noted layout rejected and the other taken. One rejected probe push was once read as the remote refusing the layout and the note flapped between the two, so a recorded gate run left on the layout just left was no longer believed and the same tree was gated twice. One rejection leaves the note; a probe that takes the noted layout clears the streak; a probe that reaches no verdict refuses the start and changes nothing.

A turn believes a recorded gate run of one tree and one gate command from either layout while the run stands. A new record is written only on the noted layout, and a red run drops the record on both. Claims and heartbeats stay on the noted layout.
