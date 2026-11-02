---
status: accepted
---

# Work accounts count tokens by worker

Amends ADR-0044 and ADR-0053 for sunfmin/control#424. A work account records each worker's usage from its harness's session, by model, in uncached input, cache read, cache write and output. Reasoning, where the harness states it, is part of output and is stated separately. A repeated streamed message or cumulative counter is counted once. A missing or unreadable record is unknown, not zero.

Unlike overlapping time rows, tokens of different workers add: the account states the known sum and the number of unknown workers. The accounts window summarizes known totals and per-issue percentiles, including by harness, and names the five highest known issue totals. A partial issue total is included, with its unknown workers counted. It applies no price: plans and prices differ and change. Older version-one records have unknown token use; adding this optional field changes no existing field's meaning.

For sunfmin/control#425, the same accounting covers host work that has no issue work account. Each watch round keeps a durable usage record beside its report, with its id, harness, model, delivery start, report end, seconds and session tokens. The window from delivery to report includes continuations and excludes standby and every other round. Cumulative harness counters establish their baseline before delivery. Missing or unreadable usage is unknown. The resident asking session keeps the same per-question record, merged across put-aside and continuation segments. Patrol usage is retained per harness before its worktree is removed; a cut runs shell steps and has no model usage to record. `accounts` summarizes each kind's count, known totals, unknown count, per-round or per-question percentiles, harness groups and five highest known totals, and answers their NDJSON rows beside work accounts.
