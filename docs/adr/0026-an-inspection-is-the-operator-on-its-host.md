---
status: accepted
---

# An inspection is the operator on its host

An inspection — of a hot worker, or of the host once admission keeps holding back (ADR-0019) — was held as a consult: it could look, not act. What it could do was a short list `afkconvoy inspected` did for it — free a path that can be rebuilt, type a line at the hot worker — chosen by a word it found (`wrong`, `heavy`, `not-its`; `reclaimable`, `fleet`, `not-its`), and for anything else it opened an issue for a human, `fleet-outgrows-host` with the escalate label. #426 sat overnight that way, though its inspection had found the fix. Adding a command for each remedy — kill, restart, tell (#437), queue the gates (#438) — does not end it: the next thing that holds the work back needs a remedy nobody listed.

An inspection is started so that the fleet's work goes on. It is the operator on its host: given what was taken down and the authority of whoever sits at this host's keyboard, it finds what holds the work back, fixes it — whatever that takes — checks that the work goes on, and says what it did. It opens no issue of what it found and hands nothing to a human.

## Its authority

What the operator would do at this host's keyboard. For example, and not a list to keep: end a runaway or orphaned process; restart, nudge or switch a worker; have a gate it stopped run again; change a host setting (`run --concurrency`, admission's shares); free disk; send heavy work through `afkconvoy exclusive`; change the repo — its `gate_command`, say — through a branch, the gate and the landing train like any worker's change, on no issue. It commits and pushes as a worker that writes does, whatever the facts of the role it was started in say: the role chooses who inspects (the stage's consult, or the stock consult), not what an inspection may do.

## A change of the repo

An inspection makes it itself and lands it, as any worker's change lands: it commits it on the branch of its own checkout, opens the PR, gates it (`afkconvoy gate --train`) and puts it on the landing train with `afkconvoy land --inspection`. It opens no issue of the change, and in a bound repo takes nothing in to a workflow: no issue, no intake, no flow label, in an unbound repo and a bound one alike.

The train takes a worker's PR by the issue it closes (`afkconvoy land --issue`), and an inspection has none of its own: a host's is on no issue, and a hot worker's issue is that worker's. So the train takes an inspection's PR by the inspection: `land --inspection`, run in its checkout, joins the open PR whose head is that checkout's branch, and its join commit names the inspection and its host (`By an inspection on <host>`) where a worker's says `Closes #<n>`. That line is how the train knows what is on it, as `Closes` is of a worker's.

Once its PR is on the train the inspection need not wait for the landing: the train lands it as any other — gated with the rest, a red train fixed as any, no issue closed — and while it is on the train every fleet of the repo tends the train as it does for a PR of its own. A fleet that gives the train up joins the PR again itself, onto the next, for nobody else is left to. While a fleet lock stands it joins nothing: an inspection's PR is no lock issue's, and the inspection says so. The PR it joined is kept in its checkout, and its record names it (below).

The gate counts toward its hour, and waits its turn on the host as every gate does: an inspection starts it as soon as the change is committed. This repo's gate takes about four and a half minutes, and may wait in the host's queue for about 32 minutes at most (`docs/agents/afkconvoy.md`): it fits. One whose time runs out first all the same is ended with its checkout, its gate with it: its branch and its PR stay open off the train, and nothing lands them — a later inspection is on a branch of its own and is not told of them.

## Its bounds

Those of any unattended worker, held by the guard where it can (ADR-0012):

- **Only what the fleets of this host started or own.** A `kill` names a pid the guard reads: a process started with a worker's `TMPDIR` — the link a fleet made for it, or the worktree's own directory a gate there sets — or one under it or left behind by it. Never the host runner, the user's programs, other apps, orca; `pkill` and `killall`, which end whatever a name matches, are refused.
- **Never delete what cannot be rebuilt.** A delete of model weights, downloads, media, Docker's images or the user's files under the home folder, or of anything outside it but the system's temporary files, is refused; so are `git clean` without `-X`, `git worktree remove --force`, `git reset --hard` and `orca worktree rm --force`, which lose a change no commit holds.
- **No force push, no push to the base branch, no merge outside the landing train.**
- **An hour**, where ADR-0019 gave 15 minutes: those were for an inspection that ran a model and a `sample` and changed nothing, and a change of the repo is gated within its time. A hot worker or the host waiting for the host's one inspection may wait an hour.
- **As before**: one inspection on the host at a time, of either kind; once per episode.

What the guard cannot hold — a delete of a path it cannot place, an unbound repo's inspection, whose launch command is its own — the brief says, as it says every bound.

## What it did is the record

`afkconvoy inspected` takes what held the work back — and what it could not fix, if anything, in the same words — and each thing it did (`--did`). No word of a finding chooses an action, and none opens an issue: the `fleet-outgrows-host` label, its one open issue and its body are gone, and so is `--reclaim`, whose checks the guard's bounds replace. Each thing it did is a line of the tick log and of the host runner's log, and how it ended — `said`, or `nothing` where it said nothing in time — is another, with what it said and each PR it put on the landing train. `doctor` lists them among its events; the panel shows the host's beside admission's reading, and a hot worker's issue gets the same as a comment, which is no stage record. What it could not fix, the next episode inspects again.

## Considered options

- **One command per remedy** (#437, #438). Each a check `inspected` makes before acting. Refused: the list is never done, and each new item waits for a release while the work stands.
- **Keep the consult, and have it open an issue for what it cannot do.** What #426 showed: the issue waits for a human, and the work waits with it. The fleet runs unattended so that nothing waits for one.
- **A new kind of role for inspections, with authority of its own.** Refused, as ADR-0019 refused it: the consult already stands for the team's senior judgement on the stage's harness. What changes is what an inspection may do, which the guard reads from its being an inspection, not from its role.
- **Bound it by an allowlist of commands.** The same list again, at a lower layer. The guard refuses what crosses a bound — whose a process is, what a path holds, what a push loses — and leaves the rest to the operator.
- **Hand a change of the repo to the fleet's workers, by an issue of the change** (#444). The train took a PR by its issue, and the gate may take most of an inspection's 15 minutes. Refused (#446): it was an issue an inspection opened all the same, and in a bound repo, where the guard refuses a flow label put on by hand and `intake` takes nothing in for an inspection, it was left open without one — an issue only a human could move. The train takes a PR by the inspection instead.
- **Say what it could not fix with a flag of its own** (`--left`, and a `held` ending). Dropped with the issue of a change, whose bound-repo path it carried: nothing acted on it but the record, which keeps what it could not fix in its own words.

## Consequences

- An inspection writes: it may commit, push a branch of its own and change the host. Its brief gives it the goal, what was taken down, its authority and its bounds.
- A process a fleet did not start, the host runner among them, is refused to an inspection; to tell, the guard reads the host's processes once a `kill` names a pid.
- The issue tracker sees no issue of an inspection: not of what it found, nor of a change of the repo, which reaches the base branch through the landing train as its PR. A host whose fleet outgrows it is inspected each episode, and each says so in the log.
- A join commit on the train names the issue its PR closes or the inspection that made it: what reads the train — what is on it, what landed, what closes — reads either.
- Supersedes, of ADR-0019, that an inspection is held as a consult and finds one of a list of words, each with its one action; that a host inspection opens one issue of the fleet outgrowing its host; and that `afkconvoy inspected` frees the paths a host inspection gives it, checking each; and that an inspection has 15 minutes. The rest of ADR-0019 — what is watched, when an inspection starts, what is taken down, its bounds of one at a time, a time limit and once per episode, the queue and admission — stands.

## Amended: a proven red base branch in an unbound repo

The one exception to opening no issue is a base branch in an unbound repo proved red the same way gated alone in a clean checkout (ADR-0011). The inspection finds the open issue for the fix or opens one with the ready label and raises the fleet lock on it, then stops running gates while the lock stands and records what it did. The defect affects every issue and its fix must be worked first; an inspection's own PR cannot join while that lock stands. Other changes of the repo still land on no issue, as above.
