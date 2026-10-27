---
status: accepted
---

# A Grok session works in its worktree

Grok 1.0.60 and later reject `additionalDirectories` on session create, session load and session resume. The error is `additionalDirectories is not supported: a session works only in its cwd`. The CLI still documents `--add-dir` and still sends that field, so a worker started with it never creates a session: the brief sits in the composer and the fleet's start proof finds no turn. ADR-0009 and ADR-0013 had a Grok role worker start in the role's copy and add the worktree and the team's copy. That is the launch that fails. Those two yield for Grok.

The worker's session cwd is the issue's worktree, because that is the only directory the session can work in. The shell `cd`s there and the launch line passes `--trust` and the model, and no `--add-dir`. The role's `CLAUDE.md` and `.claude/skills` stay the files every harness writes. They are not copied into the worktree. The role's rules are written into the kept home's `rules/told.md` with the repo's own rules and the team rules, which Grok already loads from that home. The role's `.claude/skills` is named in `[skills] paths` of the same home. When the worker is given the host user's skills, a directory of the same name under each host skill root is ignored, so the role's copy is the one that loads and the rest of the host user's skills stay.

## Considered options

- **Keep `--add-dir` and wait for a Grok that accepts it.** Refused: 1.0.70 still rejects the field, and the server, not the flag's help text, is what refuses. A role worker cannot start until the fleet stops sending it.
- **Start in the role's copy and pass no `--add-dir`.** The session would create, and it could not read or edit the worktree. A timekeeper that cannot open the repo cannot do its work.
- **Copy the role's rules and skills into the worktree.** The worktree would hold files the gate does not expect, and the role's files would have a second home. Refused: the role's files stay where they are written.

## Consequences

- A command given for Grok must take `--model`. It need not take `--add-dir`. `--trust` stays on the launch line and stays out of the flag check, because `--help` on 1.0.70 does not name it.
- An unbound worker and a watch are unchanged: the stock command, `--trust`, and the model, with no role.
- The team's copy is not a directory of the session. Its rules are the text in `rules/told.md`. A file in that copy that the rules do not carry is not visible to the worker.
- A skill file beside a role's `SKILL.md` is outside the session cwd. Grok loads the skill from the named directory at startup. A later read of a sibling file is a path the tool can open only if the tool is not confined to the cwd.
