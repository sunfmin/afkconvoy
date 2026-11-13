A patrol is a role run on a schedule rather than for an issue: it looks for work, opens issues and takes them in. Read this to add a patrol to a team, to have a repo run one, or to learn how work reaches the fleet with no human labelling it — and how a fleet lock stops everything but one fix.

## What a patrol is, and is not

It is a worker in one role, with one skill, started when its interval has passed, in a checkout of the base branch that is linked to no issue. What it finds — an issue no workflow has taken, a broken base branch, something worth proposing — it writes down as issues, and takes in the ones the fleet is to work.

It is no stage: it owns no issue, belongs to no workflow and declares no verdict. It writes no code: a run has failed if its checkout ends on a commit not on the base branch, has working-tree changes (staged, unstaged or untracked), or it opened a PR. A final commit already on the base branch, at its tip or before it, is nothing the run left behind, whoever moved its HEAD there. A clean checkout on such a commit, with no PR, is a clean run. And it is no timer for anything else: nothing but a role's skill can be run by one.

## A patrol's file

`patrols/<name>.md` in the team's directory is the patrol `<name>` — words of lowercase letters and digits, each starting with a letter, joined by `-`, {const:PATROL_NAME_MOST} characters at most. Of the file only the first `` ```yaml `` block is read:

```yaml
by: <role>
do: <skill>
every: 6h                          # required: a whole number from 1, then m, h or d
max_open: 5                        # required
max_runs_a_day: 4                  # optional: a whole number from 1
max_landed_a_day: 5                # optional: a whole number from 1
```

`by` is a short name of the team file and `do` a skill of that role. `every` is how long after its last run a patrol is due: it is due or it is not, and one that missed ten intervals runs once. Its worker runs its role's range's first ([`afkconvoy help roster`](help:roster)). `max_open` bounds what a patrol gives the fleet: it is not run while that many issues it opened are open.

`max_runs_a_day` bounds how often it runs besides `every`: a patrol whose runs that started in the last 24 hours — rolling, the same on every host — have reached it is not due, whatever `every` says, until the oldest of them is 24 hours old. Each run's record on the remote carries the starts of the day before it; nothing is kept beside it. Left out, it adds no bound. `afkconvoy check` refuses one that bounds nothing: `every: 1h` lets a patrol run at most 24 times a day, so `max_runs_a_day` is at most 23 there, and a patrol run every day or less often has no use for it.

`max_landed_a_day` bounds the issues it opened that GitHub closed as completed in the last 24 hours, rolling and the same on every host. At the bound the patrol waits until enough completions are 24 hours old to leave fewer than the bound. Issues closed as not planned, reopened issues, and issues opened by humans or other patrols do not count. Ownership comes from the latest intake record, regardless of the issue's labels now. Left out, nothing changes. `check` refuses zero, negative, fractional and nonnumeric values. Unlike runs, completions have no daily ceiling from `every` or `max_open`: any positive whole number can bound them. This limits starting runs; a run already going may open more issues. `plan` reports `standing: landed`, `max_landed_a_day`, `landed_in_day` and `until` while it is this bound that holds the patrol back.

## Which patrols a repo runs

The ones its config names, under `patrols` (`afkconvoy config --help`): a team's patrol runs in no repo that does not name it, and a name the team has no patrol for is a fault of `afkconvoy check`. Of several hosts one runs it: the tick that finds a patrol due claims the run on the remote.

## A run

A run is held at work, as a claim is, until its worker declares it over with `afkconvoy over`, the last line of its brief; a worker that never does is ended once it is gone or has stopped for a grace period. The tick that ends a run closes its terminal and removes its checkout. `afkconvoy plan` says how each patrol stands, and `afkconvoy tick --help` what a tick does about one.

A run that failed keeps its checkout, and the fleet opens an issue with the team's escalate label that says so. The patrol is not run again until a human closes it.

## A run now

A human who wants a patrol's run now — not when its interval has passed — runs `afkconvoy patrol-now <name>` in a checkout of the repo: the next tick runs it, and the command wakes this host's `run` so that tick starts at once. No file changes. The run passes `every` and `max_runs_a_day`, and nothing else: a fleet lock, `max_open` or `max_landed_a_day` reached, a failed run still reported, or a run still going refuse the ask. It is claimed as a due run is, so of several hosts one runs it. It is recorded as the patrol's run, asked for: `every` counts from its start, and `max_runs_a_day` counts it among the day's. What trips you up: `afkconvoy patrol-now --help`.

## Intake

Intake is putting a flow label on an issue: from then on the issue is the fleet's to work, at the first stage of that workflow. A human does it on GitHub, outside any limit. A worker — a patrol's, or a stage's that splits its issue — does it with `afkconvoy intake`, which records who did; the guard refuses a worker the label itself. To choose a workflow a patrol reads what each is for: `afkconvoy flows`.

Two limits hold what workers take in. The repo's `intake_limit` bounds the open issues taken in by any worker, and `afkconvoy intake` refuses past it. A patrol's `max_open` bounds the ones it opened, and refuses nothing: the patrol is not run.

## The fleet lock

A fault every issue shares — the base branch is broken, so every gate is red whatever its PR holds — is not one issue's to escalate. In a bound repo a patrol or a stage that finds it raises the lock; in an unbound repo a worker or a watch does, though it has no role. First prove the base branch red the same way gated alone in a clean checkout, then find the open issue for the fix or open one, make it dispatchable — with the ready label in an unbound repo, intake to a workflow in a bound one — and run `afkconvoy lock --issue <n>` on it. An open issue carrying the repo's lock label is a fleet lock.

While one stands the fleet works lock issues only: nothing else is dispatched, no patrol runs, no PR of another issue lands in a turn, and workers already running are left alone. The lock issue is worked as any issue is: in a bound repo it needs a flow label too. Closing the last lock issue ends the lock; a stage that declared `escalate` or stayed silent before it closed then runs again, which is no rework. What trips you up: `afkconvoy lock --help`.

## Where to read on

- [`afkconvoy help workflows`](help:workflows): what an issue taken in then passes through
- [`afkconvoy help team`](help:team): the roles a patrol's `by` names, and a team's other files
- [`afkconvoy help roles`](help:roles): what a role is, and how to write a good one
- [`afkconvoy help glossary`](help:glossary): each term used here, with its one definition
