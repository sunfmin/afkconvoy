---
status: accepted
---

# A Jira delivery owns its branches and final review

Task target routing (sunfmin/control#154) lets a task contribute to an existing
branch. A Jira delivery (sunfmin/control#155) gathers those tasks while leaving
exactly one final PR for colleague review, per ticket and code repo. Jira is
only an external reference: no Jira login, crawling or status synchronization.

The authoritative delivery definition is an immutable JSON commit subject at
`delivery/<identity>` under the repo's settled fleet ref layout. It holds the
canonical full Jira browse URL, code repo, delivery branch, explicit final
target and initial commit. The identity includes the site and context path,
so matching ticket keys on different Jira sites are separate deliveries.
Creation atomically publishes this record and a branch at the final target's
existing commit, expecting both refs to be absent. It never overwrites an
unrelated branch or adds an empty commit to create a PR. Conflicting requests
and malformed records refuse. Retrying discovers the remote state first.

An attached task's existing immutable target record holds only its tracker
issue identity and delivery identity. It carries no editable branch copy.
Resolution reads the delivery definition and refuses missing or contradictory
metadata instead of using the configured base. Claims retain the target
record's identity as before. Worker briefs identify both task and delivery.

The final PR carries a delivery marker and uses the reserved delivery branch
namespace. Ordinary task, watch and recovery PR reads exclude it, including
when someone adds a task-closing line. Its number is kept once at
`delivery-pr/<identity>`, a subordinate immutable receipt. Reconciliation
discovers all PR states before creating or recovering that receipt; an
existing closed or merged final PR is never replaced. Concurrent creation
relies on GitHub's same-head/base open-PR uniqueness and re-reads after an
uncertain create. A no-diff delivery reports pending without attempting a PR.

Successful task landing reconciles the final PR immediately. Acting ticks
also reconcile deliveries to cover interruption after task publication.
Inspection is read-only and shows the definition, final PR or pending reason,
and task references. Failures remain visible and retryable. The repo config,
base branch, cuts and human ownership of final review remain unchanged.
