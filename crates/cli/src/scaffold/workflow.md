# {workflow}

The workflow an issue follows while it carries the flow label `flow:{workflow}`: a workflow is
named by its file. This one has one stage, which the role `{role}` performs as its skill
`{skill}`. Its `pass` leads to the landing train, which a worker in the role under `land` joins.

```yaml
stages:
  - name: build
    by: {role}
    do: {skill}
    on: { pass: land }
land: { by: {role}, do: {skill} }
```

To see it drawn: `afkconvoy show {from}`. To see where a list of verdicts would take an issue,
with no worker started: `afkconvoy rehearse {from} {workflow} pass`.
