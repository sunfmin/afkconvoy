# afkconvoy user manual

[中文版](manual.zh.md)

This manual is for the person who sets afkconvoy up and lives with it: the operator of a fleet, and whoever designs the team it works as. It goes from nothing to a repo whose issues are worked by a team of roles with no human in the loop, using one worked example throughout.

The reference is the tool itself: `afkconvoy --help` and `afkconvoy <command> --help` say exactly what each command reads, writes and answers. The vocabulary is in [`GLOSSARY.md`](../GLOSSARY.md), and the reasons behind the design are in [`docs/adr/`](adr/). This manual tells you what to do, and in which order.

## Contents

1. [What afkconvoy does](#1-what-afkconvoy-does)
2. [Install](#2-install)
3. [The simple way: an unbound repo](#3-the-simple-way-an-unbound-repo)
4. [Teams: the ideas](#4-teams-the-ideas)
5. [Worked example: a hospital for code](#5-worked-example-a-hospital-for-code)
6. [Running a fleet](#6-running-a-fleet)
7. [When the fleet needs you](#7-when-the-fleet-needs-you)
8. [Writing good roles and workflows](#8-writing-good-roles-and-workflows)
9. [Reference](#9-reference)
10. [When something goes wrong](#10-when-something-goes-wrong)

## 1. What afkconvoy does

You have a GitHub repo with a backlog of issues. `afkconvoy run`, started in a checkout of it, works that backlog while you are away:

- it takes each issue that is ready and starts one coding agent — a **worker** — on it, in a git worktree of its own;
- it watches the workers, nudges one that stopped, and replaces one that died;
- it lands every finished PR on the base branch through a **landing train**, behind the repo's **gate** — the one command that says whether the tree is good;
- when it cannot go on with an issue, it says why in a comment on the issue and hands it to you.

Nothing the fleet knows is kept on your machine. Who works what, what is landing, where an issue is in its workflow: all of it is read from GitHub and the repo's remote at every **tick**. So you can kill `run` at any moment and start it again, and several machines can work one repo.

There are two ways to use it:

| | An unbound repo | A bound repo |
|---|---|---|
| The config names | no team | a team |
| An issue is worked when it carries | the ready label | a flow label, `flow:<workflow>` |
| One issue is worked by | one worker, from start to PR | one new worker per **stage**, each in its **role** |
| The worker is | any agent your command starts | Claude Code |
| Read | [section 3](#3-the-simple-way-an-unbound-repo) | sections [4](#4-teams-the-ideas) and [5](#5-worked-example-a-hospital-for-code) |

Start unbound if you only want issues turned into PRs. Bind the repo to a team when you want the work reviewed by somebody other than its author before it lands.

## 2. Install

macOS, Apple Silicon or Intel:

```sh
brew install sunfmin/tap/afkconvoy
afkconvoy --version
```

Homebrew brings `git` and `gh` with it. Three things are yours to set up:

| What | Install | Log in |
|---|---|---|
| [orca](https://github.com/stablyai/orca), which every worker is started through | `brew install --cask stablyai/orca/orca` | — |
| GitHub | (installed) | `gh auth login`, as a user who may push to the repo |
| Claude Code, when it starts your workers | `curl -fsSL https://claude.ai/install.sh \| bash` | `claude auth login` |

Give orca's tap name in full: the cask Homebrew itself calls `orca` is another program. Orca has to be running — open the app, or `orca open`.

You do not have to check any of this yourself. `plan`, `tick` and `run` check it first and, when something is missing or logged out, do nothing and print the line to run for each. afkconvoy installs nothing and logs nobody in.

## 3. The simple way: an unbound repo

**Step 1. Write the config.** Add `docs/agents/afkconvoy.md` to the repo's base branch. Only its first `yaml` block is read:

````markdown
```yaml
base_branch: main          # required: workers branch from it and PRs land on it
gate_command: ./gate.sh    # required: exits 0 only when the tree is good
concurrency: 3             # workers at once (default 3)
```
````

**Step 2. Make the gate one command.** Formatting, lint, tests — whatever must hold before a change lands, in one script that exits 0 only when all of it does. It is the only machine check between a worker and your base branch, so it has to be worth trusting.

**Step 3. Leave the base branch pushable.** The landing train lands by pushing a commit the gate passed. A branch protection that requires reviews or status checks refuses that, and the fleet refuses to start on such a repo.

**Step 4. Mark the issues.** Put the label `ready-for-agent` on each issue the fleet may take. An issue GitHub records as blocked by an open issue waits for it; an issue labelled `epic` or `prd` is never taken.

**Step 5. Look, then run**, in a checkout of the repo:

```sh
afkconvoy config    # what was read
afkconvoy plan      # what a tick would start on; changes nothing
afkconvoy run       # the fleet, until you stop it
```

Each issue ends one of three ways: its PR lands and the issue closes; the worker finds nothing to do and the issue closes; or the fleet swaps `ready-for-agent` for `ready-for-human` and says why in a comment. Fix what the comment names and put the ready label back.

## 4. Teams: the ideas

One worker that plans, writes, and judges its own work will pass its own work. A team splits those jobs between **roles**, and a **workflow** says in which order an issue passes through them.

```
        the project repo                    the team's repo                 a role's repo
  docs/agents/afkconvoy.md            team.md        ── names ──▶       CLAUDE.md          rules
     team: owner/team      ── names ▶ workflows/fix.md                  .claude/skills/*   what it can be asked to do
                                      patrols/triage.md                 .claude/settings.json   hooks: what it may never do
```

**A role is a repo.** It is an ordinary Claude Code project directory: a `CLAUDE.md` with the rules this kind of worker always works under, skills under `.claude/skills/`, and hooks in `.claude/settings.json`. A worker in a role is Claude Code started *in that directory*, with the issue's worktree added to it. Open the role's repo by hand, and you are in the same role a worker is. A worker loads nothing of your own Claude Code configuration — a role is what its repo says, on every machine.

**A team is a repo too.** It holds `team.md`, which gives each role a short name; one file per workflow under `workflows/`; and one file per patrol under `patrols/`. Of each file only the first `yaml` block is read, so the rest is yours to explain it in.

**A workflow is a line of stages.** Each stage names a role, and the skill of that role to perform:

```yaml
stages:
  - name: build
    by: dev              # a role of the team, or `human`
    do: implement        # a skill in that role's repo
    on: { pass: review }
  - name: review
    by: reviewer
    do: review
    writes: false        # this stage may not commit
    on: { pass: land, reject: build }
land: { by: dev, do: land }
```

**An issue enters a workflow by its flow label.** The file `workflows/fix.md` is the workflow `fix`, and its flow label is `flow:fix`. Put that label on an issue and it is at the workflow's first stage. An issue with no flow label is left alone.

**Every stage is a new worker, and ends with a verdict.** The fleet starts a fresh Claude Code session for each stage — a reviewer never inherits the author's reasoning. The worker ends by declaring one of five verdicts with `afkconvoy verdict`, which writes a **stage record**: a comment on the issue.

| Verdict | Meaning | Where the issue goes |
|---|---|---|
| `pass` | the stage did its work | where the stage's `on.pass` says: a stage, `land`, or `done` |
| `reject` | the work of an earlier stage is turned down | back to the stage `on.reject` names — always an earlier one |
| `blocked` | it waits on other issues | parked; the same stage runs again when they close |
| `already-satisfied` | nothing needs changing | the workflow ends |
| `escalate` | the stage cannot decide | to the stage's consult if it names one, else to a human |

Only `pass` and `reject` are written in a workflow. The other three mean the same at every stage of every workflow, so a role is taught them once.

**Where an issue is, is computed, not stored.** Each tick reads the issue's stage records and the workflow, and works out the stage. The fleet also keeps a `stage:<name>` label on the issue so that you can filter by it on GitHub — but it never reads that label back. Moving a label by hand does nothing; you steer an issue with a command ([section 7](#7-when-the-fleet-needs-you)).

**No human stands in a workflow.** An issue goes from its flow label to the base branch without anyone approving anything. What keeps that safe:

- **Breakers.** Every `reject` is counted. `max_rework` bounds the total per issue (default 3) and `max_consecutive_rework` the rejections in a row by one stage (default 2). The rejection that would pass either hands the issue to a human instead, with every round listed.
- **A consult.** A stage may name a more senior role to ask first — `escalate: { to: attending, do: consult }`. Its `escalate` then starts a worker in that role, which either rules (the stage runs again under the ruling) or lets the question through to a human. The chain is one role long.
- **Guards.** Every worker of a bound repo runs under a hook the fleet adds, which refuses what no worker may do: merging a PR, pushing the base branch, moving a flow label, forging a stage record. In a stage marked `writes: false` it also refuses committing and pushing. A role adds its own limits as hooks in its own repo.
- **The gate**, which every change still passes before it lands.

**A patrol is a role on a schedule.** It owns no issue. It looks for work — unrouted issues, a broken base branch, something worth proposing — opens issues and takes them in with `afkconvoy intake`. It may not commit. `max_open` stops it once that many issues it opened are still open.

**A fleet lock stops everything but one fix.** When the base branch is broken, every issue's gate is red whatever its PR holds. An open issue carrying the lock label makes the fleet work lock issues only, until it closes.

**A precedent is a ruling worth keeping.** When you rule on an escalated issue, a worker in the role the team names proposes your ruling as one line of the team's `precedents.md`, in a PR on the team's repo. Merge it, and every later stage reads it before it may escalate the same question again.

## 5. Worked example: a hospital for code

This section builds a whole team and runs it, start to finish. Every command and every output below is from a real run, and the repos it made are public: read the roles, the workflows, and the issues with every stage's record on them.

| | Repo |
|---|---|
| The team | [`sunfmin/hospital-team`](https://github.com/sunfmin/hospital-team) |
| The roles, each `sunfmin/hospital-<name>` | [`fellow`](https://github.com/sunfmin/hospital-fellow), [`attending`](https://github.com/sunfmin/hospital-attending), [`discharge-nurse`](https://github.com/sunfmin/hospital-discharge-nurse), [`triage-nurse`](https://github.com/sunfmin/hospital-triage-nurse), [`infection-control`](https://github.com/sunfmin/hospital-infection-control), [`research`](https://github.com/sunfmin/hospital-research), [`safety`](https://github.com/sunfmin/hospital-safety) |
| The project | [`sunfmin/hospital-demo`](https://github.com/sunfmin/hospital-demo) |

The team is modelled on the one Cockroach Labs describes in [An experiment in running a hospital for code](https://www.cockroachlabs.com/blog/experiment-running-hospital-code/): an issue is a patient, and each role does one job on it.

| The article's role | Here | What it does |
|---|---|---|
| Fellow | role `fellow` | works an issue up into a plan; treats it under the reviewed plan |
| Review Attending | role `attending` | reviews the plan and the code; is consulted when the fellow cannot decide |
| Discharge Nurse | role `discharge-nurse` | audits the record before a landing, and lands |
| Triage Nurse | role `triage-nurse`, as a patrol | routes each open issue to the workflow that fits |
| Infection Control | role `infection-control`, as a patrol | runs the gate on the base branch; locks the fleet when it is broken |
| Research | role `research`, as a patrol | proposes work it can show the need for |
| Safety | role `safety` | proposes a human's ruling as a precedent; reviews how the work went |
| Charge Nurse | no role | the fleet itself nudges a stalled worker and replaces a dead one |
| Chief of Medicine | you | rules on what is escalated; merges precedents |

### Step 1. Create a role

```sh
afkconvoy scaffold role fellow --repo sunfmin/hospital-fellow
```

```json
{"dir":"/…/hospital/fellow","files":["CLAUDE.md",".claude/skills/implement/SKILL.md",".claude/settings.json"],"repo":"sunfmin/hospital-fellow","scaffolded":"role"}
```

This writes a skeleton, commits it and creates the repo on GitHub, private (`--public` for a public one). About seven seconds. The role comes before the team because `check` reads every role from GitHub.

### Step 2. Write the role

A role is three kinds of file. Here is the fellow, shortened.

**`CLAUDE.md` — the rules it works under whatever it is asked:**

```markdown
# Fellow

- **No code before a plan that passed review.**
- **Don't improvise.** When the work needs something the reviewed plan does not hold, stop
  and declare `escalate`, saying what changed.
- **A test is evidence, not an obstacle.** Never delete, skip, loosen or rewrite a test to
  turn it green.
- **Stuck is a finding.** After two attempts at the same failure that did not move it,
  declare `escalate` with a hand-off.
- **Behaviour nobody decided is not yours to decide.** Where the issue leaves open what a
  user of the code would see, read the precedents, and declare `escalate` for what none covers.
```

**`.claude/skills/diagnose/SKILL.md` — one job it can be asked to do.** The stage `workup` will name it with `do: diagnose`:

```markdown
---
name: diagnose
description: Work an issue up into a treatment plan, changing no file. Use when the brief
  you were started with names `diagnose` as the skill to perform.
---

1. **Read the history.** The issue, every comment, every stage record your brief lists.
2. **Reproduce.** Quote the command and what it printed.
3. **Differential.** The causes that would explain it, and what rules each in or out.
4. **Size it.** Over 1,000 changed lines: split the issue as your brief says.
5. **Find the open questions.** What none of the precedents covers ends the workup: `escalate`.
6. **Write the plan** — Diagnosis, Change, Tests, Risks, Out of scope — as the text of your verdict.
7. **Declare `pass`** with the command your brief gives.
```

The skill never says which command ends the stage: the brief the fleet writes for each worker does, with the issue and the stage filled in.

**`.claude/settings.json` — what it may never do**, as a hook that Claude Code runs before each tool call:

```json
{ "hooks": { "PreToolUse": [ { "matcher": "Bash",
    "hooks": [{ "type": "command", "command": "\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/refuse.sh" }] } ] } }
```

```sh
#!/bin/sh
call=$(cat)
case "$call" in
  *--no-verify*) echo "fellow: a commit is never made with --no-verify" >&2; exit 2 ;;
  *"git rebase"*) echo "fellow: a branch is never rebased; merge instead" >&2; exit 2 ;;
esac
exit 0
```

Exit 2 refuses the call and shows the worker the reason. A hook holds even though workers run with `--dangerously-skip-permissions`.

Commit and push. **A push to the role's default branch is the only way a change reaches a worker.**

The other six roles are made the same way. To add a skill somebody else wrote, run the skills installer in the role's checkout and commit what it wrote: `npx skills add <owner/repo>`.

### Step 3. Create the team

```sh
afkconvoy scaffold team team --role fellow=sunfmin/hospital-fellow
```

Then edit. **`team.md`** gives each role its short name:

```yaml
escalate_label: ready-for-human
roles:
  fellow: sunfmin/hospital-fellow
  attending: sunfmin/hospital-attending
  triage-nurse: sunfmin/hospital-triage-nurse
  discharge-nurse: sunfmin/hospital-discharge-nurse
  infection-control: sunfmin/hospital-infection-control
  research: sunfmin/hospital-research
  safety: sunfmin/hospital-safety
precedents: { by: safety, do: record-precedent, max_open: 10 }
```

**`workflows/fix.md`** — for a defect, or any change in what the code does. "No code before a plan, no plan without a review":

```yaml
max_rework: 3
max_consecutive_rework: 2
stages:
  - name: workup
    by: fellow
    do: diagnose
    model: fable
    writes: false
    escalate: { to: attending, do: consult }
    on: { pass: plan-review }
  - name: plan-review
    by: attending
    do: review-plan
    model: opus
    writes: false
    on: { pass: treatment, reject: workup }
  - name: treatment
    by: fellow
    do: treat
    model: opus
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    model: opus
    writes: false
    on: { pass: discharge, reject: treatment }
  - name: discharge
    by: discharge-nurse
    do: audit
    model: sonnet
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```

**`workflows/chore.md`** — for a small change that needs no diagnosis. A second workflow is a second file:

```yaml
max_rework: 2
stages:
  - name: treatment
    by: fellow
    do: treat
    model: sonnet
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    model: sonnet
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```

The same two roles, the same skills, a different line. That is why a role names no workflow.

**`patrols/triage.md`** — a role on a schedule:

```yaml
by: triage-nurse
do: triage
every: 10m
model: sonnet
max_open: 10
```

### Step 4. Check it, draw it, rehearse it

```sh
afkconvoy check --dir team
```

answers what it read — every role with its skills, every workflow, every patrol — or every fault at once, each with its file and key:

```json
{"error":"…","faults":[{"file":"workflows/fix.md","key":"stages[1].on.reject","problem":"…"}]}
```

Edit and run it again until it exits 0. Then look at what you made:

```sh
afkconvoy show --dir team
```

```
fix — flow:fix — max_rework 3, max_consecutive_rework 2

    ┌─▶ workup       fellow: diagnose · model fable · writes nothing · escalate → attending: consult
    │     ↓ pass → plan-review
    └── plan-review  attending: review-plan · model opus · writes nothing · reject → workup
          ↓ pass → treatment
┌─┬───▶ treatment    fellow: treat · model opus · escalate → attending: consult
│ │       ↓ pass → code-review
│ └──── code-review  attending: review-code · model opus · writes nothing · reject → treatment
│         ↓ pass → discharge
└────── discharge    discharge-nurse: audit · model sonnet · writes nothing · reject → treatment
          ↓ pass → land
        land         discharge-nurse: land
```

And walk an imaginary issue through it. What happens when the fellow cannot decide, and the attending cannot either?

```sh
afkconvoy rehearse --dir team fix escalate escalate ruling pass
```

| Declared | In stage | The issue is then |
|---|---|---|
| `escalate` | `workup` | with the consult: `attending` |
| `escalate` | `workup` | escalating: with a human |
| `ruling` | `workup` | at `workup` again, to be performed by `fellow` |
| `pass` | `workup` | at `plan-review` |

Nothing was started and nothing was spent. Push the team:

```sh
cd team && git init && git add -A && git commit -m "The hospital"
gh repo create sunfmin/hospital-team --private --source . --push
```

### Step 5. Bind the project

In the project's repo, `docs/agents/afkconvoy.md` on the base branch:

```yaml
base_branch: main
gate_command: ./gate.sh
concurrency: 3
team: sunfmin/hospital-team
patrols: [triage, rounds]
```

`team` is what binds it. `patrols` names which of the team's patrols run *here*: the team has four, this repo runs two. Then, in a checkout:

```sh
afkconvoy check                                   # the team, as this repo names it
orca repo add --path "$(pwd)"                     # once per checkout: orca must know it
```

The second is easy to miss, and the fleet does not yet do or check it for you ([#119](https://github.com/sunfmin/afkconvoy/issues/119)).

### Step 6. Open issues

The demo project is a small Go package that reads a prescription's interval: `ParseInterval("q8h")` is eight hours, `DosesPerDay("q8h")` is 3. Three issues, opened three different ways:

| # | Issue | Opened with | Why |
|---|---|---|---|
| 1 | `DosesPerDay` panics on `"q0h"` | the label `flow:fix` | a defect: a human takes it in |
| 2 | README: list the interval forms | no label | left for the triage patrol to route |
| 3 | Read `"prn"` prescriptions | the label `flow:fix` | says outright that one answer is "not decided" |

`afkconvoy flows` is what to ask before opening the first one. It answers, for each workflow of the team, what it is for — the prose of its file above the yaml — the stage an issue enters at, and the line that opens an issue into it; and it creates each flow label the repo lacks, which `gh issue create --label` would otherwise refuse:

```sh
afkconvoy flows          # once per repo, and again when the team gains a workflow
gh issue create --label 'flow:fix' --title 'DosesPerDay panics on "q0h"' --body-file i1.md
```

```sh
afkconvoy plan
```

```json
{"frontier":[{"number":1,"stage":"workup","title":"DosesPerDay panics on \"q0h\""},
             {"number":3,"stage":"workup","title":"Read \"prn\" prescriptions"}],
 "excluded":[{"number":2,"reason":"no flow label","title":"README: list the interval forms ParseInterval reads"}],
 "free_slots":3,
 "intake":{"limit":50,"open":0},
 "patrols":[{"name":"triage","standing":"due","every":600,"last_run":null,"open":0,"max_open":10},
            {"name":"rounds","standing":"due","every":1800,"last_run":null,"open":0,"max_open":1}]}
```

Two issues at `workup`, one left alone and why, two patrols due.

### Step 7. Run

```sh
afkconvoy run --panel 7070
```

The fleet's own lines, lightly trimmed:

```
panel: http://127.0.0.1:7070/
tick 2: dispatched #3 · continued #1 · holding 2/3 · frontier 0 · next in 90s
tick 3: advanced #3 · ran the patrol triage, rounds · holding 3/3 · frontier 0 · next in 90s
tick 4: advanced #1 · dispatched #3 · holding 3/3 · frontier 0 · next in 90s
tick 6: advanced #3 · dispatched #1 · holding 3/3 · frontier 1 · next in 90s
tick 7: advanced #1 · dispatched #2 · holding 3/3 · frontier 1 · next in 90s
tick 9: advanced #2 · dispatched #1, #3 · ended the patrol triage, rounds · proposed as a precedent the ruling on #3 · holding 3/3 · frontier 0 · next in 90s
```

`dispatched` starts a worker on a stage; `advanced` is the fleet reading a verdict and ending that stage's claim. The stage that follows is dispatched a tick later, under a claim of its own; where a slot is free for it, `run` starts that tick with no sleep and the line ends `next now`. Here is what happened to each issue.

#### Issue 2: routed by a patrol, landed by a short workflow

Nobody labelled it. The `triage` patrol's worker read it, read what each workflow's file says it is for, and ran `afkconvoy intake --issue 2 --workflow chore`. That left an **intake record** on the issue and put the flow label on:

> **afkconvoy** — a worker in the role `triage-nurse`, on the patrol `triage`, took this issue in to the workflow `chore`.

Then `treatment` (fellow) wrote the README table and opened PR #4; `code-review` (attending) passed it; a worker in the landing role (discharge-nurse) gated the PR and joined the landing train; the train landed it and closed the issue. Three roles, no human.

#### Issue 1: the whole of `fix`

`workup` reproduced the panic, and found more than was reported — an hour count that overflows to exactly zero panics too:

```
DosesPerDay("q0h")  PANIC: runtime error: integer divide by zero
DosesPerDay("q2251799813685248h") PANIC: runtime error: integer divide by zero   (2^51 hours wraps int64 to 0)
```

Its plan was to refuse a zero *duration*, not a zero hour count, so that one check closes both. `plan-review` did not take that on trust: it ran the reproduction on a copy of the tree, confirmed it, and passed the plan with four notes — two more inputs to pin, and that the tests should hold the error's prefix too. `treatment` built it, test first; `code-review` and `discharge` passed it; it landed.

On the way the fellow found inputs that are wrong but do not panic (`q-8h` gives -3 doses a day). Its rules say what nobody decided is not its to decide, and the plan had put them out of scope — so it opened a new issue for them, with no label. The next `triage` run read it and did *not* take it in: the issue listed what to decide and not the decision. As its rules say, it asked once, in a comment — "This can't be worked as written: it lists what to decide but not the decision" — and left the issue for a human to answer.

Ask where an issue is at any moment:

```sh
afkconvoy show --issue 1
```

```json
{"position":{"is":"perform","stage":"discharge","by":"discharge-nurse","do":"audit"},
 "follows":{"is":"landing","by":"discharge-nurse","do":"land"},
 "rework":{"had":0,"left":3},
 "records":[{"stage":"workup","verdict":"pass","by":"worker"},
            {"stage":"plan-review","verdict":"pass","by":"worker"},
            {"stage":"treatment","verdict":"pass","by":"worker"},
            {"stage":"code-review","verdict":"pass","by":"worker"}]}
```

#### Issue 3: escalated, ruled on, and made a precedent

The issue said outright that what `DosesPerDay("prn")` answers was not decided. The fellow's `workup` did not pick an answer. It showed that the question could not be put off — reading `prn` as a zero duration would turn today's clean error into a panic — and declared `escalate` with a hand-off: the question, three options, and the one it would take.

`workup` names a consult, so the fleet did not come to a human yet. It started a worker in the role `attending`, with the skill `consult`. The attending's rules say a value a caller sees, that nobody decided, is not its to rule on either. It checked the diagnosis, added a second question the fellow had assumed an answer to, and let it through:

> **afkconvoy: escalated to a human.** The consult of the stage `workup` let its `escalate` through.

The issue now carried `ready-for-human` in place of `flow:fix`. The chain is one role long: fellow, attending, you.

```sh
afkconvoy rule --issue 3 --text 'Ruling, by the Chief:
1. `DosesPerDay("prn")` returns an error, `dose: "prn" has no doses a day`. An interval with no fixed period has no number of doses: say so with an error, never with a number.
2. `ParseInterval("prn")` returns `0, nil`: a zero duration means "no fixed interval".
3. `q0h` is not this issue: #1 decides it.'
```

```json
{"issue":3,"relabelled":true,"ruled":true,"stage":"workup","woke":true,"workflow":"fix"}
```

The flow label was back, the fleet was woken, and `workup` ran again with the ruling in its brief.

Because the team names who proposes precedents, the ruling had a second effect. A worker in the role `safety` opened [a PR on the team's repo](https://github.com/sunfmin/hospital-team/pull/1) that adds one line to `precedents.md`:

```
- An interval with no fixed period, such as `prn`, reads as a zero duration … and has no number of doses a
  day: say so with an error, never with a number. **When:** a prescription form names no fixed period between
  doses … (https://github.com/sunfmin/hospital-demo/issues/3#issuecomment-…)
```

It is yours to merge or close. Merged, the next stage that meets a form with no fixed period reads the precedent and goes on without asking.

### What it came to

| | |
|---|---|
| Issue 2, `chore`: 2 stages and a landing | opened to landed in 18 minutes |
| Issue 1, `fix`: 5 stages and a landing | opened to landed in 23 minutes |
| Issue 3, `fix`: escalated by the fellow, let through by the consult, ruled once, then 5 stages and a landing | opened to landed in 28 minutes; the code does what the ruling says |
| Human actions in all | one `afkconvoy rule`; one precedent PR left to merge |
| What a human never did | assign, review, approve, merge |

Everything that happened is on the issues: open [issue 1](https://github.com/sunfmin/hospital-demo/issues/1) and read down it. Each comment is a stage's record, with the commit of the role and of the team it ran under — so when a role's rules change, you can tell which work was done under which.

## 6. Running a fleet

**Before the first run**, in a checkout of the repo:

```sh
afkconvoy config    # the config, defaults filled in
afkconvoy check     # bound repos: does the team load?
afkconvoy plan      # the frontier: what a tick would start on, and why the rest waits
```

`plan` is where you find out why an issue is not being worked: every open issue that is not on the frontier is under `excluded`, with the reason.

**Start it.** `run` never returns, so give it a terminal of its own, or the background:

```sh
afkconvoy run --panel 7070
```

It prints one line per tick:

```
tick 1: dispatched #1, #3 · holding 3/3 · frontier 0 · next in 30s
tick 2: nothing to do · holding 3/3 · frontier 0 · next in 60s
```

`holding 3/3` is claims held out of `concurrency`; `frontier` is what waits for a free slot. A step that failed shows in the line as `FAILED <step> #<issue>: <why>`, and the loop goes on.

**Watch it.** With `--panel <port>`, http://127.0.0.1:port/ shows the fleet as a page that brings itself up to date after every tick: each workflow as a line of its stages, the issues at work and queued at each, the landing train, and what waits for a human. Each worker is also a terminal in orca, in the issue's worktree — open it to read what the worker is doing.

**Ask about one issue**:

```sh
afkconvoy show --issue 7
```

answers where it is, what a `pass` would lead to, how much rework it has left, and every stage record in order.

**Stop it** with Ctrl-C, or by killing the process. Nothing is released and no worker is stopped: the workers work on, and the next `run` on the same machine takes the claims back.

**Rules to remember**

- One `run` per machine per repo. A second one takes the first one's claims.
- Workers start in a fresh login shell that inherits nothing from the shell `run` was started in. The default worker launch command is `claude --dangerously-skip-permissions`; if you start Claude through a wrapper, name it: `afkconvoy run --worker-command <alias>`.
- A change to a role or to a team reaches a worker only once it is pushed to that repo's default branch. A stage starts under whatever is at the head when it starts.
- A change to the project's config counts once it is on the base branch.

## 7. When the fleet needs you

You are asked by exception, and always the same way: the issue loses its flow label, gets the team's escalate label (`ready-for-human` by default), and a comment says why. So the one thing to check is:

```sh
gh issue list --label ready-for-human
```

or the "With a human" part of the panel. afkconvoy sends no notification.

| What happened | What the comment says | What you do |
|---|---|---|
| A stage declared `escalate` and no consult ruled | the question, usually with options | `afkconvoy rule` |
| A breaker tripped | which breaker, and every round's stage and reason | read the rounds; `rule`, or `send` it elsewhere |
| A worker failed — went silent, no verdict, red gate, a commit from a stage that may not write | what failed | fix the cause; `rule` to run the stage again |
| The workflow changed under the issue | that a record names a stage that is gone | `send` it to a stage that exists |

**Answer where it stopped** — the stage runs again, with your text in its brief, and both rework counts start over:

```sh
afkconvoy rule --issue 7 --text "Keep the v1 schema; migrate in a later issue."
```

**Put it somewhere else** — at any stage of its workflow, whatever its records say:

```sh
afkconvoy send --issue 7 --stage plan --text "The approach changed: see #12."
```

**Declare the verdict of a stage that is yours** — one a workflow gives to `human`. The issue waits there, unclaimed, until you do:

```sh
afkconvoy verdict pass --issue 7 --stage sign-off --text "Approved."
```

Run all three in a checkout of the project repo. Do not move the labels by hand: that hands the issue back with no ruling on record, and the next tick escalates it again.

**Precedents.** If the team names who proposes them, each `rule` you give is followed by a PR on the team's repo that appends one line to `precedents.md`. Read it as "do I want every later stage to follow this without asking?" — merge it if so, close it if the ruling was about that one issue. The fleet never merges on the team's repo.

**A patrol's failed run** opens an issue of its own with the escalate label. The patrol is not run again until you close it.

## 8. Writing good roles and workflows

**Put each thing where it belongs.**

| It is | It goes in | Because |
|---|---|---|
| a rule the role follows whatever it is asked | the role's `CLAUDE.md` | it is loaded for every stage the role performs |
| how to perform one job | a skill, `.claude/skills/<name>/SKILL.md` | a stage's `do` names it |
| something the role must never do | a hook in `.claude/settings.json` | a rule can be ignored; a hook refuses |
| the order of the work, and who reviews whom | the workflow | a role serves many workflows and names none |
| what must hold before any change lands | the project's gate | a role's hooks do not see the project's rules |
| the project's own conventions | the project's `CLAUDE.md` | it is loaded from the worktree, in every role |

**Name no stage and no workflow in a role.** A skill says "the plan is the last `pass` of the workup" at most; the brief the fleet writes for each stage carries the issue, the stage, the worktree, the stage records so far, and the exact commands to end with. A skill that says "end as your brief says" stays right when the workflow changes.

**The brief says what; the skill says how.** The fleet's brief is a fixed form with nothing of how to do the work. Everything about quality — what a plan contains, what counts as a blocking finding, what a discharge checks — is yours to write in the skill.

**Make the author and the judge different roles**, and mark every judging stage `writes: false`. A reviewer that can fix what it finds stops reporting it.

**Give `reject` a bar.** A reviewer with no rule about when to reject will send work back over a name. Say what blocks and what is a note, and that notes alone are a `pass`. The breakers stop a loop; a bar keeps it from starting.

**Say what is not the role's to decide.** The most useful sentence in a role is the one that tells it when to stop and ask. Without it a worker decides product behaviour by itself, and you learn of it in the diff.

**Describe each workflow in its file**, above the yaml: what kind of issue it is for. A patrol that routes issues reads that text to choose.

**Rehearse before you run.** `afkconvoy rehearse <workflow> <verdict>…` walks an imaginary issue through the verdicts you list and starts nothing:

```sh
afkconvoy rehearse fix pass reject pass reject pass reject    # where does the breaker trip?
afkconvoy rehearse fix escalate ruling pass                   # who is asked, and what runs again?
```

**Start small.** One role and one stage that lands is a working team. Add the reviewer when you have read what the first role produces.

## 9. Reference

### The project's config — `docs/agents/afkconvoy.md`

| Key | Default | Meaning |
|---|---|---|
| `base_branch` | — required | workers branch from it; PRs land on it |
| `gate_command` | — required | exits 0 only when the tree is good |
| `concurrency` | `3` | claims held at once: a worker each |
| `epic_labels` | `[epic, prd]` | issues carrying one are never worked |
| `lock_label` | `fleet-lock` | an open issue carrying it is a fleet lock |
| `ready_label` | `ready-for-agent` | unbound only: an issue is worked while it carries it |
| `escalate_label` | `ready-for-human` | unbound only: swapped in when the fleet gives an issue up |
| `team` | — | `owner/repo` of the team; naming it binds the repo |
| `patrols` | `[]` | bound only: which of the team's patrols run here |
| `intake_limit` | `50` | bound only: open issues taken in by workers, at most |

### The team file — `team.md`

```yaml
escalate_label: ready-for-human                          # default
roles:
  <name>: owner/repo
precedents: { by: <role>, do: <skill>, max_open: 10 }    # optional
```

### A workflow — `workflows/<name>.md`

Write above the yaml what the workflow is for and what an issue of it holds: `afkconvoy flows` answers that prose to whoever opens an issue, and `afkconvoy check` warns of a file that says nothing there.

```yaml
max_rework: 3                      # default
max_consecutive_rework: 2          # default
stages:
  - name: <unique; not land, done or human>
    by: <role> | human
    do: <skill>                    # required unless by: human
    model: <model>                 # optional; passed to Claude Code as --model
    writes: false                  # optional; default true
    escalate: { to: <role>, do: <skill> }                        # optional: the consult
    on: { pass: <stage> | land | done, reject: <earlier stage> } # reject optional
land: { by: <role>, do: <skill> }  # required exactly when a pass leads to land
```

`check` refuses a workflow in which a stage cannot be reached, a stage cannot reach an end, a `reject` leads forward, or a role or skill does not exist.

### A patrol — `patrols/<name>.md`

```yaml
by: <role>
do: <skill>
every: 6h          # a whole number, then m, h or d
model: <model>     # optional
max_open: 5        # not run while this many issues it opened are open
```

### A role's repo

```
CLAUDE.md                          rules
.claude/skills/<skill>/SKILL.md    one directory per skill
.claude/settings.json              hooks
```

### The commands, by who runs them

| Who | Commands |
|---|---|
| The operator | `config` `check` `plan` `run` `panel` `tick` `takeover` |
| Whoever designs a team | `scaffold` `check` `show` `rehearse` |
| A human an issue waits for | `rule` `send` `verdict` |
| A worker (its brief names each) | `gate` `land` `verdict` `intake` `lock` `wake` `rule` (a consult) |
| A worker's hook | `guard` |
| One transition by hand | `dispatch` `recovery` `close` `escalate` `park` `nudge` `restart` `abandon` |
| The claim plumbing | `scan` `claim` `release` `heartbeat` |

Every command prints one JSON object, and `{"error": …}` with a non-zero exit on failure. The exceptions: `run` prints a line per tick, `show` a drawing, `panel` a page, and `guard` answers with its exit code.

### What a worker of a bound repo has in its environment

`AFKCONVOY_ISSUE`, `AFKCONVOY_FLOW`, `AFKCONVOY_STAGE`, `AFKCONVOY_ROLE`, `AFKCONVOY_SKILL`, `AFKCONVOY_WRITES`, `AFKCONVOY_CONSULT`, `AFKCONVOY_BASE_BRANCH`, `AFKCONVOY_ESCALATE_LABEL`, `AFKCONVOY_LOCK_LABEL`, `AFKCONVOY_ROLE_COMMIT`, `AFKCONVOY_TEAM_COMMIT`; a patrol's worker has `AFKCONVOY_PATROL` in place of the stage's facts. A role's hook may read them.

### What the guard refuses

For every worker of a bound repo: adding or removing a flow label, a `stage:` label, the escalate label or the lock label; writing a stage record or an intake record by hand; merging a PR; pushing the base branch. In a stage marked `writes: false`, for every consult and for every patrol, also: committing and pushing.

The guard recognises commands. It stops a worker that errs, not one that sets out to get past it, so the fleet checks again afterwards: a commit left by a stage that may not write escalates the issue.

## 10. When something goes wrong

Start with three questions, in this order:

```sh
afkconvoy plan --instance <id>     # what is on the frontier, what is excluded and why, what I hold
afkconvoy show --issue <n>         # where one issue is, and its records
afkconvoy takeover --list          # which instance ids exist: `run` never prints its own
```

and the lines `run` printed: a step that failed is reported there and nowhere else.

| What you see | Why | What to do |
|---|---|---|
| `could not add label: 'flow:fix' not found` | nothing has created the flow label on this repo yet | `afkconvoy flows` — it creates every flow label the repo lacks |
| `FAILED dispatch #1: orca knows no repo at …` | orca has never been told of this checkout | `orca repo add --path <the checkout>`; the next tick recovers by itself ([#119](https://github.com/sunfmin/afkconvoy/issues/119)) |
| An issue is not being worked | it is under `excluded` in `plan`, with the reason: no flow label, an epic label, an open blocker, a fleet lock, held by another instance | remove the cause the reason names |
| `holding 3/3`, a frontier that is not empty, and nothing happening | every slot is taken — a patrol's run holds one for about five minutes after its worker is done ([#122](https://github.com/sunfmin/afkconvoy/issues/122)) | raise `concurrency`, or give patrols a longer `every` |
| I changed a role and the worker did not see it | the change is not pushed to the role's default branch, or the stage had already started | push; the next stage starts under the new head |
| I moved the labels back and it escalated again | a label is not the state: the records still say "escalating" | `afkconvoy rule` or `afkconvoy send` |
| A stage's verdict "counts for nothing" | the record names another stage than the one its claim was dispatched to | `show --issue` says how each record was read; `send` the issue where it belongs |
| An issue escalated with "a commit that was not there" | a stage marked `writes: false`, or a consult, committed | decide whether the commit should stay; `rule` to run the stage again |
| `the worker launch command … does not take the flags` | in a bound repo the command must end in Claude Code | name a command or alias that reaches `claude`: `--worker-command <alias>` |
| A warning that the base branch's protection could not be read | a private repo on a free plan does not expose it | fine if the branch is unprotected; otherwise landings will be refused |
| `run` was killed | nothing is lost | start it again on the same machine: it takes its claims back |
| Another machine's fleet died | its claims go stale by themselves | wait, or `afkconvoy takeover --from <instance> --instance <yours>` |

There is not yet one command that says what is wrong with a running fleet and how to fix it; that is [#121](https://github.com/sunfmin/afkconvoy/issues/121).

**To stop everything now**: kill `run`, then close the workers' terminals in orca. Issues keep their flow labels and their records, so a later `run` goes on from where each one was.
