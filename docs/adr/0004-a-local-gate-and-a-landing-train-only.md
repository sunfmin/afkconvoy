---
status: accepted, amended by ADR-0008
---

# The gate is local

afk-fleet has two gates (GitHub checks, or a local command) and an optional adversarial verify. afkconvoy keeps one: the gate is the repo's `gate_command`, run locally.

GitHub checks are never read, and there is no adversarial verify. That removes afk-fleet's landing turn — its marker, the merge queue, giving a turn up, fixing off a turn.

## Consequences

- `gate_command` is required. A repo whose CI cannot be reproduced locally cannot use afkconvoy.
- The base branch must accept a direct fast-forward push and must not require status checks; a start on one that does is refused.

## Amended by ADR-0008

"There is no adversarial verify" now holds for an unbound repo only. In a bound repo a review is a stage a workflow may have, before the PR lands. The gate is still the only machine check, and the landing turn is still gone.

The worker that wrote a PR no longer lands it when it is done, since stages follow it. A workflow that lands names the role whose worker gates and lands after its last stage passes. What that worker changes to resolve a conflict on landing is not reviewed again; the gate alone covers it.
