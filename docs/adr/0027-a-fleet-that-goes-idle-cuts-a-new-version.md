---
status: accepted
---

# A fleet that goes idle cuts a new version

A repo whose work the fleet lands is cut by hand: someone notices that `main` is past the newest `v*` tag, bumps the version on a branch, lands it, tags it, publishes it and installs it. On this repo that was a rule in one human's notes, run in their session once the fleet had gone idle: who cut, when, and whether a cut was due were known only there. Work that landed waited for that session to come back.

The fleet cuts a version itself (#466). A **cut** is the run that makes a new version of a repo from its base branch: it sets the version, lands it, tags it, and publishes and installs it.

## When

A tick whose pass ends idle — no claim held, nothing on the landing train, an empty frontier, admission holding nothing back — in a repo that turns cutting on, whose base branch holds landed commits past its newest version tag (`v` and three numbers). Nothing else starts one: no command, no flag, no schedule. Idle is when the work that will be in the version has all landed; a cut started earlier would cut a version the next landing makes stale. `plan` says where a cut stands — due, tagged, untagged, failed, locked, running — so whoever reads it knows whether the next idle tick starts one.

## Once

A cut is claimed on the remote as a patrol's run is (`refs/convoy/claim/cut`): of the fleets of a repo, on whichever hosts, one wins; the rest say `lost`. The tick that wins reads the version tags again before it picks the version: a peer may have cut since the rebuild. One cut runs at a time; the next starts when the fleet next goes idle with commits past the tag the last one pushed.

## Who

A worker in no role, started the way an inspection is (ADR-0026) and a caster is (ADR-0020): a checkout of its own (`afkconvoy-cut`) cut from the base branch as it stands, a terminal of it under the host kit's first harness's worker environment, the bounds of any unattended worker, and a time limit. It takes no slot of the concurrency. While it runs the fleet is not idle — it keeps its heartbeat, so its claim stays live, and keeps the host awake (ADR-0016) — and dispatches what comes as ever.

It is a command, `afkconvoy cut`, not a model session. Its steps are known and the same every time; a command times each, names the step that failed, and holds the bounds by what it does — it pushes its own branch, the train's line through the join, and one tag — where a model would be held to them by a guard and a brief. Nothing in a cut calls for judgement but a failure, and that goes to a human.

## What

1. The version: the next patch past the newest tag, or the next minor, its patch 0, where a PR that landed since carries `release:minor`. Picked by the tick that starts the cut, and kept in its checkout with the PRs it holds: those the base branch's landings past the tag name, `<title> (#<pr>)`.
2. `set`: the repo's set lines, on the base branch as it stood, committed as `<repo> <version>`. `pr`: pushed to a branch of the cut's own, its PR opened. `gate`: `afkconvoy gate --train`, green and on record. `land`: joined to the landing train on no issue, as an inspection's change joins it (ADR-0026) — its join commit says `By a cut on <host>` — and waited for until it lands, joined again whenever a train given up left it off.
3. `tag`: the commit that landed it tagged `v<version>` and the tag pushed.
4. `publish`: with the tag checked out, the repo's publish lines.
5. Each step and its seconds, its PR and where it landed are kept in its checkout as it goes; the tick that finds it over says it in its line and the tick log, `doctor` lists it among its events, and the panel shows it.

It merges nothing outside the landing train and pushes nothing to the base branch.

## Failure

A cut that failed at a step, ran out of time, or whose terminal is gone with nothing said opens one issue under the escalate label, `afkconvoy: the cut of v<version> failed`: which step failed, why, what it did before, and the line that goes on from there — `afkconvoy cut --from <step>`, in its checkout, which is kept. No cut starts while that issue is open, as no patrol runs while its failed run's is (ADR-0008). Going on from the step closes it; closing it by hand has the fleet cut again from the start — but for a cut that failed at `publish`, whose tag is pushed: that version stays unpublished, and the next cut is of the version after it. A PR labelled `release:minor` that lands while a cut runs fails it at `tag`, before the tag: the version it set is a patch, and the cut that starts once the issue is closed picks the minor.

Its time: two gate limits — its own gate and the train's — and an hour beside, three hours at the default `gate_seconds`. A cut out of time at `land` leaves its PR on the train, which may still land it; the next cut finds the version set by lines that change nothing, finds that landing past the tag, and tags it.

## Turning it on

The repo names its cut steps in its config file, in a block of their own opened by ```` ```yaml cut ````: `set` and `publish`, each run by `sh` in the cut's checkout with the version in `$VERSION`. The config's own ```` ```yaml ```` block refuses a key it does not know, so a key there would make every afkconvoy older than this one refuse the repo's config once the change lands on the base branch — every fleet of the repo, on every host not yet upgraded. An older afkconvoy reads only the first block whose line is ```` ```yaml ```` alone, and reads past this one.

## Considered options

- **A model session in a role, as an inspection is.** Refused: the steps are known, and a model adds only a way to do them differently each time. What needs judgement — a step that failed — is a human's, through the issue.
- **Cut on a schedule, or by a command a human runs.** Refused: a schedule cuts in the middle of work, and a command is the human session this replaces.
- **Cut after every landing.** Refused: a version per PR, each published and installed, while the next PR is a gate away.
- **Cut steps as keys of the config.** Refused for the older afkconvoy that refuses an unknown key (above).
- **Tag the base branch's tip, and push the version commit to it.** Refused: the fleet pushes nothing to the base branch but through the landing train; the tag goes on the commit that landed the version.

## Consequences

- A repo that turns cutting on is cut whenever its fleet goes idle with landed work: its published version follows its base branch without a human session.
- A join commit names the issue its PR closes, the inspection that made it, or the cut and its host; what reads the train reads any, and a cut's PR closes no issue.
- A fleet that gives the train up joins a cut's PR again itself, as it does an inspection's.
- `plan` has a `cut` row in a repo that turns cutting on; `doctor` warns while a failed cut's issue is open.
- The rule that ran the cut from a human's session ends where a repo turns cutting on.
