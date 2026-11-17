---
status: accepted
---

# Agents record instead of report, and a lesson goes to its lasting home

An agent told to "say it in the report" writes it in a terminal or at a session's end: the operator is not there to read it, and it cannot be found afterwards; and the agent stops, or goes round, to write it. The operator wants a place to read after the fact, and a repo that holds only what lasts: what is read once and passed does not go in.

So:

- **An agent records and carries on; it does not stop to report.** Only what is the operator's to decide interrupts them — money, the one kind left to a human (ADR-0040) — through an escalate and the resident session that asks (ADR-0052).
- **A record is kept where the thing happened.** What was done and how it was checked: the commit message and the PR body. A decision made on an issue: a comment on that issue (ADR-0039, for every decision an agent takes on its own). What belongs to no issue: the **daybook**.
- **The daybook** is written by `afkconvoy daybook`, one entry at a time, each entry in one shape; where a host keeps it is ADR-0118. It is not fleet state: nothing reads it to decide what the fleet does, and it is never work.
- **Lasting memory has no store of its own: it goes to its home.** An entry may carry a **lesson**. The watch's weekly **distillation** carries each lesson through its home's normal path:
  - a repeated mistake, first as a mechanism — a hook, a gate check, a tool's change — a ticket to the repo that owns it;
  - one project's decision or term, a ticket for that project's ADR or GLOSSARY;
  - a cross-project decision or term, the control repo's own ADR or GLOSSARY, changed directly;
  - how to use a tool, a ticket for the skill that tool ships;
  - how one of Guild's roles does its kind of work, whatever the repo: a lore PR, which Guild's curator proposes and its warden reviews;
  - how the operator thinks — the global principles in sunfmin/dotfiles — a draft PR they read and no fleet merges.

  Once proposed, its daybook entry is left alone.

## Not a precedent

ADR-0039 ended automatic precedents: a precedent was a second home for a rule, and what an agent wrote read, once merged, as a human's decision. A distillation also proposes on its own, but it keeps nothing — the proposal is an issue, a doc change or a draft PR, its content in the home it belongs to, with no precedent, proposal or memory list — and whatever touches the operator's principles is a draft PR merged only once they have read it. Issues and doc changes follow their repo's own rules, as an issue a human opened does.

## Considered options

- **A file a day in the repo (`journal/YYYY-MM-DD.md`).** It greps, but fills the repo with what is read once. The operator refused it.
- **An issue a day.** It mixes with the issues, and a fleet might take it for work. Refused.
- **Every record an ADR.** An ADR is for what is hard to reverse, surprising and a real trade-off; a stream of records would bury the decisions. Refused.
- **Claude Code's auto-memory.** One per machine and project directory — a second home again — and neither the fleet nor another harness's session reads it. Refused.
- **Calling it the journal.** The GLOSSARY's tick log avoids that word: it is the daybook.

## Consequences

- The operator's global instructions say "record, do not report" and where a record goes.
- The host runner records each round the watch reports in the daybook.
