---
status: accepted
---

# A work account is one comment carrying one versioned record

When work on an issue ends, the fleet leaves one **work account**: one comment that a person can read and a later tick can read back. One comment is edited at each later end, so the issue shows one account. The fleet writes it; a worker that writes its marker is refused, as a worker that writes a stage record by hand is.

## Its facts are one versioned record

The comment carries its facts as one versioned JSON object, in a collapsed details block with a fenced JSON block; its human text is rendered from that object. Reading statistics from prose let a change of wording change the measurements and left some facts out of other systems' reach. Values are escaped in the JSON, so a command holding `--`, HTML or a fence stays data; an HTML comment holds only the account's identity marker. The help of `accounts` defines the schema. Unknown facts are null. A field's meaning never changes within a version; a breaking change raises it, and an unknown version is unreadable rather than guessed from the rendered text. The same record supplies the summary and `accounts --rows`, NDJSON with the issue URL.

## The total is the wall clock, and its rows are not added into it

The total runs from the earliest claim `since` on record to the end. The first account keeps the `since` of the claim then held, and a later edit does not move it forward: a new claim after an escalation does not reset it.

The rows under the total overlap and are not added into it. A worker keeps editing while its gate waits in the exclusive queue, and a recorded gate run times only the gate command, after the queue grants the turn; the queue wait is neither a row nor part of a gate row. A remainder would pretend the rows partition the wall clock. The landing turn is not a worker row: its gate runs are gate rows, and the rest of it sits only in the total. Earlier red gate runs are kept.

## Token use is an optional field

Each worker's usage is read from its harness's session, by model: uncached input, cache read, cache write and output, with reasoning, where the harness states it, stated apart within output. A repeated streamed message or a cumulative counter is counted once. A missing or unreadable record is unknown, not zero. Unlike time rows, tokens of different workers add: the account states the known sum and the number of unknown workers. No price is applied: plans and prices differ and change. Older records without the field have unknown token use; adding it changed no existing field's meaning.

Host work that has no issue keeps the same accounting beside it: each watch round, each question of the asking session, and each patrol run keeps a durable usage record, answered as `watch_round`, `asking_question` and `patrol_run` rows. A cut runs shell steps and has no model usage. `accounts` summarizes each kind's known totals, unknown count, percentiles, harness groups and highest known totals.
