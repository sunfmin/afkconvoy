---
status: accepted
---

# A host caps afkconvoy's own share

Amends ADR-0019 and ADR-0072 (sunfmin/control#896).

On 2026-11-14 the Guild Hub, Studio64, refused Guild's tasks: its CPU would
be 92–101% busy, over Guild's 85%. The load was mostly afkconvoy's own: the
host runner at about 65% of a core, and gates and cuts building Rust. A
Hub's job is Forgejo, the scheduler and Guild's work. Admission's shares
are of the whole host, and twice the cores of load still admits, so nothing
of afkconvoy's left room for a system beside it.

A host now says the share of its cores and of its memory afkconvoy may use
itself, each a whole percent, in its host settings: `--max-share-of-cores`
and `--max-share-of-memory`. None is the default, and a host that says none
works as before.

## What is afkconvoy's

Its host runner and keeper and every process under them, every process its
fleets started (`hot::fleets`: a worker's harness, its gates and its tests),
and the running work of its exclusive queue, a cut's build among it. Their
share of a core now and what they hold resident, read of one process table
at each look, and only where the host caps something.

## Admission

Admission starts no worker while afkconvoy's own processes take as many
cores as the cap allows, or hold as much memory: a start adds to them, so a
use at the cap already holds. It is a reading beside the others, said with
its numbers, and held back for it past the span it calls for the host's
watch as any reading does (its reason: `share`).

## The exclusive queue and cuts

Every command of the exclusive queue is given the cores the cap allows, at
least one, as `CARGO_BUILD_JOBS` and `NEXTEST_TEST_THREADS`, unless its
environment already says them; a gate beside another lane keeps the smaller
share ADR-0049's lanes give it. A cut builds through the queue, so it is held
to the same. ADR-0019 gives a build every core, for a build held to fewer is
only slower; that still holds where the host caps nothing. A host that caps
the share has said the rest belongs to another system, and a slower build
is the price.

## Moving heavy work

Where a repo's builds need more than the cap leaves, its fleet moves to a
host with room: `afkconvoy run` in a checkout there, and `afkconvoy stop
--release` in the old one (`afkconvoy run --help`, MOVING A FLEET TO
ANOTHER HOST).
