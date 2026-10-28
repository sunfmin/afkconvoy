---
status: accepted, its probe-answered ids superseded by ADR-0030
---

# A model is held by what it answers as, and any of its names matches it

One model goes by several names. Claude Code's `opus`, `default`, `claude-opus-5-5` and its display name `Opus 5.5` are one model; so are pi's `openai-codex/gpt-6-sol` and Codex's `gpt-6-sol`. ADR-0020 compared a model's name as an exact string, so a roster, a `model:` label or a stock consult that named one of these while the host kit held another found nothing.

The host kit now holds each model of a harness once, by its **id**: the model its probe answered as, which is what a worker is started on. Each harness's probe says that in its own way: Claude Code in the `modelUsage` of its JSON answer, pi in its JSON events, Codex in the session file of the thread its JSON events name, DSH through the probe plugin, which prints the model each request is sent to. The names of one harness that answered as one model are kept with it as its **names**, with the names the harness shows them by. A name whose probe says nothing of what answered is its own id. One model on two harnesses is held once on each, with one `good_at`.

A model named anywhere matches a model its harness holds when it is the id or one of the names, whatever the case, or, with the provider prefix left off, where that leaves one match. A name with two matches matches neither, and says which two: guessing would start a worker on a model nobody chose. The fleet itself writes ids in a roster's draft, so what it writes keeps matching when an alias moves to another model; a refresh of the host kit follows the alias, and an id still matches while its harness lists it. A model label is left as the human wrote it.

A name that matches nothing is said with the nearest names the harness holds. The fleet never changes a team's roster: for an entry that matches nothing, `check`, `plan` and `doctor` name it, and a tick opens one issue in the team's repo, with that repo's ready label, so the team's own fleet corrects it. A model label that is not followed is left as written and said once; the worker runs the user default. Nothing is cast again (sunfmin/control#189 supersedes casting an unmatched label again). A human's label stays.
