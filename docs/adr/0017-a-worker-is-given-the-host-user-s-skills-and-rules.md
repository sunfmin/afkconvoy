---
status: accepted
---

# A worker is given the host user's skills and rules, unless its team turns them off

ADR-0009 had a worker of a bound repo load nothing of the host user's configuration, so that a role is the same role on every host. In practice a role then works without the skills its author relies on everywhere else — modelling a domain, reviewing code, a language's toolchain — and every role copies them in, and the copies drift from the author's own.

So a worker of a bound repo is given the host user's own **skills and rules** by default, under every harness, as a session that user started by hand would have them: Claude Code's `~/.claude/skills` and `~/.claude/CLAUDE.md`, pi's skills and rules in `~/.pi/agent`, Codex's `~/.codex/skills` and `~/.codex/AGENTS.md`. A team turns this off with one key of its team file, `host_user: false`, and its workers are given what ADR-0009 gave them.

Nothing else of the host user's is loaded either way: no setting, hook, MCP server, plugin or extension. The guard and orca's status report stay the only hooks the fleet adds, and a role's limits are untouched.

## Precedence

- A skill of the role's wins over a host user's skill of the same name. Claude Code and pi do so by themselves (the role's is found first); Codex shows both, so the fleet turns the host user's off by the path of its file.
- Where rules disagree, the brief, the role's rules, the team rules and the repo's own rules win over the host user's. Every brief says so. A host rule such as "commit, push and open a PR when done" does not change how a stage ends.

## How each harness is given them

What is linked is kept in a directory of the fleet's own, apart from the one kept for a worker given none of it, and holds links to those files alone:

- Claude Code: the directory is added (`--add-dir`), and holds `.claude/skills` and `.claude/CLAUDE.md`. Claude Code loads the skills and rules of an added directory and not its settings; `--setting-sources project` still leaves the user's settings out.
- pi: the user directory the fleet keeps links to the user's skills and rules; the skills are named after the role's (`--skill`), and pi loads a user directory's rules itself.
- Codex: the user directory the fleet keeps links to the user's skills and rules, where Codex finds a user's.

## Considered options

- **Load the user's settings too** (`--setting-sources user,project` for Claude Code, the user's own directory for pi and Codex). Brings their hooks and MCP servers into every worker, beside the guard. Refused: a worker is limited by the guard and its role (ADR-0012), not by whatever a host has installed.
- **Keep ADR-0009 and have each role copy the skills it needs.** What happened, and the copies drift.

## Consequences

- A role may behave differently on two hosts: its author's skills are not another host user's. A team that needs the same role on every host says `host_user: false`.
- Under `host_user: false` a Codex worker still loads the skills of the host user's home (`~/.agents/skills`), as ADR-0013 says; a role's skill of the same name still wins there.
- `check` accepts the key, refuses a value that is not true or false, and answers with it.
