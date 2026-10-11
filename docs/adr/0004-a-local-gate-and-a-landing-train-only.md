# The gate is local, and every PR lands on the landing train

afk-fleet has two gates (GitHub checks, or a local command) and an optional adversarial verify, and from them two ways to land: a landing turn granted one PR at a time, or the landing train. afkconvoy keeps one of each: the gate is the repo's `gate_command`, run locally, and every finished PR joins the landing train.

GitHub checks are never read, and there is no adversarial verify. That removes the landing turn entirely — its marker, the merge queue, giving a turn up, fixing off a turn — because the verify and the checks gate were the only configurations in which a PR landed alone.

## Consequences

- `gate_command` is required. A repo whose CI cannot be reproduced locally cannot use afkconvoy.
- The base branch must accept a direct fast-forward push and must not require status checks; a start on one that does is refused.
- A marker comment on the PR is still needed, for the two records the landing turn's marker used to carry on a train: that a joining worker was already restarted once, and that the PR was already taken off an abandoned train once.
