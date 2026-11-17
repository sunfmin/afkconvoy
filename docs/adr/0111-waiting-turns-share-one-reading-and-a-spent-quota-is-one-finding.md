---
status: accepted
---

# Waiting turns share one reading, and a spent quota is one finding

_Amended by ADR-0117:_ the shared reading of the candidate chain this record's sunfmin/control#1321 first asked for is gone, with the chain on the remote: waiting turns read the host's own chain, which asks no remote. The shared reading of the fleet locks, below, stays as it is.

sunfmin/control#1111.

On Studio64, the `sunfmin` login's REST core quota, 5,000 calls an hour,
ran out three times in one morning. 49 `land` turns were waiting at once,
each for its round or its predecessor, and each asked GitHub whether a fleet
lock stands about once a second: one REST call a second each. With the
quota spent, the look could read no repo's readiness, and said that of each
repo, seven `remote could not be read` findings, and the watch was started
for them. Every one of those repos was worked.

## A waiting turn's lock reading is shared on the host

The fleet locks a turn asks of again and again while it waits — for its
round or its predecessor, or while the base is red — are a **shared
reading**: one process of the host reads it at most once every 30 seconds,
under a file lock beside the login's quota note, and every other waiting
turn answers from what it read. A failed read is kept the same way, so a
quota rejection is not asked again by each of them. A lock raised is seen
by a waiting turn within the interval, not within the second.

The turn's own first reading of the locks, before it takes its place, and
its reading before it is assigned a candidate are its own, as they were:
they are once a turn, not once a second.

## The host's readings stay under a share of the hour

`--github-share`, a host setting, is the share of the login's hourly quota
the shared readings stay under: 60% unless the host says otherwise. Once the
quota note says the login has used that much of it — by anyone's calls, the
workers' own `gh` calls among them — what a shared reading last read answers
until the quota resets. What is left of the hour is the workers'.

## A spent quota is one finding

A look whose login's REST core quota is spent, by the quota note at the
look, reads no tracker for unworked repos before the reset: it says one
finding, `GitHub quota is spent` (`github_quota`), with the reset, and
`host.github_quota` says {host, login, api, reset}. A quota spent during the
look's read drops the rows it could not read for want of it. Neither says
`remote could not be read` nor `no fleet has taken` of any repo. The watch
is not called for it: nothing on the host gives the quota back before the
reset.

## What stays

The other GitHub reads of a turn — its PR once it holds the turn, the
issue's blockers and its PR's head before it publishes — are read as before:
once each a turn, not once a wait.
