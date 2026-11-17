---
status: accepted
---

# A model is held by its listed id, and research names it

One model goes by several names. Claude Code's `opus`, `claude-opus-5-5` and its display name `Opus 5.5` are one model; so are pi's `openai-codex/gpt-6-sol` and Codex's `gpt-6-sol`. The host kit holds each model of a harness once, by its **id** — the id its harness lists, which is what a worker is started on — with its **names** beside it: those its listing shows it by and those research ties to it with a dated source. One model on two harnesses is held once on each, with one `good_at`.

Writing the host kit sends no model a prompt: not `afkconvoy kit`, not `run` on a host without one, not a switch and not the daily refresh. The listing comes from the login's own server, so it already says what this login can run. A probe that prompts every model is slow, opens a terminal per model, and cannot tell a model unavailable here from a login out of allowance for now.

## Matching a name

A model named anywhere — a roster entry, a `model:` label, the stock consult — matches a model its harness holds when it is the id or one of the names, whatever the case, or, with the provider prefix left off, where that leaves one match. A name with two matches matches neither and says which two: guessing would start a worker on a model nobody chose. A name that matches nothing is said with the nearest names the harness holds.

The fleet writes ids in a roster's draft, and never changes a team's roster: for an entry that matches nothing, `check`, `plan` and `doctor` name it, and a tick opens one issue in the team's repo for the team's own fleet to correct. A model label that is not followed is left as the human wrote it (ADR-0020).

## Research places a model

- **Default first.** A harness found and listed is held at once. Each model it newly lists is `research pending`, and no worker is started on it. A harness with no placed model starts its workers with no model flag, on its own login's default.
- **Codex's own preference.** Unless a model label is followed, an automatic Codex start reads `model` from the host user's `~/.codex/config.toml` and passes it when it is a nonempty string the kit lists for Codex, whatever the kit's order and research; with none it starts with no model flag. A `model` the kit does not list, that is not a string, or that is outside the role's range refuses the start. Every Codex launch, bound or unbound and whatever `host_user` says, also reads `model_reasoning_effort` and an explicit `service_tier` from that file; with none Codex uses its own defaults. Nothing else of that file is loaded, and each session's own `CODEX_HOME` keeps a session's `/model` choice from becoming a later worker's. The launch record says which source chose the model.
- **Research belongs to the watch.** The watch researches unplaced models with its own tools, including web search. Unplaced models are a low-priority `doctor` finding: a watch started for other work takes them too, and they alone start a watch at most once a day.
- **Checked answers.** The watch hands its research to `afkconvoy kit --research <file>`, which checks the whole file against the latest kit before writing anything and refuses one placing or settling nothing. Each entry names a listed id and harness, either its coding strengths and weaknesses with sourced names or a reason the identity is unsuited to workers, and at least one dated URL. A refusal names the fault and writes nothing, so the watch corrects its file in the same session; a headless research call that fails leaves nothing to fix it with. The file's order ranks new placements after established priority, which research never moves. Research is shared across harnesses for the same identity without its provider prefix, and survives relisting until the identity is dropped and listed anew. The host kit's glossary entry defines the research states.
- **Named models.** A model a human named, with no research, is started on as named until the watch supplies research; kit writes retain it rather than settling it unasked.

## Failures found at work

A listed model that fails at a worker start is found by worker health (ADR-0094), classed at model scope and left out until its hourly recovery probe answers. A harness refusing every prompt is paused, with its own hourly probe. These recovery probes are the only prompts sent to test a model; one asks the login shell a worker gets and opens no terminal.

## Considered options

- **Hold a model by the id its probe answers as.** Every kit write prompts every model, and a harness at its usage limit drops out of the kit entirely.
- **Compare names as exact strings.** A roster, label or consult that named one of a model's names while the kit held another found nothing.
