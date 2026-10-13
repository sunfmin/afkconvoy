---
status: accepted
---

# A worker starts in its role, with the issue's worktree added to it

In a bound repo a worker's working directory is its role's directory, and the issue's worktree is an additional directory. Claude Code loads rules, skills and hooks from the working directory natively, so a role needs no code to be put into effect, and a human who opens the role's repo by hand gets the same role a worker does. The other way round — start in the worktree, inject the role — means the fleet carries its own loader for rules and hooks, and the two ways of entering a role stop being one.

The worker loads nothing of the host user's own configuration. A role's behaviour is what its repo says and nothing else; otherwise the same role is a different role on each host.

## Consequences

- In a bound repo the worker launch command is no longer opaque (ADR-0001): the fleet adds the role directory, the worktree, the exclusion of user settings and the guard to it. The command must take what the fleet adds for its harness, and a start probes that; Claude Code was the only harness when this was decided, and ADR-0013 says what is added for another. An unbound repo's command stays opaque and may start any agent.
- The role directory a worker starts in is the fleet's own copy, kept per commit and read-only, and holding that directory alone when the role is a subdirectory of a repo (ADR-0014); the checkout a human edits is another. A push is the only way a change to a role reaches a worker.
- The repo's own rules are loaded from the added directory; its hooks are not. What a repo must enforce belongs in its gate.
- The orca terminal is still opened on the worktree, so reading the screen, the nudge and the restart are unchanged. The command it runs changes directory first.
- One issue has one worktree, one branch and at most one PR across all its stages, and every stage is a new session. A reviewer that inherited the implementer's context would review with the implementer's reasoning. It also means a workflow is a line with edges back, never two stages at once.
