# The gate is local, and every PR lands on the landing train

afk-fleet has two gates (GitHub checks, or a local command) and an optional adversarial verify, and from them two ways to land: a landing turn granted one PR at a time, or the landing train. afkconvoy keeps one of each: the gate is the repo's `gate_command`, run locally, and every finished PR joins the landing train.

GitHub checks are never read, and there is no adversarial verify. That removes the landing turn entirely — its marker, the merge queue, giving a turn up, fixing off a turn — because the verify and the checks gate were the only configurations in which a PR landed alone.

## Consequences

- `gate_command` is required. A repo whose CI cannot be reproduced locally cannot use afkconvoy.
- The base branch must accept a direct fast-forward push and must not require status checks; a start on one that does is refused.
- A marker comment on the PR is still needed, for the two records the landing turn's marker used to carry on a train: that a joining worker was already restarted once, and that the PR was already taken off an abandoned train once.

## Amended by ADR-0008

"There is no adversarial verify" now holds for an unbound repo only. In a bound repo a review is a stage a workflow may have, before the PR joins the landing train. The gate is still the only machine check, the landing turn is still gone, and the train is unchanged.

The worker that wrote a PR no longer joins the train when it is done, since stages follow it. A workflow that lands names the role whose worker gates and joins after its last stage passes. What that worker changes to resolve a conflict on joining is not reviewed again; the gate alone covers it, as it already does for the train worker's repairs.

## Amended by ADR-0034

A command holds a turn through merge, gate and join, and train gates use an independent lane. ADR-0034 replaces the earlier queue ordering and explains how one-at-a-time turns now feed the landing train.

## Replaced by ADR-0036

A turn now lands its gated merge commit directly on the base branch. The landing train, its worker and its lane have been removed; ADR-0036 describes certification, repair and migration of an older line.
