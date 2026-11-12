---
status: accepted
---

# The host kit gives every harness the operator's agent setup; dotfiles only declare it

Formerly the control repo's. That the host kit is a crate of its own is ADR-0062 (sunfmin/control#758); this is what it gives. It gives it on the fleet's machines only, not to Guild's instances: Guild uses the crate for tmux alone, and its instances start from factory engines and grow their own skills ([decision of 2026-11-12](https://github.com/sunfmin/control/issues/693#issuecomment-6813710721), sunfmin/control#780).

A fleet starts workers on more than one harness — Claude Code and Codex in a host kit today, Kimi, Grok, pi or DSH tomorrow — and the operator's global principles, skills and guards reached Claude Code alone. On 2026-11-03 there was no `~/.codex/AGENTS.md`, so a Codex worker read no principle; all 42 skills `npx skills ls -g` listed were installed for Claude Code alone; and both guard hooks were written only in Claude Code's `settings.json`. A stopgap in sunfmin/dotfiles#27 that day let `codex exec` see the skills and quote the first principle word for word.

So:

- **What the setup holds and where it lands are kept apart.** sunfmin/dotfiles only declares the content — the instructions file (`claude/CLAUDE.md`), the skill list (`skills.txt`) and the guard programs — none of which names a harness. The host kit owns where it lands: which harnesses the host has, where each reads its global instructions and skills, how its hooks are written. It knows these already to start workers and trust their worktrees, and it is the first to find a harness newly installed. So it gives every held harness the declared **agent setup**, and `doctor` names each harness that differs from the declaration.
- **A guard is kept below the harnesses where it can be.** The installed skills are read-only between syncs, so no harness and no shell command can change them; this replaces a hook that guarded them. A guard that must be a hook is wired into each harness's own hook settings, in its own format, merging: only the host kit's own entries are replaced, and what another program wrote is left alone.
- **It is written in Rust, as afkconvoy is.** sunfmin/dotfiles gains no shell or Python installer.

Sources: sunfmin/control#528 (instructions and skills), sunfmin/control#529 (guards).

## Considered options

- **A Claude Code plugin.** It bundles hooks and skills, and installs itself from the user settings — for Claude Code alone. Refused.
- **chezmoi.** Declarative and free of any harness, and it can edit TOML; but which harnesses a host has and where each reads would be written a second time in dotfiles, beside the host kit, and its merge rules would live in Go templates. Refused.
- **An installer of dotfiles' own, in Python or Rust.** The operator wants neither shell nor Python there, and a Rust program of its own would be a second place that knows the harnesses. Refused.
- **A global `core.hooksPath` for a guard on `git push`.** It would override every repo's own git hooks. Not taken (sunfmin/control#529).

## Consequences

- sunfmin/dotfiles keeps only its declaration files; its `install.sh` does only what afkconvoy does not reach — the terminal apps, and telling the host where the declaration is (the host settings, `run --agent-instructions`, `--agent-skills`, `--agent-guards`).
- A harness added to the host kit needs no change to dotfiles.
