# The base branch is a config key

afk-fleet keeps the base branch out of every file: a human confirms it at each launch, it is recorded on the remote, and it cannot change while any claim or live fleet stands on it. afkconvoy puts `base_branch` in the repo's config file instead, so a start asks nothing and needs no flag.

## Consequences

Nothing stops two fleets from landing one backlog on two branches when they read different versions of the config file. afk-fleet's remote record prevented that; afkconvoy accepts it.

## Task targets (sunfmin/control#154)

The config key remains the repo's ordinary destination and the sole destination of watches and cuts. An operator may select an existing same-repo branch for one task with `afkconvoy target --issue <n> --branch <branch>` before dispatch or intake. Its internal PR lands there without changing the repo's config or causing a product cut. Several tasks may contribute to one Jira delivery branch, whose final review PR remains outside this task-routing slice.

### Where the target lives (sunfmin/control#1327)

The target travels with the claim. A claim's own record carries its task's pin (`target_branch=…`, or `target_delivery=…` with `target_record=…` for a Jira delivery): a new claim pins the selection, or the configured base if none was selected, so a later config change cannot move it. A takeover by another host reads the pin from the claim it takes and keeps it; the target cannot live in a host's own database, since a dead host's claim is taken over elsewhere. When the claim ends — landed, closed or released — its ref is deleted, and nothing of the target is left per issue on the remote.

Before a claim, a branch selection is the issue's `<!--convoy:target branch=…-->` comment, and the first one stands: a selection is immutable, and a different destination requires another task. It stays on the tracker, so a released task's next claim pins the same branch again. A task attached to a Jira delivery is named in that delivery's one task record (`delivery-tasks/<id>`), which a delivery's hand-off also reads to count its closed tasks; it is per delivery, not per issue. Selection is refused once the issue is claimed or has candidates; a dispatch that finds the claim it pushed pinned otherwise than it prepared for refuses and starts again.

Earlier, selection wrote a parentless record under `target/<tracker claim key>` that was never removed: 327 of them stood against 12 open issues. Each acting tick now reads every such record once, carries it — a claim naming it gets its pin, a non-base branch becomes the issue's selection comment, a delivery's task joins its task record — and removes it; `doctor` reports any still left. Legacy tasks lacking a target retain the configured destination until their next claim resolves it.

Dispatch, stage briefs, guard settings, switches, continuation, landing and settlement read that destination. A missing branch, unreadable record or a target that contradicts its claim is a refusal with a repair diagnostic, never permission to use the configured base. The branch identity is pinned; its tip continues moving through ordinary gated turns.

## Amended by ADR-0090

A repo that names a proving branch has two branches in play (sunfmin/control#1308): a task's target branch is the proving branch, and the base branch moves only by promotion. Versions are still cut from the base branch.
