---
status: accepted
---

# A worker starts in its role, with the issue's worktree added to it

In a bound repo a worker's working directory is its role's directory, and the issue's worktree is an additional directory. Claude Code loads rules, skills and hooks from the working directory natively, so a role needs no code to be put into effect, and a human who opens the role's directory by hand gets the same role a worker does. The other way round — start in the worktree, inject the role — means the fleet carries its own loader for rules and hooks, and the two ways of entering a role stop being one. Every harness starts in the role's directory and the fleet adds, per harness, only what that harness does not load from there (ADR-0020). Grok is the one exception: it accepts no added directory, so a Grok worker starts in the issue's worktree and the fleet tells it the role's rules and names its skills from the home it keeps for that worker.

## A role is a directory, copied alone

A role is a directory: a whole repo, or a subdirectory of one. A team is the same. So one repo can hold a team and every role it names, and a change to a workflow and to the role that performs it is one commit and one review; a role kept in a repo of its own stays right for a role several teams share.

A team's file names a role as `owner/repo`, `owner/repo/path`, or `./path` — a path from the root of the team's own repo, so a team and its roles in one repo name no repo at all and can be forked or renamed without an edit. A project's config names its team as `owner/repo` or `owner/repo/path`.

The fleet's copy of a role holds that role's directory and nothing else of its repo, kept per commit and read-only. Claude Code reads a `CLAUDE.md` in every directory above the one it starts in: were the copy the whole repo, the rules at its root would be every role's rules by an accident of layout, and each role could read its neighbours. The copy of a team is its directory without its roles' directories and without a `.claude/` at its top; its `CLAUDE.md` is kept, and is the team's rules, which every worker of the team is given. A role's behaviour is what its directory says, the team's rules, and — below both — the host user's skills and rules (below).

## The host user's skills and rules

A worker is given the host user's own **skills and rules** by default, under every harness, as a session that user started by hand would have them. Without them a role works without the skills its author relies on everywhere else, every role copies them in, and the copies drift from the author's own. A team turns this off with `host_user: false` in its team file; a role may set its own by naming its directory as a map (`reviewer: { dir: ./roles/reviewer, host_user: false }`). A team that needs a role to be the same on every host says `false`.

Nothing else of the host user's is loaded either way: no setting, hook, MCP server, plugin or extension. The guard and orca's status report stay the only hooks the fleet adds, and a role's limits are untouched (ADR-0012). A skill of the role's wins over a host user's skill of the same name. Where rules disagree, the brief, the role's rules, the team rules and the repo's own rules win over the host user's; every brief says so.

DSH is the one harness whose host profile a role may choose to load whole: `dsh_profile: host-user` loads the user's own `dsh-tui` profile in place, with its plugins, MCP servers, patches and settings; `dsh_profile: fleet`, the default, keeps the fleet's own. It is set by the team and overridden per role beside `host_user`, and is independent of it. Copying a subset of a user's profile would make the fleet responsible for understanding arbitrary plugins and would drift from the session the user runs by hand. The role's overlay follows the user's layers and the fleet's overlay, with the guard, comes last; user settings cannot turn it off, and the fleet writes nothing into the user's DSH home.

## Considered options

- **The whole repo copied, the worker started in the subdirectory.** Rules above the role would be inherited without anything saying so. Rules several roles share are designed and named instead: the team's `CLAUDE.md`.
- **A role's version as the hash of its directory.** The version a stage record carries stays the repo's commit: it is what a human can check out. Whether a role changed between two commits is asked of its directory's content.
- **A ref beside the path** (`owner/repo/path@v2`). A role is read at the head of its repo's default branch; a push there is the only way a change reaches a worker.
- **Load nothing of the host user's.** A role would be the same role on every host, and every role would carry its own copies of its author's skills, which drift. `host_user: false` keeps that choice for a role that wants it.
- **Load the host user's settings too.** Brings their hooks and MCP servers into every worker beside the guard. Refused: a worker is limited by the guard and its role, not by whatever a host has installed. DSH's `host-user` profile is the one, opt-in exception.

## Consequences

- In a bound repo the worker launch command is not opaque (ADR-0001): the fleet adds the role directory, the worktree, the team's copy, the exclusion of user settings and the guard, and a start checks the command takes what its harness needs. What each harness is given and how is `afkconvoy help worker-launch`'s to say.
- The repo's own rules are loaded from the added worktree; its hooks are not. What a repo must enforce belongs in its gate.
- The orca terminal is still opened on the worktree; the line it runs changes directory first.
- One issue has one worktree, one branch and at most one PR across all its stages, and every stage is a new session. A reviewer that inherited the implementer's context would review with the implementer's reasoning. A workflow is a line with edges back, never two stages at once.
- A role's directory holds no symlink that leaves it; in the worker's copy it would point at nothing. The directory of a role and that of a team, or of another role, never contain one another; `check` refuses a team file that says otherwise.
- A role opened by hand in its subdirectory is not quite the role a worker gets, since Claude Code also loads what is above it. `check` notes a `CLAUDE.md` or `.claude/` above a role, other than the team's rules, and a team directory's `.claude/`, which no worker loads.
- A Claude Code role copy holds an empty `.git` directory, so Claude Code's own project discovery stops at the role and cannot find a host profile's skills above the cache.
- A Codex worker still loads the skills of the host user's home (`~/.agents/skills`) under `host_user: false`: Codex has no way to be told to leave them out, and the fleet does not work around a harness by starting it under another home.
- A Grok worker cannot read a file beside a role's skill unless its tools may leave the worktree, and sees of the team's copy only the rules it is told.
- A role may behave differently on two hosts when it takes the host user's skills, or DSH's host profile.
- `scaffold` writes a team with its roles under `roles/`.
