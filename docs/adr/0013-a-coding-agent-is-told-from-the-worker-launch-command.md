---
status: accepted
---

# The coding agent is told from the worker launch command, and pi is given its role by the fleet

A bound repo's worker could be started with Claude Code only: what the fleet adds to the worker launch command (ADR-0009) is four of Claude Code's flags. A bound repo's workers can now be started with pi as well, with the same guarantees: the worker starts in its role, loads nothing of the host user's configuration, runs under the guard, and is heard of by the worker backend.

**One coding agent per fleet, told from the command.** A start already asks the command for its `--help`, to refuse one that takes no flags. The same answer now says which coding agent the command starts: Claude Code if it names Claude Code's flags, pi if it names pi's, and a refusal naming both otherwise. Nothing new is configured, and an alias or a wrapper is told as well as the program itself. Choosing the agent per stage, in a workflow, was the alternative. It would put an agent's name in a team, and it is not needed to start a fleet on pi; it can be added over this without undoing it.

**A role is written once, in Claude Code's layout.** `CLAUDE.md`, `.claude/skills/` and `.claude/settings.json` stay what a role is (ADR-0008). ADR-0009 chose the role's directory as the working directory because Claude Code then puts the role into effect natively, and the fleet carries no loader. That reason does not hold for pi: it finds skills elsewhere, has no hooks file, and cannot be told to leave out the host user's own rules without leaving out the role's. So under pi the fleet does carry a loader — one extension it writes beside the worker's brief and adds with `--extension`:

- it asks the guard before every tool call, describing the call in Claude Code's words, so the guard has one reading of a call (ADR-0012);
- it runs the role's `PreToolUse` hooks as Claude Code would: a role's own limits hold under either agent, and no role is written twice;
- it adds to the prompt the rules of the role, of the worktree and of the team, which are the three Claude Code loads.

pi still starts in the role's directory, though it loads nothing from there by itself: the line, the facts and the terminal are then the same for both agents, and a hook of the role runs where it expects to. Starting pi in the worktree, with the role added by flags, was considered; it would load the project's rules natively and nothing else, and make a start under pi a different thing from a start under Claude Code.

Refusing, at `check`, a role that has hooks when the fleet runs on pi was the alternative to running them. It would make a team usable under one agent only.

## Consequences

- This replaces one sentence of ADR-0009: a bound repo's command must take the flags of Claude Code *or of pi*. The rest of it stands.
- The fleet now carries code that puts a role into effect, for pi only. It is one file, tested by running it against the real guard; what it does not carry is stated: a role's hooks on any event but `PreToolUse` are not run under pi.
- "Nothing of the host user's configuration" means, for pi: no extension, skill, prompt template, theme or rules file it would discover. pi's settings file, its credentials and a global `SYSTEM.md` are still read — pi has no flag for them, and moving its whole directory (`PI_CODING_AGENT_DIR`) would move the credentials with it.
- A workflow's `model` is handed to the agent as written. Under pi it is a pattern over the models of pi's providers, so the same word can mean another model. A team is written for the agent its fleet runs on; naming a model per agent waits for the per-stage choice.
- pi asks no trust question: it is started trusting nothing of the directory (`--no-approve`), and a `.pi` directory is no part of a role.
- The worker backend hears of a pi worker through the extension the backend itself installs for pi, which the start names. Where it installed none, the worker is read off its terminal, as a Claude Code worker without the hook is.
- An unbound repo's command stays opaque and may start any agent.
