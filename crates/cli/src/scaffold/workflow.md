# {workflow}

The workflow an issue follows while it carries the flow label `flow:{workflow}`: a workflow is
named by its file. A `label` key names another flow label — `label: ready-for-agent` takes
the issues opened with the ready label. This one has one stage, which the role `{role}` performs as its skill
`{skill}`. Its `pass` leads to the landing train, which a worker in the role under `land` joins.

```yaml
stages:
  - name: build
    by: {role}
    do: {skill}
    on: { pass: land }
land: { by: {role}, do: {skill} }
```

A stage more — a review, say — is a skill written in the role that is to perform it, as
`.claude/skills/<skill>/SKILL.md` in that role's directory, and a stage here that names the two.
No other role written with this file holds a skill yet: none of them is here to `{skill}`. A whole
team to copy from, with a review stage, a role that only reviews and a landing role, is written by
`afkconvoy scaffold team <dir> --example hospital` and read file by file in `afkconvoy help example`.

To see it drawn: `afkconvoy show {from}`. To see where a list of verdicts would take an issue,
with no worker started: `afkconvoy rehearse {from} {workflow} pass`.
