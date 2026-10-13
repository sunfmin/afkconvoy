---
status: accepted
---

# A stage names its harness, and a role carries each harness's limits

Until now every worker of a bound repo was a Claude Code session, and the fleet's start of a worker was written in Claude Code's flags, hooks and paths (ADR-0009). A team should be able to put one stage on another harness — pi is the first — to have work done, or reviewed, by something other than what wrote it.

A stage, and a patrol, names the harness its worker runs, beside its model. One that names none takes the default, which the project's config names; a config that names none means Claude Code. A consult, a landing and the proposal of a precedent run on the default. The worker starts in its role's directory whatever its harness (ADR-0009 holds), and the fleet adds, per harness, what that harness does not load from there.

A role has one core every harness works under: its rules and its skills, each written once. What a harness needs of its own is added to that core and never stands in its place. A role's limits are the part that cannot be shared, so they are written once per harness, in that harness's own form. The guard is the fleet's and not a role's: it is decided in one place for every harness, and each harness is given a way to ask it.

## Considered options

- **One harness for a fleet instance, chosen where it is started.** Smaller, and a team would name no harness. Refused: the reason to want a second harness is to set it against the first within one workflow, and which harness did the work changes the result, so it belongs in what is committed and not in how one host was started.
- **A role that names no harness, its files translated by the fleet.** One layout, and a stage's harness could change without touching a role. Refused with the next.
- **A role's limits written once and bridged.** The fleet would read a role's Claude Code hooks and run them for another harness. That makes the fleet the loader ADR-0009 was written to avoid, and a bridge that carries some hooks and silently drops the rest is worse for a limit than a refusal.
- **Model names a team writes once, mapped to each harness.** A table to keep, and one more place where which model ran is hidden.

## Consequences

- A limit a role holds under two harnesses is written twice. The fleet's check can tell that a role holds limits for a stage's harness; it cannot tell that they refuse the same things. A stage whose role holds limits for another harness and none for its own is refused, so a harness is never changed into fewer limits unseen.
- A team's files name harnesses, and a role's repo may hold files for more than one. A role is right for the harnesses its files cover and no others.
- A stage's model is passed as written and is its harness's own name for it. A team that names models and no harness is written for one default, and a repo that binds it under another is told so before a worker is started.
- A fleet instance has a worker launch command for each harness its workers run. Which harness is committed; how one is started stays the host's. A start that cannot start every harness it could be asked for is refused: none is ever stood in for by another.
- A worker loads nothing of the host user's configuration under any harness. For a harness with no way to say so, the fleet keeps a user directory of its own for it, holding only what the host knows of providers. That directory is not all a harness reads of the host user's: pi takes skills from the home directory wherever its user directory is, so the start also leaves skill discovery out and names the role's skills.
- What the fleet's user directory holds of the host's is linked to, not copied: a login a harness renews is written where the host user's own is, so neither goes stale, and a file the user no longer has is gone from the directory at the next start. What the fleet itself gives a harness — the extension through which pi asks the guard — is kept there too, and named on the line that starts a worker, not left for the harness to find: a worker whose guard is missing does not start.
- A harness that asks before it loads a role's own files is answered on the line that starts it, never at its terminal. The worker backend knows Claude Code's question and not pi's, and a role's limits in pi's form are not loaded until it is answered.
- A worker's stage record names the harness it ran on. It cannot be worked out from the commits the record already carries, since the default is the project's.
- An unbound repo is as it was: one opaque worker launch command, and no harness the fleet knows of.
