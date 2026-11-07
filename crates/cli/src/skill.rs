//! `afkconvoy skill`: the skill file an agent installs to use afkconvoy,
//! held in the binary.

/// The skill file `skill` prints, and the one `npx skills add` installs: a
/// user's own agent learns from it that afkconvoy is there.
pub(crate) const SKILL: &str = include_str!("../../../skills/afkconvoy/SKILL.md");

/// What `afkconvoy skill --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

Once for each agent of your own that is to learn of afkconvoy: it prints the skill file to install into it. The file says when to reach for afkconvoy, and sends the agent to `afkconvoy --help`. It repeats nothing the help says, so it has nothing to go stale: a newer release asks for no newer file.

Where to put it, for Claude Code — for a user, on every project of theirs:

```
mkdir -p ~/.claude/skills/afkconvoy
afkconvoy skill > ~/.claude/skills/afkconvoy/SKILL.md
```

and for a project, from its root, committed for everybody who works in it:

```
mkdir -p .claude/skills/afkconvoy
afkconvoy skill > .claude/skills/afkconvoy/SKILL.md
```

## Who runs it

Whoever sets an agent up: a human, or the agent a human is talking to. In any directory: it is asked nothing about a repo.

## What it reads and writes

Nothing. The file is part of the binary, and it is printed: where it goes is yours to say, with a redirection as above.

## What it answers

The file's text and no JSON object: a frontmatter of `name` and `description`, as a Claude Code skill needs, and under it the one instruction. It exits 0.

Errors: none of its own.

## What trips you up

- It installs nothing. Printed to a terminal, the file is nowhere an agent reads it.
- A worker needs none of it: a worker is told what to run by its prompt and its role.
- The directory is named as the skill is: `afkconvoy`, holding `SKILL.md`."#;
