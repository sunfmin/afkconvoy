---
status: proposed
---

# An issue is cast within its role's range on the host

ADR-0013 had a stage name its harness and model, and the config name a default. Which harness and model suit a piece of work depends on the issue — how hard it is and what kind of work it is — more than on the stage it is at, and a stage cannot know that when the team is written. And which harnesses and models can run at all is a fact of each host: what is installed, logged in, and answering.

So the choice is made per issue, inside bounds two places set:

- A team's **roster** lists, for each role, the harnesses and models its workers may run, in order of preference, with what each model is good at and how to cast among them. It is committed with the team, as ADR-0013 wanted which harness does the work to be.
- A host's **host kit** lists the harnesses and models it can start now — each harness's worker launch command and each model seen to answer — and the harness and model of the stock consult. It is generated when `run` starts on a host without one, by finding the harnesses installed and sending each model a prompt, and written once the human at the terminal confirms it; a `run` with no terminal and no host kit does not start. It is never pushed. A harness that stops working is paused on the host and left out of it until a probe finds it working again.
- A role's **range** on a host is its roster entry cut to the host kit, in the roster's order. An unbound repo has no team and ranges over the host kit whole.

An issue's **casting** is its `harness:` and `model:` labels, the model in the harness's own name. A stage whose range holds the casting runs it; one whose range does not runs its range's first; a range of one leaves nothing to choose. An issue none of whose stages has a choice to make is not cast. One that has, and carries a flow label and no casting, is cast by a **caster**: a worker in no role, started by a tick that finds such an issue on the frontier and none running, which casts every such issue of the repo in one session and says on each why. The tick holds those issues back while it runs, for ten minutes at most, and dispatches them uncast after that. A human's labels are never recast; a casting no stage's range holds is not dispatched, and `plan` says why. Issues cast to a harness that is paused are cast again.

From the highest: a switch, then the range's answer to the casting. A human clears a switch with `switch --clear`, which replaces `--harness config`: the casting stands again for every worker started after it, and a worker it names is moved onto what its range and casting say, as a switch moves one. The stock consult runs what the host kit names for it, or the host kit's first; every other worker in no role — the caster, the proposal of a precedent — runs the host kit's first, and a landing runs its issue's casting where the host kit holds it.

## Considered options

- **Keep a stage's harness and model as an override.** Two places would decide which harness runs, and a wrong one would be looked for in both. A role that must run on one harness is given a range of one.
- **The roster on each host.** Which harness does the work changes the result, so it belongs with the team on every host, not with whichever host started the worker; what a host adds is only what it has.
- **A caster's judgement at each start, kept nowhere.** Not reviewable, paid again at every stage, and different from one start to the next. A label is on the issue, where a human sees and changes it.
- **One casting label per role.** Exact, but twice the labels and harder to read; in practice one step chooses and the rest are held to one harness.

## Consequences

- Supersedes ADR-0013's first paragraph: a stage, a patrol and the config name no harness or model, and `check` and `run` refuse a team file or a config that still does, saying what to delete and what the roster says instead. ADR-0013's other half stands: a role writes its limits once per harness, and `run` refuses a role whose range holds a harness it has no limits for, or a range the host kit leaves empty.
- `--worker-command` is gone: a harness's worker launch command is in the host kit.
- In an unbound repo the fleet now appends a harness's model flag to the worker launch command when an issue's casting names a model; with none, the command is still started as written.
- A team's roster is drafted by a command, from the host kit, into the team's working tree, and committed by a human; the fleet never changes it.
- ADR-0008 left out a stage choosing a later stage's model; the caster chooses for an issue's stages before any starts, which is that, and is now in.
