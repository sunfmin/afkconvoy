---
name: propose
description: Survey the codebase and open issues for work worth doing. Use when the brief you were started with names `propose` as the skill to perform.
---

# propose

1. **Read what is known**: the open issues, the issues closed in the last month (`gh issue list --state closed --limit 50`), and the repo's own documents.
2. **Survey**, in the checkout your brief names, for what your rules count as a proposal: a defect you can show, a case the documents promise and the code does not hold, a test that holds nothing, a dependency with a known fault.
3. **Rank** what you found by what it costs a user of the code today. Keep the top three at most.
4. **Open an issue for each**: what is wrong or missing, the evidence, and the acceptance criteria. Then take it in to the workflow that fits, with the command your brief gives — each workflow's file, under `workflows/` of the team's directory your worker was given, says what it is for.
5. **Stop.** A run that found nothing worth proposing opens nothing, and that is a good run.
