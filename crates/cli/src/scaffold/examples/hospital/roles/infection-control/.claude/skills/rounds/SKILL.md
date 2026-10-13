---
name: rounds
description: Run the gate on the base branch, and raise a fleet lock when it is broken. Use when the brief you were started with names `rounds` as the skill to perform.
---

# rounds

1. **Is a lock standing?** `gh issue list --state open --label <the lock label>` — the label is `lock_label` in `docs/agents/afkconvoy.md` of the checkout, `fleet-lock` when the file names none. One standing: stop, there is nothing for you to do.
2. **Run the gate** in the checkout your brief names: the `gate_command` of `docs/agents/afkconvoy.md`. Green: remove what it left behind, and stop.
3. **Red: run it again.** Green this time: open one issue that says the gate is flaky, with both runs' output, take it in to the workflow for small changes, and stop. No lock.
4. **Red twice: the base branch is broken.**
   1. Find the commit that broke it, if a look at `git log` and the failure shows it; do not bisect for more than a few minutes.
   2. Open one issue. Title: what fails. Body: the gate's command, the failing output, the commit you hold responsible and why — or that you found none.
   3. Take it in, with the command your brief gives, to the workflow with the fewest stages that ends in a landing — count the stages in the team's `workflows/` files, do not go by the name. The base branch is broken for everyone until this lands: every extra stage is minutes the whole fleet waits.
   4. Raise the fleet lock on it, with the command your brief gives.
5. **Stop.** There is nothing to declare.
