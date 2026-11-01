---
status: accepted
---

# Work accounts count tokens by worker

Amends ADR-0044 and ADR-0053 for sunfmin/control#424. A work account records each worker's usage from its harness's session, by model, in uncached input, cache read, cache write and output. Reasoning, where the harness states it, is part of output and is stated separately. A repeated streamed message or cumulative counter is counted once. A missing or unreadable record is unknown, not zero.

Unlike overlapping time rows, tokens of different workers add: the account states the known sum and the number of unknown workers. The accounts window summarizes known totals and per-issue percentiles, including by harness, and names the five highest known issue totals. A partial issue total is included, with its unknown workers counted. It applies no price: plans and prices differ and change. Older version-one records have unknown token use; adding this optional field changes no existing field's meaning.
