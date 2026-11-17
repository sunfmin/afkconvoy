---
status: accepted
---

# A host's issues are in its own local tracker

Amends ADR-0027. Decided by Felix, control session 2026-11-17.

Every issue of every fleet was a GitHub issue, and every claim comment, stage record, verdict, work account, label and blocker was read and written through GitHub's API. Those reads are what spend the login's hourly quota (ADR-0079), each is a round trip, and none of it works while GitHub cannot be reached. Yet no host needs another host's issues: a host works the issues it opened, and two hosts that work one repo meet only in that repo's git — its claims, branches, PRs and base branch.

So a host keeps its issues in a **local tracker**: one SQLite database on the host, which every fleet, worker, watch and human on that host reads and writes through `afkconvoy issue`. An issue opened on a host is worked by that host's fleets alone. Nothing is sent from one host's tracker to another's.

## What is where

- **The local tracker** holds the issue — its repo, title, body, state, labels — its comments, claim comments, stage records, verdicts and work account among them, and its blockers. It is kept in the host's data directory, not its cache: it holds what nothing else can give back.
- **The repo** keeps what it kept: claims, branches, worktrees, PRs, the gate and the landing turns. Two hosts that work one repo still meet there and only there.
- **An issue's repo** is a field of the issue, not a label. An issue is opened for a repo the host has a checkout of; one for a repo it has none of is refused, naming the repo: no fleet of the host would ever work it.
- **Its name** is `local/<host>#<n>`, `<host>` the host's name in lower case with its `.local` suffix dropped. `<n>` counts the host's issues from 1. So a claim ref is `claim/local/<host>/<n>`, and a worktree and its branch are `local-<host>-<n>-<slug>`, as ADR-0027 names every tracker's: two hosts' issues of one number never meet in the repo. A PR closes one with `Closes local/<host>#<n>`, and the landing turn of that host closes it.
- **Blockers** are edges between the host's own issues. An issue cannot be blocked by another host's.
- **A fleet lock** holds the fleets of its host alone. Another host's fleet of the same repo goes on working and landing.
- **Workers and humans** read and write the local tracker with `afkconvoy issue` — `create`, `view`, `list`, `comment`, `edit`, `close`, `block` — never with `gh issue`. A worker's brief, an escalation's lines, `plan`'s and `doctor`'s fixes give these. The guard refuses a worker's `gh issue …` and `gh api …/issues…` where its fleet's tracker is local.
- **What stays on GitHub**: PRs, branches and the base branch; the daybook (ADR-0065), which is not fleet state; and the decisions asked of Felix through the asking session (ADR-0052).

## Taking the issues off GitHub

`afkconvoy issue import --from <owner>/<tracker>` moves a GitHub tracker's open issues into this host's local tracker: those of the repos this host has a checkout of, claimed by this host or by no host. Each moved GitHub issue is closed with a comment naming its new name, so no other host takes it too, and an import run again moves nothing twice. An issue another host has claimed stays for that host's own import.

A checkout says `--tracker local` to work its host's local tracker. A GitHub tracker stays readable until every host has moved its issues; then its adapter is removed.

## Considered options

- **Move only the creation of issues.** Refused: an issue's claims, records, blockers and closing are its own, and would then live in two places.
- **One tracker shared by every host, on a server of its own.** Refused: no host needs another's issues, and a shared server is one more thing that must be up for any host to work.
- **One repo to one host.** Refused: a repo may be worked on many hosts; only its issues are each host's.
- **A fleet lock every host of the repo sees, kept in the repo's git.** Refused by Felix: a lock holds its own host's fleets only.

## Consequences

- The tracker's reads spend no GitHub quota, and a host whose GitHub is unreachable still reads its issues.
- A human reads a host's issues on that host, with `afkconvoy issue` or the panel; GitHub's web pages no longer show them.
- Two hosts may find and fix the same defect apart. Their PRs meet on the base branch, where the later turn finds the change already there or conflicts.
- A host's disk is its tracker's only copy until a backup is made of it.
- ADR-0027's public-tracker refusal applies to a GitHub tracker only: a local tracker is written by the host's own user alone.
