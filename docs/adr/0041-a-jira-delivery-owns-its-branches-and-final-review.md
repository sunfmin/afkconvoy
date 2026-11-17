---
status: accepted
---

# A Jira delivery owns its branches and final review

A task may contribute to an existing branch. A **Jira delivery** gathers such tasks while leaving exactly one final PR for colleague review, per ticket and code repo. Jira is only an external reference: no Jira login, crawling or status synchronization.

## Its records

The delivery definition is an immutable JSON commit at `delivery/<identity>` under the repo's fleet ref layout. It holds the canonical Jira browse URL, code repo, delivery branch, explicit final target and initial commit. The identity includes the Jira site and context path, so matching ticket keys on different sites are separate deliveries. Creation publishes this record and a branch at the final target's existing commit, expecting both refs to be absent. It never overwrites an unrelated branch or adds an empty commit to create a PR. Conflicting requests and malformed records refuse; a retry reads the remote state first.

An attached task's target record holds only its tracker issue identity and delivery identity, never an editable copy of the branch. Resolution reads the delivery definition and refuses missing or contradictory metadata rather than falling back to the configured base. Worker briefs name both task and delivery.

## The final PR

The final PR carries a delivery marker and uses the reserved delivery branch namespace. Fleet ticks, ordinary task landing, recovery and watches never act on it, even when someone adds a task-closing line or it is marked ready. Its number is kept once at `delivery-pr/<identity>`. Reconciliation discovers every PR state before creating or recovering that receipt; a closed or merged final PR is never replaced. A no-diff delivery reports pending without opening a PR. A task's landing reconciles the final PR at once, and acting ticks reconcile deliveries to cover an interruption. Final review stays with humans.

## A handoff certifies the aggregate

Each task's landing gates that task. Those green runs do not say the combined delivery is ready for a human, and they never publish the delivery branch onto the final target.

`afkconvoy delivery --handoff` is the explicit check. An open, not-planned, abandoned or unlanded task, an unresolved claim, retained work, an open internal PR, a failed or cancelled landing, or an open blocker keeps the delivery unready and names the reason; so does an empty set of tasks. It merges the delivery head onto the current final-target head in the object store only. A conflict leaves both branches where they are. A clean merge is gated with the repo's gate in a temporary worktree, and that merge commit is not pushed.

A green result records one evidence snapshot — the delivery head, the final-target head, the task results, the gate tree and command — writes them with the Jira reference into the final PR's body, and marks it ready. It never opens a second final PR and never dismisses reviews. Readiness is whatever `gh pr ready` says, rechecked against the snapshot and a gate record that still stands for that tree: a later task, a new delivery commit or a moved final target withdraws it. A ready snapshot never certifies later work.

Once the final PR has merged or closed, handoff records that once. Completion is that merge onto the declared final target while every attached task is implemented. A merge while attached work is incomplete, or a merged flag whose commit is not on the final target, is a discrepancy; a PR closed without merging needs action. A later live state that differs from the recorded observation is reported, not rewritten.
