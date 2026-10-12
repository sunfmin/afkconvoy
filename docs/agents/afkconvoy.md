# afkconvoy config

What `afkconvoy` reads for this repo: the yaml block below, and nothing else in
this file. An unknown key or a wrong type is an error, not a default.

```yaml
base_branch: main                 # workers cut from it, open their PR against it, land on it
gate_command: ./gate.sh           # what must pass before a PR lands
ready_label: ready-for-agent      # an issue is dispatchable while it carries this
epic_labels: [epic, prd]          # never dispatched
concurrency: 3                    # claims held at once, a worker each
escalate_label: ready-for-human   # swapped in for ready_label when the fleet gives an issue up
```

`base_branch` and `gate_command` have no default. The rest default to the values
shown, and so does `lock_label`, left out here: `fleet-lock`.

This repo names no `team`, so it is unbound. A bound repo's three keys more —
`team`, `patrols`, `intake_limit` — are in `afkconvoy config --help`.
