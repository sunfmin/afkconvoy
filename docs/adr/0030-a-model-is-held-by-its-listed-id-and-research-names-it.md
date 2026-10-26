---
status: accepted
---

# A model is held by its listed id, and research names it

Probing every listed model made kit writes slow and could not distinguish an unavailable model from a login temporarily out of allowance. The host kit now holds each model by the id its harness lists, with the listing's display names and the names research ties to it with a dated source; no kit write, switch or daily refresh sends a model prompt. A harness starts on its login's default model until sourced research places its models, and a model left `not researched` is not started on. A listed model that fails at a worker start is handled by worker health at model scope and returns through the existing hourly recovery probe.

This supersedes ADR-0022's probe-derived ids and alias tracking. Its matching rule stands: a roster entry, `model:` label or stock consult matches a placed model by its id or any of its names, case-insensitively, or with a provider prefix left off where that leaves one match. Ambiguous names match neither and unmatched names are said with the nearest names. Older kits carrying probe evidence are still read, but that evidence is not written again.
