---
status: accepted
---

# The fleet cuts its own versions

A repo the fleet lands work in, and whose versions are installed through Homebrew, is cut by its fleet and by no human's session. Only the fleet knows when it is idle, or when its oldest landing past the newest version tag has waited too long, and at those times nobody is usually at the keyboard: landed work waited for a session to notice. A **cut** is the run that makes a new version of a repo from its base branch: it commits the version on a branch of its own from the base tip, gates that commit, tags it, and publishes and installs it. The concept is called a cut because "release" already names a claim's last step.

How a cut is run, step by step, is `afkconvoy cut --help`; this records what was decided.

## When

A tick whose pass ends idle — no claim held, an empty frontier, admission holding nothing back — starts a cut when the base branch holds landed commits past the newest version tag (`v` and three numbers), or the remote has no version tag and the cut block declares `initial: X.Y.Z`.

A busy fleet cuts too, once the oldest landing past the newest tag reaches the repo's age bound (`busy_after_seconds`), measured from when that commit landed on the base branch's first-parent line. Claims, the frontier and admission do not hold that cut back; a fleet lock and a live cut claim do. Waiting for every claim would let the slowest worker set the release time; a version the next landing makes stale is fine, since the next cut picks that work up. An untagged initial cut stays idle-only.

An operator may ask for the due cut now: `afkconvoy cut --now` records the ask on the remote and wakes the fleet, and the next tick starts the cut whatever claims, frontier, admission or age bound say. It passes only that wait: nothing past the tag, a fleet lock, the same failure repeated with nothing landed since, or a cut still going refuse it. The cut that starts answers the ask, whichever host claims it.

## Once, and by a command

A cut is claimed on the remote as a patrol's run is: of the fleets of a repo, on whichever hosts, one wins. The winner reads the version tags again before it picks the version. One cut runs at a time.

The cut is a command, `afkconvoy cut`, run in a checkout of its own (`afkconvoy-cut`) cut from the base branch as it stands, by a worker in no role that takes no slot of the concurrency. It is not a model session: its steps are known and the same every time, a command times each and names the one that failed, and it holds its bounds by what it does — it pushes its own branch and one tag, and nothing to the base branch. Nothing in a cut calls for judgement but a failure.

## What

The version is the next patch past the newest tag, or the next minor (patch 0) where a PR that landed since carries `release:minor` or adds or changes a file under a path the repo declares in `minor_paths`. The middle number marks a design change: a repo names where its design is recorded (such as `docs/adr/`), and each landing is read against its first parent, so no worker has to remember a label. A patch cut that finds such a landing at `tag`, landed while it ran, stops without tagging and opens no failure; the next tick cuts the minor.

The steps are `set`, `push`, `gate`, `tag`, `publish`. On the snapshot `S`, the base tip the checkout was cut at, the repo's `set` lines run and their result — even nothing — is committed as the version's commit, whose only parent is `S`. That commit is pushed to the cut's own branch (`afkconvoy-cut/v<version>-<stamp>`), gated, tagged `v<version>`, and with the tag checked out the repo's `publish` lines run.

**A branch of its own.** The base branch never receives the version commit. The cut is no candidate, opens no PR and reserves no barrier, so turns keep landing while it runs, whether it succeeds, fails or hangs: one host's hung cut once held every host's landings, and a release must block nothing merged after it. The version's home is the tag; version files on the base branch stay as they are, and each cut's `set` lines rewrite the version on its own snapshot. The landings past the tag are still `v<tag>..<base>`, since `S` is an ancestor of the base.

The cut gate is the whole gate. Where the repo declares `gates`, it runs as a round's does (ADR-0083): the fast gate in a worker lane and the slow gate in the host's slow lanes, each to its own limit, so the slow tests get a slow lane's share of the host, not a worker's. Otherwise the whole command runs in a worker lane. Its worker lane shares the repo as a candidate's does, and a cut's queued commands go ahead of waiting worker commands, so an arbitrary queue of workers' gates cannot eat the cut's time limit; they never interrupt a running command.

A cut interrupted after tagging goes on from the tag, and one interrupted before from its branch: `set` takes up the commit the remote's tag or a branch of that version already holds, and makes no second branch.

## The first cut

An untagged repo may declare `initial: X.Y.Z` (three whole numbers, no `v`, no suffix) in its cut block. The next idle tick cuts exactly that version, through every step: an empty `set` still makes a version commit, so the branch and its recorded gate run stay the only way to tag a release. Once a version tag exists the declaration is ignored. An explicit version keeps the version's one home in the repo's declaration; afkconvoy need not know each language's manifest.

## A failed cut never holds the next cut back

A cut that failed at a step, ran out of time, lost its terminal, its checkout or its host opens one issue under `convoy-cut-failed`: which step failed, why, the evidence, and the line that goes on from there (`afkconvoy cut --from <step>`, in the checkout it kept). Later failures add to the open issue rather than opening another.

That issue is a record, not a gate. The next due cut cuts again from the start, on the base tip as it stands, whether or not the issue is open; nothing waits for a `--from` line, a watch or a human. The next cut removes the checkout a failed one kept and deletes every branch an earlier cut pushed but the one it takes up. A cut that publishes closes every open failure issue of its repo, naming the version. A cut that failed at `publish` has pushed its tag: that version stays unpublished, and the next is the version after it. Holding every cut on an open failure once left a repo's landed work unreleased for days, and held back the very fix that would have unblocked it.

Where the same step fails the same way (`REPEATS`, three cuts in a row — for a red gate, the same failing tests), the failure becomes an ordinary ticket of the fleet's: the issue takes the ready label, not the escalate label, and the next cut waits for a landing past the base the last failure was cut from, so the fleet does not cut the same broken tree in a loop. A failing step is code or configuration the fleet can fix. Only a failure that needs money spent, the one kind left to a human (ADR-0040), is swapped to the escalate label by the watch; cuts are still attempted meanwhile.

A gate red only on TIMEOUTs the gate itself says it cannot classify because the host is overloaded says nothing of the commit and is no failure: the step gives its lane back, waits, and gates again while its time limit allows. The rule is judged in one place for every gate run (`gate::judge`). A FAIL, a TIMEOUT the gate does not excuse, or a run that reached the gate limit fails the step.

Every repo the fleet cuts follows these rules: they live in the cut, not in a repo's steps.

## Turning it on

A repo turns cutting on in its config file, in a fenced block of its own opened by ```` ```yaml cut ````, naming `set` and `publish`, each run by `sh -e` in the cut's checkout with the version in `$VERSION`: the first line that fails fails the step, where by `sh` alone a publish that failed followed by an install that found nothing newer would pass. Optional keys (`busy_after_seconds`, `initial`, `minor_paths`) go in a further cut block. The blocks are separate because an afkconvoy refuses a config key it does not know: a new key in the config's first block, or in the first cut block, would make every older afkconvoy of the repo refuse its config once the change landed. An older afkconvoy skips the blocks it does not read.

## Considered options

- **A model session in a role.** Refused: the steps are known, and a model adds only a way to do them differently each time.
- **A cut issue given to the fleet.** Refused: a worker lands only through a turn, and the issue would hold a claim of the very fleet whose idleness a cut waits on.
- **The operator's control session cuts.** Refused: it cuts only once the operator opens a session, and guesses from outside whether the fleet is idle.
- **Cut on a schedule.** Refused in favour of the age bound: a schedule can cut moments after one landing and make another wait a whole interval.
- **Cut after every landing.** Refused: a version per PR, each published and installed, while the next PR is a gate away.
- **Tag the base tip, or land the version commit through a landing turn.** Refused: the fleet pushes nothing to the base branch but through a gated turn, and a cut held as a barrier let one host's hung cut stop every host's landings.
- **Hold cuts while a failure is open, and have the watch answer faster.** Refused: the watch was itself the thing that could not ship.
- **Retry a repeated failure forever, or hand it to a human.** Refused: the same failure on the same tree spends a lane and says nothing new, and the fleet can fix it.

## Consequences

- A repo that turns cutting on is cut whenever its fleet goes idle with landed work, or its oldest landing reaches the age bound while busy, with no human session. The operator's global rules say a Homebrew repo is cut by its fleet and never by hand; those rules are the operator's to change.
- A cut adds no commit to the base branch and opens no PR; its branch and its tag are its record. A failed cut costs only its own version.
- `plan` has a `cut` row where cutting is on; `doctor` warns while a failed cut's issue is open, which holds nothing back.
- Once a newer afkconvoy is installed, the host runner moves onto it between ticks, without waiting for every fleet to go idle (ADR-0023).
