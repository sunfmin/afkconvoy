---
name: audit
description: Audit an issue's record before its PR lands, and pass or reject it. Use when the brief you were started with names `audit` as the skill to perform.
---

# audit

Check each box, in the worktree your brief names. The stage records are the ones your brief lists; `gh issue view <n> --comments` shows their text.

- [ ] **The code was reviewed as it stands.** The last record of the code review is a `pass`, and no commit that changes code was pushed to the branch after it (`git log` against the record's time; a merge of the base branch or of the landing train is not such a commit).
- [ ] **The plan was reviewed**, where the workflow has a plan review: its last record is a `pass`.
- [ ] **Every blocking finding was answered.** For each `reject` among the records, the stage it sent the issue back to answered each finding, in the text of its next `pass`.
- [ ] **One open PR closes the issue.** `gh pr list --search "<n> in:body"` shows it, and its body holds `Closes #<n>`.
- [ ] **The PR's body says how the change was verified**, and what was not.
- [ ] **The diff holds only the change.** No file the plan does not name; nothing a tool left behind — a log, a binary, an editor's file.
- [ ] **The history reads.** Each commit's message says what it does; none says "wip" or "fix" alone.
- [ ] **No test was removed or loosened** outside a commit of its own that says why.

Then declare, with the command your brief gives:

- `pass` — every box checked. The text is the checklist, each box with what you looked at.
- `reject` — any box unchecked. The text is the checklist, with what is missing under each unchecked box.

You do not run the gate: the landing does, and you are not here to judge the code.
