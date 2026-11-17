---
status: accepted
---

# The base branch is a config key

afk-fleet keeps the base branch out of every file: a human confirms it at each launch, it is recorded on the remote, and it cannot change while any claim or live fleet stands on it. afkconvoy puts `base_branch` in the repo's config file instead, so a start asks nothing and needs no flag.

## Consequences

- Nothing stops two fleets from landing one backlog on two branches when they read different versions of the config file. afk-fleet's remote record prevented that; afkconvoy accepts it.
- Versions are cut from the base branch. A repo that names a proving branch lands its work there, and the base branch moves only by promotion (ADR-0090).

## A task's target

The base branch is the repo's ordinary destination. Before a task is claimed, an operator may send it elsewhere: to an existing branch of the same repo with `afkconvoy target`, or into a Jira delivery with `afkconvoy delivery --issue` (ADR-0041). A branch selection is the issue's first `<!--convoy:target …-->` comment and is immutable: another destination needs another task.

The target travels with the claim. A new claim pins the selected target, or the configured base if none was selected, in its own record, so a later config change cannot move it and a takeover by another host keeps it. A host's own database cannot hold it, since a dead host's claim is taken over elsewhere. Nothing of the target is left per issue on the remote once the claim ends.

Every step that acts on the task reads that pinned destination. A missing branch, an unreadable record or a target that contradicts its claim is a refusal, never permission to use the configured base.
