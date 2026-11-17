---
status: accepted, amended by ADR-0035
---

# An inspection is the operator on its host

An inspection — of a hot worker, or of the host once admission keeps holding back (ADR-0019) — was held as a consult: it could look, not act. What it could do was a short list `afkconvoy inspected` did for it — free a path that can be rebuilt, type a line at the hot worker — chosen by a word it found (`wrong`, `heavy`, `not-its`; `reclaimable`, `fleet`, `not-its`), and for anything else it opened an issue for a human, `fleet-outgrows-host` with the escalate label. #426 sat overnight that way, though its inspection had found the fix. Adding a command for each remedy — kill, restart, tell (#437), queue the gates (#438) — does not end it: the next thing that holds the work back needs a remedy nobody listed.

The inspection is now the host's watch (ADR-0035). It is started so that the fleet's work goes on. It is the operator on its host: given what was taken down and the authority of whoever sits at this host's keyboard, it finds what holds the work back, fixes it — whatever that takes — checks that the work goes on, and says what it did. It opens no issue of what it found and hands nothing to a human.

## Its authority

What the operator would do at this host's keyboard. For example, and not a list to keep: end a runaway or orphaned process; restart, nudge or switch a worker; have a gate it stopped run again; change a host setting (admission's shares, say); free disk; send heavy work through `afkconvoy exclusive`; change the repo — its `gate_command`, say — through a branch of its own and a turn like any worker's change, on no issue. It commits and pushes as a worker that writes does.

## Its bounds

Those of any unattended worker, held by the guard where it can (ADR-0012):

- **Only what the fleets of this host started or own.** A `kill` names a pid the guard reads: a process started with a worker's `TMPDIR` — the link a fleet made for it, or the worktree's own directory a gate there sets — or one under it or left behind by it. Never the host runner, the user's programs, other apps, orca; `pkill` and `killall`, which end whatever a name matches, are refused.
- **Never delete what cannot be rebuilt.** A delete of model weights, downloads, media, Docker's images or the user's files under the home folder, or of anything outside it but the system's temporary files, is refused; so are `git clean` without `-X`, `git worktree remove --force`, `git reset --hard` and `orca worktree rm --force`, which lose a change no commit holds.
- **No force push, no push to the base branch, no landing outside a turn.**

What the guard cannot hold — a delete of a path it cannot place — the brief says, as it says every bound.

## Considered options

- **One command per remedy** (#437, #438). Each a check `inspected` makes before acting. Refused: the list is never done, and each new item waits for a release while the work stands.
- **Keep the consult, and have it open an issue for what it cannot do.** What #426 showed: the issue waits for a human, and the work waits with it. The fleet runs unattended so that nothing waits for one.
- **Bound it by an allowlist of commands.** The same list again, at a lower layer. The guard refuses what crosses a bound — whose a process is, what a path holds, what a push loses — and leaves the rest to the operator.

## Consequences

- The operator writes: it may commit, push a branch of its own and change the host. Its brief gives it the goal, what was taken down, its authority and its bounds.
- A process a fleet did not start, the host runner among them, is refused to it; to tell, the guard reads the host's processes once a `kill` names a pid.
- The issue tracker sees no issue of what it found, nor of a change of the repo, which reaches the base branch through a turn as its PR.

## Amended: a proven red base branch in an unbound repo

The one exception to opening no issue is a base branch in an unbound repo proved red the same way gated alone in a clean checkout (ADR-0011). The operator finds the open issue for the fix or opens one with the ready label and raises the fleet lock on it, then stops running gates while the lock stands and records what it did. The defect affects every issue and its fix must be worked first; the operator's own PR cannot land while that lock stands. Other changes of the repo still land on no issue, as above.
