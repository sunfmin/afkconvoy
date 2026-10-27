One whole team, which the binary holds: the team this project demonstrates itself with, to read before you design your own, or to start from. It writes it where you say:

```sh
afkconvoy scaffold team hospital --example hospital --repo <owner/repo>
afkconvoy check --team <owner/repo>
```

It is written as one repo holds a team: the team at the root, each role under `roles/`. Every file written is named below — each as an item of a list, a role's under its directory — with the decision it holds and the rule of afkconvoy's it shows.

## What the team is for

It is modelled on the one Cockroach Labs describes in "An experiment in running a hospital for code". An issue is a patient: routed, worked up into a plan, the plan reviewed, treated under it, the code reviewed, its record audited, and discharged — its PR lands in a turn. Each is a stage a new worker performs in one role: none reviews what it wrote, and none writes code before the plan passed. The Chief is a human: an issue reaches the Chief by escalating, and a consult is asked first.

## The team

- `team.md`: Names the seven roles, each as `./roles/<name>`: a path from the root of the team's own repo, so that the team names no repo and is pushed, forked or renamed with no edit.
- `CLAUDE.md`: The team rules: three, given to every worker whatever its role. They say how this team works, and nothing of one codebase or of one role.
- `demo-rules.md`: The interval rule for hospital-demo with its source link, for that repo alone. The rule for continuing after a base-branch fix lives in the team rules, also with its source.

## The workflows

The prose above each yaml block says what its workflow is for: `afkconvoy flows` answers with it, and the role `triage-nurse` routes by it.

- `workflows/fix.md`: Five stages, then the landing.
  - A reviewer that writes nothing: `plan-review` and `code-review` are marked `writes: false`, as `workup` and `discharge` are, and the guard refuses their workers a commit and a push.
  - A role on another harness: the roster lets the attending run pi first, on pi's own name for its model, so `plan-review` has the plan reviewed by something other than what wrote it; the caster puts its code reviews on Claude Code.
  - A stage with a consult: `workup` and `treatment` carry `escalate: { to: attending, do: consult }`. Their `escalate` goes first to a worker in that role, which lets through to a human only what nobody decided.
  - Rework and its breakers: a `reject` leads back to an earlier stage, and past `max_rework: 3` or `max_consecutive_rework: 2` the issue escalates.
  - `land: { by: discharge-nurse, do: land }`: the landing turn is taken in the role that audited the record, not by the worker that wrote the code.
- `workflows/chore.md`: Two stages, then the landing: the fewest of any that lands, so it is where the patrol `rounds` takes in the fix of a broken base branch.
- `workflows/design.md`: A workflow that ends with a human: `proposal`, then `sign-off`, a stage `by: human` whose `pass` leads to `done`. Nothing lands. No human stands in a workflow unless its file says so: here the fleet waits for `afkconvoy rule`.

## The patrols

Each names a role, a skill, how often it runs and `max_open`; a repo's config says which of them it runs.

- `patrols/triage.md`: Every ten minutes: takes each open issue that is in no workflow in to the one that fits. It is how an issue opened with no flow label is worked at all.
- `patrols/rounds.md`: A patrol that locks the fleet. Every half hour it runs the gate on the base branch; red twice, it opens an issue, takes it in to `chore` and raises a fleet lock on it. `max_open: 1` is one lock at a time.
- `patrols/research.md`, `patrols/safety-review.md`: Daily, work proposed with its evidence; weekly, the stage records of what went badly, read for what would have caught it a stage earlier. Each opens issues.

## The roles

Each holds its rules, its skills — every `do` above names one — and its limits for claude-code, `.claude/settings.json`. [`afkconvoy help roles`](help:roles) says what goes where.

- `roles/fellow/`: `CLAUDE.md`, `.claude/skills/diagnose/SKILL.md`, `.claude/skills/treat/SKILL.md`, `.claude/settings.json`, `.claude/hooks/refuse.sh`, `.claude/hooks/skills-loaded.sh`, `skills-lock.json`
  Works an issue up and treats it. Two hooks, written executable: `refuse.sh` refuses what its rules say a fellow never does — `--no-verify`, a forced push, `git rebase` — and `skills-loaded.sh` a `pass` or a `reject` declared before the skills its skill says to load were loaded in the session.
- `roles/attending/`: `CLAUDE.md`, `.claude/skills/review-plan/SKILL.md`, `.claude/skills/review-code/SKILL.md`, `.claude/skills/consult/SKILL.md`, `.claude/settings.json`, `.claude/hooks/refuse.sh`, `.claude/hooks/skills-loaded.sh`, `.pi/extensions/limits.ts`, `skills-lock.json`
  Reviews, and is the consult. A `reject` needs a blocking finding, which keeps rework from the breaker over a name. `refuse.sh` refuses a commit and a push in every stage: the role writes no code, whatever a workflow left unmarked. `limits.ts` is both hooks again, as a pi extension: pi runs no hook of `.claude/settings.json`, so a limit held under two harnesses is written twice. Under pi a skill is loaded by reading its `SKILL.md`, which is what it counts. Without it `check` refuses `plan-review`: its role would hold no limits there.
- `roles/discharge-nurse/`: `CLAUDE.md`, `.claude/skills/audit/SKILL.md`, `.claude/skills/land/SKILL.md`, `.claude/settings.json`
  The last check, and the landing role. `audit` is a checklist of the record, not a second look at the code; `land` says only how a conflict is resolved.
- `roles/safety/`: `CLAUDE.md`, `.claude/skills/review/SKILL.md`, `.claude/settings.json`
  `review` is asked by a patrol.
- `roles/triage-nurse/`: `CLAUDE.md`, `.claude/skills/triage/SKILL.md`, `.claude/settings.json`
- `roles/infection-control/`: `CLAUDE.md`, `.claude/skills/rounds/SKILL.md`, `.claude/settings.json`
- `roles/research/`: `CLAUDE.md`, `.claude/skills/propose/SKILL.md`, `.claude/settings.json`

The three that only patrol. The guard refuses a patrol's worker a commit and a push, so their `settings.json` holds no hook, as those of the two above do.

## Where to read on

- [`afkconvoy help example-run`](help:example-run): what a host needs before a fleet works with this team, and one issue followed from its label to the base branch
