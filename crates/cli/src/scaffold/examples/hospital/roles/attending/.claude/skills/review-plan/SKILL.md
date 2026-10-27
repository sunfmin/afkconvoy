---
name: review-plan
description: Review an issue's treatment plan before any code is written, and pass or reject it. Use when the brief you were started with names `review-plan` as the skill to perform.
---

# review-plan

The plan is the text of the last `pass` of the workup among the stage records your brief lists.

1. **Read the issue first, then the plan.** Write down, before reading the plan, what the issue asks for. Then hold the plan against that.
2. **Check the diagnosis.** Run what the plan says reproduces the problem. A diagnosis you cannot reproduce is a blocking finding.
3. **Check the change against the diagnosis.** Does it treat the cause the plan names, or a symptom? Does it touch what it need not?
4. **Check the design.** Load the `codebase-design` and `single-source-of-truth` skills and hold the plan's `Change` against them. A plan that gives a fact a second home — a constant, a list, a rule copied where it could be read — with nothing that fails when the two drift has a blocking finding. Anything else these skills turn up is a note.
5. **Check the tests.** Would each test named fail today and pass after? Is there a case the issue names that no test holds?
6. **Check the open questions.** A plan that decides what a user would see, where the issue, applicable rules and source decisions do not, has a blocking finding: that question is not the plan's to settle.
7. **Declare**, with the command your brief gives:
   - `pass` — no blocking finding. The text lists your notes; the worker who treats the issue reads them.
   - `reject` — at least one blocking finding. The text lists each with its evidence, then the notes. It is all the next workup is told: write it for someone who has not read your mind.
