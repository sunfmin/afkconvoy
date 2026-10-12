---
status: accepted
---

# A worker is limited by guards, not by its identity

Much of a workflow rests on what a worker may not do: move a flow label, write a stage record by hand, commit in a stage that reviews, merge a PR, push the base branch. Every worker runs unattended under the one GitHub login of its host, so none of that is beyond its reach, and GitHub cannot tell a record the fleet wrote from one a worker wrote.

The limits are guards: hooks that run before each thing a worker does. A role's own limits are hooks in the role's repo. The limits that hold for every worker are added by the fleet at launch and decided in one place, so that no role can be created without them. After a stage the fleet checks two things a guard may have missed: that the record's stage is the one it dispatched, and that a stage marked as writing nothing left no commit.

Giving workers a GitHub identity of their own, narrower than the host's, was considered. It means managing several credentials and still does not separate two processes of one user on one machine.

## Consequences

- A guard recognises commands. A worker that wraps the same act in a script gets past it. This protects against a worker that errs, not one that has been turned.
- What makes that acceptable is elsewhere: the gate is run by the fleet, and nothing reaches the base branch except through the landing train. The worst a stray worker does is confuse one issue, which shows as an escalation.
- Text a worker reads in the repo — an issue body, a comment, a document — can carry instructions. A repo that takes issues from strangers needs the separate identity first.
