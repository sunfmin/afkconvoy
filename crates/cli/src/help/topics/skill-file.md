What a skill's file holds: where it is, what it opens with, and what `afkconvoy check` holds it to. What belongs in a skill and what in a role's rules or its limits is in [`afkconvoy help roles`](help:roles); the other files of a role are in [`afkconvoy help team`](help:team).

## Where it is

A skill is a directory of its role's `.claude/skills/`, and its file is the `SKILL.md` there. The directory's name is the skill's: it is what a `do` says — a stage's, a consult's, a landing's, a patrol's — and what the file's `name` says. A role with the skills `review` and `rule` holds:

```text
.claude/skills/review/SKILL.md
.claude/skills/rule/SKILL.md
```

The skills are written once, there, whatever harness a stage runs the role on.

## What it holds

```markdown
---
name: review
description: Review the code an issue's PR holds, and pass or reject it. Use
  when the brief you were started with names `review` as the skill to perform.
---

# review

1. Read the issue, every comment on it, and the diff of its PR.
2. Reject what blocks; say what is only a note, and pass on notes alone.
3. End as the brief you were started with says to.
```

The file opens with frontmatter: a first line `---`, yaml, and a line `---`. Every harness reads two keys of it, and both must be there:

- `name`: the skill's name: the name of its directory, letter for letter
- `description`: what the skill does and when to use it, in a sentence or two. A harness shows a worker this and not the file, until the worker loads the skill: say there that the brief names it

Any other key is one harness's own, and the fleet reads none. What stands under the frontmatter is the skill: markdown, in whatever form says how the work is done well. Other files of the directory are the skill's to name and a worker's to open.

## What `check` holds it to

`afkconvoy check` reads the `SKILL.md` of every skill a `do` names, and a fault at that `do` is one of:

- the skill's directory holds no `SKILL.md`
- the file does not open with frontmatter, or its yaml cannot be read
- its `name` is missing, or is not the name of its directory
- its `description` is missing or empty

A skill no `do` names is not read: a role may hold skills by other authors, which a worker loads when its own skill says to. What the file says under its frontmatter is nobody's to check but yours: [`afkconvoy help roles`](help:roles) says what makes it good.

## See also

- [`afkconvoy help roles`](help:roles): what goes in a skill, and what in rules and limits
- [`afkconvoy help team`](help:team): every file of a role, path by path
- `afkconvoy scaffold --help`: a role's skeleton, with one example skill
- `afkconvoy check --help`: every fault, and its key
