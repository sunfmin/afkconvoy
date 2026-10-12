---
status: proposed
---

# The stage an issue is at is derived from its stage records

Every stage ends with a verdict, declared through the fleet and written in a stage record on the issue. Where an issue is in its workflow, and how much rework it has had, are computed each tick from those records and the workflow's transition table. No label and no ref stores either. A `stage:` label is written for humans to filter by and is never read back, like the status board.

A label as the state — one per stage, moved by the fleet — was the obvious alternative. It would be a second record beside the comments, and the two disagree the first time a label write fails or a human moves a label.

A workflow states its transitions outright: each stage says where `pass` leads and, if it may reject, where `reject` leads. The other three verdicts mean the same in every workflow — `blocked` parks, `already-satisfied` ends, `escalate` asks upward — because a role is used by many workflows and has to be told once when to declare each.

## Consequences

- A worker never moves an issue. It declares a verdict; the fleet reads the table.
- Opening a PR declares nothing. A stage that wrote one still declares `pass`, after the gate is green.
- `needs-human` is renamed `escalate`: the worker says it cannot decide, and the workflow says who is asked.
- `reject` may only lead to an earlier stage, so every cycle in a workflow passes through a counted rejection and no separate check for unbounded loops is needed. What is checked when a team is loaded: every stage can be reached, every stage can reach an end, and every role and skill named exists.
- A workflow changed under an issue in flight can leave a record naming a stage that no longer exists. The fleet does not guess; it escalates and says the workflow changed.
- A human steers an issue by declaring a verdict or a ruling through the fleet, which adds a record. Moving a label does nothing.
- Splitting an issue needs no state of its own: it is a park on issues the stage itself opened (see Split).
