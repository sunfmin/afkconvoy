---
name: review-code
description: Review the code an issue's PR holds against its reviewed plan, and pass or reject it. Use when the brief you were started with names `review-code` as the skill to perform.
---

# review-code

1. **Read the issue, the plan and its review** among the stage records your brief lists, then the diff: what the branch holds past the base your brief names.
2. **Hold the diff against the plan.** A file changed that the plan does not name, or a part of the plan with no change, is a blocking finding unless the treatment's text says why.
3. **Run it.** In the worktree, run the repo's gate and the tests the plan names. Then try what the tests do not: the edge the issue is about, one step either side of it. Quote what you ran and what it printed.
4. **Read the tests as a sceptic.** Would each fail without the fix? A test that was changed or removed to pass is a blocking finding, unless it sits in a commit of its own that says why the test was wrong.
5. **Review the design of the diff.** Load the `complexity-review` and `single-source-of-truth` skills and run them over the diff and the interfaces it touches. Blocking: a fact the diff gives a second home with nothing that fails when the two drift. Every other finding is a note, written in the `complexity-review` skill's output form.
6. **On rework**, your brief names the rejection that sent the issue back. Go through its blocking findings one by one: answered, or not. Then review what changed since.
7. **Declare**, with the command your brief gives:
   - `pass` — no blocking finding. The text says what you ran, and lists your notes.
   - `reject` — at least one blocking finding, each with its evidence, then the notes.

Leave nothing behind in the worktree: what running the tests wrote, remove.
