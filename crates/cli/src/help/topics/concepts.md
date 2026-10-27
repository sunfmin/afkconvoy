What becomes of one issue, from the moment it is the fleet's to the moment it lands, and the four things that carry it there. Each term has its one definition in [`afkconvoy help glossary`](help:glossary); each paragraph ends with what says more.

## The four things

- **The fleet**: A fleet instance is one `afkconvoy run` on one host, for one repo. It works in ticks: each reads fleet state anew from GitHub and the repo's remote, then does everything that calls for. No process holds fleet state, so a `run` that is killed loses nothing and several hosts can work one repo. `afkconvoy run --help`, `afkconvoy tick --help`.
- **A worker**: A session of one harness — Claude Code, pi, Codex, DSH or Kimi Code — started for one issue, in a git worktree of that issue's own. It reports through GitHub alone: a PR, a comment. The fleet never waits on one; it reads what the worker left at the next tick. `afkconvoy dispatch --help`.
- **A claim**: What marks one issue as one fleet instance's. Creating it succeeds for exactly one instance, so no issue is worked twice. It is live while its owner's heartbeat is; past that another host may take it. `afkconvoy scan --help`, `afkconvoy takeover --help`.
- **A turn**: One command makes a PR's merge commit on the current base tip, gates it, records green and fast-forwards the base to that same commit. Red, conflict or rejected push lands nothing. `afkconvoy land --help`.
- **The gate budget**: A fixed duration budget declared as `gate_budget_seconds` in the repo config. Recent recorded landings on one host that all exceed it make a slow gate. It never ends a run; where absent, the warning threshold is {const:SLOW_GATE_PERCENT}% of the gate limit. It never learns from past runs.
- **The gate limit**: How long the gate may run: `gate_seconds` in the repo's config, {const:GATE_SECONDS} where it sets none. A run that reaches it is ended and is red, for every gate run. `afkconvoy config --help`.

An unrecorded base tip is gated alone first. A red tip holds ordinary turns while the fleet dispatches its own repair, outside product workflows. That repair still takes a turn and gates its complete merge green before landing.

## One issue, in an unbound repo

An unbound repo's config names no team: every issue takes the same path, with one worker from start to landing. It is where to start when issues turned into PRs is all you want; bind the repo to a team once the work is to be judged by another than the worker that wrote it before it lands.

1. Intake. A human puts the ready label on the issue. It is on the frontier once it is open, unclaimed, under no epic label, with no open blocker and no open PR linked to it but one of the fleet's own that an escalation or a park kept, not landed: its worker takes that PR up. `afkconvoy plan` shows the frontier, and why each other issue waits. Blockers outside the tracker, or without a single repo label of a checkout serving it, are named in the findings of `plan` and `doctor`, with transfer and label lines. They hold build work back, but a named consult or declared hand-off proceeds when they are the only blockers. An unlabelled issue in a checkout's own tracker is that checkout's to work.
2. Dispatch. A tick claims it and starts a worker in a new worktree, on a new branch cut from the base branch, with a prompt that names the issue and every command the worker ends with.
3. The work. The worker commits and pushes its change, opens its PR, and runs `afkconvoy land --issue <n>`. That command holds the turn through merge, gate, record and landing. Then it wakes the fleet: `afkconvoy wake`.
4. Settling. The push merges the PR and closes the issue. The next tick settles its claim and worktree.
5. Or no PR. The worker says so in a comment on the issue instead: there was nothing to change, and the issue closes; it waits on other issues, and is parked until they close; or it cannot go on, and the issue escalates — it is handed to a human, with why. A worker that stops having left nothing is nudged once, and escalated after that. Every failure escalates: [`afkconvoy help humans`](help:humans).
6. Or a red base branch. A turn certifies an unrecorded tip alone. If it is red, ordinary turns wait and the fleet opens a repair issue with its lock. Its operator worker runs outside product workflows, so a human stage cannot hold the repair back. Only that repair may take a turn on the red tip, and it must gate green before landing. `afkconvoy lock --help`.

## One issue, in a bound repo

A bound repo's config names a team. The issue passes through the stages of one of the team's workflows, and each stage is performed by a new worker, started in the role that stage names.

1. Intake. The issue is given a flow label — `flow:<workflow>`, or the one the workflow's file names with `label`: by a human on GitHub, or by a patrol's worker through `afkconvoy intake`. `afkconvoy flows` says which workflow takes which issue, and creates the labels. An issue with no flow label is left alone.
2. Its position. Each tick derives where the issue is from the stage records on it — comments — and from the workflow. Nothing stores it: the `stage:<name>` label the fleet keeps on the issue is for a human to filter by, and is never read back. `afkconvoy show --issue <n>` says where an issue is, and what led there.
3. A stage. A tick claims the issue and starts a worker in the stage's role, in the issue's worktree, to perform the skill the stage names. The worker ends by declaring a verdict with `afkconvoy verdict`, which leaves the stage record:
   - `pass`: on to where the workflow says: a later stage, the landing, or the end of the workflow
   - `reject`: back to an earlier stage, which runs again: rework. Breakers bound it, and the rework past one escalates
   - `blocked`: parked until the issues it waits on close
   - `already-satisfied`: nothing needs changing: the workflow ends
   - `escalate`: to the stage's consult — a worker in a named senior role who rules or lets it through — or escalates for the host's watch where no consult is named
4. The next stage. The tick that reads a verdict ends that stage's claim and starts the stage that follows, under a claim of its own, with a new worker: the one that judges a piece of work never inherits the reasoning of the one that wrote it.
5. Landing. Once the last stage passes, a new worker in the workflow's landing role takes the same merge, gate, record and landing turn as an unbound worker.

## What holds all the way

- The gate is the only machine verification between a worker and the base branch: one command of the repo's, which exits 0 only on a good tree. `afkconvoy gate --help`.
- Nothing is ever rebased. A branch is brought up to date by merging into it.
- The fleet reads only the PRs of the repo's own branches, which only a login with write access can push: a PR from a fork is anyone's, and the fleet acts as if it did not exist. Fleet state, in [`afkconvoy help glossary`](help:glossary).
- A fleet's issues are in its tracker: the repo's own, unless its host says another with `afkconvoy run --tracker` — a private one, where the repo is public, so that no stranger writes what a worker reads: the fleet refuses a public tracker. There the fleet works only the issues that carry its repo's repo label. Its PRs, branches, gate and landing turns stay the repo's. Tracker, in [`afkconvoy help glossary`](help:glossary).
- No human stands in a workflow unless a stage is given to one. A human is asked by exception, and each way that happens has its command: [`afkconvoy help humans`](help:humans).
- Every worker of a bound repo works under the guard, which refuses what no worker may do — merging a PR, pushing the base branch, moving a flow label. `afkconvoy guard --help`.
- A fleet lock — an open issue under the lock label — makes the fleet work lock issues only until it closes: for a fault every issue shares, as a broken base branch is. `afkconvoy lock --help`.

## Where to read on

- [`afkconvoy help roles`](help:roles): what a role is, and how to write a good one
- [`afkconvoy help humans`](help:humans): every way the fleet stops for a human
- [`afkconvoy help troubleshooting`](help:troubleshooting): a symptom, its cause and its fix
- [`afkconvoy help team`](help:team): the files of a team and of a role, key by key
