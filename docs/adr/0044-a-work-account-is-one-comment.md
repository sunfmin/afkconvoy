---
status: accepted
---

# A work account is one comment, and its rows are not the total

When work on an issue ends, the fleet leaves one work account: a comment a person can read, and that a later tick can read back. The total is the wall clock from the earliest claim `since` on record to that end.

The rows under the total are not added into it. A worker keeps editing while its gate waits in the exclusive queue, and a recorded gate run times only the gate command, after the queue grants the turn. Those clocks overlap, and the queue wait is neither a row nor part of a gate row. A remainder would pretend the rows partition the wall clock.

The landing turn is not a worker row. The worker that joins the issue's pull request is started to land, not to perform a stage. Its gate runs are gate rows. The rest of that turn sits only in the total.

One comment is edited at each later end, so the issue shows one account. The first account keeps the `since` of the claim then held. A later edit does not move that start forward: a new claim after an escalation does not reset it. The fleet writes the comment. A worker that writes its marker is refused, as a worker that writes a stage record by hand is refused.
