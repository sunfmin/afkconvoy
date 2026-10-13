---
name: review
description: Review how the fleet's work went since the last review, and open issues for what should change. Use when the brief you were started with names `review` as the skill to perform.
---

# review

1. **Collect the cases.** Among the issues closed or escalated since the last review (`gh issue list --state all --search "updated:>=<date>"`), the ones worth a look: any with more than three stage records that are a `reject`, any handed to a human, any whose PR was reverted.
2. **For each, read its stage records in order** and say what happened in two sentences. Then ask what would have caught it one stage earlier: a missing test, a gate that does not check it, a document that says nothing, a rule a role lacks.
3. **Group.** One corrective action per cause, not per case.
4. **Open an issue for each action that is a change to this repo** — a test, a check in the gate, a document — with the cases as its evidence, and take it in with the command your brief gives.
5. **What is a change to a role or to the team** is not this repo's to make: open one issue that lists those, titled "Process review: changes proposed to the team", and take it in to nothing. A human reads it.
6. **Stop.**
