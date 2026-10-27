# The base branch is a config key

afk-fleet keeps the base branch out of every file: a human confirms it at each launch, it is recorded on the remote, and it cannot change while any claim or live fleet stands on it. afkconvoy puts `base_branch` in the repo's config file instead, so a start asks nothing and needs no flag.

## Consequences

Nothing stops two fleets from landing one backlog on two branches when they read different versions of the config file. afk-fleet's remote record prevented that; afkconvoy accepts it.

## Task targets (sunfmin/control#154)

The config key remains the repo's ordinary destination and the sole destination of watches and cuts. An operator may select an existing same-repo branch for one task with `afkconvoy target --issue <n> --branch <branch>` before dispatch or intake. Its internal PR lands there without changing the repo's config or causing a product cut. Several tasks may contribute to one Jira delivery branch, whose final review PR remains outside this task-routing slice.

Selection writes a parentless remote record under `target/<tracker claim key>` in the settled ref layout. A first claim pins the configured base if none was selected. Competing parentless records cannot fast-forward one another; the winning record is immutable, including between stages and after a claim is released. A different destination requires another task. This deliberately gives up editing a selection after it has been recorded, in exchange for no selection/claim race or second routing authority. Claims retain the target record's identity, and takeover preserves that reference. Legacy tasks lacking a target retain the configured destination until their next claim or turn resolves it.

Dispatch, stage briefs, guard settings, switches, continuation, landing and settlement read that destination. A missing branch, unreadable record or a target that contradicts its claim is a refusal with a repair diagnostic, never permission to use the configured base. The branch identity is pinned; its tip continues moving through ordinary gated turns.
