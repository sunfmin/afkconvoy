---
name: triage
description: Take the open issues that are in no workflow in to the workflow that fits each. Use when the brief you were started with names `triage` as the skill to perform.
---

# triage

1. **List what waits.** In the checkout your brief names:

   ```bash
   gh issue list --state open --limit 100 --json number,title,labels,body
   ```

   An issue waits for you when it carries no label that starts with `flow:`, and none of: the escalate label and the lock label (`docs/agents/afkconvoy.md` and the team file name them; the defaults are `ready-for-human` and `fleet-lock`), an epic label, `wontfix`, `needs-info`.
2. **Read each one in full**, comments included (`gh issue view <n> --comments`).
3. **Choose its workflow.** The workflows of the team are in your brief; each is a file under `workflows/` of the team's directory your worker was given, and its opening lines say what it is for. Read them — do not guess from a name.
4. **Can it be worked as written?** It says what is wrong or wanted, and how one would know it is done. If so, take it in with the command your brief gives. If not, and you have not asked before, comment what is missing and go on to the next.
5. **Stop** when the list is done, or when the command refuses for a limit: that is the fleet saying enough is open.

You open no issue of your own: triage routes what is there.
