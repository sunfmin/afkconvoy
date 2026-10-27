# Human wait episode evidence

For sunfmin/control#153, doctor now identifies the human question by its
issue, source URL, source time, reason and answer command. Its long-wait
warning and `waiting_on_human` entry share that condition, scoped to the
checkout. Displayed age and issue title do not change the identity. An
observed absence still ends the episode, and a changed question queues new
work. Existing reports migrate using the structured look of the round that
actually received the item; an unrelated round's look grants no coverage.

The deterministic regression first failed with two pending items for the
single question at `9h 27m`. It now passes through doctor warning generation,
actionable observation, retained coverage, scheduler serialization/reload,
and another observation at `9h 33m`. Additional scenarios cover the first
long-wait warning after an earlier report, recovery and return, missing
reports, changed escalation sources and answers, unrelated checkouts, and
both pre-resident and resident scheduler state migration. Migration also
releases inherited coverage when the latest round did not report.

The CLI scenario
`a_reported_human_wait_sends_no_more_prompts_across_looks_and_runner_restart`
passed in 13.17 seconds. It runs the real binary against GitHub and Orca
stand-ins, checks doctor at the two observed ages, submits an explicit human
report through `watched`, observes repeated looks, kills/restarts the host
runner, and confirms one Terminal creation and one model prompt. A changed
escalation then receives one new round in that Terminal. The underlying
issue retains its human label.

The finding audit also removed presentation-based identity from stopped
workers, long stages, failed steps (including moving tick windows and the
warning-to-critical transition), and an undispatched frontier's free-slot
count. Resource readings already carry structured episode identities;
patrol counts and gate durations live in evidence rather than identity.
Other changing subjects name different work, such as a new installed
version or a different frontier.

## Live host verification still pending the fleet cut

The read-only pre-landing check used installed afkconvoy `0.5.26`.
At doctor boundary `1793072068`, bizkernel still reported tracker issue
sunfmin/control#11 with `since=1793036772`, source
`https://github.com/sunfmin/control/issues/11#issuecomment-6423248030`, and
answer `afkconvoy rule --issue 11 --text-file <file>`. Its previous watch
`1793071683-74345-8857` had reported and watch
`1793071982-74345-9287` was running. The issue was read, never modified.

The task brief requires this worker to wake the fleet and stop immediately
after the landing turn reports `merged`. Consequently this worker cannot
claim post-cut verification. Let the declared fleet cut publish and install
the release, and let the host runner move naturally. Then, if #11 remains
pending, compare its source and answer across at least two scheduled looks
and confirm the reported episode stays covered without another round or
model prompt for that question. Other genuinely new work may still start
rounds. Do not answer or relabel #11 to manufacture recovery.
