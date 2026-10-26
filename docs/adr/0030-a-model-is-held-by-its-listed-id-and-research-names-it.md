---
status: accepted
---

# A model is held by its listed id, and research names it

ADR-0022 held each model by the id its probe answered as, so writing the host kit sent every listed model a prompt. On 25 October 2026 a rebuild opened a stack of orca tabs titled "afkconvoy question", eight at a time, and the 21 models that answered took 177 seconds between them while the rest waited up to 60 seconds each. A probe also cannot tell a model that is unavailable here from a login that is out of allowance for now. Codex was at its usage limit during that rebuild, so none of its models answered and it was left out of the kit. Meanwhile research already ordered every model the kit held (sunfmin/control#81).

Writing the host kit now sends no model a prompt: not `afkconvoy kit`, not `run` on a host without one, not a switch and not the daily refresh. The listing comes from the login's own server, so it already says what this login can run.

- **Ids and names.** A model's id is the id its harness lists, and a worker is started on it. Its names are those its listing shows it by and those research ties to it with a dated source. ADR-0022's matching stands: a roster entry, a `model:` label or the stock consult matches by the id or any name, whatever the case, or with the provider prefix left off where that leaves one match.
- **Default first.** A harness found and listed is held at once. Each model it newly lists is `research pending`, and no worker is started on it. A harness with no placed model starts its workers with no model flag, on its own login's default model. That holds for Codex too: on its own default, with its model recorded as null and the source `codex_default`. Before, Codex started no worker without a fleet-selected model.
- **Research places.** One research session ranks the pending models. Those it ranks with a dated source are placed, in its order, after the models placed before them; established order still wins. One it omits or gives no dated source is `not researched`: settled, never started on and not researched again.
- **Failed research is asked again.** A failed or timed-out session, or an invalid answer, applies nothing. The harness keeps its default and the models already placed, and what was pending is researched at the next kit write. Otherwise one slow research would leave a harness on its default for good, with no write able to place its models.
- **Named models.** A model a human or an older release named, with no research, is started on as named until research answers for it. The next kit write researches it rather than settling it unasked.
- **Failures found at work.** A listed model that fails at a worker start is found by worker health, as ADR-0020 has it. It is classed `model` at model scope and left out, and its hourly recovery probe brings it back. A harness refusing every prompt is still paused, with its own hourly recovery probe. These recovery probes are the only prompts sent to test a model.
- **Older kits.** An older kit's `probed` fields are read and not written again. `left_out` no longer says `silent to its probe` or `no model answered its probe`.

This supersedes ADR-0022's ids, which came from what a probe answered as, and the alias tracking that went with them. Its matching rule and its handling of unmatched names stand.
