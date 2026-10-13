---
name: record-precedent
description: Propose a human's ruling on an escalated issue as an entry of the team's precedents. Use when the brief you were started with names `record-precedent` as the skill to perform.
---

# record-precedent

Your brief quotes the ruling, names the team's repo and the branch to open the PR from, and gives the form of an entry. Follow its form and its hard rules to the letter: a PR that does anything else is closed.

1. **Read the issue and the escalation** the ruling answers: what was asked, and why no stage could decide it.
2. **Is there a precedent in it?** There is when the question will come up again on another issue, and the ruling would be the same there. A ruling that only settles this issue — "use approach B here" — has none: open no PR, and stop.
3. **Read the precedents kept**, in the file your brief names. One that covers the case already: open no PR, and stop.
4. **Word the entry.** One sentence a stage can act on: the situation, then what to do. "When an interval names no fixed period, …" — not "see issue 12". Add when it applies, where that is narrower than the sentence.
5. **Open the PR**, in a clone of the team's repo of your own, outside the checkout: `gh repo clone <team> <a new directory under $TMPDIR>`, the branch your brief names, the one line appended at the end of the precedents file — the file is created when it is not there — commit, push the branch, `gh pr create`. The PR's body quotes the ruling and links the issue.
6. **Stop.** A human merges it, or does not.
