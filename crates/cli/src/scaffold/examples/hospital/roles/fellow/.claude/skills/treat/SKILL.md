---
name: treat
description: Carry out an issue's reviewed plan in its worktree, open its PR and turn the gate green. Use when the brief you were started with names `treat` as the skill to perform.
---

# treat

1. **Find the plan.** It is the text of the last `pass` of the workup among the stage records your brief lists, or of the issue itself when its workflow has no workup. Read the review of it too: a reviewer's notes are part of what you carry out. When the brief has a `Rework` section, read the rejection first — it is the list of what to change, and you change that and nothing else.
2. **Test first.** For a defect, write the test the plan names and run it: it has to fail, for the reason the diagnosis gives. A test that passes before the fix proves nothing — find out why before going on.
3. **Make the change the plan names**, in the issue's worktree. When it needs what the plan does not hold, stop: your rules say to declare `escalate`, not to widen the work.
4. **Commit in steps that each stand**: the failing test, the fix, anything else the plan names. A message says what the commit does, in the imperative.
5. **Review your own diff.** Load the `complexity-review` and `single-source-of-truth` skills and run them over what the branch holds past the base. Fix what the plan lets you fix. A finding whose fix is outside the plan is not fixed here: it goes in the PR's body under `Not verified`, for the reviewer.
6. **Open the PR**, once, against the base branch your brief names. Its body:

   ```
   Closes #<the issue>

   ## What changed
   ## How it was verified
   The commands run, and what each printed.
   ## Not verified
   ```

   A PR that is open already is not opened again: push to its branch.
7. **Turn the gate green**, with the command and in the way your brief says under "The gate, before a `pass`".
8. **Declare `pass`**. The text: what changed, by commit; what was verified and how; what was not.
