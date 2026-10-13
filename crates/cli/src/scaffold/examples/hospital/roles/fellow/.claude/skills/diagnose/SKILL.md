---
name: diagnose
description: Work an issue up into a treatment plan, changing no file. Use when the brief you were started with names `diagnose` as the skill to perform.
---

# diagnose

A workup. It ends in a plan somebody else reviews, and it changes nothing: no edit, no commit.

1. **Read the history.** The issue, every comment on it, and every stage record your brief lists. When the brief has a `Rework` or a `Ruling` section, that is why you are here: the plan you write answers it point by point.
2. **Reproduce.** For a defect, find the smallest input that shows it and run it: quote the command and what it printed. For a change that is no defect, show what the code does today. What you could not reproduce, say so — do not plan around a guess.
3. **Differential.** List the causes that would explain what you saw, and what rules each one in or out. Name the one you hold, and the evidence.
4. **Size it.** Estimate the changed lines, tests included. Over 1,000: split the issue as your brief says, and stop there.
5. **Find the open questions.** Anything a user of the code would see that the issue does not decide. Read the precedents; what one covers is decided. What none covers ends the workup: declare `escalate` with a hand-off, as your rules say.
6. **Design the change before writing it down.** Load the `codebase-design` and `single-source-of-truth` skills. Decide which module the new behaviour lives behind and what its callers have to learn; a fact the change needs that already has a home is read from that home, never copied. The plan's `Change` section says where the behaviour lives and why there.
7. **Write the plan**, as the text of your verdict:

   ```
   ## Diagnosis
   What is wrong, or what is missing, and the evidence.

   ## Change
   Each file to touch and what changes in it. Nothing else will be touched.

   ## Tests
   Each test to add, by name, and what it holds. For a defect: the test that fails today.

   ## Risks
   What could break, and how the tests would show it.

   ## Out of scope
   What a reader might expect here that this issue does not do.
   ```

8. **Declare `pass`** with the plan as the text, using the command your brief gives.
