---
status: accepted
---

# The host kit's agent setup may be used by another program

Follows ADR-0058, ADR-0059 and ADR-0060 (sunfmin/control#758).

Guild machines carry no afkconvoy (control ADR 0012, 2026-11-12), yet each harness on one is to hold the operator's agent setup — the instructions, the skills and the guards — and the machine what workers need, tmux. The host kit is the one place that knows where each harness reads its setup (control ADR 0009): a second copy in Guild would drift from it.

What the host kit does to a machine is its own crate, `crates/host-kit` (`afkconvoy-host-kit`), which afkconvoy's host kit and `doctor` go through, and Guild's `guild install` calls:

- `found`: the harnesses on the machine, each known harness whose program the login shell finds.
- `instructions`: the operator's instructions file linked at each harness's global instructions location, what stood there kept aside.
- `skills`: the skill list installed through `npx skills` for each harness's agent, what it does not name removed, the shared copies read-only between syncs.
- `guards`: the guards wired into each harness's own hook settings, only the kit's own entries replaced.
- `tmux`: tmux installed on a Guild machine that has none, where a standard account can.
- `give`, `differences`, and `machine` and `report` that find the harnesses first: the setup `Declared` — where the instructions file, the skill list and the guard list are — given to harnesses, and each way a machine differs from it.

How a line is run in the login shell of the host's user is the embedder's (`Shell`): afkconvoy's is the login shell a worker gets; the crate's own `LoginShell` is the user's `$SHELL` run as one. Where the declaration is named, what was given is recorded, when a harness is given it again and how a difference is reported stay the embedder's: afkconvoy's host settings, `agent-setup.json` beside the host kit, a tick's harness held since, and a `doctor` finding with the line `afkconvoy kit`.

Another program depends on it as a git dependency pinned to a revision of this repo; it is not published. It depends on the harness crate, for where each harness reads its setup, and on nothing of the fleet: no team, role, stage, issue, claim, patrol, intake or tracker type appears in its API. It is not checked by Flux: it is effects on a machine's files and shell, moved from the cli, which Flux does not check either.

## Considered options

- **Let Guild run `afkconvoy kit`.** Refused by control ADR 0012: Guild machines carry no afkconvoy.
- **Put it in the harness crate.** Refused: the harness crate says what each harness is, and runs nothing; the host kit writes files, runs `npx skills` and installs tmux.
- **Copy the agent setup into Guild.** Refused: two places would know where each harness reads its setup (control ADR 0009).

## Consequences

- Where a harness reads its instructions, skills and hooks is still spelled only in the harness crate; what is done there, only in the host kit crate. afkconvoy's `agent_setup` and `tmux` modules map its host settings and its findings onto it.
- A change to the crate's API is a change to Guild's dependency: it stays neutral.
- A test of the crate from outside gives a scratch home the declared setup for every harness a shell finds, installs tmux on a Guild machine, reads back that nothing differs after, and checks the crate depends on nothing of the fleet, nor names a fleet concept in a public item.
