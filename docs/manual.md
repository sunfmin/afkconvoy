# afkconvoy manual

> This page is generated, by `cargo run -p afkconvoy --example manual`, from afkconvoy's help topics: the texts `crates/cli/src/topic.rs` lists, each as `afkconvoy help <topic>` prints it, in the order `afkconvoy --help` lists them. Do not edit it: edit a topic, and run that command. `afkconvoy --help` is the way in, and `afkconvoy <command> --help` says what one command reads, writes and answers.

## Contents

- [`afkconvoy help concepts`](#afkconvoy-help-concepts) — one issue from intake to landing, and the four things that carry it
- [`afkconvoy help team`](#afkconvoy-help-team) — a team's directory, its file, how it names a role, binding a repo
- [`afkconvoy help roles`](#afkconvoy-help-roles) — what a role is, what goes where in one, and the four kinds of text
- [`afkconvoy help roster`](#afkconvoy-help-roster) — what each role's workers may run, a role's range, an issue's casting
- [`afkconvoy help skill-file`](#afkconvoy-help-skill-file) — a skill's file: where it is, its frontmatter, what `check` holds it to
- [`afkconvoy help workflows`](#afkconvoy-help-workflows) — stages, verdicts, rework, a consult, a landing: a workflow's file
- [`afkconvoy help patrols`](#afkconvoy-help-patrols) — a role run on a schedule: a patrol's file, intake, the fleet lock
- [`afkconvoy help example`](#afkconvoy-help-example) — the team `scaffold --example hospital` writes, read file by file
- [`afkconvoy help example-run`](#afkconvoy-help-example-run) — that team at work: what a host needs, and one issue's run
- [`afkconvoy help cache`](#afkconvoy-help-cache) — the cache directory: where it is on this host, what moves it, what it holds
- [`afkconvoy help humans`](#afkconvoy-help-humans) — every way the fleet stops for a human, and the command that answers
- [`afkconvoy help troubleshooting`](#afkconvoy-help-troubleshooting) — a symptom, its cause and its fix: each finding, each refusal
- [`afkconvoy help glossary`](#afkconvoy-help-glossary) — every term the help uses, each with its one definition

## afkconvoy help concepts

````text
What becomes of one issue, from the moment it is the fleet's to the moment it lands,
and the four things that carry it there. Each term has its one definition in
`afkconvoy help glossary`; each paragraph ends with what says more.
````

### THE FOUR THINGS

````text
The fleet          A fleet instance is one `afkconvoy run` on one host, for one repo.
                   It works in ticks: each reads fleet state anew from GitHub and the
                   repo's remote, then does everything that calls for. No process
                   holds fleet state, so a `run` that is killed loses nothing and
                   several hosts can work one repo. `afkconvoy run --help`,
                   `afkconvoy tick --help`.
A worker           A session of one harness — Claude Code, pi or Codex — started for one
                   issue, in a git worktree of that issue's own. It reports through
                   GitHub alone: a PR, a comment. The fleet never waits on one; it
                   reads what the worker left at the next tick.
                   `afkconvoy dispatch --help`.
A claim            What marks one issue as one fleet instance's. Creating it succeeds
                   for exactly one instance, so no issue is worked twice. It is live
                   while its owner's heartbeat is; past that another host may take
                   it. `afkconvoy scan --help`, `afkconvoy takeover --help`.
The landing train  The one line of merge commits ahead of the base branch that
                   finished PRs join, one merge commit each. One worker gates its tip
                   and pushes it to the base branch as a fast-forward: what lands is
                   a tree the gate passed. `afkconvoy land --help`.
The gate limit     How long the gate may run: `gate_seconds` in the repo's config,
                   3600 where it sets none. A run that reaches it is ended and is
                   red, a worker's and the train's alike. `afkconvoy config --help`.
````

### ONE ISSUE, IN AN UNBOUND REPO

````text
An unbound repo's config names no team: every issue takes the same path, with one
worker from start to landing. It is where to start when issues turned into PRs is
all you want; bind the repo to a team once the work is to be judged by another than
the worker that wrote it before it lands.

1. Intake. A human puts the ready label on the issue. It is on the frontier once it
   is open, unclaimed, under no epic label, with no open blocker and no open PR
   linked to it but one of the fleet's own that an escalation or a park kept, off the
   landing train: its worker takes that PR up. `afkconvoy plan` shows the frontier,
   and why each other issue waits.
2. Dispatch. A tick claims it and starts a worker in a new worktree, on a new branch
   cut from the base branch, with a prompt that names the issue and every command
   the worker ends with.
3. The work. The worker changes the code, runs `afkconvoy gate` until the gate is
   green on what it committed, opens a PR, and joins it to the landing train with
   `afkconvoy land`. Then it wakes the fleet: `afkconvoy wake`.
4. Landing. The train worker gates the train's tip and pushes it to the base
   branch. The PR is merged by that push, the issue closes, the claim is released.
5. Or no PR. The worker says so in a comment on the issue instead: there was
   nothing to change, and the issue closes; it waits on other issues, and is parked
   until they close; or it cannot go on, and the issue escalates — it is handed to a
   human, with why. A worker that stops having left nothing is nudged once, and
   escalated after that. Every failure escalates: `afkconvoy help humans`.
6. Or a red gate that is not its own. A worker that proves its gate red for a defect
   outside its issue — what its gate merged in is red the same way gated alone —
   records the defect itself: it comments on the open issue that already records
   it, or opens one with the ready label, and parks its issue on that one with
   `blocked`. It is the one `blocked` whose blocker the worker opens. When that issue
   closes, its own is on the frontier again. The train worker does the same for a
   base branch that is red without the train, with no issue to park — in a bound
   repo taking the issue in to a workflow (`afkconvoy intake --train`) in place of
   the ready label.
````

### ONE ISSUE, IN A BOUND REPO

````text
A bound repo's config names a team. The issue passes through the stages of one of
the team's workflows, and each stage is performed by a new worker, started in the
role that stage names.

1. Intake. The issue is given a flow label — `flow:<workflow>`, or the one the
   workflow's file names with `label`: by a human on GitHub,
   or by a patrol's worker through `afkconvoy intake`. `afkconvoy flows` says which
   workflow takes which issue, and creates the labels. An issue with no flow label
   is left alone.
2. Its position. Each tick derives where the issue is from the stage records on it —
   comments — and from the workflow. Nothing stores it: the `stage:<name>` label the
   fleet keeps on the issue is for a human to filter by, and is never read back.
   `afkconvoy show --issue <n>` says where an issue is, and what led there.
3. A stage. A tick claims the issue and starts a worker in the stage's role, in the
   issue's worktree, to perform the skill the stage names. The worker ends by
   declaring a verdict with `afkconvoy verdict`, which leaves the stage record:
     pass               on to where the workflow says: a later stage, the landing,
                        or the end of the workflow
     reject             back to an earlier stage, which runs again: rework. Breakers
                        bound it, and the rework past one escalates
     blocked            parked until the issues it waits on close
     already-satisfied  nothing needs changing: the workflow ends
     escalate           to the stage's consult — a worker in a more senior role, or
                        afkconvoy's stock consult where it names none, who rules or
                        lets it through — or to a human where it says so
4. The next stage. The tick that reads a verdict ends that stage's claim and starts
   the stage that follows, under a claim of its own, with a new worker: the one that
   judges a piece of work never inherits the reasoning of the one that wrote it.
5. Landing. Once the last stage passes, a new worker in the workflow's landing role
   gates the PR and joins it to the landing train. From there it lands as an unbound
   repo's does.
````

### WHAT HOLDS ALL THE WAY

````text
- The gate is the only machine verification between a worker and the base branch:
  one command of the repo's, which exits 0 only on a good tree. `afkconvoy gate --help`.
- Nothing is ever rebased. A branch is brought up to date by merging into it.
- No human stands in a workflow unless a stage is given to one. A human is asked by
  exception, and each way that happens has its command: `afkconvoy help humans`.
- Every worker of a bound repo works under the guard, which refuses what no worker
  may do — merging a PR, pushing the base branch, moving a flow label.
  `afkconvoy guard --help`.
- A fleet lock — an open issue under the lock label — makes the fleet work lock
  issues only until it closes: for a fault every issue shares, as a broken base
  branch is. `afkconvoy lock --help`.
````

### WHERE TO READ ON

````text
`afkconvoy help roles`            what a role is, and how to write a good one
`afkconvoy help humans`           every way the fleet stops for a human
`afkconvoy help troubleshooting`  a symptom, its cause and its fix
`afkconvoy help team`             the files of a team and of a role, key by key
````


## afkconvoy help team

````text
A team is a directory — a whole repo, or a subdirectory of one — that names a set of
roles and holds the workflows, patrols and precedents they work by. Read this to write a
team, bind a repo to one, or understand a fault from `afkconvoy check`.
````

### WHAT ITS DIRECTORY HOLDS

````text
    team.md                 the team file
    CLAUDE.md               the team rules: what every worker of the team works under
    workflows/<name>.md     one file per workflow: `afkconvoy help workflows`
    patrols/<name>.md       one file per patrol: `afkconvoy help patrols`
    precedents.md           the precedents: one line per ruling a human accepted

Only each definition file's first ```yaml block is read; the rest explains it.
````

### THE TEAM FILE

````text
    escalate_label: ready-for-human    # default
    roles:
      <short name>: <its directory>    # owner/repo, owner/repo/path or ./path
      <short name>: { dir: <its directory>, host_user: false }   # its own host_user
    roster:                            # optional: `afkconvoy help roster`
      <short name>: { cast: <how to cast>, run: [{ harness: pi, model: <model>,
        good_at: <what it does best> }] }   # cast, model and good_at optional
    precedents: { by: <role>, do: <skill>, max_open: 10 }   # optional
    host_user: true                    # default
    dsh_profile: fleet                # fleet (default) or host-user; DSH only
    split: true                        # default; false omits splitting

`escalate_label` is the label an issue handed to a human carries in place of its flow
label. `roles` gives each role the short name a `by` or a `to` says. `precedents`
names who proposes a human's ruling as a precedent: for each ruling a human gives an
escalated issue (`afkconvoy rule`) a worker in the role `by`, with the skill `do`,
opens a PR on the team's repo that appends one entry to `precedents.md`, unless
`max_open` are open; none without the key (`afkconvoy tick --help`). `host_user`,
true or false, says whether a worker is given the host user's own skills and rules
beside its role's (`afkconvoy help roles`); a role named by `dir` may set its own.
`dsh_profile`: `fleet` (default) or `host-user`; independent of `host_user`.
A role's `{ dir, dsh_profile }` says its own. Details: `afkconvoy run --help`.
`split: false` omits splitting from briefs; true or unsaid offers it, with no guard.
````

### HOW IT NAMES A ROLE

````text
A role is a directory too, and the team file names it in one of three ways:

    owner/repo                         a whole repo
    owner/repo/path                    a subdirectory of a repo: the first two segments
                                       are the repo, the rest the path from its root
    ./path                             a subdirectory of the repo the team itself is in

A `./path` is read from the root of the team's repo, not from the team's directory: a
team and its roles in one repo name no repo at all, and two teams of one repo name the
same role alike. Every role is read at the head of its repo's default branch, and
nothing names a branch or a tag. A role's directory is not the team's and does not
contain it, is not inside the team's `workflows/` or `patrols/`, and neither contains
another role's nor is inside one. It holds:

    CLAUDE.md                          its rules: what a worker in it always works under
    .claude/skills/<skill>/SKILL.md    one directory per skill it can be asked for
    .claude/settings.json              its limits for claude-code: hooks, and the
                                       permissions it denies
    .pi/extensions/                    its limits for pi: extensions
    .codex/hooks.json                  its limits for codex: hooks
    .dsh/cordis.patch.yml              its limits for dsh: a DSH overlay

Only a skill's file has keys `check` reads: `afkconvoy help skill-file`. A `do` names
an entry of `.claude/skills/` in the role. Rules and skills are written once; limits
per harness, in its own form (`afkconvoy run --help` says how each loads a role).

A worker is given its role's directory alone, and nothing else of its repo: a
`CLAUDE.md` or a `.claude/` in a directory above the role is loaded for a session
opened there by hand, and a worker never sees it — and a symlink that leaves the
role's directory would point at nothing. The team's directory is given to every
worker beside its role's, without the directories of its roles and without a
`.claude/` at its top: a worker's skills and hooks are its role's.
````

### THE TEAM RULES

````text
The `CLAUDE.md` in the team's directory, and optional. Every worker of the team is
given it, whatever its role and whatever it was started for — a stage, a consult, a
patrol, the proposal of a precedent, a landing — and no key turns it off. A role two
teams name works under the rules of the team whose stage it is in.

They are one of four kinds of text a worker reads, beside the repo's own rules, its
role's rules and the precedents; the host user's rules give way to all four. Whose:

    its role's        the CLAUDE.md of the role's directory, under every harness
    the team rules    the CLAUDE.md of the team's directory, under every harness
    the repo's own    of the repo an issue is of: under claude-code its CLAUDE.md;
                      under pi and codex its AGENTS.md, or CLAUDE.md where it has none

Claude Code and pi load the three; the fleet names the other two to Codex.
What each is for, and where a sentence belongs: `afkconvoy help roles`.
````

### HOW A REPO IS BOUND TO IT

````text
By its config, on its base branch (`afkconvoy config --help` shows the file whole):

    team            owner/repo or owner/repo/path: naming it is what binds the repo
    patrols         which of the team's patrols run here, by name; none by default
    intake_limit    the open issues taken in by workers, at most; 50 by default

The team is read at the head of its repo's default branch: pushing is the only way an
edit reaches a worker. An issue of a bound repo is the fleet's to work only while it
carries a flow label, which `afkconvoy flows` creates: `afkconvoy help workflows`.
````

### WHAT CHECK HOLDS IT TO

````text
`afkconvoy check` loads a team and answers every fault at once, each with its file and
key; `afkconvoy check --help` lists them all. Those a role's directory can hold:
- A role's directory that holds an `AGENTS.md`, an `AGENTS.MD` or an
  `AGENTS.override.md` is a fault: pi and Codex load one in place of `CLAUDE.md`.
- A role that holds limits for one harness and none for another is right for the
  first alone: a roster entry that lets it run the other is a fault. One with no
  limits is right for every harness; two that refuse alike are the role's to see to.
- A `.claude/` in the team's directory is no fault and is loaded by no worker: a note.
````

### WHERE TO READ ON

````text
`afkconvoy help roles`      what goes where in a role, and what makes one good
`afkconvoy help roster`     what each role's workers may run, and an issue's casting
`afkconvoy help workflows`  the stages an issue passes through, and every key of them
`afkconvoy help patrols`    a role run on a schedule, intake and the fleet lock
`afkconvoy help glossary`   each term used here, with its one definition
````


## afkconvoy help roles

````text
What a role is, what goes where in one, and what makes one good. The files of a
role, path by path and harness by harness, are in `afkconvoy help team`; a
skeleton of one is written by `afkconvoy scaffold role`. What a skill's file holds —
its path, its frontmatter, its keys — is in `afkconvoy help skill-file`.
````

### WHAT A ROLE IS

````text
A role is a directory that defines one kind of worker. A worker of a bound repo is a
session of its harness started in that directory, with the issue's worktree beside
it: open the directory by hand and you are in the role a worker is in. Of your own
configuration it loads your skills and rules alone (below). The directory holds
three things:

    its rules     what a worker in it always works under, whatever it is asked
    its skills    each one thing it can be asked to perform; a stage's `do` names one
                  (`afkconvoy help skill-file`)
    its limits    what it may never do, in the form its harness enforces: hooks for
                  Claude Code and for Codex, extensions for pi

A role names no workflow and no stage. The workflow says in which order the work is
done and who judges whom; a role serves as many workflows as name it.

A change to a role reaches a worker only once it is pushed to the default branch of
the role's repo: a stage starts under what is at the head when it starts.
````

### WHAT GOES WHERE

````text
a rule the role follows whatever it is asked
    its rules: they are loaded for every stage the role performs
how to perform one kind of work
    a skill: a stage's `do` names it
something the role must never do
    a limit: a sentence can be ignored; a hook refuses
the order of the work, and who judges whom
    the workflow: a role serves many workflows and names none
what must hold before any change lands
    the repo's gate: a role's limits do not see the repo's code
what one codebase asks of whoever changes it
    the repo's own rules: they are loaded from the worktree, in every role
how the team works, in every repo bound to it
    the team rules: every worker of the team is given them
````

### WHY A LIMIT IS A HOOK AND NOT A SENTENCE

````text
A worker reads its rules and still does, now and then, what they forbid: a rule is
text a model weighs against the task in front of it. A hook is not weighed. The
harness runs it before each thing the worker does, and when it refuses, the thing is
not done and the worker is shown why. It holds although a worker runs with every
permission prompt bypassed, since nobody is at its terminal to be asked.

So write as a sentence what you want done well, and as a limit what must not happen
at all: a commit made past its hooks, a branch rebased, a test deleted. The fleet
adds a limit of its own to every worker of a bound repo — the guard, for what no
worker of any role may do (`afkconvoy guard --help`); a role's limits add what that
role may not. Either stops a worker that errs, not one that sets out to get past it:
the gate still stands between every change and the base branch.
````

### THE FOUR KINDS OF TEXT A WORKER READS

````text
A worker of a bound repo works under four kinds of text. Each is for one thing, and
a sentence belongs in exactly one of them:

    the repo's own rules   what one codebase asks: how it is built and tested, what
                           its conventions are. True for every role that touches it,
                           and for no other repo.
    the team rules         how the team works: what every role holds to, in every
                           repo bound to the team. True whatever the codebase.
    the role's rules       what one kind of worker does and does not do, whichever
                           stage it is in and whichever repo it is in.
    the precedents         what a human ruled before, on a question a stage could
                           not decide: followed instead of asking again.

To place a sentence, ask whom it would still be true for. For this codebase under
any team: the repo's own rules. For this team in any codebase: the team rules. For
this kind of worker alone: its role's. And if nobody could have written it down
before the question came up, it is a ruling, and becomes a precedent when a human
accepts it (`afkconvoy help humans`). The four have no order: two that contradict
each other are a defect of the files, and nothing says which holds.
````

### THE HOST USER'S SKILLS AND RULES

````text
A worker is also given the skills and the rules of the host's own user, as a session
that user opens by hand has them — Claude Code's ~/.claude/skills and CLAUDE.md, pi's
and Codex's in ~/.pi/agent and ~/.codex — so a role need not copy in the skills its
author relies on everywhere. Except for DSH's explicit `dsh_profile: host-user`
choice (`afkconvoy help team`), never their settings, hooks, MCP servers or extensions:
the guard and orca's report stay the only hooks the fleet adds. A role's skill wins
over one of theirs of the same name, and the brief, the role's rules, the team rules
and the repo's own rules win over theirs; the brief says so. The cost: a role may
behave differently on two hosts. A team file that says `host_user: false` gives its
workers none of it, and every role is what its directory says on every host. One role
may say it for itself, whatever the team says — `reviewer: { dir: ./roles/reviewer,
host_user: false }` — and its workers alone are given none of it, on every harness;
the team's other roles keep the team's (`afkconvoy help team`).
````

### WHAT MAKES A ROLE GOOD

````text
- It names no stage and no workflow. The brief the fleet writes for each worker
  carries the issue, the stage, the worktree, the stage records so far and the exact
  commands to end with. A skill that says "end as your brief says" stays right when
  the workflow changes.
- Its skills say how; the brief says what. The brief is a fixed form with nothing of
  how to do the work well. What a plan contains, what a judge looks for, what is
  looked at before a landing: that is the skill's to say.
- The one who writes and the one who judges are different roles, and every judging
  stage is `writes: false`. A judge that can fix what it finds stops reporting it.
- `reject` has a bar. A judge with no rule about when to reject sends work back over
  a name. Say what blocks and what is a note, and that notes alone are a `pass`. The
  breakers end a loop; a bar keeps it from starting.
- It says what is not its to decide. The most useful sentence in a role is the one
  that tells it when to stop and declare `escalate`. Without it a worker decides
  what a user of the code will see, and you learn of it in the diff.
- It is taught the three verdicts no workflow writes — `blocked`, `already-satisfied`
  and `escalate` — once, in its rules: they mean the same at every stage.
- It is small. One role and one stage that lands is a working team; add the judge
  once you have read what the first role produces. `afkconvoy rehearse` tries a
  workflow out with no worker started, and `afkconvoy check` holds every edit to
  what the fleet will accept.
````


## afkconvoy help roster

````text
A team's roster says what each of its roles' workers may run: which harnesses, on
which models, in what order. Read this to write one, to see why a worker started on
the harness it did, or to understand a refusal of `afkconvoy run` that names a range.
````

### THE ROSTER

````text
The `roster` of the team file (`afkconvoy help team`) holds an entry per role it
names, by the role's short name:

    roster:
      dev:
        cast: Hard or wide work on fable; the rest on opus.   # optional
        run:                           # one at least, in order of preference
          - { harness: claude-code, model: opus, good_at: most work }
          - { harness: claude-code, model: fable, good_at: hard or wide work }
          - { harness: pi }            # any model the host kit holds for pi
      reviewer:
        run: [{ harness: codex, model: gpt-5.5 }]

Each of `run` names a `harness` — claude-code, pi, codex or dsh — and may name a
`model`, its harness's own name for it, and `good_at`, what it is good at, which the
caster is told. One that names no model allows any model the host kit holds for the
harness, or the harness's command as written where the host kit holds none. `cast`
says, in the team's words, how an issue is to be cast among them: the caster is told
it too. A role the roster names no entry for may run whatever the host kit holds.

A roster is drafted rather than written from nothing: `afkconvoy draft`, in a
checkout of the team, writes one into team.md from this host's host kit — every
role, every harness and model in the host kit's order, what the host kit's first
model says each is good at and how to cast — and commits nothing. A human reads
it, edits it and commits it; the fleet never changes a team's roster.
````

### A ROLE'S RANGE

````text
On a host, a role's range is its entry cut to the host kit (`afkconvoy kit --help`),
in the roster's order: a harness, or a model, the host kit does not hold is left out.
`afkconvoy check` answers each role's entry and its range on the host it runs on;
`afkconvoy show` draws the roster under the workflows.
````

### AN ISSUE'S CASTING

````text
An issue's casting is its `harness:` and `model:` labels, which the caster
(`afkconvoy cast --help`) or a human puts on; the caster is given each stage's range,
and casts only an issue some range leaves a choice in. A worker for an issue — its
stage's, its consult's, its landing's — runs the casting when its role's range holds
it, the range's first when it does not or the issue has none, and the range's one
where it holds one harness and one model, whatever the casting. `afkconvoy plan`
says which, and why. A casting no range of the issue's workflow's stages holds is
not dispatched, and `plan` says why. A patrol's worker, which has no issue, runs its
role's range's first. A harness `afkconvoy switch` chose on a host runs every worker
there, whatever the roster and the casting say.
````

### WHAT IS REFUSED

````text
- A stage, a patrol and a config name no harness and no model: `check` refuses one
  that does, with the line to delete and the roster entry to write in its place.
- In the roster, `check` refuses an entry for a name that is no role, a `run` that is
  missing or empty, a harness that is none, an allowance written twice, and a
  harness the role holds no limits for while it holds limits for another.
- `afkconvoy run`, `tick` and `plan` refuse a role whose range on their host is
  empty, or holds a harness the role holds no limits for, naming what the roster
  allows it and what the host kit holds: add to the host kit with `afkconvoy kit`,
  or change the roster.
````

### WHERE TO READ ON

````text
`afkconvoy help team`       the team file the roster is a key of
`afkconvoy draft --help`    a roster drafted from the host kit
`afkconvoy kit --help`      what this host can start, and on which models
`afkconvoy cast --help`     the caster, and the labels it puts on
`afkconvoy help glossary`   each term used here, with its one definition
````


## afkconvoy help skill-file

````text
What a skill's file holds: where it is, what it opens with, and what `afkconvoy check`
holds it to. What belongs in a skill and what in a role's rules or its limits is in
`afkconvoy help roles`; the other files of a role are in `afkconvoy help team`.
````

### WHERE IT IS

````text
A skill is a directory of its role's `.claude/skills/`, and its file is the `SKILL.md`
there. The directory's name is the skill's: it is what a `do` says — a stage's, a
consult's, a landing's, a patrol's — and what the file's `name` says. A role with
the skills `review` and `rule` holds:

    .claude/skills/review/SKILL.md
    .claude/skills/rule/SKILL.md

The skills are written once, there, whatever harness a stage runs the role on.
````

### WHAT IT HOLDS

````text
    ---
    name: review
    description: Review the code an issue's PR holds, and pass or reject it. Use
      when the brief you were started with names `review` as the skill to perform.
    ---

    # review

    1. Read the issue, every comment on it, and the diff of its PR.
    2. Reject what blocks; say what is only a note, and pass on notes alone.
    3. End as the brief you were started with says to.

The file opens with frontmatter: a first line `---`, yaml, and a line `---`. Every
harness reads two keys of it, and both must be there:

    name           the skill's name: the name of its directory, letter for letter
    description    what the skill does and when to use it, in a sentence or two. A
                   harness shows a worker this and not the file, until the worker
                   loads the skill: say there that the brief names it

Any other key is one harness's own, and the fleet reads none. What stands under the
frontmatter is the skill: markdown, in whatever form says how the work is done
well. Other files of the directory are the skill's to name and a worker's to open.
````

### WHAT CHECK HOLDS IT TO

````text
`afkconvoy check` reads the `SKILL.md` of every skill a `do` names, and a fault at
that `do` is one of:

    the skill's directory holds no `SKILL.md`
    the file does not open with frontmatter, or its yaml cannot be read
    its `name` is missing, or is not the name of its directory
    its `description` is missing or empty

A skill no `do` names is not read: a role may hold skills by other authors, which a
worker loads when its own skill says to. What the file says under its frontmatter
is nobody's to check but yours: `afkconvoy help roles` says what makes it good.
````

### SEE ALSO

````text
`afkconvoy help roles`        what goes in a skill, and what in rules and limits
`afkconvoy help team`         every file of a role, path by path
`afkconvoy scaffold --help`   a role's skeleton, with one example skill
`afkconvoy check --help`      every fault, and its key
````


## afkconvoy help workflows

````text
A workflow is the stages one issue passes through and where each verdict leads, ending
in the landing train or in nothing more to do. Read this to write a file of a team's
`workflows/`, or to tell why an issue went where it did.
````

### A WORKFLOW'S FILE

````text
`workflows/<name>.md` is the workflow `<name>`: an issue that carries its flow label is
at the first stage. That is `flow:<name>`, or what `label` names — `ready-for-agent`
takes issues opened the usual way; an epic stays out. Above the yaml block, say what
the workflow is for and what an issue of it holds: `afkconvoy flows` answers that
prose to whoever opens an issue, and `afkconvoy check` warns of a file that says
nothing there.

    label: <label>                     # optional; default flow:<name>
    max_rework: 3                      # default
    max_consecutive_rework: 2          # default
    stages:
      - name: <unique; not land, done or human>
        by: <role> | human
        do: <skill>                    # required unless by: human
        writes: false                  # optional; default true
        escalate: { to: <role>, do: <skill> } | { to: human }  # optional: the consult
        on: { pass: <stage> | land | done, reject: <earlier stage> }   # reject optional
    land: { by: <role>, do: <skill> }  # required exactly when a pass leads to land
````

### STAGES

````text
A stage is one skill of one role, performed by one new worker: `by` is a short name of
the team file, `do` an entry of that role's `.claude/skills/`. The worker of a stage
knows nothing of the one before it but what the stage records say, so a stage that
judges never inherits the reasoning of the one that wrote.

A stage names neither a harness nor a model. What its worker runs is its role's
range's to say, picked among by the issue's casting (`afkconvoy help roster`).

`writes: false` has the guard refuse the stage's worker a commit and a push: mark
every stage that judges so.
````

### VERDICTS, AND WHERE EACH LEADS

````text
A worker ends its stage with `afkconvoy verdict`, which leaves a stage record on the
issue. Where an issue is, is derived from those records each tick and stored nowhere:
`afkconvoy show --issue <n>` reads it.

    pass               where the stage's `on.pass` says: a stage, `land` or `done`
    reject             back to the stage `on.reject` names: always an earlier one
    blocked            parked; the same stage runs again when its blockers close
    already-satisfied  the workflow ends, with nothing landed
    escalate           to the stage's consult (below), or a human where it says so

Only `pass` and `reject` are keys of `on`: the other three mean the same at every
stage. `done` ends the workflow with nothing to land; `land` hands the issue's PR to
the landing (below). A stage with no `reject` may not declare one. Following `pass`
alone from any stage must come to `land` or `done`, and every stage must be reached
from the first: a loop back is made with `reject` only.

A stage may split its issue: open issues, take each in, then declare `blocked` on
them. When they close, it judges the whole. The team file's `split` key may be false
to omit this procedure from briefs (`afkconvoy help team`); true is the default.

A stage that may commit and finds its gate red for a defect outside its issue — what
the gate merged in is red the same way gated alone — opens that defect's issue
itself, unless an open one already records it, takes it in the same way, and
declares `blocked` on it. When it closes, the stage runs again and gates again.
The train worker does the same for a base branch that is red without the train: it
takes the defect's issue in with `afkconvoy intake --train`, to the team's first
workflow unless another fits it clearly better, and has no issue to park.
````

### REWORK AND ITS BREAKERS

````text
Rework is a stage running again because a later stage rejected its work or a consult
ruled on it. It is counted per issue from the stage records, from where a human last
ruled on the issue or sent it to a stage. Two breakers bound it:

    max_rework                 the rework of one issue, in total
    max_consecutive_rework     the rejections in a row by one stage

A count never passes its breaker: the rework that would is not started, and the issue
is handed on (below), naming the breaker and every rework's stage and reason.
`afkconvoy rehearse` shows which verdict trips one, with no worker started.
````

### A CONSULT

````text
A stage's consult is asked before a human is: whatever would hand the issue to a human
there — its `escalate`, a tripped breaker, a failure, a workflow changed under it —
starts it where that human would stand, free to run what they would (`afkconvoy help
humans`). `escalate: { to, do }` names a more senior role, run within its range
(`afkconvoy help roster`); where a stage names none, the stock consult, afkconvoy's
own, is asked on what the host kit names for it — its one instruction is to decide as
the human would. `{ to: human }` names none: a human, straight. A consult rules
(`afkconvoy rule`: the stage runs again; on an `escalate` that is one more rework in
total), sends, closes, parks or lets it through.
Close and park add no rework (`afkconvoy help humans` says how to bring a park back).
The same hand-off again goes to a human. A consult writes no code.
````

### A STAGE BY: HUMAN

````text
No human stands in a workflow unless it names one. A stage `by: human` has no `do`;
it starts no worker and holds no claim. The issue waits
there until a human declares the stage's verdict with `afkconvoy verdict`.

Anywhere else a human is reached by an escalation: what a consult let through, or
what a stage with `to: human` hands on. The human answers with `afkconvoy rule` — the
stage runs again under the ruling — or `afkconvoy send`, to a stage of their choosing.
````

### A LANDING

````text
A workflow in which a pass leads to `land` names who lands: `land: { by, do }`. The
tick after that `pass` starts a new worker in the role `by`, with the skill `do`, within
its role's range. It runs the gate on the issue's PR and joins it to the landing
train (`afkconvoy land --help`), which lands it on the base branch as it lands any
PR. A `land:` no pass leads to is a fault, and so is a `pass: land` with no `land:`.
````

### WHERE TO READ ON

````text
`afkconvoy help team`       the roles a `by` names, and the rest of a team's directory
`afkconvoy help roles`      what a role is, and how to write a good one
`afkconvoy help patrols`    how an issue comes to carry a flow label with no human
`afkconvoy help glossary`   each term used here, with its one definition
````


## afkconvoy help patrols

````text
A patrol is a role run on a schedule rather than for an issue: it looks for work, opens
issues and takes them in. Read this to add a patrol to a team, to have a repo run one,
or to learn how work reaches the fleet with no human labelling it — and how a fleet lock
stops everything but one fix.
````

### WHAT A PATROL IS, AND IS NOT

````text
It is a worker in one role, with one skill, started when its interval has passed, in a
checkout of the base branch that is linked to no issue. What it finds — an issue no
workflow has taken, a broken base branch, something worth proposing — it writes down
as issues, and takes in the ones the fleet is to work.

It is no stage: it owns no issue, belongs to no workflow and declares no verdict. It
writes no code: a run that left a commit or opened a PR has failed. And it is no
timer for anything else: nothing but a role's skill can be run by one.
````

### A PATROL'S FILE

````text
`patrols/<name>.md` in the team's directory is the patrol `<name>` — words of lowercase
letters and digits, each starting with a letter, joined by `-`, 40 characters at most.
Of the file only the first ```yaml block is read:

    by: <role>
    do: <skill>
    every: 6h                          # required: a whole number from 1, then m, h or d
    max_open: 5                        # required
    max_runs_a_day: 4                  # optional: a whole number from 1
    max_landed_a_day: 5                # optional: a whole number from 1

`by` is a short name of the team file and `do` a skill of that role. `every` is how
long after its last run a patrol is due: it is due or it is not, and one that missed
ten intervals runs once. Its worker runs its role's range's first
(`afkconvoy help roster`). `max_open` bounds what a patrol gives the fleet: it is
not run while that many issues it opened are open.

`max_runs_a_day` bounds how often it runs besides `every`: a patrol whose runs that
started in the last 24 hours — rolling, the same on every host — have reached it is
not due, whatever `every` says, until the oldest of them is 24 hours old. Each run's
record on the remote carries the starts of the day before it; nothing is kept beside
it. Left out, it adds no bound. `afkconvoy check`
refuses one that bounds nothing: `every: 1h` lets a patrol run at most 24 times a
day, so `max_runs_a_day` is at most 23 there, and a patrol run every day or less
often has no use for it.

`max_landed_a_day` bounds the issues it opened that GitHub closed as completed in
the last 24 hours, rolling and the same on every host. At the bound the patrol
waits until enough completions are 24 hours old to leave fewer than the bound.
Issues closed as not planned, reopened issues, and issues opened by humans or
other patrols do not count. Ownership comes from the latest intake record,
regardless of the issue's labels now. Left out, nothing changes. `check` refuses
zero, negative, fractional and nonnumeric values. Unlike runs, completions have
no daily ceiling from `every` or `max_open`: any positive whole number can bound
them. This limits starting runs; a run already going may open more issues.
`plan` reports `standing: landed`, `max_landed_a_day`, `landed_in_day` and `until`
while it is this bound that holds the patrol back.
````

### WHICH PATROLS A REPO RUNS

````text
The ones its config names, under `patrols` (`afkconvoy config --help`): a team's
patrol runs in no repo that does not name it, and a name the team has no patrol for
is a fault of `afkconvoy check`. Of several hosts one runs it: the tick that finds a
patrol due claims the run on the remote.
````

### A RUN

````text
A run takes one slot of `concurrency` until its worker declares it over with
`afkconvoy over`, the last line of its brief; a worker that never does is ended once
it is gone or has stopped for a grace period. The tick that ends a run closes its
terminal and removes its checkout. `afkconvoy plan` says how each patrol stands, and
`afkconvoy tick --help` what a tick does about one.

A run that failed keeps its checkout, and the fleet opens an issue with the team's
escalate label that says so. The patrol is not run again until a human closes it.
````

### A RUN NOW

````text
A human who wants a patrol's run now — not when its interval has passed — runs
`afkconvoy patrol-now <name>` in a checkout of the repo: the next tick runs it, and the
command wakes this host's `run` so that tick starts at once. No file changes. The run
passes `every` and `max_runs_a_day`, and nothing else: a fleet lock, `max_open`
or `max_landed_a_day` reached, a failed run still reported, or a run still going refuse
the ask. It is claimed as a due run is, so of several hosts one runs it. It is recorded
as the patrol's run, asked for: `every` counts from its start, and `max_runs_a_day`
counts
it among the day's. What trips you up: `afkconvoy patrol-now --help`.
````

### INTAKE

````text
Intake is putting a flow label on an issue: from then on the issue is the fleet's to
work, at the first stage of that workflow. A human does it on GitHub, outside any
limit. A worker — a patrol's, or a stage's that splits its issue — does it with
`afkconvoy intake`, which records who did; the guard refuses a worker the label
itself. To choose a workflow a patrol reads what each is for: `afkconvoy flows`.

Two limits hold what workers take in. The repo's `intake_limit` bounds the open issues
taken in by any worker, and `afkconvoy intake` refuses past it. A patrol's `max_open`
bounds the ones it opened, and refuses nothing: the patrol is not run.
````

### THE FLEET LOCK

````text
A fault every issue shares — the base branch is broken, so every gate is red whatever
its PR holds — is not one issue's to escalate. A patrol that finds it, or a stage,
opens an issue for the fix and raises the lock on it with `afkconvoy lock`: an open
issue carrying the repo's lock label is a fleet lock, in a bound repo or an unbound
one.

While one stands the fleet works lock issues only: nothing else is dispatched, no
patrol runs, no PR of another issue joins the landing train, and workers already
running are left alone. The lock issue is worked as any issue is: in a bound repo
it needs a flow label too. Closing the last lock issue ends the lock; a stage that
declared `escalate` or stayed silent before it closed then runs again, which is no
rework. What trips you up: `afkconvoy lock --help`.
````

### WHERE TO READ ON

````text
`afkconvoy help workflows`  what an issue taken in then passes through
`afkconvoy help team`       the roles a patrol's `by` names, and a team's other files
`afkconvoy help roles`      what a role is, and how to write a good one
`afkconvoy help glossary`   each term used here, with its one definition
````


## afkconvoy help example

````text
The binary holds one team whole, and writes it where you say:

    afkconvoy scaffold team hospital --example hospital --repo <owner/repo>
    afkconvoy check --team <owner/repo>

It is the team this project demonstrates itself with, as one repo holds a team: the
team at the root, each role under `roles/`. Read it before you design your own, or
start from it. Every file written is named below — the lines four spaces in, a role's
under its directory — with the decision it holds and the rule of afkconvoy's it shows.
````

### WHAT THE TEAM IS FOR

````text
It is modelled on the one Cockroach Labs describes in "An experiment in running a
hospital for code". An issue is a patient: routed, worked up into a plan, the plan
reviewed, treated under it, the code reviewed, its record audited, and discharged —
its PR joins the landing train. Each is a stage a new worker performs in one role:
none reviews what it wrote, and none writes code before the plan passed. The Chief is
a human: an issue reaches the Chief by escalating, and a consult is asked first.
````

### THE TEAM

````text
team.md
    Names the seven roles, each as `./roles/<name>`: a path from the root of the
    team's own repo, so that the team names no repo and is pushed, forked or renamed
    with no edit. Its `precedents` has the fleet start a worker in the role `safety`
    for each ruling a human gives, to propose it as a precedent: none is asked twice.
CLAUDE.md
    The team rules: three, given to every worker whatever its role. They say how
    this team works, and nothing of one codebase or of one role.
precedents.md
    Two precedents a human accepted, each saying when it applies. The first is about
    the demonstration's own code: take it out when the team works yours.
````

### THE WORKFLOWS

````text
The prose above each yaml block says what its workflow is for: `afkconvoy flows`
answers with it, and the role `triage-nurse` routes by it.

  workflows/fix.md
      Five stages, then the landing. A reviewer that writes nothing: `plan-review`
      and `code-review` are marked `writes: false`, as `workup` and `discharge` are,
      and the guard refuses their workers a commit and a push.
      A role on another harness: the roster lets the attending run pi first, on pi's
      own name for its model, so `plan-review` has the plan reviewed by something
      other than what wrote it; the caster puts its code reviews on Claude Code.
      A stage with a consult: `workup` and `treatment` carry
      `escalate: { to: attending, do: consult }`. Their `escalate` goes first to a
      worker in that role, which lets through to a human only what nobody decided.
      Rework and its breakers: a `reject` leads back to an earlier stage, and past
      `max_rework: 3` or `max_consecutive_rework: 2` the issue escalates.
      `land: { by: discharge-nurse, do: land }`: the join is made in the role that
      audited the record, not by the worker that wrote the code.
  workflows/chore.md
      Two stages, then the landing: the fewest of any that lands, so it is where the
      patrol `rounds` takes in the fix of a broken base branch.
  workflows/design.md
      A workflow that ends with a human: `proposal`, then `sign-off`, a stage
      `by: human` whose `pass` leads to `done`. Nothing lands. No human stands in a
      workflow unless its file says so: here the fleet waits for `afkconvoy rule`.
````

### THE PATROLS

````text
Each names a role, a skill, how often it runs and `max_open`; a repo's config says
which of them it runs.

  patrols/triage.md
      Every ten minutes: takes each open issue that is in no workflow in to the one
      that fits. It is how an issue opened with no flow label is worked at all.
  patrols/rounds.md
      A patrol that locks the fleet. Every half hour it runs the gate on the base
      branch; red twice, it opens an issue, takes it in to `chore` and raises a fleet
      lock on it. `max_open: 1` is one lock at a time.
  patrols/research.md  patrols/safety-review.md
      Daily, work proposed with its evidence; weekly, the stage records of what went
      badly, read for what would have caught it a stage earlier. Each opens issues.
````

### THE ROLES

````text
Each holds its rules, its skills — every `do` above names one — and its limits for
claude-code, `.claude/settings.json`. `afkconvoy help roles` says what goes where.

roles/fellow/
  CLAUDE.md  .claude/skills/diagnose/SKILL.md  .claude/skills/treat/SKILL.md
  .claude/settings.json  .claude/hooks/refuse.sh  .claude/hooks/skills-loaded.sh
  skills-lock.json
      Works an issue up and treats it. Two hooks, written executable: `refuse.sh`
      refuses what its rules say a fellow never does — `--no-verify`, a forced push,
      `git rebase` — and `skills-loaded.sh` a `pass` or a `reject` declared before the
      skills its skill says to load were loaded in the session.
roles/attending/
  CLAUDE.md  .claude/skills/review-plan/SKILL.md  .claude/skills/review-code/SKILL.md
  .claude/skills/consult/SKILL.md  .claude/settings.json  .claude/hooks/refuse.sh
  .claude/hooks/skills-loaded.sh  .pi/extensions/limits.ts  skills-lock.json
      Reviews, and is the consult. A `reject` needs a blocking finding, which keeps
      rework from the breaker over a name. `refuse.sh` refuses a commit and a push in
      every stage: the role writes no code, whatever a workflow left unmarked.
      `limits.ts` is both hooks again, as a pi extension: pi runs no hook of
      `.claude/settings.json`, so a limit held under two harnesses is written twice.
      Under pi a skill is loaded by reading its `SKILL.md`, which is what it counts.
      Without it `check` refuses `plan-review`: its role would hold no limits there.
roles/discharge-nurse/
  CLAUDE.md  .claude/skills/audit/SKILL.md  .claude/skills/land/SKILL.md
  .claude/settings.json
      The last check, and the landing role. `audit` is a checklist of the record, not
      a second look at the code; `land` says only how a conflict is resolved.
roles/safety/
  CLAUDE.md  .claude/skills/record-precedent/SKILL.md  .claude/skills/review/SKILL.md
  .claude/settings.json
      `record-precedent` is asked by the team file, `review` by a patrol.
roles/triage-nurse/
  CLAUDE.md  .claude/skills/triage/SKILL.md  .claude/settings.json
roles/infection-control/
  CLAUDE.md  .claude/skills/rounds/SKILL.md  .claude/settings.json
roles/research/
  CLAUDE.md  .claude/skills/propose/SKILL.md  .claude/settings.json
      The three that only patrol. The guard refuses a patrol's worker a commit and a
      push, so their `settings.json` holds no hook, as those of the two above do.
````

### WHERE TO READ ON

````text
`afkconvoy help example-run`  what a host needs before a fleet works with this team,
                              and one issue followed from its label to the base branch
````


## afkconvoy help example-run

````text
The team `afkconvoy help example` reads file by file, pushed, bound to a repo and
run. First what must be in place; then one issue of its `fix` workflow, followed
from its flow label to the base branch, with how long each stage took and what it
cost.
````

### BEFORE A FLEET WORKS WITH IT

````text
`check` accepts the team as written, once it is pushed: it reads a role from GitHub.
Three skills that the fellow's and the attending's own skills say to load are other
authors' and are not written. Each role's `skills-lock.json` names them and where
each is from: add them in each of the two directories, and commit them.

    npx skills add <source> --skill <name> --agent claude-code

Until then `skills-loaded.sh` refuses every `pass` and `reject` of those two roles,
and the attending's `limits.ts` does under pi. The hooks call `jq`: every host of
the fleet needs it.

The roster lets the attending run pi first, so a host whose host kit holds pi needs
`pi`, logged in to the provider of its model: `openai-codex/gpt-5.5`, as written —
`afkconvoy kit` adds it to the host kit, and probes it. Write another model pi lists
— or take pi out of the attending's entry, and it runs Claude Code as the others do.
A host kit holding pi on a host without it starts nothing: `plan`, `tick` and `run`
refuse, `doctor` finds the same, and each names the range that calls for it and the
line that installs it:

    pi is not installed, called for by the range of the role `attending`
````

### ONE ISSUE, FROM ITS LABEL TO THE BASE BRANCH

````text
A repo holding one small Go package was bound to the team; its gate formats, vets
and tests. One issue was opened with the label `flow:fix`: a named form the package
was to read, with what each call should give written out. Nobody touched it again.
One fleet instance on one laptop worked it, `concurrency: 3`:

    stage         role             harness      model                 worked    cost
    workup        fellow           claude-code  fable                 1m 44s   $1.39
    plan-review   attending        pi           openai-codex/gpt-5.5     51s   $0.41
    treatment     fellow           claude-code  opus                  1m 06s   $0.45
    code-review   attending        claude-code  opus                  1m 02s   $0.47
    discharge     discharge-nurse  claude-code  sonnet                   28s   $0.14
    the landing   discharge-nurse  claude-code  its default              31s   $0.19

From opened to merged on the base branch: 9 minutes 29 seconds and $3.05, with no
rework and no human. The repo's two patrols, `triage` and `rounds`, ran three times
beside it, for $0.23 more.

`worked` is a worker's session, from its first word to its last. The rest of the
nine minutes is the fleet's: one tick reads a verdict and starts the next stage's
worker, and a worker's `wake` ends the sleep between two. `cost` is what each
harness reckoned for its session at its provider's listed prices: under a
subscription login nothing is billed by the call.

Every stage left its record on the issue, a comment that names the harness it ran
on beside the commits of the role and of the team it ran under:

    <!--convoy:stage stage=plan-review verdict=pass … harness=pi-->
````

### WHAT HELD THE PI WORKER

````text
The issue asked the plan's reviewer to try one commit, so that the refusal could be
seen. pi made the call, it was refused, and the worker quoted the refusal in the
text of its verdict:

    afkconvoy guard: the stage `plan-review` is marked `writes: false`: `git commit`
    is refused. A stage that writes nothing leaves no commit and pushes nothing; say
    what you found in the text of your verdict

That is the fleet's guard, which a pi worker asks through the extension the fleet
adds to it (`afkconvoy run --help`). It answered before the role's own limits were
reached, so `limits.ts` refused nothing in this run: it is what refuses the same
commit in a stage a workflow left unmarked. Its other limit was met and let the
worker through — the reviewer read the `SKILL.md` of both skills its skill names
before it declared `pass`.
````

### WHERE TO READ ON

````text
`afkconvoy help example`    the team's files, each with the decision it holds
`afkconvoy help workflows`  `harness` and `model`, and every other key of a stage
`afkconvoy help team`       where a role's limits for each harness go
````


## afkconvoy help cache

````text
The one directory of its own the fleet keeps on a host: where it is, what moves it,
and what it holds. The helps that speak of a file under "the cache directory" mean
this one.
````

### WHERE IT IS

````text
    $XDG_CACHE_HOME/afkconvoy    where XDG_CACHE_HOME is set and not empty
    $HOME/.cache/afkconvoy       otherwise — on macOS too: never ~/Library/Caches

It is made when something is first kept in it. With neither variable set, a command
that needs it fails and says so.
````

### WHAT MOVES IT

````text
Setting XDG_CACHE_HOME, and nothing else: there is no flag and no config key for it.
To keep everything under a directory of your choosing:

    XDG_CACHE_HOME="$PWD/.cache" afkconvoy check

Each command reads the variable for itself, so two commands find the same copies and
the same log only under the same value: a `doctor` run without the one a `run` was
started with reads another directory, and finds no log there.
````

### WHAT IT HOLDS

````text
    copies/     the fleet's copy of a team's repo and of each role's, one per commit,
                read-only: copies/<owner>/<repo>/<commit>. A worker of a bound repo
                starts in one; an unbound repo has none.
    harness/    the user directory the fleet keeps for pi and for Codex, a worker's
                in place of yours: harness/pi, harness/codex.
    runs/       the log of each repo's `run`, one file per repo: what `doctor` reads.
                It keeps every line of the last 24 hours, and never fewer than 200
                lines nor more than 5000, the oldest dropped first.
    instances/  the socket each `run` of this host listens on, whatever its repo,
                named for its fleet instance: how they find one another before one
                puts the host to sleep. A `run` removes its own as it exits.
    host-inspection/  what the last inspection of the host was given (evidence.md)
                and what it found (last.json), whichever fleet of the host started
                it: what `doctor` and the panel show beside admission's reading.

None of it is fleet state: that is in GitHub and on the repo's remote.
````

### SEE ALSO

````text
`afkconvoy check --help`     when a copy is cloned, and what is read from it
`afkconvoy run --help`       the log, and what each harness's user directory holds
`afkconvoy doctor --help`    what is read back from the log and the copies
````


## afkconvoy help humans

````text
Every way the fleet stops for a human, and the command that answers each.
````

### WHERE TO LOOK

````text
`afkconvoy doctor` lists every open issue that waits for a human under
`waiting_on_human`, with why, since when and `answer`: the line to run for that issue.
`afkconvoy plan` gives the same list; the panel shows it as "With a human". Waiting
more than an hour is a finding.

An escalated issue carries the escalate label (`ready-for-human` unless renamed)
in place of its flow label. Its comment says why and ends with the line to answer.
Run every command below in a checkout of the repo.
````

### AN ESCALATION

````text
A stage failed or declared `escalate`. Its consult — the stock consult where none
is named, and in an unbound repo — was asked first and let it through, unless the
stage says `escalate: { to: human }` or the config `escalate_to: human`. The comment
says what failed or gives the question that needs your answer.

For a failure, fix what failed first: `afkconvoy rule` starts the same stage anew, and
what failed it once fails it again. In an unbound repo there is no stage to run again:
fix what the comment names and put the ready label back, with the line the comment
ends with. A PR the escalation kept is not started anew: the next worker takes it up
— it reads the issue's comments, merges the base branch, gates and joins the
landing train — so say on the issue what is to be done on it.
````

### ANSWERING AN ESCALATION

````text
Every command a human runs to answer one, `<n>` the issue's number. A consult may run
each, and any read-only command, the gate too; never commit, push or change the host.

    afkconvoy rule --issue <n> --text-file <file>
        The stage runs again under your ruling; both rework counts start over
        (a consult's starts none over). `afkconvoy rule --help`.
    afkconvoy send --issue <n> --stage <stage> --text-file <file>
        Put it at another stage, when the approach or workflow changed, or bring it
        back from a park. `afkconvoy send --help`.
    gh issue close <n> --comment <why>
        Nothing is left to do on it: the fleet leaves a closed issue alone.
    gh issue create --title <title> --body-file <file> --label <ready label>
        Open what it found outside this issue — a broken base branch, a host
        problem, a defect of afkconvoy — for the fleet to work.
    gh issue edit <n> --body-file <file>
        Keep its body's "## Blocked by" in step with an edge added or removed.
    gh api repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by -F issue_id=<id>
        Record it as blocked by the issue whose database id is `<id>`.
    gh api -X DELETE repos/{owner}/{repo}/issues/<n>/dependencies/blocked_by/<id>
        Remove that edge: one whose waiting would never end, say.
````

### PARKED FOR EVIDENCE

````text
A consult's close or park carries a reason and adds no rework. `plan` and `doctor`
list parks under `waiting_on_human`. To bring one back in a bound repo, explicitly
send it with new evidence or your reason (a plain comment does not bring it back):
    afkconvoy send --issue <n> --stage <stage> --text-file <evidence-file>
In an unbound repo, comment with the evidence, then restore its ready label:
    gh issue edit <n> --add-label <ready label> --remove-label convoy:parked
````

### A TRIPPED BREAKER

````text
Rework went past what the workflow allows — in total, or as rejections in a row by
one stage — and the issue went to the consult in place of that rework, and to a
human where the stage says `to: human` or it trips again after the consult ruled. The
comment names the breaker and lists every rework with its stage and why. Answer as
an escalation: `afkconvoy rule` with what settles it, or `afkconvoy send`.
`afkconvoy doctor` says beforehand when an issue is one rework from a breaker.
````

### A STAGE `by: human`

````text
A workflow may give a stage to a human. An issue at one waits there, claimed by
nobody, under its flow label and with no escalate label: only the lists above show it.

    afkconvoy verdict pass --issue <n> --stage <stage> --text-file <file>
        Declare the stage's verdict: `pass`, or `reject` where the stage has that
        edge. The issue goes where the workflow leads from it.
        `afkconvoy verdict --help`.
````

### A PATROL'S FAILED RUN

````text
A run of a patrol that failed opens an issue of its own, under the escalate label,
which says what failed. The patrol is not run again while that issue is open.

    gh issue close <n>
        Once you have looked: the patrol runs again when it is next due.
        `afkconvoy plan` gives each patrol's standing.
````

### A FLEET LOCK

````text
An open issue under the lock label is a fleet lock: the fleet works lock issues
only, and hands no other issue to a human, until the last one closes. A worker
raises one when the fault is one every issue shares; a human may too:

    afkconvoy lock --issue <n>

Closing a lock's issue ends it. A lock needs you when nothing can work its issue:
it has no flow label or an open blocker. `afkconvoy doctor` gives a critical finding
with a `fix` to run. Close a lock raised by mistake. `afkconvoy lock --help`.
````

### A PRECEDENT TO ACCEPT

````text
Where the team names who proposes precedents, each ruling a human gives with
`afkconvoy rule` is followed by a proposal: a PR on the team's repo that appends one
entry to its precedents — the ruling in a sentence, when it applies, and a link to
the ruling. Nothing waits on it: the stage that asked has already run again.

    gh pr merge <pr> --repo <the team's repo>
        Merging accepts the precedent: every later stage reads it, and follows it
        in place of declaring `escalate` on the same question.
    gh pr close <pr> --repo <the team's repo>
        The ruling was about that one issue, and no stage should follow it unasked.

The fleet never merges on the team's repo. `afkconvoy plan` lists the proposals
under way and open under `proposals`.
````

### AN ISSUE PAST THE END OF ITS WORKFLOW

````text
A workflow that ended without a landing leaves its issue open: `gh issue close <n>`.
````

### WHAT NOT TO DO

````text
Do not move the labels by hand: an issue's stage records say where it is, so one
handed back with no ruling on record is escalated again. Rule on it, or send it.
````


## afkconvoy help troubleshooting

````text
Start with `afkconvoy doctor`, in the checkout the fleet runs in: it reads everything
and changes nothing. Each finding has a `fix` — a line to run, or a file and key to
edit — and `evidence`. Below, a row for each: the words its `what` holds, why, and
what to do. Then what a start refuses with, and what neither says.
````

### WHAT `doctor` FINDS — critical: the fleet does not work until it is fixed

````text
is not installed
    A command the fleet needs is not on this host. Run `fix`: the line that installs it.
is not logged in
    `gh` or `claude` is logged out, as a login shell sees it. Run `fix`. Of `codex`:
    it has no login in ~/.codex/auth.json, the file a worker is given. One Codex
    keeps in a keyring is in no file: have Codex keep it in one, and run `fix`.
holds credentials for no provider
    A stage or a patrol runs on pi, logged in nowhere. Start `pi` and type `/login`.
is not running
    orca is not running, and every worker is started through it. Run `fix`.
cannot be asked about this checkout
    orca runs but did not say which repos it knows; `evidence` says why. Run `fix`.
does not know this checkout
    orca was never given the checkout worktrees are cut from. Run `fix`, once.
cannot start a worker
    The first word of a worker launch command resolves to nothing in a login shell, as
    an alias of your own shell's may. Name one that does: `afkconvoy run --help`.
cannot start a worker in its role
    In a bound repo the worker launch command must end in its harness, taking flags:
    the fleet adds what puts the worker in its role. `afkconvoy run --help`.
no `afkconvoy run` is alive
    The `run` of this host died holding claims. Start it here: it takes them back.
failed in each of the last
    One step fails tick after tick; `evidence` is its error, naming what refused. `fix`.
the team this repo names cannot be read
    The team's repo, or a role's, could not be read. `afkconvoy check` says which.
has a fault at its head
    A team's or a role's file is wrong on its default branch. `fix` is the file and
    key; `afkconvoy check` gives every fault. Push the edit: no stage starts until then.
stands and cannot be worked
    A fleet lock is on an issue no worker can be started on, so the fleet works
    nothing. `fix` gives the issue what it lacks; `afkconvoy help humans`.
the fleet's state cannot be read
    GitHub, the remote or the config failed to answer; `evidence` says which.
harness is down
    Identical refusals hold claims and pause it; a prompt probes every 5m. Run `fix`.
````

### WHAT `doctor` FINDS — warning: one issue or one patrol is held up. A warning may be the fleet mid-step: run `doctor` again after a tick before you act on one.

````text
has no worker
    A claim's worker is gone; a tick starts one. `fix` is for a claim it leaves so.
has been stopped for
    A worker stopped half an hour ago and declared nothing: read its terminal; `fix`.
the last tick started no worker
    Slots are free, the frontier is not empty, and nothing was dispatched: `evidence`
    is the tick's line, and `fix` the step that failed or the dispatch by hand.
follows no workflow of the team
    An issue's flow label names no workflow, or it has two. `afkconvoy flows` lists all.
longer than any stage before it took
    Nothing is wrong yet: an issue has been at one stage for long. `fix` shows it.
is one rework from a breaker
    The next `reject` escalates the issue. Nothing to do yet: `afkconvoy help humans`.
is paused on this host
    It refused every prompt: no worker starts on it till an hourly probe answers.
does not run: its last run failed
    The issue that reports a patrol's failed run is open. Look, then close it: `fix`.
have reached its `max_open`
    A patrol is not run while that many of its issues are open. Close some, or raise it.
has waited for a human for
    Nothing in the fleet moves that issue. `fix` answers it: `afkconvoy help humans`.
is held off by a PR not the fleet's
    No worker starts: it would open a second PR. `fix` closes it, or merge it yourself.
is claimed but GitHub has it no more
    Deleted or transferred: its fleet's next tick releases it. `fix` is for one stopped.
the gate is slow
    On one host, the last 5 landings with a recorded gate run all took more than 80%
    of the gate limit. A tick opens one issue for the fleet to make the gate faster:
    `fix` shows it. Raising `gate_seconds` is no fix, nor is deleting or skipping tests.
````

### WHAT `doctor` FINDS — info: nothing is held up

````text
moved since the stage
    A role or the team was pushed after a stage started: that worker goes by the
    older one, the next stage by the head. `afkconvoy doctor --help` says what moved is.
declares no gate limit
    The config sets no `gate_seconds`: the gate may run 3600 seconds before anything
    ends it. Set it to the most the gate should take: `fix` names the file and key.
is kept with no claim
    A worktree holds what the remote lacks, or an earlier release kept it. `fix`
    removes it, pushing first what only it holds: read `what` before you run it.
````

### WHAT A START REFUSES WITH — `plan`, `tick` and `run` check first, and start nothing

````text
this host cannot work a fleet yet
    What `doctor` finds on a host, said at once: run each line after a colon.
cannot read the config file
    The repo has no config on the branch checked out. `afkconvoy config --help`.
the remote has no branch
    The config's `base_branch` is not on the remote. Push it: the fleet creates none.
that push would be refused
    The base branch's protection requires a PR, checks or a named pusher, and the
    landing train lands by pushing. Lift the rule, or pick another base branch.
the remote rejects the fleet's refs under both
    The remote takes neither hidden refs nor `convoy-*` branches, so no claim can be
    kept. A push rule of the remote's is what refuses: lift it for one of the two.
the host kit has no worker launch command
    Run `afkconvoy kit` at a terminal to add the harness and confirm its command.
cannot read the host kit
    Run `afkconvoy kit` at a terminal on this host before starting the fleet.
--worker-command is gone
    Run `afkconvoy kit` to edit this host's worker launch commands.
is no harness
    Use claude-code, pi, codex or dsh.
````

### WHAT NEITHER SAYS

````text
An issue is not worked
    `afkconvoy plan` has it under `excluded`, with why. Remove what the reason names.
A changed role or team did not reach a worker
    It is not pushed to its default branch, or the stage had already started.
The labels were moved back, and the issue escalated again
    A label is not where an issue is: `afkconvoy help humans`.
A stage's verdict counted for nothing
    Its record names another stage than its claim was for. `afkconvoy show --issue <n>`
    says how each record was read; `afkconvoy send` puts the issue where it belongs.
Another host's fleet died
    Its claims go stale by themselves and are taken. Or: `afkconvoy takeover --help`.
Everything must stop now
    Kill `run`, then close the workers' terminals in orca. Every issue keeps its
    labels and stage records: a later `run` goes on from where each one was.
````


## afkconvoy help glossary

A command-line tool that works a repo's ready GitHub-issue backlog unattended: it dispatches one worktree-isolated coding agent per issue and lands their finished PRs on the base branch behind a local gate. It is a trimmed Rust port of the `afk-fleet` skill (`docs/adr/0001`).

### Language

#### The fleet

**Fleet instance**:
One `afkconvoy run` process and everything it owns: the workers it started and the claims it holds, under an id minted when it starts. One host runs at most one per repo.
_Avoid_: launcher, coordinator, node

**Tick**:
One reconciliation pass: rebuild the working set from fleet state, then perform every transition it calls for, without waiting for the workers it started.
_Avoid_: cycle, poll, batch

**Plan**:
A tick stopped after its rebuild, which reports what an acting tick would do and leaves nothing written: its startup probe, when it makes one, is the only thing it pushes, and it deletes that.
_Avoid_: dry run, preview

**Startup probe**:
What `run`, `tick` and `plan` ask of a host and a repo before anything else: that every command the fleet needs is installed and logged in, that orca is running and knows the checkout, that the base branch exists and no rule of its protection refuses a push, where the remote takes the fleet's refs, and whether it takes recorded gate runs. A host or a repo the fleet cannot work is refused here, not at its first tick or landing. `run` makes one at every start; one that passed stands for an hour on the host that made it, and `tick` and `plan` make none while it does.
_Avoid_: bootstrap, preflight, health check

**Ref layout**:
One of the two ways every ref the fleet writes is named: hidden refs (`refs/convoy/*`), or — only where the remote rejects those — ordinary branches (`convoy-*`), on which push-triggered CI fires.
_Avoid_: namespace, fallback mode

**Fleet state**:
The authoritative record of what is claimed, in flight, landed, parked and escalated. It lives entirely in GitHub; no process holds it.
_Avoid_: run state, memory (what a host holds resident, as under Hot worker)

**Working set**:
A tick's view of the fleet at one moment, derived from fleet state and dropped when the tick ends.
_Avoid_: session state

**Rebuild**:
The read-only pass at the top of every tick that derives the working set. Any fleet instance rebuilds the same working set from the same GitHub. It lists the remote once and asks GitHub a fixed number of times, however many issues are open.
_Avoid_: refresh (a heartbeat is refreshed), resync

**Frontier**:
The issues a worker can be started on now: open, carrying a flow label, not an epic, unclaimed, with no open blocker, and at a stage a role performs or whose `escalate` a consult is to rule on — or landing, with an open PR to join. In an unbound repo: carrying the ready label, with no open linked PR but one of the fleet's own that a released claim kept, off the landing train — which its worker takes up. While a fleet lock stands: of these, the lock issues only.
_Avoid_: queue (the exclusive queue of a host's heavy commands), backlog (all the issues a repo has open)

**Idle**:
A fleet instance with nothing to do now: it holds no claim — on an issue, a patrol's run or the proposal of a precedent — nothing is on its landing train, and its frontier is empty. An escalated issue waiting for a human leaves it idle, and so does a patrol not yet due.
_Avoid_: drained, done (what a step or a command did), finished (a PR its worker is through with)

**Host sleep**:
A fleet instance putting its host to sleep: every fleet instance on the host has been idle for ten minutes, each has ticked once more and found nothing, no one has used the host's keyboard or mouse for ten minutes, the one that puts it to sleep has worked since the host last woke, and no instance on the host was told not to. A host woken stays awake until some instance on it has worked again.
_Avoid_: sleep (the wait between ticks), hibernate, suspend, shutdown

**Base branch**:
The one branch a repo's fleet work is cut from and lands on.
_Avoid_: trunk, main, default branch (a team's or a role's repo is read at its own)

#### Teams and roles

**Role**:
A directory that defines one kind of worker: the rules it always works under, the limits it cannot cross, and the skills it can be asked to perform. It is a whole repo or a subdirectory of one. A worker starts in it, and is given nothing else of its repo.
_Avoid_: agent (as under Worker), persona, profile (as under DSH profile)

**Team**:
A directory that names a set of roles and holds the workflows, patrols and precedents they work by. It is a whole repo or a subdirectory of one, and its roles may be directories of the same repo.
_Avoid_: org, crew, squad

**Team rules**:
The rules every role of a team works under, whatever its own rules add. They say how the team works, and hold in any repo bound to it; what one codebase asks is that repo's to say, and what one kind of worker does is its role's.
_Avoid_: global rules, shared rules

**Admission**:
Whether a tick starts new workers on its host: not while the host is past a share of its load, under memory pressure, or low on free disk under the checkout. What it holds back stays on the frontier for the next tick; workers already running, the landing train and the worker a declared verdict starts are never held back. The shares have defaults taken from the host's cores, memory and disk, and the host can say its own. Holding back for one reason — the load, memory or disk — at every tick past a span (ten minutes, or the host's own) starts an inspection of the host, once an **episode**: the episode ends once admission has admitted for five minutes. Holding back for reasons that change starts none.
_Avoid_: throttling, backpressure

**Host user's skills and rules**:
The skills and the rules the host's own user has for a harness, which a session they start by hand loads: a worker of a bound repo is given them beside its role's, unless its team file says `host_user: false`. A role may say it for itself, whatever the team says: `reviewer: { dir: ./roles/reviewer, host_user: false }` in `roles`; a role named by its directory alone takes the team's. Its role's skills win over them, and the brief, its role's rules, the team rules and the repo's own rules win over theirs. DSH alone may load those too when its role chooses the host user's DSH profile.
_Avoid_: global rules, user config, personal skills

**DSH profile**:
What a DSH worker loads of bundles, plugins, MCP servers, patches, themes and settings: the fleet's own (`dsh_profile: fleet`, the default), or the host user's `dsh-tui` profile (`dsh_profile: host-user`). The team chooses for every role; a role may choose its own, independently of `host_user`, and both keep the guard and the role's limits.
_Avoid_: provider profile, persona

**Bound repo**:
A repo whose config names a team. An unbound repo has no roles and one fixed path for every issue: one worker, then the landing train.
_Avoid_: team mode, managed repo

**Guard**:
The check made before each thing a worker does, refusing what its role or its stage may not do. It stops a worker that errs, not one that means to get past it.
_Avoid_: permission (what a harness's own settings deny), sandbox (what a harness may shut a worker's commands in)

#### Workflows

**Workflow**:
The stages one issue passes through and where each verdict leads, ending in the landing train or in nothing more to do.
_Avoid_: pipeline, process (a program that is running)

**Flow label**:
The label that names the workflow an issue follows: `flow:<name>`, unless the workflow's file names another with `label` — the ready label, say, so that an issue opened the usual way is worked. A bound repo's issue is the fleet's to work only while it carries one; there the ready label is one only where a workflow names it.
_Avoid_: type label

**Intake**:
Putting a flow label on an issue. A human does it on GitHub; a worker does it through the fleet, which records who did and holds the number of such issues still open under a limit.
_Avoid_: enroll, admit (a tick starting new workers on a host not busy: admission), triage

**Stage**:
One step of a workflow: one skill of one role, performed by one new worker — or a decision waited for from a human.
_Avoid_: step (one thing a tick does), phase, state (fleet state and worker state are terms)

**Stage record**:
The comment a stage leaves on its issue: what it produced, its verdict, the versions of the role and team it ran under, and the harness its worker ran on, with the model where it was started on one. A human who declares for a stage, rules on it or sends the issue to it leaves one too, which says a human did; so does the fleet, for a stage that runs again because a fleet lock closed. The stage an issue is at is derived from these and stored nowhere.
_Avoid_: handoff note, log (the tick log; what a gate prints), stage label

**Position**:
Where an issue is in its workflow: at a stage a worker is to perform, waiting on a human, with a consult, parked, landing, ended, or escalating. It is derived from the stage records each tick and stored nowhere.
_Avoid_: state (as under Stage), status (the status board; a key of what a command answers), current stage

**Rework**:
A stage running again because a later stage rejected its work or a consult ruled on it. It is counted per issue from the stage records, from where a human last ruled on the issue or sent it to a stage.
_Avoid_: retry, round, attempt (what an issue was worked into before a fresh start)

**Breaker**:
A limit on rework — in total for an issue, or as rejections in a row by one stage — past which the issue escalates.
_Avoid_: retry limit, circuit

**Consult**:
A worker in a more senior role, asked before a human is whenever its stage would hand the issue to a human — an `escalate` verdict, a failure, a tripped breaker, a workflow changed under the issue. It stands where that human stands: told what the human would be, and free to run what a human answering runs. It rules, sends the issue to a stage, closes it, or lets it through; it commits, pushes and changes the host never. The same hand-off again, and its own failure, go to a human. Where a stage names none, and in an unbound repo, the **stock consult** stands in: afkconvoy's own, in no role of a team, whose one instruction is to decide as the human would. A stage that says `escalate: { to: human }`, or an unbound repo whose config says `escalate_to: human`, has no consult.
_Avoid_: review (a stage may be named it), second opinion, handoff, default consult

**Ruling**:
What a consult or a human decides on an issue handed on. The stage runs again under it; a human's also hands the issue back to the fleet and starts the rework counts over, a consult's starts none over.
_Avoid_: answer (what a command prints), decision (what a `by: human` stage waits for), override

**Precedent**:
A ruling kept so that later stages follow it instead of escalating. A worker proposes it; a human accepts it.
_Avoid_: rule (a role's rules and the team rules; the command `rule`), policy, guideline

**Proposal**:
One human's ruling put forward as a precedent: a PR on the team's repo that appends one entry to its precedents file. A ruling gets one; a human merging it is what accepts the precedent.
_Avoid_: suggestion, draft (a roster `afkconvoy draft` writes), candidate (a key of what `recovery` answers)

**Split**:
Parking an issue on issues its own stage opened. When they have all closed, that stage runs again and judges the whole.
_Avoid_: decompose, epic (an issue under an epic label), fan out

**Patrol**:
A role run on a schedule rather than for an issue. It opens issues and takes them in; it writes no code. It is due once its interval has passed since its last run, and — where its file says `max_runs_a_day` — while fewer than that many of its runs started in the last 24 hours, rolling. Where it says `max_landed_a_day`, it is also held while that many issues it opened were closed as completed in the last rolling 24 hours, until enough completions are 24 hours old to fall below the bound. Its run takes one slot until its worker declares it over, which the next tick acts on; a worker that never does is ended once it is gone or has stopped for a grace period. Each run has a checkout of its own, removed as the run ends unless it failed.
_Avoid_: cron, job, scheduled worker

**Fleet lock**:
An open issue carrying the lock label, in a bound repo or an unbound one. While one stands the fleet works lock issues only — nothing else is dispatched, no PR of another issue joins the landing train, and workers already running are left alone, what they declare being acted on afterwards — and closing the last one ends it. A stage's `escalate` or failure the lock explains — declared before it closed, and not acted on since — is then not carried out: the stage runs again, which is no rework.
_Avoid_: freeze, pause (a harness paused on a host, as under Host kit), kill switch

#### Workers

**Harness**:
The program a worker is a session of: it runs a model, with tools, on the work it is handed. Claude Code is one, pi another, Codex the third, and DeepSeek Harness (DSH) the fourth. An issue's casting names the one its workers run, within their role's range; the host kit names the stock consult's. A host where a switch chose one starts every worker of the repo on that one.
_Avoid_: agent (as under Worker), coding agent (as under Worker), CLI, provider (whose models a harness runs)

**Roster**:
A team's list, for each of its roles, of the harnesses and models that role's workers may run, in order of preference, with what each model is good at and how an issue is to be cast among them. It is committed with the team, written under the team file's `roster`: for each role it names, a `run` listing `{ harness, model, good_at }` in that order of preference — a harness named with no model allows any model the host kit holds for it — and a `cast` saying how to cast among them. A role's **range** on a host is its entry cut to the host kit, in the roster's order; a role it names no entry for, and an unbound repo, having no team, range over the host kit whole. A stage, a patrol and a config name no harness and no model: what a worker runs is its role's range's to say. A roster is drafted by `afkconvoy draft` from a host kit into the team's working tree — every role, every harness and model in the host kit's order, what the host kit's first model says each is good at and how to cast — and committed by a human; the fleet never changes it.
_Avoid_: harness config, model table, routing rules

**Host kit**:
The harnesses and models one host can start now — each harness's worker launch command, each model it was seen to answer with, in order of preference, and the harness and model of its stock consult. `afkconvoy kit` asks each harness for its models, or the human names them, sends each a one-line prompt, and keeps those that answered, with how long they took and when; the human sets the order. Its **first** is its first harness on that harness's first model, which the stock consult runs where the host kit names none for it. Kept on that host and never pushed; a harness paused on the host is left out of it until it works again.
_Avoid_: roster (the team's), host config, inventory

**Casting**:
The harness and model an issue's workers run, named by the issue's `harness:` and `model:` labels — and the act of choosing them, by a human or by a caster. A stage whose range holds the casting runs it, one whose range does not runs its range's first, and one whose range holds one harness and one model runs that, whatever the casting; a casting none of an issue's stages' ranges holds is not dispatched, and `plan` says why; an issue none of whose stages has a choice to make is not cast. In an unbound repo the host kit is the range: its worker runs the casting where the host kit holds it, and the host kit's first where the issue has none; one it does not hold is not dispatched, and `plan` says which label and what the host kit holds. A harness named with no model starts its command as written. A label a human put on is never recast.
_Avoid_: assignment (a GitHub assignee), routing, dispatch (starting a worker)

**Caster**:
A worker in no role that casts, in one session, every open issue of the repo's that carries a flow label, has a choice to make and no casting, by the roster and the host kit. A tick that finds such an issue on the frontier and no caster running starts one, which runs `afkconvoy cast` on the host kit's first; the tick holds those issues back while it runs, for ten minutes at most from its start, and dispatches what it did not cast uncast, on its range's first. A human runs the same casting with `afkconvoy cast`.
_Avoid_: assigner, router, dispatcher

**Worker**:
A session of one harness, started in one role to perform one stage of one issue — or, after its last stage, to join its PR to the landing train — in that issue's worktree, and to report through GitHub. In an unbound repo it has no role and owns its issue from start to landing. It is ended — its terminal closed — by the step that ends what it was started for: its stage, its claim, or its run.
_Avoid_: agent (whoever reads the help; and a worker, to whoever has not met the term), subagent, child (an issue a split opened)

**Switch**:
Moving running work onto another harness in one command: the host's choice of harness for a repo, kept on that host and never pushed — whoever else works the repo runs the harness they have — and each worker named ended and one of that harness started in its place, in the same worktree, on the same branch, at the same stage, briefed to continue what is there. Every worker the host starts for the repo afterwards is of the chosen harness, whatever an issue's casting names, until a switch chooses again or the choice is cleared — `switch --clear` forgets it — and the casting stands again: every worker started afterwards, and each the clear names, runs what its range and casting say. The claim, the branch and the PR are untouched, the switch is recorded on the PR, and it is no restart, rework or nudge.
_Avoid_: harness change (what a new casting makes)

**Worker launch command**:
The shell string every worker of one harness is started with, in a fleet instance: one for each harness its workers run. The host kit says it, per harness, kept on the host at `$XDG_CONFIG_HOME/afkconvoy/kit.json` or `$HOME/.config/afkconvoy/kit.json` and edited with `afkconvoy kit`; a harness absent from it is refused; a start checks that of every harness it could be asked for, and starts no worker when one cannot be started. In an unbound repo it is opaque: the fleet never parses it, appends to it nothing but its harness's model flag where the worker runs a model — its issue's casting, the host kit's first, or the stock consult's — and only starts it — as every worker — under `nice`, with a `TMPDIR` of its worktree's own. In a bound repo the fleet adds to it what puts the worker in its role.
_Avoid_: agent command, provider profile

**Worker state**:
Whether a worker is busy or has stopped, as its harness reports it to the worker backend — or, where the backend has no report of it, as its terminal's last output shows. Stopped never means finished.
_Avoid_: liveness, status (as under Position)

**Verdict**:
What ends a stage, declared on the issue by whoever performed it: `pass`, `reject`, `blocked`, `already-satisfied`, or `escalate`. Opening a PR declares nothing.
_Avoid_: outcome (what a worker leaves on GitHub: its PR on the landing train, or its verdict), result, needs-human

**Silent worker**:
A worker that stopped, declared nothing, and for the grace period showed no sign of life — its terminal's output, a commit, a file touched, being started or nudged — while waiting on no background work of its own: a shell or a monitor it started that its harness shows still running, or a command of its in the exclusive queue. One waiting on such work is not silent until that work ends and it stays still past its nudge, or until it has waited longer than the gate limit.
_Avoid_: hung, idle (a fleet instance with nothing to do)

**Hot worker**:
A worker whose processes — every one started with its `TMPDIR`, and every one under those — take more than their share of the host, or whose worktree has outgrown its build. Its processes: more than half the host's cores (one at the least) held for two minutes, more than a quarter of its memory resident, the most of any worker while the host is under memory pressure, or more written to disk in ten minutes than a tenth of the disk free (a GiB at the least). Its worktree: what git ignores there past `OUTGROWN_TIMES` its **fresh build** and past `OUTGROWN_FLOOR` — the train worktree's rule, judged once its first green `afkconvoy gate` has recorded that fresh build. The host can say other limits (`--hot-cores`, `--hot-memory`, `--hot-written`). `run` samples each worker every ten seconds, with no root, and measures its worktree every minute; one that has stayed under every limit for five minutes — its worktree back under the rule among them — stops being hot, and that ends its **episode**: hot again later is a new one. The landing train's gate runs are the fleet's own and no worker's.
_Avoid_: hog

**Inspection**:
A worker in the role of the consult of a hot worker's stage — the stock consult where the stage names none, at no stage, and in an unbound repo — started in a checkout of its own once per episode, and given what was taken down as that worker turned hot (its process tree, a `sample` of its hottest process, its resident memory over the last minutes, what its processes wrote, the largest directories of its worktree), the issue, the worktree and the hot worker's terminal. It finds the work wrong, that heavy, or what takes so much not the worker's, and says so with `afkconvoy inspected`; the fleet types the first two at the hot worker and leaves what it found as a comment on the issue that is no stage record. One runs on a host at a time, across its fleets, takes no slot of the `concurrency`, and has 15 minutes. It is held as a consult and hands nothing on; nobody inspects where the stage or the repo says `human`, and a worker hot again after its inspection is never escalated for it. An inspection of the **host** is started by admission instead, on no worker: the stock consult, given what was taken down as it started — admission's reading and shares; for disk, each of the host's worktrees of every fleet with its issue, its stage, whether a worker holds it and what it holds, the landing train's, the fleet's caches and the largest directories of the whole volume under the home folder outside them, found within five minutes; for load or memory, the process trees of each worker and of the train and the hottest processes outside them. It finds what holds it `reclaimable` — something under the home folder can be rebuilt, which its `afkconvoy inspected` frees once it has checked each path: what git ignores in a git checkout no process uses and no worker holds, never a file git tracks or a change no commit holds, a closed issue's worktree holding no change, a tool's cache; what cannot be rebuilt or is expensive to fetch again — model weights, downloads, media, Docker's images, a user's files — it names, and nothing deletes — `fleet` — the fleet's own shape outgrows the host, for which the fleet opens one issue with the escalate label, and none other while it is open — or `not-its`; the log, the panel and `doctor` show it beside admission's reading. Of either kind, one runs on a host at a time, and none where another fleet of the host inspected it in the episode.
_Avoid_: investigation, diagnosis

**Exclusive queue**:
The queue of the commands run through `afkconvoy exclusive` on one host, shared by every fleet on it whatever its repo: one runs at a time, in the order they asked, and the rest wait. A command whose process dies leaves it. A worker with a command in it is busy, not silent, for no longer than the gate limit.
_Avoid_: mutex, semaphore, heavy lock (the fleet lock is another thing)

**Nudge**:
The one line the fleet types at a silent worker with no outcome, telling it to carry on. Sent once per worker.
_Avoid_: ping, reminder, retry

**Restart**:
Replacing a silent worker with a new one in the same worktree and the same role, with its PR, branch and worktree untouched. Made once, and only for a worker that is joining the landing train.
_Avoid_: retry, re-dispatch

**Continuation**:
Recovering a claim whose worker died from the progress that survived: its worktree if still on this host, else its pushed branch, else a fresh start from the base branch.
_Avoid_: resume, checkpoint restore

**Wake**:
A signal that ends the sleep between ticks of its host's fleet instance early: every command that changes fleet state sends one as it ends — a worker's last among them, once its outcome is on GitHub — naming itself and what it acted on, and carrying no state. The `run` that hears it answers at once, naming the tick it starts or the tick in progress it follows.
_Avoid_: notification, callback, done signal

#### Claims

**Claim**:
The atomic lock marking one issue as owned by one fleet instance. Its creation succeeds for exactly one instance. It takes one slot of the `concurrency` while its worker is at work, and none once a verdict is declared under it: it is still held until that verdict is acted on, and a worker started under it again takes a slot as any start does.
_Avoid_: assignee, assignment, lock (the fleet lock)

**Heartbeat**:
The timestamp a fleet instance publishes for itself while it holds claims, stamped with its host. A claim is live while its owner's heartbeat is within the lease. A `run` told to stop while it holds no claim removes its own; a start removes the dead ones that hold nothing: those of the instances it took from, its own host's past the lease, and anyone's older than a day.
_Avoid_: ping, keepalive

**Orphaned claim**:
A claim of this fleet instance with no PR and no live worker. It is recovered by continuation, never released.
_Avoid_: stuck issue, zombie

**Stale claim**:
Another fleet instance's claim whose heartbeat is past the lease. It is the only claim a fleet takes from another host unattended.
_Avoid_: dead claim, abandoned claim

**Takeover**:
Taking another fleet instance's claims — on issues, on patrols' runs, on proposals — without waiting for the lease. It happens unconditionally at start for claims stamped with this host, and by explicit command for another host's. A claim taken — by a takeover, or as a stale claim — is still the claim it was created as: what was declared under it stands.
_Avoid_: failover, stale reclaim

#### Outcomes

**Park**:
Recording a dependency a worker discovered as a blocker on its issue and releasing the claim, so the frontier holds the issue back until the blocker closes.
_Avoid_: defer, snooze, hold (a fleet instance holds its claims)

**Escalate**:
Handing an issue to a human: the reason is commented, with the line that answers it, the flow label is swapped for the escalate label, and the claim is released. Every hand-off — a failure, a tripped breaker, an `escalate` verdict, a changed workflow — goes first to the consult where the stage names one, and escalates when the consult lets it through or where none is named; nothing is retried. The issue then waits for a human, with every other open issue only one can move: `plan`, `doctor` and the panel list them, each with its line.
_Avoid_: fail (what a command, a gate or one thing a tick does may), retry, give up (a landing train is given up: Abandon)

**Status board**:
The single comment on a claimed issue that shows a human how far along it is, and what the afkconvoy that works it was built from. It is rendered from fleet state and never read back.
_Avoid_: progress log, worklog

**Panel**:
The web page that shows a human where the whole fleet stands: each workflow as the line of its stages, the issues at work, queued and held back at each, and the landing train. A fleet instance serves it on its own host, made anew from what each tick read; it is never read back.
_Avoid_: dashboard, status page, status board (the comment on one issue), UI

**Tick log**:
The file a fleet instance's `run` keeps on its own host: one line as it starts, a line as each tick starts — with why: its timer, a wake and the command that sent it, the `run`'s start, or a follow-up — and one as it ends — what it did, and what changed on GitHub or by another host since the tick before where it acted on that — plus the lines of the events that reached the `run`: a wake heard during a tick, a check for host sleep, the host waking, being told to stop. Every command that changes fleet state, whoever runs it, appends a line of its own to the file of its host and repo as it acts — the command, the issue, who ran it, and how it came out or that it failed — whether a `run` is there or not. `run` prints each of its own lines as it is written. Each line is appended whole, and only `run` cuts the file back: it keeps every line of the last day, and never fewer than 200 lines nor more than 5000. It is a log and not fleet state: nothing reads it to decide what the fleet does.
_Avoid_: state file, journal, history

**Finding**:
One thing `doctor` says is wrong with the fleet on this host, with how bad it is, what shows it, its fix — a line to run, or a file and the key in it to edit — and the help topic that explains it. `doctor` fixes none itself.
_Avoid_: diagnosis, alert, warning (one of its severities), error (what a command that failed answers with)

#### Landing

**Gate**:
The repo's build-and-test command. It is the only machine verification a change must pass before it lands.
_Avoid_: CI (what a push to an ordinary branch starts), checks (what `check` and the startup probe make), local CI

**Recorded gate run**:
The evidence that the gate passed on one committed tree, with how long the run took and on which host, made by the tool that saw the exit code and kept where every host can read it.
_Avoid_: gate cache, CI status

**Sync**:
Bringing a branch up to date by merging into it. Nothing is ever rebased.
_Avoid_: rebase, update branch

**Landing train**:
The one append-only line of merge commits ahead of the base branch that finished PRs join, one merge commit per PR.
_Avoid_: merge queue, merge batch, landing turn

**Join**:
Putting a finished PR on the landing train. In an unbound repo the worker that wrote it does; in a bound repo a new worker does, in the landing role its workflow names, after the last stage passed.
_Avoid_: merge (what git does to two branches, and a human to a proposal's PR), enqueue

**Train worker**:
The one worker per repo that gates the landing train's tip and pushes it to the base branch as a fast-forward. It holds no claim and wrote none of the PRs it lands.
_Avoid_: merger, integrator, batch worker

**Abandon**:
Giving up a landing train whose worker stayed silent, with nothing landed; its PRs join the next train.
_Avoid_: fail (as under Escalate), bisect
