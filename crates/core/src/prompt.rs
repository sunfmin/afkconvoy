//! The worker prompt: a template, filled by code. No worker is ever started on
//! a prompt with a placeholder left in it.

use crate::position::Declared;
use crate::recovery::Variant;
use crate::stage_record::{Author, StageRecord, Stated};
use crate::verdict;

/// What a worker prompt is filled with: the issue, and the branch and path
/// orca actually returned.
#[derive(Debug, Clone)]
pub struct WorkerFields<'a> {
    pub number: u64,
    pub title: &'a str,
    pub branch: &'a str,
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The line the worker runs the gate with: `afkconvoy gate --train`.
    pub gate_command: &'a str,
    /// The line the worker joins the landing train with: `afkconvoy land --issue <n>`.
    pub land_command: &'a str,
    /// The line the worker wakes the fleet with: `afkconvoy wake`.
    pub wake_command: &'a str,
}

const OPENING_FRESH: &str = "You are an afkconvoy worker. You own exactly ONE GitHub issue and work in an isolated git worktree.";

const OPENING_CONTINUE: &str = "\
You are an afkconvoy worker **continuing** an issue a previous worker started but did not finish.
You own exactly ONE GitHub issue and work in its worktree, on its branch, which already carries that
earlier progress.";

const STEP1_FRESH: &str = "\
1. **Read the ground truth first.** `gh issue view {n} --comments`, then this repo's `GLOSSARY.md`,
   the relevant `docs/adr/*`, and `docs/agents/*`. Use the glossary's exact vocabulary.";

const STEP1_CONTINUE: &str = "\
1. **Inspect the existing progress first.** See what the previous worker left: `git status`,
   `git diff origin/{base_branch}...HEAD`, `git log origin/{base_branch}..HEAD`, and any notes. It is
   partial work toward the same acceptance criteria — do not discard it, but verify it against them.
   **Then read the ground truth:** `gh issue view {n} --comments`, then this repo's `GLOSSARY.md`,
   the relevant `docs/adr/*`, and `docs/agents/*`. Use the glossary's exact vocabulary. Continue from
   the first step that is not yet done.";

/// The gate and the join, as every worker that has them is told them. `<g>` and
/// `<j>` are the numbers the two steps carry in the prompt they are put in.
const GATE_STEP: &str = "\
<g>. **Gate.** Commit everything, then run this in your worktree:
   ```bash
   {gate_command}
   ```
   It first merges in what your PR will land behind (the landing train, or `{base_branch}`), then
   runs the repo's gate, and ends with one JSON object. On `\"status\": \"conflict\"` nothing ran:
   resolve every file it lists, `git add` it, commit the merge, and run it again. Otherwise fix the
   code, commit, and run it again until it says `\"status\": \"green\"` and `\"recorded\": true` —
   its `detail` says what is missing. Any further commit needs another run. Never rebase.";

const JOIN_STEP: &str = "\
<j>. **Join the landing train.** Run this in your worktree:
   ```bash
   {land_command}
   ```
   It ends with one JSON object. `\"outcome\": \"joined\"` (or `\"merged\"`): you are done — wake the
   fleet (below) and stop. On `\"conflict\"` the train was merged into your branch and left in
   progress: resolve every file it lists, `git add` it, commit the merge,
   go back to step <g>, and run this again. On `\"locked\"` a fleet lock stands and nothing but its
   own PR joins: stop here, waking nobody, and run this again when you are told to carry on.";

/// The wake, as every worker with an outcome of its own is told it.
const WAKE: &str = "\
## Wake the fleet

Once your outcome is on GitHub<outcomes>, run this once, exactly as written, and then stop. If it
fails, ignore it:
```bash
{wake_command}
```";

const JOINER: &str = "\
You are an afkconvoy worker taking over an issue whose work is finished: PR #{pr} is open for it, and
is not on the landing train yet. Your ONLY job is to get that PR on the train. Write no feature and
open no other PR. Nobody reads this terminal: never wait for an answer.

**Your issue:** #{n} — {title}
**Your PR:** #{pr}, from the branch `{branch}` (already checked out; the base is `{base_branch}`).
**Your worktree:** `{worktree_path}` — work only here.
<landing>
## Steps

1. **See where the worker before you stopped.** Run `git status`. A merge left in progress is a
   conflict it was resolving: resolve every unmerged file so both sides' intent survives, `git add`
   it, and commit the merge. Commit whatever else belongs to the PR, then `git push origin HEAD`.
<gate_step>
<join_step>

<wake>

## Hard rules

- End with your PR on the landing train — never just stop.
- Never close the PR, never open another, never rebase.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

const WORKER: &str = "\
<opening>
Nobody reads this terminal: never wait for an answer.

**Your issue:** #{n} — {title}
**Your branch:** `{branch}` (already checked out; the base is `{base_branch}`).
**Your worktree:** `{worktree_path}` — work only here.

## Publish progress as you go

After each completed piece of work — never a half-edited file — and before any long-running
operation:
```bash
git add -A && git commit -m \"<what this step did>\" && git push origin HEAD
```

## Steps

<step1>
2. **Implement** the issue's acceptance criteria, matching the surrounding code's conventions.
<gate_step>
4. **Open a PR** against `{base_branch}` whose body contains the line `Closes #{n}`:
   ```bash
   git push origin HEAD && gh pr create --base {base_branch} --head {branch} --title \"<title>\" --body \"<what changed>

   Closes #{n}\"
   ```
   Do not merge it.
<join_step>

## If you will not open a PR

Post a comment on the issue instead (`gh issue comment {n} --body \"...\"`), wake the fleet (below)
and stop. Its **first line** must be exactly this marker, on one line, followed by a human-readable
body:
```
{verdict_marker}
```
- **`already-satisfied`** — the issue is already implemented in `{base_branch}`: you changed nothing.
- **`blocked`** — the issue needs something that belongs to another issue and does not exist yet. Do
  not create it yourself. Name every blocking issue in `blocked_by=` (e.g. `blocked_by=41,42`) and
  say what is missing.
- **`escalate`** — after a genuine effort you cannot complete the work or make the gate pass, or
  the issue as written cannot be done. Say where you are stuck, or the decision that has to be made,
  so that a human can act on it in one reply.

<wake>

## Hard rules

- End with your PR on the landing train, or with that comment — never just stop.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

/// What the join-only brief adds for a worker started in a workflow's landing
/// role: the role, and the skill of it the join is performed as.
const LANDING: &str = "\
**Your role:** `{role}`, which you were started in. Perform its skill `{skill}` as you carry out the
steps below. You were started in your role's directory, not in the worktree: the commands below
change to the worktree themselves.
";

/// The brief of a worker started in its role: a fixed form that says what the
/// stage is and nothing of how to perform it — that is the role's to say.
const STAGE: &str = "\
You are an afkconvoy worker, started in your role to perform one stage of one GitHub issue. Nobody
reads this terminal: never wait for an answer.

- **Issue:** #{n} — {title}
- **Stage:** `{stage}`, of the workflow `{flow}`
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Worktree:** `{worktree_path}`, on the branch `{branch}` (the base is `{base_branch}`)
- **History:** the issue and every comment on it (`gh issue view {n} --comments`), and what the
  branch holds past `origin/{base_branch}`
- **Precedents:** `{precedents}`

## Stage records

{records}
<rework><gate>
## Splitting the issue

A stage that finds its issue too large for one piece of work may split it, with these commands and
in this order:

1. Open one issue per piece (`gh issue create`), each saying what it is to deliver. Put no label of
   a workflow on it yourself.
2. Take each in to the workflow it is to follow — the team has {workflows} — naming with
   `--blocked-by` the ones among them it has to wait for; the command's `--help` says what it takes:
   ```bash
   {intake_command} --issue <n> --workflow <workflow> [--blocked-by <n,…>]
   ```
3. Declare `blocked` with the command below, with every issue you opened in `--blocked-by`.

The issue is then parked on them, and when they have all closed this stage runs again, to judge the
whole.

## Declare a verdict

The stage ends when you declare its verdict with this command, and not before; its `--help` says
what it takes. Then stop.
```bash
{verdict_command}
```
";

/// What a stage running again is told of the rejection that sent the issue
/// back to it. `<rejecter>` is the stage that rejected.
const REWORK: &str = "
## Rework

This stage runs again: the stage `{rejecter}` rejected the work it was handed. Its reason:

{reason}
";

/// What a stage running again after a park is told: what it waited on has
/// closed, and — the park being how an issue is split — the whole is its to
/// judge.
const PARKED: &str = "
## After a park

This stage runs again: it declared `blocked`, and {waited}. If those are the issues this stage
split its issue into, judge the whole now: declare `already-satisfied` when they left nothing more
to do, or go on with what is left. What the stage said as it parked:

{reason}
";

/// What a stage that may commit is told of the gate: a PR it wrote is handed
/// on green (docs/adr/0010).
const GATE_BEFORE_PASS: &str = "
## The gate, before a `pass`

Opening a PR declares nothing, and no stage is handed red code. If this stage opened the issue's PR
or pushed to its branch, turn the gate green before you declare `pass`:

<gate_step>
";

/// What a stage is told of the human's ruling it runs again under.
const RULED: &str = "
## Ruling

This stage runs again under a human's ruling on it. Follow it:

{reason}
";

/// What a stage is told of its consult's ruling it runs again under.
const CONSULTED: &str = "
## Ruling

This stage declared `escalate`, and its consult ruled on that. It runs again under the ruling.
Follow it:

{reason}
";

/// The brief of a consult: a fixed form that says what was asked and how to
/// answer, and nothing of how to decide — that is the role's to say.
const CONSULT: &str = "\
You are an afkconvoy worker, started in your role as a **consult**: a stage of one GitHub issue
declared `escalate`, and you rule on that before a human is asked. Nobody reads this terminal: never
wait for an answer.

- **Issue:** #{n} — {title}
- **Stage that asked:** `{stage}`, of the workflow `{flow}`
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Worktree:** `{worktree_path}`, on the branch `{branch}` (the base is `{base_branch}`). It is
  yours to read only: a consult writes no code, so never commit and never push
- **History:** the issue and every comment on it (`gh issue view {n} --comments`), and what the
  branch holds past `origin/{base_branch}`
- **Precedents:** `{precedents}`

## Stage records

{records}

## What was asked

The stage `{stage}` could not decide, and said why:

{reason}

## Rule, or let it through

End with exactly one of these two commands, and not before; each one's `--help` says what it takes.
Then stop.

- **Rule.** The stage `{stage}` runs again under your ruling, which is the text you give:
  ```bash
  {rule_command} --text-file <file>
  ```
- **Let it through.** The question is not yours to rule on either: the issue goes to a human, and
  your text says what has to be decided:
  ```bash
  {verdict_command} escalate --text-file <file>
  ```
";

/// What a stage is told of the human who sent the issue to it.
const SENT: &str = "
## Sent here

A human sent the issue to this stage. What they said of it:

{reason}
";

/// One stage record that counted, as a worker's brief names it.
#[derive(Debug, Clone, Copy)]
pub struct Said<'a> {
    pub record: &'a StageRecord,
    /// The comment the record is.
    pub url: &'a str,
}

/// What the brief of a worker started in its role is filled with.
#[derive(Debug, Clone)]
pub struct StageFields<'a> {
    pub number: u64,
    pub title: &'a str,
    /// The workflow's name.
    pub flow: &'a str,
    pub stage: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    pub branch: &'a str,
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The precedents file, in the fleet's copy of the team.
    pub precedents: &'a str,
    /// What was declared on the issue so far, oldest first: the worker's
    /// history. When the last is a `reject`, a ruling or a human's sending,
    /// the stage runs for it; a consult rules on the last, an `escalate`.
    pub records: &'a [Said<'a>],
    /// The line the worker declares its verdict with.
    pub verdict_command: &'a str,
    /// The line the worker takes an issue in with, before what names the
    /// issue and its workflow: a stage that splits its issue takes each
    /// child in.
    pub intake_command: &'a str,
    /// The team's workflows, by name: what a child may be taken in to.
    pub workflows: &'a [String],
    /// The line the worker runs the gate with, when its stage may commit: a
    /// stage that writes nothing has no PR of its own to turn green.
    pub gate_command: Option<&'a str>,
}

/// `text` as a quotation: nothing in it reads as part of the brief.
pub(crate) fn quoted(text: &str) -> String {
    let lines: Vec<String> = text.trim().lines().map(|line| format!("> {line}")).collect();
    lines.join("\n")
}

/// The stage records of a brief, as a list a worker reads.
fn records_said(records: &[Said]) -> String {
    let mut records_said = match records {
        [] => "None yet: no stage of this issue has declared a verdict.".to_string(),
        _ => "What the stages before this one declared, oldest first — each is a comment on \
              the issue, with what the stage produced:\n"
            .to_string(),
    };
    for said in records {
        let (stage, word) = (&said.record.stage, said.record.stated.word());
        let line = match said.record.by {
            Author::Worker { consult: false, .. } => format!("`{stage}` declared `{word}`"),
            Author::Worker { consult: true, .. } | Author::Human => said.record.headline(),
        };
        records_said.push_str(&format!("\n- {line} — {}", said.url));
    }
    records_said
}

/// The brief a worker started in its role is given.
pub fn stage_prompt(fields: &StageFields) -> Result<String, String> {
    let number = fields.number.to_string();
    let records = records_said(fields.records);
    // Only the last record says why the stage runs now.
    let last = fields.records.last().map(|said| said.record);
    let (why, reason) = match last {
        Some(last) if last.stated == Stated::Sent => (SENT, quoted(&last.text)),
        Some(last) if last.puts() => (RULED, quoted(&last.text)),
        Some(last) if last.stated == Stated::Declared(Declared::Ruling) => {
            (CONSULTED, quoted(&last.text))
        }
        Some(last) if last.stated == Stated::Declared(Declared::Reject) => {
            (REWORK, quoted(&last.text))
        }
        Some(last) if last.stated == Stated::Declared(Declared::Blocked) => {
            (PARKED, quoted(&last.text))
        }
        _ => ("", String::new()),
    };
    let waited = match last.map_or(&[][..], |last| &last.blocked_by) {
        [] => "it named no issue to wait on".to_string(),
        on => {
            let on: Vec<String> = on.iter().map(|blocker| format!("#{blocker}")).collect();
            format!("every issue it waited on has closed ({})", on.join(", "))
        }
    };
    let workflows: Vec<String> = fields.workflows.iter().map(|name| format!("`{name}`")).collect();
    let gate = match fields.gate_command {
        Some(_) => GATE_BEFORE_PASS.replace("<gate_step>", &GATE_STEP.replace("<g>", "1")),
        None => String::new(),
    };
    let template = STAGE.replace("<rework>", why).replace("<gate>", &gate);
    fill(
        &template,
        &[
            ("records", &records),
            ("rejecter", last.map_or("", |last| &last.stage)),
            ("reason", &reason),
            ("waited", &waited),
            ("workflows", &workflows.join(", ")),
            ("intake_command", fields.intake_command),
            ("n", &number),
            ("title", fields.title),
            ("flow", fields.flow),
            ("stage", fields.stage),
            ("role", fields.role),
            ("skill", fields.skill),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("precedents", fields.precedents),
            ("verdict_command", fields.verdict_command),
            ("gate_command", fields.gate_command.unwrap_or_default()),
        ],
    )
}

/// The brief a consult is given: `fields` are those of the stage that asked,
/// with the consult's role and skill, and `rule_command` is the line it rules
/// with. The last of the records is the `escalate` it rules on.
pub fn consult_prompt(fields: &StageFields, rule_command: &str) -> Result<String, String> {
    let number = fields.number.to_string();
    let asked = fields.records.last().map_or(String::new(), |said| quoted(&said.record.text));
    fill(
        CONSULT,
        &[
            ("records", &records_said(fields.records)),
            ("reason", &asked),
            ("n", &number),
            ("title", fields.title),
            ("flow", fields.flow),
            ("stage", fields.stage),
            ("role", fields.role),
            ("skill", fields.skill),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("precedents", fields.precedents),
            ("verdict_command", fields.verdict_command),
            ("rule_command", rule_command),
        ],
    )
}

const TRAIN: &str = "\
You are the afkconvoy **train worker**. You own no issue and you write no feature. Finished PRs join
the **landing train**: a line of commits ahead of `{base_branch}`, one merge commit per PR. Your job
is to run one command, which gates the train as it stands and lands it on `{base_branch}`, and to act
on what it says — again and again, until the train is empty. Nobody reads this terminal: never wait
for an answer.

**Your worktree:** `{worktree_path}` — work only here, on the branch it is on: never check out a
commit or another branch.

**This command is the only way the train lands.** Run it in your worktree, exactly as written:
```bash
{land_command}
```
It takes the train's tip (your own commits are kept on top), merges `{base_branch}` in if it moved,
runs the gate once on that commit, and pushes the commit to `{base_branch}` as a fast-forward. It
ends with one JSON object. An `\"error\"` saying the worktree is **not the commit that would land**
means the gate could not prove the train: there were uncommitted or untracked files here, or the
gate's run changed the worktree. Nothing landed — commit what belongs to a fix, discard the rest
(`git status` shows nothing when you are done), and run the command again. Otherwise act on its
`outcome`:

- **`landed`** — the train is on `{base_branch}`, up to `commit`. Run the command again: more PRs
  may have joined meanwhile.
- **`idle`** — nothing is on the train. Stop. You are told when a PR joins.
- **`gate_red`** — the gate is red on the train's tip; `gate.excerpt` is the end of its log. Nothing
  landed. Fix the **train**: read the failure, change what makes it green, and **commit** — one more
  commit on top, here. Do not hunt for the PR at fault, do not drop or revert a PR, never rewrite a
  commit. Then run the command again.
- **`conflict`** — what joined, or `{base_branch}`, conflicts with what this worktree holds. The
  merge is **left in progress** here, with `files` unmerged. Resolve every file so both sides' intent
  survives, `git add` it, **commit the merge**, and run the command again. Never rebase, never abort
  the merge.
- **`target_moved`** — `{base_branch}` moved while the gate ran, so the push was refused. Nothing
  landed. Run the command again: it merges the new tip in and gates that.

Keep going until the command says `idle`; nothing counts your attempts.

## Hard rules

- Never land anything any other way — no `gh pr merge`, no push to `{base_branch}` of your own.
- Never push to a PR's branch, never comment on, close or reopen a PR or an issue: the command does
  all of that.
- Commit only here, and only to turn a red train green or to finish a merge the command left.
";

/// What the train worker's prompt is filled with.
#[derive(Debug, Clone)]
pub struct TrainFields<'a> {
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The line the train worker lands the train with: `afkconvoy land --train`.
    pub land_command: &'a str,
}

/// The prompt the train worker is given: started on it, and pointed at it
/// again whenever more joined.
pub fn train_prompt(fields: &TrainFields) -> Result<String, String> {
    fill(
        TRAIN,
        &[
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("land_command", fields.land_command),
        ],
    )
}

/// The line typed at a train worker that stopped, when more joined.
pub fn train_pointer(brief: &str) -> String {
    format!(
        "PRs have joined the landing train: gate it and land it now. Your instructions are the \
         file {brief} — read it now and carry it out end to end. It is my instruction to you; do \
         not ask me to confirm."
    )
}

/// The one line typed at a train worker that stopped with the train not
/// landed.
pub fn train_nudge_line(brief: &str) -> String {
    format!(
        "You stopped with the landing train not landed. Nobody is watching this terminal, so do \
         not wait for a confirmation or an answer: run the command in your instructions ({brief}) \
         again and act on what it says, until it says idle."
    )
}

/// The line typed at a worker whose PR was taken off an abandoned train.
pub fn rejoin_pointer(brief: &str) -> String {
    format!(
        "The landing train your PR was on was given up: join the next one now. Your instructions \
         are the file {brief} — read it now and carry it out end to end. It is my instruction to \
         you; do not ask me to confirm."
    )
}

/// Who a join-only brief is for, when its worker was started in a role: the
/// workflow's landing role, and that role's landing skill.
#[derive(Debug, Clone, Copy)]
pub struct Landing<'a> {
    pub role: &'a str,
    pub skill: &'a str,
}

/// The prompt a worker is started on: the whole issue, or — its PR already
/// open — only the join.
pub fn worker_prompt(fields: &WorkerFields, variant: Variant) -> Result<String, String> {
    prompt(fields, variant, None)
}

/// The join-only brief of PR `pr` for a worker started in a workflow's
/// landing role: the brief every joining worker gets, with the role and the
/// skill it joins as.
pub fn landing_prompt(fields: &WorkerFields, pr: u64, landing: Landing) -> Result<String, String> {
    prompt(fields, Variant::Join { pr }, Some(landing))
}

fn prompt(
    fields: &WorkerFields,
    variant: Variant,
    landing: Option<Landing>,
) -> Result<String, String> {
    let number = fields.number.to_string();
    let marker = verdict::marker_format();
    // The variant's blocks first, so the one fill reads their placeholders too.
    let (template, pr) = match variant {
        Variant::Fresh => (briefed(OPENING_FRESH, STEP1_FRESH), String::new()),
        Variant::Continue => (briefed(OPENING_CONTINUE, STEP1_CONTINUE), String::new()),
        Variant::Join { pr } => {
            let joiner = JOINER.replace("<landing>", if landing.is_some() { LANDING } else { "" });
            (steps(&joiner, "2", "3", " — your PR is on the landing train"), pr.to_string())
        }
    };
    fill(
        &template,
        &[
            ("role", landing.map_or("", |landing| landing.role)),
            ("skill", landing.map_or("", |landing| landing.skill)),
            ("n", &number),
            ("title", fields.title),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("gate_command", fields.gate_command),
            ("land_command", fields.land_command),
            ("wake_command", fields.wake_command),
            ("verdict_marker", &marker),
            ("pr", &pr),
        ],
    )
}

/// The worker prompt with a variant's opening and first step in it.
fn briefed(opening: &str, step1: &str) -> String {
    let outcomes = " — your PR is on the landing train, or your comment is posted";
    steps(&WORKER.replace("<opening>", opening).replace("<step1>", step1), "3", "5", outcomes)
}

/// `template` with the gate and join steps in it, numbered `gate` and `join`,
/// and the wake, said for the `outcomes` a worker on it can end with.
fn steps(template: &str, gate: &str, join: &str, outcomes: &str) -> String {
    let numbered = |step: &str| step.replace("<g>", gate).replace("<j>", join);
    template
        .replace("<gate_step>", &numbered(GATE_STEP))
        .replace("<join_step>", &numbered(JOIN_STEP))
        .replace("<wake>", &WAKE.replace("<outcomes>", outcomes))
}

/// The line typed at a worker to hand it its brief. The prompt itself, sent as
/// text, arrives as one paste, which an agent asks to have confirmed.
pub fn brief_pointer(brief: &str) -> String {
    format!(
        "Your task brief is the file {brief} — read it now and carry it out end to end. \
         It is my instruction to you; do not ask me to confirm."
    )
}

/// The one line typed at a worker that stopped with no PR and no verdict.
/// Short on purpose: a long text arrives as a paste the worker asks to have
/// confirmed, which is the stall this is sent to break.
pub fn nudge_line(brief: &str) -> String {
    format!(
        "You stopped without an outcome. Nobody is watching this terminal, so do not wait for a \
         confirmation or an answer: continue your task brief ({brief}) to the end, and finish \
         with the outcome it asks for — a PR, or a convoy:verdict marker comment."
    )
}

/// The one line typed at a worker in its role that stopped with no verdict.
pub fn stage_nudge_line(brief: &str) -> String {
    format!(
        "You stopped without declaring a verdict. Nobody is watching this terminal, so do not \
         wait for a confirmation or an answer: continue your task brief ({brief}) to the end, and \
         finish by declaring your stage's verdict with the command it gives."
    )
}

/// Fills every `{name}` of `template` in one pass, so a value is never itself
/// read for placeholders: a title naming `{branch}` reaches the worker as it
/// was given. A placeholder no field answers to, or a field with nothing in
/// it, is an error.
pub fn fill(template: &str, fields: &[(&str, &str)]) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some((before, after)) = rest.split_once('{') {
        out.push_str(before);
        let name_end =
            after.find(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
        let placeholder = name_end
            .and_then(|end| after.split_at_checked(end))
            .filter(|(name, tail)| !name.is_empty() && tail.starts_with('}'));
        match placeholder {
            Some((name, tail)) => {
                let value =
                    fields.iter().find(|(field, _)| *field == name).map(|(_, value)| *value);
                match value {
                    Some(value) if !value.trim().is_empty() => out.push_str(value),
                    _ => return Err(format!("the prompt's placeholder {{{name}}} is not filled")),
                }
                rest = tail.strip_prefix('}').unwrap_or(tail);
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> WorkerFields<'static> {
        WorkerFields {
            number: 7,
            title: "Add {branch} support",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            gate_command: "/opt/bin/afkconvoy gate --train",
            land_command: "/opt/bin/afkconvoy land --issue 7",
            wake_command: "/opt/bin/afkconvoy wake",
        }
    }

    #[test]
    fn the_prompt_names_what_orca_returned() {
        let prompt = worker_prompt(&fields(), Variant::Fresh).unwrap();
        assert!(prompt.starts_with("You are an afkconvoy worker. You own exactly ONE"));
        assert!(
            prompt.contains("\n1. **Read the ground truth first.** `gh issue view 7 --comments`")
        );
        assert!(
            prompt.contains("**Your issue:** #7 — Add {branch} support"),
            "a value is not re-read"
        );
        assert!(prompt.contains("**Your branch:** `tester/issue-7-add`"));
        assert!(prompt.contains("**Your worktree:** `/wt/issue-7`"));
        assert!(prompt.contains("   /opt/bin/afkconvoy gate --train\n"));
        assert!(prompt.contains("   /opt/bin/afkconvoy land --issue 7\n"));
        assert!(prompt.contains("gh pr create --base develop --head tester/issue-7-add"));
        assert!(prompt.contains("Closes #7"));
        assert!(prompt.contains(&verdict::marker_format()));
    }

    #[test]
    fn a_continuing_worker_is_told_to_inspect_what_is_there_first() {
        let prompt = worker_prompt(&fields(), Variant::Continue).unwrap();
        assert!(prompt.starts_with("You are an afkconvoy worker **continuing** an issue"));
        assert!(prompt.contains("\n1. **Inspect the existing progress first.**"));
        assert!(prompt.contains("`git log origin/develop..HEAD`"));
        assert!(prompt.contains("partial work toward the same acceptance criteria"));
        assert!(prompt.contains("gh issue view 7 --comments"));
        assert!(!prompt.contains("<opening>") && !prompt.contains("<step1>"));
        // everything after the first step is the fresh prompt's
        let fresh = worker_prompt(&fields(), Variant::Fresh).unwrap();
        let rest = |prompt: &str| prompt[prompt.find("2. **Implement**").unwrap()..].to_string();
        assert_eq!(rest(&prompt), rest(&fresh));
    }

    #[test]
    fn a_joining_worker_is_briefed_only_to_join_the_train() {
        let prompt = worker_prompt(&fields(), Variant::Join { pr: 70 }).unwrap();
        assert!(prompt.contains("PR #70 is open for it"));
        assert!(prompt.contains("**Your PR:** #70, from the branch `tester/issue-7-add`"));
        assert!(prompt.contains("\n2. **Gate.**") && prompt.contains("\n3. **Join the landing"));
        assert!(prompt.contains("   /opt/bin/afkconvoy gate --train\n"));
        assert!(prompt.contains("   /opt/bin/afkconvoy land --issue 7\n"));
        assert!(prompt.contains("go back to step 2, and run this again"));
        for absent in ["**Implement**", "gh pr create", "convoy:verdict", "<g>", "<j>", "_step>"] {
            assert!(!prompt.contains(absent), "{absent}");
        }
        // the same two steps the whole brief gives, under its own numbers
        let whole = worker_prompt(&fields(), Variant::Fresh).unwrap();
        assert!(whole.contains("\n3. **Gate.**") && whole.contains("\n5. **Join the landing"));
        assert!(whole.contains("go back to step 3, and run this again"));
        assert!(!whole.contains("<g>") && !whole.contains("_step>"));
    }

    #[test]
    fn every_worker_with_an_outcome_is_told_to_wake_the_fleet() {
        let line = "run this once, exactly as written, and then stop. If it\nfails, ignore it:\n\
                    ```bash\n/opt/bin/afkconvoy wake\n```";
        for variant in [Variant::Fresh, Variant::Continue] {
            let prompt = worker_prompt(&fields(), variant).unwrap();
            assert!(prompt.contains(line), "{prompt}");
            assert!(prompt.contains("your PR is on the landing train, or your comment is posted"));
            assert!(prompt.contains("you are done — wake the\n   fleet (below) and stop"));
            assert!(prompt.contains("wake the fleet (below)\nand stop. Its **first line**"));
            assert_eq!(prompt.matches("afkconvoy wake").count(), 1, "said in one place");
            assert!(!prompt.contains("<wake>") && !prompt.contains("<outcomes>"));
        }
        let joiner = worker_prompt(&fields(), Variant::Join { pr: 70 }).unwrap();
        assert!(joiner.contains(line), "{joiner}");
        assert!(
            joiner.contains("Once your outcome is on GitHub — your PR is on the landing train,")
        );
        assert!(!joiner.contains("comment is posted") && !joiner.contains("<wake>"));
    }

    #[test]
    fn the_brief_of_a_worker_in_its_role_is_a_fixed_form() {
        let fields = StageFields {
            number: 7,
            title: "Add {branch} support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            precedents: "/copies/acme/team/def/precedents.md",
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            intake_command: "/opt/bin/afkconvoy intake",
            workflows: &["feature".to_string(), "small fix".to_string()],
            gate_command: None,
        };
        let prompt = stage_prompt(&fields).unwrap();
        for said in [
            "- **Issue:** #7 — Add {branch} support\n",
            "- **Stage:** `build`, of the workflow `feature`\n",
            "- **Skill to perform:** `implement`, of the role you were started in (`dev`)\n",
            "- **Worktree:** `/wt/issue-7`, on the branch `tester/issue-7-add` (the base is `develop`)",
            "- **History:** the issue and every comment on it (`gh issue view 7 --comments`)",
            "- **Precedents:** `/copies/acme/team/def/precedents.md`\n",
            "```bash\n/opt/bin/afkconvoy verdict --issue 7 --stage build\n```",
        ] {
            assert!(prompt.contains(said), "{said}\n{prompt}");
        }
        // How the work is done is the role's to say: none of the whole brief's steps.
        for absent in ["Implement", "gh pr create", "Gate", "landing train", "git add", "wake"] {
            assert!(!prompt.contains(absent), "{absent}");
        }
        assert!(prompt.contains("## Stage records\n\nNone yet: no stage of this issue has"));
        assert!(!prompt.contains("## Rework") && !prompt.contains("<rework>"), "{prompt}");

        // The stage records are its history, and a rejection's reason is in the brief.
        let left = |stage: &str, stated, by, text: &str| StageRecord {
            stage: stage.to_string(),
            stated,
            by,
            blocked_by: vec![],
            text: text.to_string(),
        };
        let worker = |stage: &str, declared, text: &str| {
            let by = Author::Worker {
                consult: false,
                claim: "c".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
            };
            left(stage, Stated::Declared(declared), by, text)
        };
        let reason = "## Not yet\n\nThe {parser} drops a `-->`.";
        let (built, turned_down) =
            (worker("build", Declared::Pass, "built"), worker("review", Declared::Reject, reason));
        let passed = [Said { record: &built, url: "https://gh/c/1" }];
        let rejected =
            [&passed[..], &[Said { record: &turned_down, url: "https://gh/c/2" }]].concat();
        let again = stage_prompt(&StageFields { records: &rejected, ..fields.clone() }).unwrap();
        assert!(
            again.contains(
                "\n\n- `build` declared `pass` — https://gh/c/1\n\
                 - `review` declared `reject` — https://gh/c/2\n"
            ),
            "{again}"
        );
        assert!(
            again.contains(
                "## Rework\n\nThis stage runs again: the stage `review` rejected the work it was \
                 handed. Its reason:\n\n> ## Not yet\n> \n> The {parser} drops a `-->`.\n\n\
                 ## Splitting the issue\n"
            ),
            "{again}"
        );
        // A stage that follows a pass runs for the first time: no rework.
        let on = stage_prompt(&StageFields { records: &passed, ..fields }).unwrap();
        assert!(on.contains("- `build` declared `pass`") && !on.contains("## Rework"), "{on}");

        // A human's ruling is what the stage runs again under, and a sending why it runs.
        let ruling = left("build", Stated::Declared(Declared::Ruling), Author::Human, "Keep v1.");
        let ruled = [&rejected[..], &[Said { record: &ruling, url: "https://gh/c/3" }]].concat();
        let under = stage_prompt(&StageFields { records: &ruled, ..fields.clone() }).unwrap();
        assert!(
            under.contains(
                "- a human ruled on the stage `build`, which runs again under the ruling — \
                 https://gh/c/3\n\n## Ruling\n\nThis stage runs again under a human's ruling on \
                 it. Follow it:\n\n> Keep v1.\n\n## Splitting the issue\n"
            ),
            "{under}"
        );
        assert!(!under.contains("## Rework"), "{under}");
        let sending = left("build", Stated::Sent, Author::Human, "Start over.");
        let sent = [Said { record: &sending, url: "https://gh/c/4" }];
        let here = stage_prompt(&StageFields { records: &sent, ..fields.clone() }).unwrap();
        assert!(
            here.contains(
                "- a human sent this issue to the stage `build` — https://gh/c/4\n\n\
                 ## Sent here\n\nA human sent the issue to this stage. What they said of it:\n\n\
                 > Start over.\n"
            ),
            "{here}"
        );
        assert!(stage_nudge_line("/b.md").contains("task brief (/b.md)"));

        // A stage its consult ruled on runs again under that ruling; the
        // consult's brief is the `escalate` it rules on and the two lines it
        // ends with.
        let consult = |declared, text: &str| {
            let by = Author::Worker {
                consult: true,
                claim: "c".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
            };
            left("build", Stated::Declared(declared), by, text)
        };
        let (asked, answer) = (
            worker("build", Declared::Escalate, "v1 or v2?"),
            consult(Declared::Ruling, "Keep {v1}."),
        );
        let escalated = [Said { record: &asked, url: "https://gh/c/5" }];
        let rule = "/opt/bin/afkconvoy rule --issue 7 --stage build";
        let consulted = StageFields { role: "boss", skill: "rule", ..fields.clone() };
        let brief = consult_prompt(&StageFields { records: &escalated, ..consulted.clone() }, rule)
            .unwrap();
        for said in [
            "started in your role as a **consult**",
            "- **Stage that asked:** `build`, of the workflow `feature`\n",
            "- **Skill to perform:** `rule`, of the role you were started in (`boss`)\n",
            "a consult writes no code, so never commit and never push\n",
            "- `build` declared `escalate` — https://gh/c/5\n",
            "## What was asked\n\nThe stage `build` could not decide, and said why:\n\n> v1 or v2?\n",
            "```bash\n  /opt/bin/afkconvoy rule --issue 7 --stage build --text-file <file>\n",
            "/opt/bin/afkconvoy verdict --issue 7 --stage build escalate --text-file <file>\n",
        ] {
            assert!(brief.contains(said), "{said}\n{brief}");
        }
        assert!(consult_prompt(&consulted, rule).is_err(), "nothing was asked");
        let answered =
            [&escalated[..], &[Said { record: &answer, url: "https://gh/c/6" }]].concat();
        let under = stage_prompt(&StageFields { records: &answered, ..fields.clone() }).unwrap();
        assert!(
            under.contains(
                "- the consult of the stage `build` ruled on its `escalate`: the stage runs \
                 again under the ruling — https://gh/c/6\n"
            ),
            "{under}"
        );
        assert!(
            under.contains(
                "## Ruling\n\nThis stage declared `escalate`, and its consult ruled on that. It \
                 runs again under the ruling.\nFollow it:\n\n> Keep {v1}.\n"
            ),
            "{under}"
        );
    }

    #[test]
    fn a_stage_is_told_how_to_split_its_issue_and_that_it_judges_the_whole_after_the_park() {
        let fields = StageFields {
            number: 7,
            title: "Add support",
            flow: "feature",
            stage: "plan",
            role: "dev",
            skill: "plan",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            precedents: "/copies/acme/team/def/precedents.md",
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage plan",
            intake_command: "cd '/wt/issue-7' && /opt/bin/afkconvoy intake",
            workflows: &["feature".to_string(), "fix".to_string()],
            gate_command: None,
        };
        let prompt = stage_prompt(&fields).unwrap();
        for said in [
            "## Splitting the issue\n",
            "1. Open one issue per piece (`gh issue create`)",
            "the team has `feature`, `fix` —",
            "   cd '/wt/issue-7' && /opt/bin/afkconvoy intake --issue <n> --workflow <workflow> \
             [--blocked-by <n,…>]\n",
            "3. Declare `blocked` with the command below, with every issue you opened in \
             `--blocked-by`.",
            "when they have all closed this stage runs again, to judge the\nwhole.",
        ] {
            assert!(prompt.contains(said), "{said}\n{prompt}");
        }
        let split = prompt.find("## Splitting the issue").unwrap();
        assert!(split < prompt.find("## Declare a verdict").unwrap(), "before the verdict");
        assert!(!prompt.contains("## After a park"), "{prompt}");

        // The stage that parked runs again once what it waited on closed.
        let parked = StageRecord {
            stage: "plan".to_string(),
            stated: Stated::Declared(Declared::Blocked),
            by: Author::Worker {
                claim: "c".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
            },
            blocked_by: vec![11, 12],
            text: "Split in two:\n#11 the parser, #12 the printer.".to_string(),
        };
        let records = [Said { record: &parked, url: "https://gh/c/1" }];
        let again = stage_prompt(&StageFields { records: &records, ..fields.clone() }).unwrap();
        assert!(
            again.contains(
                "- `plan` declared `blocked` — https://gh/c/1\n\n## After a park\n\nThis stage \
                 runs again: it declared `blocked`, and every issue it waited on has closed (#11, \
                 #12)."
            ),
            "{again}"
        );
        assert!(again.contains("declare `already-satisfied` when they left nothing more\nto do"));
        assert!(again.contains("> Split in two:\n> #11 the parser, #12 the printer.\n"), "{again}");
        assert!(!again.contains("## Rework") && !again.contains("<rework>"), "{again}");
        // A human's `blocked` names no issue, and parks nothing.
        let unnamed = StageRecord { blocked_by: vec![], by: Author::Human, ..parked.clone() };
        let records = [Said { record: &unnamed, url: "https://gh/c/2" }];
        let human = stage_prompt(&StageFields { records: &records, ..fields }).unwrap();
        assert!(human.contains("it declared `blocked`, and it named no issue to wait on."));
    }

    #[test]
    fn a_stage_that_may_commit_is_told_to_turn_the_gate_green_before_it_passes() {
        let fields = StageFields {
            number: 7,
            title: "Add support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            precedents: "/copies/acme/team/def/precedents.md",
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            intake_command: "/opt/bin/afkconvoy intake",
            workflows: &["feature".to_string()],
            gate_command: Some("cd '/wt/issue-7' && /opt/bin/afkconvoy gate --train"),
        };
        let prompt = stage_prompt(&fields).unwrap();
        assert!(
            prompt.contains(
                "## The gate, before a `pass`\n\nOpening a PR declares nothing, and no stage is \
                 handed red code."
            ),
            "{prompt}"
        );
        assert!(prompt.contains("turn the gate green before you declare `pass`:\n\n1. **Gate.**"));
        assert!(prompt.contains("   cd '/wt/issue-7' && /opt/bin/afkconvoy gate --train\n"));
        // The gate step every worker with a PR is given, said in one place.
        let whole = worker_prompt(&super::tests::fields(), Variant::Fresh).unwrap();
        let step = |prompt: &str| {
            let from = prompt.find("It first merges in what your PR will land behind").unwrap();
            prompt[from..prompt[from..].find("Never rebase.").unwrap() + from].to_string()
        };
        assert_eq!(step(&prompt), step(&whole));
        let gate = prompt.find("## The gate").unwrap();
        assert!(gate < prompt.find("## Declare a verdict").unwrap(), "before the verdict");
        assert!(!prompt.contains("<gate") && !prompt.contains("<g>"), "{prompt}");
        // A stage that writes nothing has no PR of its own to turn green.
        let reads = stage_prompt(&StageFields { gate_command: None, ..fields }).unwrap();
        assert!(!reads.contains("gate") && !reads.contains("<gate>"), "{reads}");
    }

    #[test]
    fn a_landing_worker_gets_the_join_only_brief_with_its_role_and_skill() {
        let landing = Landing { role: "dev", skill: "land" };
        let prompt = landing_prompt(&fields(), 70, landing).unwrap();
        assert!(
            prompt.contains(
                "**Your worktree:** `/wt/issue-7` — work only here.\n**Your role:** `dev`, which \
                 you were started in. Perform its skill `land` as you carry out the\nsteps below."
            ),
            "{prompt}"
        );
        // But for those lines it is the brief every joining worker gets.
        let joiner = worker_prompt(&fields(), Variant::Join { pr: 70 }).unwrap();
        let role = prompt.find("**Your role:**").unwrap();
        let steps = prompt.find("\n## Steps").unwrap();
        assert_eq!(format!("{}{}", &prompt[..role], &prompt[steps..]), joiner);
        assert!(!joiner.contains("<landing>") && !joiner.contains("Your role"), "{joiner}");
    }

    #[test]
    fn the_train_prompt_hands_over_the_one_command() {
        let prompt = train_prompt(&TrainFields {
            worktree: "/wt/convoy-train",
            base_branch: "develop",
            land_command: "/opt/bin/afkconvoy land --train",
        })
        .unwrap();
        assert!(prompt.contains("\n/opt/bin/afkconvoy land --train\n"));
        assert!(prompt.contains("**Your worktree:** `/wt/convoy-train`"));
        assert!(prompt.contains("lands it on `develop`"));
        for outcome in ["landed", "idle", "gate_red", "conflict", "target_moved"] {
            assert!(prompt.contains(&format!("**`{outcome}`**")), "{outcome}");
        }
        assert!(train_pointer("/b.md").contains("/b.md"));
    }

    #[test]
    fn a_prompt_with_an_unfilled_placeholder_is_refused() {
        let blank = WorkerFields { branch: "", ..fields() };
        assert!(worker_prompt(&blank, Variant::Fresh).unwrap_err().contains("{branch}"));
        assert!(fill("a {nope} b", &[("n", "1")]).unwrap_err().contains("{nope}"));
    }

    #[test]
    fn braces_that_are_no_placeholder_are_text() {
        assert_eq!(fill("{ x } ${Y} {n} {", &[("n", "1")]).unwrap(), "{ x } ${Y} 1 {");
    }
}
