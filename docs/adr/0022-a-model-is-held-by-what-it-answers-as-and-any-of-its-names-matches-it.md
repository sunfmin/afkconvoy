---
status: accepted; where a model's id and names come from is ADR-0093's
---

# Any of a model's names matches it

One model goes by several names. Claude Code's `opus`, `default`, `claude-opus-5-5` and its display name `Opus 5.5` are one model; so are pi's `openai-codex/gpt-6-sol` and Codex's `gpt-6-sol`. ADR-0020 compared a model's name as an exact string, so a roster, a `model:` label or a stock consult that named one of these while the host kit held another found nothing.

The host kit holds each model of a harness once, by its **id**, which is what a worker is started on, with its **names** beside it (ADR-0093). One model on two harnesses is held once on each, with one `good_at`.

A model named anywhere matches a model its harness holds when it is the id or one of the names, whatever the case, or, with the provider prefix left off, where that leaves one match. A name with two matches matches neither, and says which two: guessing would start a worker on a model nobody chose. The fleet itself writes ids in a roster's draft. A model label is left as the human wrote it.

A name that matches nothing is said with the nearest names the harness holds. The fleet never changes a team's roster: for an entry that matches nothing, `check`, `plan` and `doctor` name it, and a tick opens one issue in the team's repo, with that repo's ready label, so the team's own fleet corrects it. A model label that is not followed is left as written and said once; the worker runs the user default. A human's label stays.
