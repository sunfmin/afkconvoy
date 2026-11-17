---
status: accepted
---

# An issue's worker runs within its role's range on the host

A team should be able to have work done, or reviewed, by a harness other than the one that wrote it. Which harness and model suit a piece of work depends on the issue more than on the stage it is at, and which harnesses can run at all is a fact of each host: what is installed, logged in and answering. So a stage, a patrol and a repo's config name no harness and no model; `check` and `run` refuse one that does and say what the roster says instead. The choice is made per start, inside bounds that two places set:

- A team's **roster** lists, for each role, the harnesses and models its workers may run, with what each model is good at. It is committed with the team: which harness did the work changes the result, so it belongs in what is reviewed, not in how one host was started. A command drafts it from the host kit's research; a human commits it, and the fleet never changes it.
- A host's **host kit** lists what the host can start now: each harness's worker launch command and its listed models. It is written with no human and never pushed (ADR-0093 says how models are held and researched). `run` never refuses for want of one.
- A role's **range** on a host is its roster entry cut to the host kit, in the kit's order, less each harness the role holds no limits for while it holds limits for another. An unbound repo has no team and ranges over the host kit whole. `check` and `plan` name each harness a range leaves out, and why. A range left empty starts no worker of that role and refuses nothing else.

## Harness priority

Harness priority has one home: the order of the host kit. Every start of a checkout that names no harness — an issue's stage, a consult, a patrol, a landing, a cut, an inspection — takes the first eligible harness in that order within its role's range. `switch` reorders the kit and keeps no record of its own; a harness the operator takes out with `kit --remove` stays out until a switch brings it back. A `harness:` label never selects a harness, and a `model:` label, a roster or a stock-consult preference cannot select a later one: a model label applies only within the harness already selected. A harness that refused an issue's task is excluded from that issue's later starts, on every host, and stays healthy for other issues. Changing priority alone interrupts no running worker.

A checkout may name the one harness its workers start on, on its entry of the host's checkout list (`run <checkout> --harness <name>`, cleared with `--harness default`); the repo config still names none. Its workers start on that harness or not at all: a role with no limits for it starts no worker, and an account fault, a model fault that leaves the harness no placed model, or a launch that cannot start never starts one on the next harness of the kit. The kit still orders that harness's models. A switch does not change a checkout's named harness, nor move its workers. The host's watch is no checkout's worker and never starts by a named harness. `checkouts`, `plan` and `doctor` say the harness in force for each checkout and where it comes from.

## The model

An issue's **model label** is its one `model:` label. It is followed only when it names exactly one model of the selected harness that the range holds (ADR-0093 says how a name matches). Otherwise — no label, a name matching nothing or two models, two labels, or a model outside the range — the worker runs the harness's user default in the same start, the label is left as written, and it is said once on the issue with the nearest names. A watch, a cut and an inspection have no model label. No caster chooses a model: `afkconvoy cast` and `kit --cast` fail, and an old `cast` key in a roster or a kit is accepted and ignored.

## A role's limits are written per harness

A role has one core every harness works under: its rules and skills, written once. Its limits cannot be shared, so they are written once per harness, in that harness's own form, and never translated. The guard is the fleet's, not a role's: decided in one place, and each harness is given a way to ask it. A harness that asks before loading a role's files is answered on the line that starts it, never at its terminal. What the fleet adds for each harness is `afkconvoy help worker-launch`'s to say.

## Considered options

- **A stage names its harness and model.** A stage cannot know, when the team is written, how hard an issue is. Two places deciding which harness runs would mean a wrong one is looked for in both; a role that must run on one harness is given a range of one.
- **One harness for a fleet instance, chosen where it is started.** Smaller, but the reason for a second harness is to set it against the first within one workflow.
- **The roster on each host.** A host adds only what it has; which harness does the work belongs with the team.
- **A model chosen afresh at each start, kept nowhere.** Not reviewable and different from one start to the next. A label is on the issue, where a human sees and changes it.
- **A role's limits written once and bridged.** The fleet would read Claude Code hooks and run them for another harness, becoming the loader ADR-0009 avoids; a bridge that carries some hooks and silently drops the rest is worse for a limit than a refusal.
- **A checkout's harness falling back to the kit's next.** A checkout names a harness to have its work done there; a silent fallback would do it elsewhere.

## Consequences

- A limit a role holds under two harnesses is written twice. The fleet can tell that a role holds limits for a harness, not that two sets refuse the same things; a harness the role holds no limits for is left out of its range, so no worker is changed into fewer limits unseen.
- A role is right for the harnesses its files cover and no others. A role that holds no limits at all is right for every harness.
- A worker's stage record names the harness it ran on.
- In an unbound repo the fleet appends a model flag only when a model label is followed, or for Codex the user's own preference (ADR-0093); otherwise the worker starts on its harness's user default.
