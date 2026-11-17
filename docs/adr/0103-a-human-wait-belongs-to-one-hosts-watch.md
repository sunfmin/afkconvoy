---
status: accepted
---

# A human wait belongs to one host's watch

Several hosts working one repo from one GitHub tracker see the same human
waits. Before queueing a wait, a host's watch (ADR-0040) takes it with an
atomic claim on that repo's remote, named for the tracker and issue
separately from worker and patrol claims. Other hosts' doctors name the live
holder as info; they start no round for that wait. Host conditions remain
local. This matters where a checkout names a GitHub tracker (ADR-0091): a
host's local tracker is its own, so its waits' claims are named for it and
no other host contends for them.

The watch publishes its holder's heartbeat while the wait stands, even
without a worker claim or a running round. The ordinary claim lease applies:
another host takes a stale wait at its next look, with a push leased to the
claim it read. A restart on the holding host keeps the claim and its
heartbeat identity. Its next look removes claims whose issues no longer wait
on a human. An unread checkout never counts as having no waits.

Only the holding host queues the wait and its human report entries, and asks
it in its resident session (ADR-0052). A round for a local host condition
omits other hosts' waits and answer lines from its full look as well as its
actionable conditions. A recorded report remains immutable. On entering the
report outbox, and immediately before each notify-command sending or
asking-session question, the host re-reads the claim. A host that lost
ownership drops the durable item and logs the holder's name: check before
acting, with no reconciliation protocol between hosts.
