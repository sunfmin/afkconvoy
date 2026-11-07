---
status: accepted
---

# A fleet cuts landed work when idle or past an age bound

A repo whose work the fleet lands is cut by hand: someone notices that `main` is past the newest `v*` tag, bumps the version on a branch, lands it, tags it, publishes it and installs it. On this repo that was a rule in one human's notes, run in their session once the fleet had gone idle: who cut, when, and whether a cut was due were known only there. Work that landed waited for that session to come back.

The fleet cuts a version itself (#466). A **cut** is the run that makes a new version of a repo from its base branch: it commits the version on a branch of its own from the base tip, gates that commit, tags it, and publishes and installs it.

Amended by sunfmin/control#541: a cut no longer lands on the base branch, and so no longer holds the landing train (below, **A branch of its own**).

## When

In a repo that turns cutting on, a tick whose pass ends idle — no claim held, an empty frontier, admission holding nothing back — starts a cut when its base branch holds landed commits past its newest version tag (`v` and three numbers), or its remote has no version tag and its cut block declares `initial: X.Y.Z`.

Landed work also starts a cut at the first tick at or after the oldest landing past the newest tag reaches the repo's age bound: `busy_after_seconds` in the cut block. Leaving the key out means 18000 seconds (five hours) (sunfmin/control#197); sunfmin/control#106 set that default to 3600 seconds. Its age is measured from the committer time of the first first-parent commit past that tag, the time it landed, rather than a side branch's earlier commits. Claims, the frontier and admission holding workers back do not hold this cut back. A fleet lock, an open cut failure and a live cut claim still do. An untagged initial cut remains idle-only.

At concurrency eight the fleet went 158 ticks without an idle end on 26 October 2026, leaving fixes already landed uninstalled for hours. Waiting for every claim makes the slowest worker set the release time. A version that the next landing makes stale is acceptable: the next cut picks that work up, and the age bound prevents indefinite waiting without stopping admission. The cut starts from the base branch as it stands; work still in flight goes into a later version. No command, flag or fixed schedule starts one.

`plan` says where a cut stands — due, tagged, untagged, failed, locked, running — and gives `oldest_landing_age_seconds` and `busy_in_seconds` (zero once the bound is reached, null without a landing past a tag). The tick line that starts a cut says `fleet idle` or `fleet busy`, after the pass has admitted work.

## Once

A cut is claimed on the remote as a patrol's run is (`refs/convoy/claim/cut`): of the fleets of a repo, on whichever hosts, one wins; the rest say `lost`. The tick that wins reads the version tags again before it picks the version: a peer may have cut since the rebuild. One cut runs at a time; the next starts when the fleet next goes idle with commits past the tag the last one pushed, or their oldest landing reaches the age bound.

## Who

A worker in no role, started the way an inspection is (ADR-0026): a checkout of its own (`afkconvoy-cut`) cut from the base branch as it stands, a terminal of it under the host kit's first harness, on that harness's user default, the bounds of any unattended worker, and a time limit. It takes no slot of the concurrency. While it runs the fleet is not idle — it keeps its heartbeat, so its claim stays live, and keeps the host awake (ADR-0016) — and dispatches what comes as ever.

It is a command, `afkconvoy cut`, not a model session. Its steps are known and the same every time; a command times each, names the step that failed, and holds the bounds by what it does — it pushes its own branch and one tag, and nothing to the base branch — where a model would be held to them by a guard and a brief. Nothing in a cut calls for judgement but a failure, and that goes to a human.

## What

1. The version: the next patch past the newest tag, or the next minor, its patch 0, where a PR that landed since carries `release:minor`. Picked by the tick that starts the cut, and kept in its checkout with the PRs it holds: those the base branch's landings past the tag name, `<title> (#<pr>)`.
2. `set`: the base tip as it stood when the checkout was cut is the cut's snapshot `S`. The repo's set lines run on it, and what they wrote — even nothing — is committed as `<repo> <version>`: the version's commit, whose only parent is `S`. `push`: that commit pushed to the cut's own branch, `afkconvoy-cut/v<version>-<stamp>`. It opens no PR. `gate`: the cut gate — the fast gate and the slow gate, the whole `gate_command`, run on that commit in the cut's checkout, in a worker lane like any heavy command; a green run on record for its tree answers. It is not a candidate on `candidates/<base>`, and it reserves no barrier.
3. `tag`: that commit tagged `v<version>` and the tag pushed.
4. `publish`: with the tag checked out, the repo's publish lines.
5. Each step and its seconds, its branch and its commit are kept in its checkout as it goes; the tick that finds it over says it in its line and the tick log, `doctor` lists it among its events, and the panel shows it.

The base branch never receives the version commit. Turns keep landing on it while a cut runs, whether the cut succeeds, fails or hangs.

## A branch of its own (sunfmin/control#541)

On 2026-11-03 a cut of v0.5.64 started on FelixMBP14 at 00:46Z and, between then and 02:18Z, opened four PRs (#783, #787, #790, #791) about thirty minutes apart, none of which landed. Its serial barrier on `candidates/main` stood while its host's heartbeat stayed fresh, so no other host took it over, and five gated PRs on macs-Mac-Mini waited behind it with `waiting_for_capacity`, the longest for 1.5 hours. One host's cut stopped every host's landings for the repo. Felix decided (2026-11-03): "我觉着发版应该切一个分支来做。不要阻碍后边的往里面 merge。" — a release is cut on a branch, and blocks nothing merged after it.

The cut gate's worker lane shares its repo as a candidate's does: its checkout holds one commit nothing moves while it runs, as a candidate worktree does, so issue turns' candidates validate and land beside it. Ordinary heavy commands and the serial barriers — watches, fleet-lock repairs, base certification — keep their exclusion from it, as from a candidate, and capacity is still the host's worker lanes: a host with one worker lane runs the cut's gate and a turn one after the other.

The version's home is the tag. The next cut picks its version from the newest tag, as before, and its set lines rewrite the version on its own snapshot; version files on the base branch stay where they are. Landed work past the newest tag is still `v<tag>..<base>`: the tag's parent `S` is an ancestor of the base, so the count is unchanged. A cut that fails now costs only that version.

A cut interrupted after tagging goes on from the tag, and one interrupted before tagging from its branch: its `set` step takes up the commit the remote's tag `v<version>`, or a branch a cut of that version pushed, already holds, and makes no second branch. Records an earlier afkconvoy kept read on: a step `pr` as `push`, `land` as `gate`, and the commit that landed as the commit it tags. Cut candidates and barriers an earlier afkconvoy left on the remote chain are read as before and left to drain; nothing removes a record a host still owns.

The four restarts opened no failure issue. A claimed cut was unclaimed with no issue when the tick ending it found no checkout of it on its host — read as a cut that "started nothing" — and when a peer's claim was presumed dead. The candidate of the first cut's PR #783 is recorded `cancelled` by `operator_cleared`, from outside the fleet, while its fast and slow gates were green, consistent with its checkout being taken from outside before the tick read it. Unclaimed with nothing open, the next idle tick cut v0.5.64 again. Both cases now open the one failure issue, naming the version the cut's branch says: no cut of that version starts again on its own.

## Failure

A cut that failed at a step, ran out of time, whose terminal is gone with nothing said, whose checkout is gone from its host, or whose host went silent opens one issue under the escalate label, `afkconvoy: the cut of v<version> failed`: which step failed, why, what it did before, and the line that goes on from there — `afkconvoy cut --from <step>`, in its checkout, which is kept. No cut starts while that issue is open, as no patrol runs while its failed run's is (ADR-0008). Going on from the step closes it; closing it by hand has the fleet cut again from the start — but for a cut that failed at `publish`, whose tag is pushed: that version stays unpublished, and the next cut is of the version after it.

A gate red only on TIMEOUTs the gate says it cannot classify because the host is overloaded is no failure (sunfmin/control#599). The cut of v0.5.72 failed at `gate` on one nextest TIMEOUT at 6.004s, its gate saying `host overloaded or load unavailable (load 12.71, CPUs 12); cannot classify TIMEOUT tests`; run alone, the test passed in 3.6s, and the cut gone on from `gate` passed. Its issue stopped every cut until a watch answered it. Such a run says nothing of the commit, so the step gives its worker lane back, waits five minutes and gates again, its place in the queue kept, while that wait and a whole gate run fit in the cut's time limit. Every failing test must be a `TIMEOUT` and each one covered by a gate line that says `host overloaded` and `cannot classify TIMEOUT tests` or `cannot classify <binary> <test>`: the gate, which measures the load beside its own run, is the one to say it. A FAIL, a TIMEOUT on a host the gate does not say is overloaded, a run that reached the gate limit, or such a red with no room left for another run fails the step and opens the issue as before: a cut failure reaches a human only for a real red gate. The rule is judged in one place for every gate run, the cut's, a base tip's, a candidate's, a round's, a delivery's and a worker's (`gate::judge`, sunfmin/control#619), from the run's whole output as it streamed: no caller reads the kept output file or classifies the TIMEOUTs itself, and a red is worded once (`Red::said`).

A PR labelled `release:minor` that lands while a patch cut runs is an expected stop at `tag`: it tags and publishes nothing, records why it was superseded and opens no failure issue. The tag step checks landings up to the current base branch, including a minor that landed after the version's own PR. The next tick ends that cut, releases its remote claim, removes its checkout, and starts the minor cut in the same pass, unless a fleet lock stands. A declared initial version remains exact.

Its time is three hours beside the two gate limits, five hours at the default `gate_seconds` (sunfmin/control#195), allowing for its own gate and its wait for a worker lane. A repo that sets another gate limit is given twice that plus the same three hours.

A cut's gate commands and exclusive commands go ahead of waiting worker commands, determined by its cut worktree and preserved in its ticket across a host runner replacement. Cuts keep request order among themselves; ordinary commands keep theirs. Neither interrupts a running command; ADR-0031's contention bound still applies. This amends ADR-0034's FIFO rule for cuts now that ADR-0037 has removed the train lane: an arbitrary queue of workers' gates must not consume the cut's time limit. Queue waits still count toward the cut's overall limit; the gate's own limit starts only after its worker lane is granted.

## Turning it on

The repo names its cut steps in its config file, in a block of their own opened by ```` ```yaml cut ````: `set` and `publish`, each run by `sh -e` in the cut's checkout with the version in `$VERSION`: the first line that fails fails the step, as a release script's would. By `sh` alone the lines after it would run, and the last one's status would stand for them all — a publish that failed, followed by an install that found nothing newer, would pass. The config's own ```` ```yaml ```` block refuses a key it does not know, so a key there would make every afkconvoy older than this one refuse the repo's config once the change lands on the base branch — every fleet of the repo, on every host not yet upgraded. An older afkconvoy reads only the first block whose line is ```` ```yaml ```` alone, and reads past this one.

The optional `busy_after_seconds` key belongs in a second ```` ```yaml cut ```` block, not in the config's first block or its first cut block. Leaving it out means 18000 seconds; zero means the next tick may cut while busy. The installed 0.5.20 already reads the first cut block and refuses unknown keys there: reading only the first config block is not the only older behavior to preserve. It skips later cut blocks, so an additive block also keeps that version able to read the config (verified by running its `config` on this repo's amended file). New versions combine all cut blocks into one YAML mapping and refuse duplicate keys; steps are never copied into the addition.

## The first cut (sunfmin/control#67)

An untagged repo can declare `initial: X.Y.Z` beside `set` and `publish` in its cut block: three whole numbers, without leading zeroes, a `v` prefix or a prerelease suffix. Without that declaration it stays untagged, as before. With it, the next idle tick starts a cut of that exact version under the same admission, fleet lock and remote claim rules. The winner reads the remote tags again: if a peer has cut already, the usual tagged behavior applies. Once a version tag exists, the declaration is ignored; subsequent cuts pick the patch or minor past the newest tag as before.

The first cut has no previous tag (`from: null` in its record), and holds the PRs all of the base branch's landings name. Their labels do not change the declared initial version. Every step still runs: `set`, `push`, `gate`, `tag`, `publish`. If `set` changes nothing, the cut makes an empty version commit, so that its branch and its recorded gate run remain the only way to tag a release. There is no seed tag or manual release outside that path.

Choosing an explicit version instead of inferring it from files keeps the version's one home in the repo's cut declaration; afkconvoy need not know each language's manifest. Existing cut blocks without `initial`, and existing records naming a previous tag, keep their behavior.

## Considered options

- **A model session in a role, as an inspection is.** Refused: the steps are known, and a model adds only a way to do them differently each time. What needs judgement — a step that failed — is a human's, through the issue.
- **Cut on a schedule.** Refused in favor of the oldest landing's age bound: a fixed schedule can cut moments after one landing and make another wait a whole interval. The bound follows actual landed work, while idle still cuts promptly. Cutting in the middle of work is acceptable; unbounded delay is not.
- **Cut by a command a human runs.** Refused: it is the human session this replaces, and ordinary landings would still wait.
- **Cut after every landing.** Refused: a version per PR, each published and installed, while the next PR is a gate away.
- **Cut steps as keys of the config.** Refused for the older afkconvoy that refuses an unknown key (above).
- **Tag the base branch's tip, and push the version commit to it.** Refused: the fleet pushes nothing to the base branch but through a gated turn. Its premise that the tag must sit on a commit the base branch carries gives way (sunfmin/control#541): the tag sits on the release branch's commit, whose parent is a base commit.
- **Land the version commit through a landing turn, as a serial barrier.** Superseded by sunfmin/control#541: one host's cut that hung held every host's landings, and a cut that failed restarted without telling anyone.

## Consequences

- A repo that turns cutting on is cut whenever its fleet goes idle with landed work, or the oldest landing reaches its age bound while busy: its published version follows its base branch without a human session.
- A landing commit names the issue its PR closes or the watch that made it. Landings an earlier afkconvoy's cuts made still name the cut and its host, and are read as before.
- An interrupted cut goes on from its tag, or from its branch, and pushes no second branch for the same version.
- A cut adds no commit to the base branch. The version tag points at the cut's own commit, whose only parent is the snapshot `S`, an ancestor of the base. A version tag already published stays on the commit it names.
- A cut opens no PR. Its branch and its tag are its record.
- Turns keep landing while a cut runs: a cut that fails, or hangs until its time limit, costs only its own version.
- `plan` has a `cut` row in a repo that turns cutting on; `doctor` warns while a failed cut's issue is open.
- The rule that ran the cut from a human's session ends where a repo turns cutting on.
