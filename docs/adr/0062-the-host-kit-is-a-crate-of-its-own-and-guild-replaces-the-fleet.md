---
status: accepted
---

# The host kit is a crate of its own, and Guild replaces the fleet

Source: the decision of 2026-11-12 on sunfmin/control#693 ([comment](https://github.com/sunfmin/control/issues/693#issuecomment-6812741625)). Guild's side of it is in sunfmin/guild's `docs/adr/` (sunfmin/control#764).

## The host kit is a crate of its own

What the host kit does to a machine is its own crate, `crates/host-kit` (`afkconvoy-host-kit`). afkconvoy's host kit and `doctor` use it, and so does Guild's `guild install`; a Guild machine carries no afkconvoy. It is the one place that knows where each harness's setup is written — through the harness crate, which says where each harness reads it — and Guild keeps no copy of that.

The crate does what the host kit does, and nothing else:

- `found`: the harnesses on the machine, each known harness whose program the login shell finds.
- `instructions`: the operator's instructions file linked at each harness's global instructions location, what stood there kept aside.
- `skills`: the skill list installed through `npx skills` for each harness's agent, what it does not name removed, the shared copies read-only between syncs.
- `guards`: the guards wired into each harness's own hook settings, only the kit's own entries replaced.
- `tmux`: tmux installed on a Guild machine that has none, where a standard account can.
- `give` and `differences`, and `machine` and `report`, which find the harnesses first: the setup `Declared` — where the instructions file, the skill list and the guard list are — given to harnesses, and each way a machine differs from it.

The operator's declaration stays where it is: sunfmin/dotfiles holds the content, and the host settings name where it is. How a line is run in the login shell of the host's user is the embedder's (`Shell`): afkconvoy's is the login shell a worker gets; the crate's own `LoginShell` is the user's `$SHELL` run as one. Where the declaration is named, what was given is recorded, when a harness is given it again and how a difference is reported are the embedder's too: afkconvoy's host settings, `agent-setup.json` beside the host kit, a tick's harness held since, and a `doctor` finding with the line `afkconvoy kit`.

Another program depends on it as a git dependency pinned to a revision of this repo; it is not published. It depends on the harness crate and on nothing of the fleet: no team, role, stage, issue, claim, patrol, intake or tracker type appears in its API. It is effects on a machine's files and shell, and is not checked by Flux.

## Guild replaces the fleet

afkconvoy is to be replaced by Guild. What is replaced is the fleet. The harness, gate, landing and host kit crates stay, as libraries Guild uses pinned by revision (ADR-0058, ADR-0059, ADR-0060 and this one).

Until the replacement is done, a repo belongs to one of them alone: a repo whose truth is GitHub is afkconvoy's, its issues on sunfmin/control; one whose truth is Guild's Forgejo is Guild's. When a repo moves to Guild is Felix's to decide.

## Consequences

- Where a harness reads its instructions, skills and hooks is spelled only in the harness crate; what is done there, only in the host kit crate. afkconvoy's `agent_setup` and `tmux` modules map its host settings and its findings onto it.
- A change to any of the four crates' API is a change to Guild's dependency: it stays neutral of the fleet.
- A test of the host kit crate from outside gives a scratch home the declared setup for every harness a shell finds, installs tmux on a Guild machine, reads back that nothing differs after, and checks the crate depends on nothing of the fleet, nor names a fleet concept in a public item.
