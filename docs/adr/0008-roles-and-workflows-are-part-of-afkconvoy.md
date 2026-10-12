---
status: proposed
---

# Roles and workflows are part of afkconvoy, and a team repo holds them

An issue should be able to pass through several stages — plan, review of the plan, implementation, review of the code — each performed by a different role. Running a stage needs exactly what afkconvoy already has: claims, the tick, workers, continuation, the landing train. So roles and workflows are added to afkconvoy rather than built as a tool beside it, and what afkconvoy does today becomes the one path of an unbound repo.

A role is a repo. A team is a repo that names roles and holds workflows, patrols and precedents. A repo binds itself to a team with one `team:` key in its own config.

## Considered options

- **A tool on top, with afkconvoy as its engine.** Both would have to know what the labels and comments on an issue mean; a change to that protocol would touch two tools.
- **A separate tool.** It would rewrite the claims, the tick and the landing train.
- **Workflows in each repo.** One workflow serves many repos; a copy per repo drifts.

## Consequences

- The frontier is no longer a set of issues but of issues at a stage, and the core's rules are restated and proved again (ADR-0007).
- Config splits by who knows the fact. The repo keeps what only it knows — `base_branch`, `gate_command`, `epic_labels`, `concurrency` — and gains `team`, `patrols`, `lock_label` and `intake_limit`. The team holds `escalate_label`. `ready_label` means something only in an unbound repo; a bound repo's issues carry a flow label instead.
- A role's skills are committed into the role repo, not installed per host: several hosts work one repo, and a role must be the same on each.
- A team and its roles are followed at their default branch, not pinned. The commit of each is fixed when a stage starts and written in its stage record, which is what says afterwards which rules let a change through.
- The four things a human does while designing — check a team, scaffold a repo, show a workflow or an issue's place in it, rehearse a workflow against a list of verdicts — are subcommands. Everything else is editing a file.
- Not in the first version: running anywhere but a host, two stages of one issue at once, a stage choosing a later stage's model, cost accounting, notifications.
