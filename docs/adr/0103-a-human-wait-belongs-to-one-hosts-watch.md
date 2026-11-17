---
status: accepted
---

# A human wait belongs to one host's watch

Amends ADR-0033 and ADR-0052 for sunfmin/control#442. Several hosts working
one repo see the same human waits. Before queueing a wait, a host's watch
takes it with an atomic claim on that repo's remote, named for the tracker
and issue separately from worker and patrol claims. Other hosts' doctors
name the live holder as info; they start no round for that wait. Host
conditions remain local.

The watch publishes its holder's heartbeat while the wait stands, even
without a worker claim or a running round. The existing 1800-second lease
applies: another host takes a stale wait at its next look, with a push leased
to the claim it read. A restart on the holding host keeps the claim and its
heartbeat identity. Its next look removes claims whose issues no longer wait
on a human. An unread checkout never counts as having no waits.

Only the holding host queues the wait and its human report entries. A round
for a local host condition omits other hosts' waits and answer lines from
its full look as well as its actionable conditions. A
recorded report remains immutable. On entering the report outbox, and
immediately before each notify-command sending or asking-session question,
the host re-reads the claim. A host that lost ownership drops the durable
item and logs the holder's name. This follows the operator's ruling on the
issue: check before acting, with no reconciliation protocol between hosts.
