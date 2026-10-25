---
status: accepted
---

# An inspection is the operator on its host

An inspection — of a hot worker, or of the host once admission keeps holding back (ADR-0019) — was held as a consult: it could look, not act. What it could do was a short list `afkconvoy inspected` did for it — free a path that can be rebuilt, type a line at the hot worker — chosen by a word it found (`wrong`, `heavy`, `not-its`; `reclaimable`, `fleet`, `not-its`), and for anything else it opened an issue for a human, `fleet-outgrows-host` with the escalate label. #426 sat overnight that way, though its inspection had found the fix. Adding a command for each remedy — kill, restart, tell (#437), queue the gates (#438) — does not end it: the next thing that holds the work back needs a remedy nobody listed.

An inspection is started so that the fleet's work goes on. It is the operator on its host: given what was taken down and the authority of whoever sits at this host's keyboard, it finds what holds the work back, fixes it — whatever that takes — checks that the work goes on, and says what it did. It opens no issue of what it found and hands nothing to a human.

## Its authority

What the operator would do at this host's keyboard. For example, and not a list to keep: end a runaway or orphaned process; restart, nudge or switch a worker; have a gate it stopped run again; change a host setting (`run --concurrency`, admission's shares); free disk; send heavy work through `afkconvoy exclusive`; change the repo — its `gate_command`, say — through a branch, the gate and the landing train like any worker's change. It commits and pushes as a worker that writes does, whatever the facts of the role it was started in say: the role chooses who inspects (the stage's consult, or the stock consult), not what an inspection may do.

## A change of the repo

The landing train takes a PR by the issue it closes (`afkconvoy land --issue`), and an inspection has none of its own: a host's is on no issue, and a hot worker's issue is that worker's. So a change of the repo is handed to the fleet's workers by an issue of the change itself — what to change and why — opened as `afkconvoy flows` says one the fleet works is, once the inspection has looked for an open one an earlier episode opened. That issue is the fleet's work, not a finding for a human: it carries no escalate label, and nobody is asked anything. A worker of the fleet makes the change, which is gated and lands as any other; the inspection does not make it itself, for the gate alone may take most of its 15 minutes, and a worker of the fleet could take the same issue.

In a bound repo the guard refuses a flow label put on by hand, and `afkconvoy intake` takes an issue in for a worker on an issue, a patrol or the train, which an inspection is not: there the inspection leaves the issue open without its label, and says so with `--left`.

## Its bounds

Those of any unattended worker, held by the guard where it can (ADR-0012):

- **Only what the fleets of this host started or own.** A `kill` names a pid the guard reads: a process started with a worker's `TMPDIR` — the link a fleet made for it, or the worktree's own directory a gate there sets — or one under it or left behind by it. Never the host runner, the user's programs, other apps, orca; `pkill` and `killall`, which end whatever a name matches, are refused.
- **Never delete what cannot be rebuilt.** A delete of model weights, downloads, media, Docker's images or the user's files under the home folder, or of anything outside it but the system's temporary files, is refused; so are `git clean` without `-X`, `git worktree remove --force`, `git reset --hard` and `orca worktree rm --force`, which lose a change no commit holds.
- **No force push, no push to the base branch, no merge outside the landing train.**
- **As before**: one inspection on the host at a time, of either kind; 15 minutes; once per episode.

What the guard cannot hold — a delete of a path it cannot place, an unbound repo's inspection, whose launch command is its own — the brief says, as it says every bound.

## What it did is the record

`afkconvoy inspected` takes what held the work back, each thing it did (`--did`) and each thing it could not fix (`--left`). No word of a finding chooses an action, and none opens an issue: the `fleet-outgrows-host` label, its one open issue and its body are gone, and so is `--reclaim`, whose checks the guard's bounds replace. Each thing it did is a line of the tick log and of the host runner's log, and how it ended — `goes-on`, or `held` where it left something — is another, with what it said. `doctor` lists them among its events; the panel shows the host's beside admission's reading, and a hot worker's issue gets the same as a comment, which is no stage record. What it could not fix, the next episode inspects again.

## Considered options

- **One command per remedy** (#437, #438). Each a check `inspected` makes before acting. Refused: the list is never done, and each new item waits for a release while the work stands.
- **Keep the consult, and have it open an issue for what it cannot do.** What #426 showed: the issue waits for a human, and the work waits with it. The fleet runs unattended so that nothing waits for one.
- **A new kind of role for inspections, with authority of its own.** Refused, as ADR-0019 refused it: the consult already stands for the team's senior judgement on the stage's harness. What changes is what an inspection may do, which the guard reads from its being an inspection, not from its role.
- **Bound it by an allowlist of commands.** The same list again, at a lower layer. The guard refuses what crosses a bound — whose a process is, what a path holds, what a push loses — and leaves the rest to the operator.

## Consequences

- An inspection writes: it may commit, push a branch of its own and change the host. Its brief gives it the goal, what was taken down, its authority and its bounds.
- A process a fleet did not start, the host runner among them, is refused to an inspection; to tell, the guard reads the host's processes once a `kill` names a pid.
- The issue tracker sees no issue of what an inspection found, only one of a change of the repo it hands to the fleet. A host whose fleet outgrows it is inspected each episode, and each says so in the log.
- Open: an inspection of a bound repo cannot take its issue of a change in to a workflow. An `afkconvoy intake` for an inspection, as `--train` is for the train worker, would close it.
- Supersedes, of ADR-0019, that an inspection is held as a consult and finds one of a list of words, each with its one action; that a host inspection opens one issue of the fleet outgrowing its host; and that `afkconvoy inspected` frees the paths a host inspection gives it, checking each. The rest of ADR-0019 — what is watched, when an inspection starts, what is taken down, its bounds of one at a time, a time limit and once per episode, the queue and admission — stands.
