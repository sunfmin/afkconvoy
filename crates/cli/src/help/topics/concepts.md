What becomes of one issue, from the moment it is the fleet's to the moment it lands, and the four things that carry it there. Each term has its one definition in [`afkconvoy help glossary`](help:glossary); each paragraph ends with what says more.

## The four things

- **The fleet**: A fleet instance is one `afkconvoy run` on one host, for one repo. It works in ticks: each reads fleet state anew from GitHub and the repo's remote, then does everything that calls for. No process holds fleet state, so a `run` that is killed loses nothing and several hosts can work one repo. `afkconvoy run --help`, `afkconvoy tick --help`.
- **A worker**: A session of one harness — Claude Code, pi, Codex, DSH or Kimi Code — started for one issue, in a git worktree of that issue's own. It reports through GitHub alone: a PR, a comment. The fleet never waits on one; it reads what the worker left at the next tick. `afkconvoy dispatch --help`.
- **A claim**: What marks one issue as one fleet instance's. Creating it succeeds for exactly one instance, so no issue is worked twice. It is live while its owner's heartbeat is; past that another host may take it. `afkconvoy scan --help`, `afkconvoy takeover --help`.
- **The landing train**: The one line of merge commits ahead of the base branch that finished PRs join, one merge commit each. One worker gates its tip and pushes it to the base branch as a fast-forward: what lands is a tree the gate passed. `afkconvoy land --help`.
- **The gate limit**: How long the gate may run: `gate_seconds` in the repo's config, {const:GATE_SECONDS} where it sets none. A run that reaches it is ended and is red, a worker's and the train's alike. `afkconvoy config --help`.

The train worker runs on the host kit's first eligible harness, skipping paused or faulted harnesses. When a PR joins, a live worker on that harness whose agent has not failed is told once it stops; a failed worker or one on another harness is ended and replaced with the train's brief in the same worktree, with everything on the train kept. A continuous worker-health hold of {const:RECOVERY_SECONDS} seconds also replaces it, without waiting for another join, and the tick log says why. `afkconvoy switch --harness X` reaches the train at the next join; `switch --all` moves a train worker at work immediately and lists it in the answer. `afkconvoy switch --help`, `afkconvoy land --help`.

## One issue, in an unbound repo

An unbound repo's config names no team: every issue takes the same path, with one worker from start to landing. It is where to start when issues turned into PRs is all you want; bind the repo to a team once the work is to be judged by another than the worker that wrote it before it lands.

1. Intake. A human puts the ready label on the issue. It is on the frontier once it is open, unclaimed, under no epic label, with no open blocker and no open PR linked to it but one of the fleet's own that an escalation or a park kept, off the landing train: its worker takes that PR up. `afkconvoy plan` shows the frontier, and why each other issue waits.
2. Dispatch. A tick claims it and starts a worker in a new worktree, on a new branch cut from the base branch, with a prompt that names the issue and every command the worker ends with.
3. The work. The worker changes the code, runs `afkconvoy gate` until the gate is green on what it committed, opens a PR, and joins it to the landing train with `afkconvoy land`. Then it wakes the fleet: `afkconvoy wake`.
4. Landing. The train worker gates the train's tip and pushes it to the base branch. The PR is merged by that push, the issue closes, the claim is released.
5. Or no PR. The worker says so in a comment on the issue instead: there was nothing to change, and the issue closes; it waits on other issues, and is parked until they close; or it cannot go on, and the issue escalates — it is handed to a human, with why. A worker that stops having left nothing is nudged once, and escalated after that. Every failure escalates: [`afkconvoy help humans`](help:humans).
6. Or a red gate that is not its own. A worker that proves its gate red for a defect outside its issue — what its gate merged in is red the same way gated alone — records the defect itself: it comments on the open issue that already records it, or opens one with the ready label, and parks its issue on that one with `blocked`. It is the one `blocked` whose blocker the worker opens. When that issue closes, its own is on the frontier again. If the base branch itself is red the same way gated alone in a clean checkout, it first raises the fleet lock on the fix's issue with `afkconvoy lock --issue <n>`; a defect confined to the train raises none. The train worker does the same for a base branch that is red without the train, with no issue to park — in a bound repo taking the issue in to a workflow (`afkconvoy intake --train`) in place of the ready label — and stops until told to carry on. A watch of an unbound repo that proves the base branch red records the defect and raises the lock too.

## One issue, in a bound repo

A bound repo's config names a team. The issue passes through the stages of one of the team's workflows, and each stage is performed by a new worker, started in the role that stage names.

1. Intake. The issue is given a flow label — `flow:<workflow>`, or the one the workflow's file names with `label`: by a human on GitHub, or by a patrol's worker through `afkconvoy intake`. `afkconvoy flows` says which workflow takes which issue, and creates the labels. An issue with no flow label is left alone.
2. Its position. Each tick derives where the issue is from the stage records on it — comments — and from the workflow. Nothing stores it: the `stage:<name>` label the fleet keeps on the issue is for a human to filter by, and is never read back. `afkconvoy show --issue <n>` says where an issue is, and what led there.
3. A stage. A tick claims the issue and starts a worker in the stage's role, in the issue's worktree, to perform the skill the stage names. The worker ends by declaring a verdict with `afkconvoy verdict`, which leaves the stage record:
   - `pass`: on to where the workflow says: a later stage, the landing, or the end of the workflow
   - `reject`: back to an earlier stage, which runs again: rework. Breakers bound it, and the rework past one escalates
   - `blocked`: parked until the issues it waits on close
   - `already-satisfied`: nothing needs changing: the workflow ends
   - `escalate`: to the stage's consult — a worker in a more senior role, or afkconvoy's stock consult where it names none, who rules or lets it through — or to a human where it says so
4. The next stage. The tick that reads a verdict ends that stage's claim and starts the stage that follows, under a claim of its own, with a new worker: the one that judges a piece of work never inherits the reasoning of the one that wrote it.
5. Landing. Once the last stage passes, a new worker in the workflow's landing role gates the PR and joins it to the landing train. From there it lands as an unbound repo's does.

## What holds all the way

- The gate is the only machine verification between a worker and the base branch: one command of the repo's, which exits 0 only on a good tree. `afkconvoy gate --help`.
- Nothing is ever rebased. A branch is brought up to date by merging into it.
- The fleet reads only the PRs of the repo's own branches, which only a login with write access can push: a PR from a fork is anyone's, and the fleet acts as if it did not exist. Fleet state, in [`afkconvoy help glossary`](help:glossary).
- A fleet's issues are in its tracker: the repo's own, unless its host says another with `afkconvoy run --tracker` — a private one, where the repo is public, so that no stranger writes what a worker reads: the fleet refuses a public tracker. There the fleet works only the issues that carry its repo's repo label. Its PRs, branches, gate and landing train stay the repo's. Tracker, in [`afkconvoy help glossary`](help:glossary).
- No human stands in a workflow unless a stage is given to one. A human is asked by exception, and each way that happens has its command: [`afkconvoy help humans`](help:humans).
- Every worker of a bound repo works under the guard, which refuses what no worker may do — merging a PR, pushing the base branch, moving a flow label. `afkconvoy guard --help`.
- A fleet lock — an open issue under the lock label — makes the fleet work lock issues only until it closes: for a fault every issue shares, as a broken base branch is. `afkconvoy lock --help`.

## Where to read on

- [`afkconvoy help roles`](help:roles): what a role is, and how to write a good one
- [`afkconvoy help humans`](help:humans): every way the fleet stops for a human
- [`afkconvoy help troubleshooting`](help:troubleshooting): a symptom, its cause and its fix
- [`afkconvoy help team`](help:team): the files of a team and of a role, key by key
