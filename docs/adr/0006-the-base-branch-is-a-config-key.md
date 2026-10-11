# The base branch is a config key

afk-fleet keeps the base branch out of every file: a human confirms it at each launch, it is recorded on the remote, and it cannot change while any claim or live fleet stands on it. afkconvoy puts `base_branch` in the repo's config file instead, so a start asks nothing and needs no flag.

## Consequences

Nothing stops two fleets from landing one backlog on two branches when they read different versions of the config file. afk-fleet's remote record prevented that; afkconvoy accepts it.
