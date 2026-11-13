---
status: accepted
---

# An issue that needs the next version waits on the cut

sunfmin/control#551 (mytokens) could be done only once the next mytokens version was installed: it checks the released build on the host. On 2026-11-03 18:24 JST it was handed back to the fleet while a cut was due (`"cut":{"busy_in_seconds":12204,"newest_tag":"v0.1.5",…,"standing":"due"}`). A worker claimed it at once, found no new version, and settled into polling the installed version for up to four hours. Its claim kept the fleet from being **idle**, and a due **cut** starts only when the fleet is idle or the oldest landing reaches `busy_after_seconds` (ADR-0028). The worker waited for the cut and the cut for the worker: nothing moved for about 3.4 hours, and the worker held a lane the whole time (sunfmin/control#566).

## Decision

A `blocked` verdict can name the next cut of its own repo in place of issues, or beside them: `blocked_by=cut` in an unbound repo's verdict marker, `--blocked-by cut` to `afkconvoy verdict`. One stage record, one marker field: what a verdict waits on is the issues it numbers and the word `cut`, read and written by one type (`blocked::BlockedOn`).

The fleet parks such a claim as it parks one on issues: it puts the label `convoy-waits-on-cut` on the issue — and an edge to each open issue named beside it — before it releases the claim, so the issue is held off the frontier from the moment no claim holds it. Off the frontier and unclaimed, it leaves the fleet idle, and the due cut starts at that tick's end.

When the tick ends a cut that took every step — its version tagged, published and installed — it takes the label off every open issue that carries it, read then and not at the rebuild, so an issue parked during that tick returns too. Each returns at the stage it left: a bound repo's position is `Parked` at that stage, an unbound issue keeps its ready label. A failed cut takes the label off nothing: the issues come back once a later cut installs a version, never on the old one — the next due cut cuts again from the base tip (ADR-0072). A patch a minor landing overtook installs nothing, and the minor cut releases them.

A verdict on the cut escalates instead of parking where no cut will come: the repo names no cut steps, or nothing has landed past its newest version tag and no cut is running. A worker's need for a version that holds nothing new is not for a cut to answer.

`plan` lists the issues waiting in its `cut` row's `waiting`, and gives each the excluded reason `waits on the next cut`; `doctor` names them (`wait on the next cut`). The worker briefs say to declare it instead of polling for a version.

## Considered options

- **Keep the worker polling, and let the age bound cut.** Refused: it holds a lane for hours, and the bound is five hours by default.
- **Count a worker that waits on the cut as idle.** Refused: the claim still stands, and the frontier, admission and every other reader of claims would each need the exception.
- **A tracker issue for the next cut, blocked-by edges to it, closed by the cut.** Refused: an issue no fleet works reads as an unmet blocker, so a blocker standing would need the exception anyway, and the issue would be a second home for the cut's state.
- **Record the newest tag in the verdict and compare it with the tags.** Refused: a tag says a version was tagged, not that it was published and installed — a cut that failed at `publish` leaves its tag (ADR-0028). The cut's own end is where that is known.
- **Release only issues that waited before the cut started.** Refused: an issue labelled while a cut ran from a snapshot that already held its fix would wait for a cut that may never be due. Released early, its worker looks once more and waits again; the next cut is due, for its fix is past the tag.

## Consequences

- An issue that needs the next version holds no claim and no lane while it waits, and a due cut is not held by it.
- The label is the one record that an issue waits on the cut. A human who takes it off starts the issue again at once; one who puts it on parks an issue nobody holds.
- The cut that installs on one host releases the issues for every host's fleet of the repo; a worker on a host that has not installed the version yet finds the old one, and may wait again.
