---
name: land
description: Put an audited issue's PR on the landing train. Use when the brief you were started with names `land` as the skill to perform.
---

# land

Your brief gives the steps and the commands, in order. Follow them as written; this says only how.

- **A conflict** is resolved so that both sides' intent survives: the change this PR makes, and the change it conflicts with. Read both before touching either. Then commit the merge and run the gate again, as the brief says.
- **A red gate after a merge** means the two changes do not work together. Fix only what joins them. When that would change what either one does, stop: say on the issue, in a comment, which two changes conflict and why neither can give way. The fleet hands an issue whose landing stopped to a human.
- **Joined is where you stop.** The train lands the PR; you do not wait for it.
