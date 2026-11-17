---
status: accepted
---

# A fleet's issues are in its host's local tracker, or a private tracker its checkout names

A fleet's issues — the issue, its labels, its comments (claim comments, stage records, verdicts, work account), its blockers, its escalation and its closing — live in its **tracker**. Everything else is the repo's: claims, branches, worktrees, PRs, the gate and the landing turns. Two hosts that work one repo meet there and only there.

## The local tracker is the default

Every GitHub issue read spends the login's hourly quota, is a round trip, and fails while GitHub cannot be reached. Yet no host needs another host's issues: a host works the issues it opened. So a host keeps its issues in a **local tracker**: one SQLite database on the host, which every fleet, worker, watch and human on that host reads and writes through `afkconvoy issue`. Nothing is sent from one host's tracker to another's.

- It is kept in the host's data directory, not its cache: it holds what nothing else can give back. A host's disk is its only copy until a backup is made.
- **An issue's repo** is a field of the issue, not a label. An issue for a repo the host has no checkout of is refused, naming the repo: no fleet of the host would ever work it.
- **Its name** is `local/<host>#<n>`, `<host>` the host's name in lower case with its `.local` suffix dropped, `<n>` counted from 1. A claim ref is `claim/local/<host>/<n>`, and a worktree and its branch are `local-<host>-<n>-<slug>`, so two hosts' issues of one number never meet in the repo. A PR closes one with `Closes local/<host>#<n>`, and that host's landing turn closes it.
- **Blockers** are edges between the host's own issues. **A fleet lock** holds the fleets of its host alone; another host's fleet of the same repo goes on working and landing.
- **Workers and humans** use `afkconvoy issue` — `create`, `view`, `list`, `comment`, `edit`, `close`, `block` — never `gh issue`. A worker's brief, an escalation's lines, and `plan`'s and `doctor`'s fixes give these. The guard refuses a worker's `gh issue …` and `gh api …/issues…` where its fleet's tracker is local.
- A checkout that says no tracker, `--tracker local` or `--tracker default` works the local tracker; one whose `origin` names no repo on GitHub works its own repo.

Moving a GitHub tracker's open issues into the local tracker (`afkconvoy issue import --from <owner>/<tracker>`) is decided but not yet built.

## A checkout may name a GitHub tracker

`afkconvoy run <checkout> --tracker <owner>/<repo>`, its own repo among them, has the fleet work a GitHub repo's issues. The choice is kept with the checkout's entry on the checkout list, never pushed, and read as each tick opens the fleet: whose issues a fleet works is the choice of whoever runs it, and two people may work one repo from two trackers.

- A GitHub tracker may hold the issues of many repos. The **repo label**, `repo:<owner>/<repo>`, makes one an issue of that repo: a fleet reads only the open issues that carry its repo's label, an issue with two repo labels is dispatched by no fleet, and every issue the fleet or a worker opens there carries it. Intake and a fleet lock give it to an issue that lacks it and refuse one that carries another repo's.
- `#<n>` alone, in a PR or a commit, is always the repo's own issue; a tracker's issue is `<owner>/<tracker>#<n>`. A claim's ref is `claim/<owner>/<tracker>/<n>` and a worktree and its branch are `<owner>-<tracker>-<n>-<slug>`, so a fleet takes no claim of another tracker for its own.
- Every line a worker or human is given names the tracker. The fleet keeps it in the worker's worktree (`convoy-worker-tracker`) before it starts the worker, and every command run there reads it; one that finds none and no checkout entry is an error, never the repo's own issue of that number.
- `run --tracker` makes the repo label and the config's ready, escalate and lock labels where they are missing. It refuses to change the tracker while the fleet holds a claim on an issue of the tracker it had, or a legacy line still carries a PR of one. The old tracker's issues stay where they are.
- A team's roster issue is opened in the team's repo, whatever the fleet's tracker.

## A public tracker is refused

On a public GitHub tracker strangers write what the fleet reads as its state and what its workers read as their task: every record is a comment, read without asking who wrote it. So the fleet refuses a GitHub tracker GitHub says is `public` (`private` and `internal` are not). The startup probe asks at every start; `run --tracker` asks before it changes anything; a check that found it private is noted on the host (`convoy-tracker`) and stands for an hour, after which a running fleet instance asks again before its next tick. A fleet instance that finds its tracker public ends before the tick and is broken, so it holds neither the host's sleep nor a move onto a newer release back; `doctor` names it with the line that fixes it. A local tracker is written by the host's own user alone, so the refusal does not apply to it.

## Considered options

- **Keep the issues in the repo, and read only what trusted logins wrote.** It puts the boundary in a filter at every place the fleet reads, and any place missed is a hole; no filter keeps a stranger's comment from a worker that runs `gh issue view`. A private tracker puts the boundary in access control.
- **Name the tracker in the repo config.** It is pushed, so every host would work one tracker.
- **Fail every tick of an instance whose tracker is public, and keep it running.** Its host would never sleep and never move onto a newer release while the tracker stays public.
- **Move only the creation of issues off GitHub.** An issue's claims, records, blockers and closing would then live in two places.
- **One tracker shared by every host, on a server of its own.** No host needs another's issues, and a shared server is one more thing that must be up for any host to work.
- **A fleet lock every host of the repo sees, kept in the repo's git.** Refused by Felix: a lock holds its own host's fleets only.

## Consequences

- The local tracker's reads spend no GitHub quota, and a host whose GitHub is unreachable still reads its issues. A human reads a host's issues on that host, with `afkconvoy issue` or the panel; GitHub's pages do not show them.
- Two hosts may find and fix the same defect apart. Their PRs meet on the base branch, where the later turn finds the change already there or conflicts.
- A repo may be public while its GitHub tracker may not (ADR-0012). A fleet may work a tracker made public for up to an hour before it is caught.
- Two fleets of one repo that work two trackers land on one base branch, and each closes only its own tracker's issues.
