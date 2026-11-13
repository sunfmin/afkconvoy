---
status: accepted
---

# A failed cut never holds the next cut back

Amends ADR-0028's **Failure** (sunfmin/control#814).

A cut that fails opens one issue, `afkconvoy: the cut of v<version> failed`, under the label `convoy-cut-failed`: which step failed, why, and the evidence. That issue holds no cut back. The next due cut — an idle tick, or a busy tick past the age bound — cuts again from the start, on the base tip as it stands, whether or not the issue is open. Nothing waits for a `--from` line, a watch or a human to close it.

While the issue is open, each later failure adds its report to it as a comment rather than opening another. A cut that publishes a version — of the same version or a later one — closes every open failure issue of its repo, with a comment naming the version that went out.

Where the same step fails the same way on `REPEATS` (three) cuts in a row, the failure becomes an ordinary ticket for the fleet to fix: the issue takes the ready label, beside the repo label it already carries — not the escalate label, and no human. The same way is the same step and, for a gate that ran red, the same failing tests; the why, which names a commit and a run's own output file, is no part of it. From then on the next cut waits for a landing past the base the last failure was cut from, so the fleet does not cut the same broken tree in a loop: cuts are attempted again after each landing. `plan` says the cut `failed` only then.

The next cut, as it starts, removes the checkout a failed cut kept on its host and deletes every branch an earlier cut pushed, but the one it takes up: a branch of its own version standing on the base tip. A version tag keeps the commit of a branch that was tagged.

A failure of the four kinds left to a human is still swapped to the escalate label by the watch; cuts are still attempted meanwhile.

Every repo the fleet cuts follows this rule: it lives in the cut, not in a repo's steps.

## Why

The rule before held every cut while a failed cut's issue stayed open, so a failed cut waited for whoever closed it. v0.5.74 failed at `gate` and 204 commits went unreleased for five days (sunfmin/control#640); it failed again on a test that fails only inside a cut's own worktree (sunfmin/control#802), and waited a day because the watch was down (sunfmin/control#807) — whose fix could not ship because cuts were blocked. guild's cut failed once on a timing race, `brew update` running seconds after the tap push, and waited 15 hours (sunfmin/control#803). Felix (2026-11-13): 「不要卡在哪里。也不要卡在我这里。」

## Considered options

- **Keep the hold, and have the watch answer faster.** Refused: the watch was itself the thing that could not ship.
- **Retry forever, with no bound.** Refused: a cut that fails the same way on the same tree spends a worker lane each time and says nothing new. Three in a row hand it to the fleet as a ticket, and a landing — the ticket's fix, or anything else — starts the next attempt.
- **Hand the repeated failure to a human, under the escalate label.** Refused: a failing step is code or configuration the fleet can fix; only the four human kinds go to a human.

## Consequences

- A flaky step, or one a later landing fixes, costs one version and no wait.
- The failure issue is a record, not a gate: closing it by hand changes nothing of when the next cut starts.
- `doctor`'s `cut_failed` finding is `its last cut failed`; where the same failure repeated, it begins `the fleet cuts no version until more lands`.
