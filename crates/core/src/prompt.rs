//! The worker prompt: a template, filled by code. No worker is ever started on
//! a prompt with a placeholder left in it.

use crate::ending::Ending;
use crate::position::Declared;
use crate::recovery::Variant;
use crate::stage_record::{Author, StageRecord, Stated};
use crate::tracker::Tracker;
use crate::verdict;
use crate::waiting::answering;

/// What a worker prompt is filled with: the issue, and the branch and path
/// orca actually returned.
#[derive(Debug, Clone)]
pub struct WorkerFields<'a> {
    pub number: u64,
    /// The fleet's tracker, which holds the issue: every issue command of
    /// the brief names it, and the PR's closing line names the issue as the
    /// repo names one of its (`Tracker::issue`).
    pub tracker: &'a Tracker,
    pub title: &'a str,
    pub branch: &'a str,
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The line the worker runs the gate with: `afkconvoy gate`.
    pub gate_command: &'a str,
    /// The line the worker gates another checkout with, in that checkout:
    /// `afkconvoy gate`.
    pub gate_alone_command: &'a str,
    /// The line that raises a fleet lock on the issue of a proven red base branch.
    pub lock_command: &'a str,
    /// The label that makes an issue dispatchable: what an issue a worker
    /// opens for a defect it found carries.
    pub ready_label: &'a str,
    /// The line the worker lands in a turn with: `afkconvoy land --issue <n>`.
    pub land_command: &'a str,
    /// The line the worker wakes the fleet with: `afkconvoy wake`.
    pub wake_command: &'a str,
    /// The line the worker runs the fast gate with, `afkconvoy gate --fast`,
    /// where the repo splits its gate in two (docs/adr/0047); `None` where
    /// it does not.
    pub fast_gate_command: Option<&'a str>,
}

const OPENING_FRESH: &str = "You are an afkconvoy worker. You own exactly ONE GitHub issue and work in an isolated git worktree.";

const ISSUE_UPDATES: &str = "\n\n## Later comments and body edits\n\nThe fleet tells you later issue comments and body edits while you work, with their links.\nDecide each one: within this issue, carry it out; beyond this issue, open an issue and say so\non this issue. Name the receiving repo's main checkout as `<target-checkout>`.\nUse the issue-opening line `afkconvoy flows <target-checkout>` answers, and\n`afkconvoy intake -C <target-checkout>` where the receiving repo is bound.\n";

const OPENING_CONTINUE: &str = "\
You are an afkconvoy worker **continuing** an issue a previous worker started but did not finish.
You own exactly ONE GitHub issue and work in its worktree, on its branch, which already carries that
earlier progress.";

const STEP1_FRESH: &str = "\
1. **Read the ground truth first.** `gh issue view {n}<tracker> --comments`, then this repo's `GLOSSARY.md`,
   the relevant `docs/adr/*`, and `docs/agents/*`. Use the glossary's exact vocabulary.";

const STEP1_CONTINUE: &str = "\
1. **Inspect the existing progress first.** See what the previous worker left: `git status`,
   `git diff origin/{base_branch}...HEAD`, `git log origin/{base_branch}..HEAD`, and any notes. It is
   partial work toward the same acceptance criteria — do not discard it, but verify it against them.
   **Then read the ground truth:** `gh issue view {n}<tracker> --comments`, then this repo's `GLOSSARY.md`,
   the relevant `docs/adr/*`, and `docs/agents/*`. Use the glossary's exact vocabulary. Continue from
   the first step that is not yet done.";

/// The gate and the join, as every worker that has them is told them. `<g>` and
/// `<j>` are the numbers the two steps carry in the prompt they are put in.
const GATE_STEP: &str = "\
<g>. **Prepare to land.** The PR must already be open. Commit everything and push your branch.
   The next command holds one turn through merge, gate, recording green and landing.
   Never rebase.";

const STAGE_GATE_STEP: &str = "\
1. **Gate.** Commit everything, then run this in your worktree:
   ```bash
   {gate_command}
   ```
   This checks the committed change before review. Red: fix, commit and run it again until
   `\"status\": \"green\"` and `\"recorded\": true`. Declare `pass` only then. The landing
   worker takes the merge, gate and land turn after the workflow's last stage has passed.";

const JOIN_STEP: &str = "\
<j>. **Land.** Run this in your worktree:
   ```bash
   {land_command}
   ```
   It asks for a turn, makes your PR's merge commit on the current `{base_branch}` tip,
   gates that commit, records green, and pushes that same commit as a fast-forward.
   Its answer's `\"outcome\"` is one of these words:
<endings>
   An unrecorded base tip is gated alone first. Ordinary turns wait while it is red;
   the fleet dispatches a repair whose own turn must still gate green before landing.";

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
has not landed yet. Your ONLY job is to land that PR in a turn. Write no feature and
open no other PR. Nobody reads this terminal: never wait for an answer.

**Your issue:** {issue} — {title}
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

- End with your PR landed on the base branch — never just stop.
- Never close the PR, never open another, never rebase.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

/// The brief of a worker that takes up a PR a released claim kept: the issue
/// went to a human, or was parked, and is the fleet's again.
const TAKER: &str = "\
You are an afkconvoy worker taking up an issue that is the fleet's again: PR #{pr} is open for it,
kept when an earlier worker's claim was released — the issue was escalated or parked — and is not
on the base branch. The PR already exists: finish it and land it in a turn. Open
no other PR. Nobody reads this terminal: never wait for an answer.

**Your issue:** {issue} — {title}
**Your PR:** #{pr}, from the branch `{branch}` (already checked out; the base is `{base_branch}`).
**Your worktree:** `{worktree_path}` — work only here.

## Steps

1. **Read why it stopped.** `gh issue view {n}<tracker> --comments`: its latest comments say why the issue
   left the fleet and, where a human answered, what is to be done on this PR — do that, on this
   branch. Then run `git status`: a merge left in progress is a conflict the worker before you was
   resolving — resolve it and commit the merge.
2. **Merge the base branch.** `git fetch origin && git merge origin/{base_branch}`. Resolve every
   conflicting file so both sides' intent survives, `git add` it, and commit the merge. Commit
   whatever else belongs to the PR, then `git push origin HEAD`.
<gate_step>
<join_step>

<defect><cross_repo>
## If you cannot finish it

After a genuine effort, post a comment on the issue instead (`gh issue comment {n}<tracker> --body \"...\"`),
wake the fleet (below) and stop. Its **first line** must be exactly this marker, with `phase=escalate`,
on one line, followed by where you are stuck, so that a human can act on it in one reply — or,
with `phase=blocked` and every existing blocking issue in `blocked_by=`:
```
{verdict_marker}
```
<escalation>

<wake>

## Hard rules

- End with your PR landed on the base branch, or with that comment — never just stop.
- Never close the PR, never open another, never rebase.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

const WORKER: &str = "\
<opening>
Nobody reads this terminal: never wait for an answer.

**Your issue:** {issue} — {title}
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
2. **Implement** the issue's acceptance criteria, matching the surrounding code's conventions.<fast_gate>
3. **Open a PR** against `{base_branch}` whose body contains the line `Closes {issue}`:
   ```bash
   git push origin HEAD && gh pr create --base {base_branch} --head {branch} --title \"<title>\" --body \"<what changed>

   Closes {issue}\"
   ```
   Do not merge it.
<gate_step>
<join_step>

<defect><cross_repo>
## If you will not open a PR

Post a comment on the issue instead (`gh issue comment {n}<tracker> --body \"...\"`), wake the fleet (below)
and stop. Its **first line** must be exactly this marker, on one line, followed by a human-readable
body:
```
{verdict_marker}
```
- **`already-satisfied`** — the issue is already implemented in `{base_branch}`: you changed nothing.
- **`blocked`** — the issue needs something that belongs to another issue and does not exist yet.
  Reuse its issue, or open one for another repo of this host following the cross-repo blocker
  procedure above. Name every blocking issue in `blocked_by=` (e.g. `blocked_by=41,42`) and say
  what is missing.
- **`escalate`** — a human must act, as described below.

<escalation>

<wake>

## Hard rules

- End with your PR landed on the base branch, or with that comment — never just stop.
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

- **Issue:** {issue} — {title}
- **Stage:** `{stage}`, of the workflow `{flow}`
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Worktree:** `{worktree_path}`, on the branch `{branch}` (the base is `{base_branch}`)
- **History:** the issue and every comment on it (`gh issue view {n}<tracker> --comments`), and what
  the branch holds past `origin/{base_branch}`
<rules>

## Stage records

{records}
<rework><gate><split><cross_repo>
## Before an `escalate`

<escalation>
<decisions>
## Declare a verdict

The stage ends when you declare its verdict with this command, and not before; its `--help` says
what it takes. Then stop.
```bash
{verdict_command}
```
";

/// Where another checkout's missing work enters the fleet, shared by the
/// blocked, defect and split paths.
const CROSS_REPO_BLOCKER: &str = "
## Cross-repo blockers

For a blocker or defect belonging to another repo of this host, open it on the tracker
`<blocker_tracker>`, with that repo's `repo:<owner>/<repo>` label. Name that repo's main checkout as
`<target-checkout>` and run `afkconvoy flows <target-checkout>`
and use its `line` (the matching workflow's in a bound repo), with a title and body file. In a bound
repo omit the line's flow label while opening, then take it in there with
`afkconvoy intake -C <target-checkout> --issue <n> --workflow <workflow>` and your worker identity; keep the line's ready
label when it is unbound. Record the tracker issue as the blocker, then declare `blocked` on it.
";

/// The procedure offered only by teams that allow splitting.
const SPLIT: &str = "
## Splitting the issue

A stage that finds its issue too large for one piece of work may split it, with these commands and
in this order:

1. Open one issue per piece (`gh issue create<tracker><labelled>`), each saying what it is to
   deliver. Put no label of a workflow on it yourself.
2. Take each in to the workflow it is to follow — the team has {workflows} — naming with
   `--blocked-by` the ones among them it has to wait for; the command's `--help` says what it takes:
   ```bash
   {intake_command} --issue <n> --workflow <workflow> [--blocked-by <n,…>]
   ```
3. Declare `blocked` with the command below, with every issue you opened in `--blocked-by`.
   A piece belonging to another repo of this host follows the cross-repo blocker procedure below.

The issue is then parked on them, and when they have all closed this stage runs again, to judge the
whole.
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

This stage runs again: it declared `blocked`, and {waited}. <after_park> declare `already-satisfied` when they left nothing more
to do, or go on with what is left. What the stage said as it parked:

{reason}
";

/// What a stage running again after a fleet lock closed is told: what
/// stopped it before may have been the base branch, and is to be looked at
/// again on the base branch as it is now.
const LOCK_CLOSED: &str = "
## After a fleet lock

This stage runs again: a fleet lock stood while it was at work, and has closed. A lock is raised
when the base branch is broken for every issue, so what this stage declared before, or how it
stopped, may have been that fault and not this issue's. Take in the base branch as it is now and
judge again from there; conclude nothing from a gate that was red while the lock stood. What the
fleet recorded:

{reason}
";

/// What a stage that may commit is told of the gate: a PR it wrote is handed
/// on green (docs/adr/0010).
const GATE_BEFORE_PASS: &str = "
## The gate, before a `pass`

Opening a PR declares nothing, and no stage is handed red code. If this stage opened the issue's PR
or pushed to its branch, turn the gate green before you declare `pass`:

<gate_step>

<defect>";

/// What a stage is told of the human's ruling it runs again under.
const RULED: &str = "
## Ruling

This stage runs again under a human's ruling on it. Follow it:

{reason}
";

/// What a stage is told of its consult's ruling it runs again under.
const CONSULTED: &str = "
## Ruling

This stage's consult ruled on what it handed on. It runs again under the ruling. Follow it:

{reason}
";

/// The brief of a consult: a fixed form that says what was asked, where to
/// look and what it may run — as a human handed the issue would be told —
/// and nothing of how to decide: that is the role's to say.
const CONSULT: &str = "\
You are an afkconvoy worker, started in your role as a **consult**: the fleet would hand one GitHub
issue to a human, and you stand where that human stands. You are asked first, and may answer as
they would. Nobody reads this terminal: never wait for an answer.

- **Issue:** {issue} — {title}
- **Stage that asked:** `{stage}`, of the workflow `{flow}`
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Worktree:** `{worktree_path}`, on the branch `{branch}` (the base is `{base_branch}`). It is
  yours to read only: a consult writes no code, so never commit and never push
- **History:** the issue and every comment on it (`gh issue view {n}<tracker> --comments`), and what
  the branch holds past `origin/{base_branch}`
<rules>

## Stage records

{records}

## What was asked

{asked}

## Where to look further

What a human answering this would look at, and nothing you must: you decide what to look into.

- The issue, its comments and its blocked-by edges both ways: `gh issue view {n}<tracker> --comments`,
  `gh api repos/<issues>/issues/{n}/dependencies/blocked_by` and `…/blocking`
- Where it is and its rework: `{show_command}`
- The worktree, its branch and what only it holds: `git -C {worktree_path} status`, `git log`
- Its worker's terminal: `orca terminal list --worktree path:{worktree_path}`, then `orca terminal
  read --terminal <handle> --screen`
- The gate, run again in the worktree: `{gate_command}`
- The fleet and its host: `{doctor_command}`, `{plan_command}`, and any other read-only command

## What you may run

Every command a human runs to answer an escalated issue (`afkconvoy help humans`), and any
read-only command:

{answering}

What you may not do is what a stage that may not write may not: commit, push, or change the host —
delete files, caches or worktrees, or stop processes. What needs that, rule to the stage that
writes, or open as an issue.

## Before you let it through

<decisions>
## End

End with one answer, and not before; each command's `--help` says what it takes. Then stop.

- **Rule.** The stage `{stage}` runs again under your ruling, which is the text you give:
  ```bash
  {rule_command} --text-file <file>
  ```
- **Close or park**, with your reason. Neither is rework. A park waits for new evidence and an
  explicit `afkconvoy send`; it never runs the stage again by itself:
  ```bash
  {verdict_command} close --text-file <file>
  {verdict_command} park --text-file <file>
  ```
- **Send** it to another stage of its workflow with the lines above.
- **Let it through.** It is not yours to answer either: the issue goes to a human, and your text
  says what has to be decided:
  ```bash
  {verdict_command} escalate --text-file <file>
  ```
";

/// What a consult is told of the `escalate` it answers.
const ASKED_ESCALATE: &str = "The stage `{stage}` could not decide, and said why:\n\n";

/// What a consult is told of a hand-off it answers.
const ASKED_HAND_OFF: &str =
    "The fleet would have handed this issue to a human, and told them:\n\n";

/// A ruling answers its issue; enduring rules belong to the documents their owners maintain.
const DECISIONS: &str = "\
Follow the repo, team and role rules and the issue's rulings. A lasting rule change goes through
ordinary work in its owning ADR, team or role rules, amending conflicting older wording and
preserving source links. A human decision needs the actual human instruction or confirmation and
its source: a shared GitHub identity, an agent's ruling or a merged document alone proves none.
Preserve actual user decisions and escalate unresolved conflicts within the existing boundaries.
";

/// The same boundary for an unbound worker and a stage declaring a verdict.
const ESCALATION: &str = "\
A worker's turn has no fixed length. A large, multi-part or long change is not a reason to
escalate: work it in committed, pushed pieces where your role permits writing.

Escalate only when a human must act: spending money, or needing an account, a credential or a
person's hands; what cannot be undone; a secret or private data leaving the host; what a product
is for or who it is for; or a conflict between written decisions you cannot settle by the issue's
goal. Name which of these requires a human in your escalate comment, and say what they must decide
or do so they can act in one reply.
";

/// Where the rules a worker works under are: a line
/// for each kind. A harness loads them; a brief copies none. They are given
/// no order, and nothing here says which holds when two disagree: a team's
/// rules and a role's that contradict each other are a defect of the team's
/// files. The host user's skills and rules, which a harness may load beside
/// them (docs/adr/0017), are the one exception: everything above them wins.
const RULES: &str = "\
- **The repo's rules:** the rules file at the top of the <place>, where it has one
<team_rules>- **Your role's rules:** the rules file of the directory you were started in
- **The host user's own rules and skills:** loaded too, unless the team turns them off. Where they \
disagree with this brief or the rules above, this brief and the rules above win, a skill of your \
role's over one of theirs of the same name; you end only as this brief says
";

/// The line of `RULES` for the team rules, of a team that has them.
const TEAM_RULES: &str = "- **Team rules:** `{team_rules}`\n";

/// `template` with `RULES` where it says `<rules>`: of a worker whose repo is
/// at `place` — its worktree, or its checkout — and whose team has rules or
/// has none.
fn with_rules(template: &str, place: &str, team_rules: Option<&str>) -> String {
    let team = if team_rules.is_some() { TEAM_RULES } else { "" };
    template.replace("<rules>", &RULES.replace("<place>", place).replace("<team_rules>", team))
}

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
    /// The fleet's tracker, which holds the issue: every issue command of
    /// the brief names it (`WorkerFields::tracker`).
    pub tracker: &'a Tracker,
    pub title: &'a str,
    /// The workflow's name.
    pub flow: &'a str,
    pub stage: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    pub branch: &'a str,
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The team rules, in the fleet's copy of the team: `None` of a team that
    /// has none.
    pub team_rules: Option<&'a str>,
    /// What was declared on the issue so far, oldest first: the worker's
    /// history. When the last is a `reject`, a ruling or a human's sending,
    /// the stage runs for it; a consult rules on the last, an `escalate`.
    pub records: &'a [Said<'a>],
    /// Whether the team offers splitting in its stage briefs.
    pub split: bool,
    /// The line the worker declares its verdict with.
    pub verdict_command: &'a str,
    /// The line that raises a fleet lock from the issue's worktree.
    pub lock_command: &'a str,
    /// The line the worker takes an issue in with, before what names the
    /// issue and its workflow: a stage that splits its issue takes each
    /// child in.
    pub intake_command: &'a str,
    /// The team's workflows, by name: what a child may be taken in to.
    pub workflows: &'a [String],
    /// The lines the worker runs the gate with, when its stage may commit: a
    /// stage that writes nothing has no PR of its own to turn green.
    pub gate: Option<Gate<'a>>,
}

/// The two lines a worker that may commit runs the gate with.
#[derive(Debug, Clone, Copy)]
pub struct Gate<'a> {
    /// In its worktree: `afkconvoy gate`.
    pub standalone: &'a str,
    /// In another checkout, gated alone: `afkconvoy gate`.
    pub alone: &'a str,
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
            Author::Worker { consult: true, .. } | Author::Human | Author::Fleet => {
                said.record.headline()
            }
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
        Some(last) if last.stated == Stated::LockClosed => (LOCK_CLOSED, quoted(&last.text)),
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
            let on: Vec<String> = on.iter().map(|&blocker| fields.tracker.issue(blocker)).collect();
            format!("every issue it waited on has closed ({})", on.join(", "))
        }
    };
    let workflows: Vec<String> = fields.workflows.iter().map(|name| format!("`{name}`")).collect();
    let gate = match fields.gate {
        Some(_) => GATE_BEFORE_PASS.replace("<gate_step>", STAGE_GATE_STEP).replace("<defect>", ""),
        None => String::new(),
    };
    let template = with_rules(STAGE, "worktree", fields.team_rules)
        .replace("<rework>", why)
        .replace("<gate>", &gate)
        .replace("<split>", if fields.split { SPLIT } else { "" })
        .replace(
            "<after_park>",
            if fields.split {
                "If those are the issues this stage\nsplit its issue into, judge the whole now:"
            } else {
                "Judge what is left now:"
            },
        )
        .replace("<decisions>", DECISIONS)
        .replace("<escalation>", ESCALATION);
    let template = format!("{template}{ISSUE_UPDATES}");
    fill(
        &in_tracker(&template, fields.tracker),
        &[
            ("records", &records),
            ("rejecter", last.map_or("", |last| &last.stage)),
            ("reason", &reason),
            ("waited", &waited),
            ("workflows", &workflows.join(", ")),
            ("intake_command", fields.intake_command),
            ("n", &number),
            ("issue", &fields.tracker.issue(fields.number)),
            ("title", fields.title),
            ("flow", fields.flow),
            ("stage", fields.stage),
            ("role", fields.role),
            ("skill", fields.skill),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("team_rules", fields.team_rules.unwrap_or_default()),
            ("verdict_command", fields.verdict_command),
            ("lock_command", fields.lock_command),
            ("gate_command", fields.gate.map_or("", |gate| gate.standalone)),
            ("gate_alone_command", fields.gate.map_or("", |gate| gate.alone)),
        ],
    )
}

/// The brief of a patrol's worker: a fixed form that says what the run is
/// and nothing of what to look for — that is the role's to say.
const PATROL: &str = "\
You are an afkconvoy worker, started in your role to run one **patrol**. You own no issue. Nobody
reads this terminal: never wait for an answer.

- **Patrol:** `{patrol}`
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Checkout:** `{worktree_path}`, the base branch `{base_branch}` as it stood when you started.
  It is read-only to you: commit nothing in it and push nothing
<rules>
- **Workflows of the team:** {workflows}

## What a patrol does

A patrol finds work; it writes no code. For each thing you find that the fleet should work, open an
issue for it (`gh issue create<tracker><labelled>`) and take it in to the workflow that fits, with
this command — its `--help` says what it takes:
```bash
{intake_command}
```
You may have {room} more such issue(s) open: past that the patrol is not run again until some have
closed. Open none that an open issue already covers (`gh issue list<tracker><labelled> --state open`).

If you find the base branch broken for every issue, raise a fleet lock on the issue you opened for
the fix:
```bash
{lock_command}
```

## Hard rules

- Leave no commit and open no PR: a run that does has failed, and is reported to a human.
- Put no label on an issue yourself: the commands above do.

## Declare the run over

When you have nothing more to open, run this once, exactly as written, and then stop. It tells the
fleet that the run is over; until it does, the run holds a slot no issue can be worked in:
```bash
{over_command}
```
";

/// What the brief of a patrol's worker is filled with.
#[derive(Debug, Clone)]
pub struct PatrolFields<'a> {
    pub patrol: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// The patrol's checkout of the base branch.
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The team rules, in the fleet's copy of the team: `None` of a team that
    /// has none.
    pub team_rules: Option<&'a str>,
    /// The names of the team's workflows: what an issue can be taken in to.
    pub workflows: &'a [&'a str],
    /// How many more issues the patrol may have open.
    pub room: u64,
    /// The lines the worker takes an issue in and raises a fleet lock with.
    pub intake_command: &'a str,
    pub lock_command: &'a str,
    /// The line the worker declares the run over with.
    pub over_command: &'a str,
    /// The fleet's tracker, where the issues the patrol opens are opened
    /// (`WorkerFields::tracker`).
    pub tracker: &'a Tracker,
}

/// The brief a patrol's worker is given.
pub fn patrol_prompt(fields: &PatrolFields) -> Result<String, String> {
    let workflows: Vec<String> = fields.workflows.iter().map(|name| format!("`{name}`")).collect();
    let workflows = match workflows.as_slice() {
        [] => "none — there is nothing to take an issue in to".to_string(),
        named => named.join(", "),
    };
    fill(
        &in_tracker(&with_rules(PATROL, "checkout", fields.team_rules), fields.tracker),
        &[
            ("patrol", fields.patrol),
            ("role", fields.role),
            ("skill", fields.skill),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("team_rules", fields.team_rules.unwrap_or_default()),
            ("workflows", &workflows),
            ("room", &fields.room.to_string()),
            ("intake_command", fields.intake_command),
            ("lock_command", fields.lock_command),
            ("over_command", fields.over_command),
        ],
    )
}

/// The lines a consult's brief names beside its stage's fields.
#[derive(Debug, Clone, Copy)]
pub struct Consulting<'a> {
    /// The line it rules with.
    pub rule: &'a str,
    /// What a human handed the issue would be told, of a hand-off; `None`
    /// of an `escalate`, which is the last of the records.
    pub handed: Option<&'a str>,
    /// The lines that show the issue's position, run the gate in its
    /// worktree, and look at the fleet.
    pub show: &'a str,
    pub gate: &'a str,
    pub doctor: &'a str,
    pub plan: &'a str,
}

/// The brief a consult is given: `fields` are those of the stage that asked,
/// with the consult's role and skill. What it answers is the hand-off
/// `consulting` names, or else the last of the records, an `escalate`.
pub fn consult_prompt(fields: &StageFields, consulting: &Consulting) -> Result<String, String> {
    let number = fields.number.to_string();
    let asked = match consulting.handed {
        Some(handed) => format!("{ASKED_HAND_OFF}{}", quoted(handed)),
        None => {
            let said = fields.records.last().ok_or("nothing was asked")?;
            let asked = ASKED_ESCALATE.replace("{stage}", fields.stage);
            format!("{asked}{}", quoted(&said.record.text))
        }
    };
    let answering: Vec<String> = answering(fields.tracker)
        .iter()
        .map(|line| format!("    {}", line.replace("<n>", &number)))
        .collect();
    let template =
        with_rules(CONSULT, "worktree", fields.team_rules).replace("<decisions>", DECISIONS);
    fill(
        &in_tracker(&template, fields.tracker),
        &[
            ("records", &records_said(fields.records)),
            ("asked", &asked),
            ("answering", &answering.join("\n")),
            ("n", &number),
            ("issue", &fields.tracker.issue(fields.number)),
            ("title", fields.title),
            ("flow", fields.flow),
            ("stage", fields.stage),
            ("role", fields.role),
            ("skill", fields.skill),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("team_rules", fields.team_rules.unwrap_or_default()),
            ("verdict_command", fields.verdict_command),
            ("rule_command", consulting.rule),
            ("show_command", consulting.show),
            ("gate_command", consulting.gate),
            ("doctor_command", consulting.doctor),
            ("plan_command", consulting.plan),
            // `gh api` fills these in itself.
            ("owner", "{owner}"),
            ("repo", "{repo}"),
        ],
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
    prompt(fields, Variant::Land { pr }, Some(landing))
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
        Variant::Land { pr } => {
            let joiner = JOINER.replace("<landing>", if landing.is_some() { LANDING } else { "" });
            (steps(&joiner, "2", "3", " — your PR has landed"), pr.to_string())
        }
        Variant::TakeUp { pr } => {
            let outcomes = " — your PR has landed, or your comment is posted";
            let taker = TAKER.replace("<defect>", "");
            (steps(&taker, "3", "4", outcomes), pr.to_string())
        }
    };
    let template = template
        .replace("<fast_gate>", fields.fast_gate_command.map_or("", |_| FAST_GATE))
        .replace("<escalation>", ESCALATION);
    let template = format!("{template}{ISSUE_UPDATES}");
    fill(
        &in_tracker(&template, fields.tracker),
        &[
            ("fast_gate_command", fields.fast_gate_command.unwrap_or("")),
            ("role", landing.map_or("", |landing| landing.role)),
            ("skill", landing.map_or("", |landing| landing.skill)),
            ("n", &number),
            ("issue", &fields.tracker.issue(fields.number)),
            ("title", fields.title),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("gate_command", fields.gate_command),
            ("gate_alone_command", fields.gate_alone_command),
            ("lock_command", fields.lock_command),
            ("ready_label", fields.ready_label),
            ("land_command", fields.land_command),
            ("wake_command", fields.wake_command),
            ("verdict_marker", &marker),
            ("pr", &pr),
        ],
    )
}

/// What a worker is told of the fast gate, in a repo that declares one: to
/// pass it before its PR is opened or updated.
const FAST_GATE: &str = "
   Before you open or update the PR, commit and pass the fast gate: `{fast_gate_command}`, until its
   `status` is green. It is not the whole gate: the landing turn still runs that. A test it ends as
   TIMEOUT is a slow test: do as its output says.";

/// The worker prompt with a variant's opening and first step in it.
fn briefed(opening: &str, step1: &str) -> String {
    let outcomes = " — your PR has landed, or your comment is posted";
    let worker =
        WORKER.replace("<opening>", opening).replace("<step1>", step1).replace("<defect>", "");
    steps(&worker, "4", "5", outcomes)
}

/// `template` with the gate and join steps in it, numbered `gate` and `join`,
/// and the wake, said for the `outcomes` a worker on it can end with.
fn steps(template: &str, gate: &str, join: &str, outcomes: &str) -> String {
    let numbered = |step: &str| step.replace("<g>", gate).replace("<j>", join);
    template
        .replace("<gate_step>", &numbered(GATE_STEP))
        .replace("<join_step>", &numbered(&JOIN_STEP.replace("<endings>", &endings())))
        .replace("<wake>", &WAKE.replace("<outcomes>", outcomes))
}

/// What the land step tells a worker of each word its turn can answer, as
/// the turn's endings say it — but of a wait (`Ending::waits`): the worker
/// stops, and its fleet asks for the turn again.
pub fn endings() -> String {
    Ending::listed(|word, told| {
        let waits = Ending::ALL.iter().any(|e| e.word() == word && e.waits());
        match told.rsplit_once(": ").filter(|_| waits) {
            Some((situation, _)) => format!(
                "   On `\"{word}\"`, {situation}: nothing in your tree is to change. Stop without \
                 waking anyone; the fleet asks for the turn again itself, and starts a worker \
                 here if the tree needs changing."
            ),
            None => format!("   On `\"{word}\"`, {told}."),
        }
    })
}

/// The opening words of the fleet's brief pointer, also recognised in a
/// harness's initial user messages to identify a role worker's session.
pub(crate) const BRIEF_POINTER_PREFIX: &str = "Your task brief is the file ";

/// The line typed at a worker to hand it its brief. The prompt itself, sent as
/// text, arrives as one paste, which an agent asks to have confirmed.
pub fn brief_pointer(brief: &str) -> String {
    format!(
        "{BRIEF_POINTER_PREFIX}{brief} — read it now and carry it out end to end. \
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

/// What a worker switched onto another harness finds of the work it takes
/// over (`switched`).
#[derive(Debug, Clone)]
pub struct Switched<'a> {
    /// The harness the worker before it ran on.
    pub from: &'a str,
    /// The base branch, which the commits are ahead of.
    pub base_branch: &'a str,
    /// The commits on the branch ahead of the base branch, newest first, each
    /// as `git log --oneline` says it.
    pub commits: &'a [String],
    /// The open PR of the issue, when it has one.
    pub pr: Option<u64>,
    /// The last lines the worker before it showed in its terminal; none when
    /// it had no terminal left.
    pub said: &'a [String],
}

/// What the brief of a worker started by a switch says after the rest: that
/// it continues the work of a worker of another harness, ended mid-way, and
/// what is there of it.
pub fn switched(switched: &Switched) -> String {
    let mut text = format!(
        "\n\n## You continue earlier work\n\nA worker on `{}` was at this before you, and was \
         ended mid-way so that this work goes on on another harness: nothing of it is thrown \
         away. Take up where it stood, and redo nothing it finished.\n",
        switched.from
    );
    match switched.commits {
        [] => text.push_str(&format!(
            "\n- **Commits:** none on the branch ahead of `origin/{}` yet; see `git status` for \
             what it left uncommitted.\n",
            switched.base_branch
        )),
        commits => {
            text.push_str(&format!(
                "\n- **Commits:** {} on the branch ahead of `origin/{}`:\n",
                commits.len(),
                switched.base_branch
            ));
            for commit in commits {
                text.push_str(&format!("  - `{commit}`\n"));
            }
        }
    }
    text.push_str(&match switched.pr {
        Some(pr) => format!("- **PR:** #{pr} is open; push to its branch, and open no other.\n"),
        None => "- **PR:** none is open yet.\n".to_string(),
    });
    match switched.said {
        [] => text.push_str("- **Last said:** its terminal was already gone.\n"),
        said => {
            text.push_str("- **Last said**, the end of its terminal:\n\n```\n");
            for line in said {
                text.push_str(line.trim_end());
                text.push('\n');
            }
            text.push_str("```\n");
        }
    }
    text
}

/// `template` with the fleet's tracker in each of its issue commands, where
/// its marks say: `<tracker>` after a `gh issue` command, `<issues>` in a
/// `gh api` path, `<labelled>` on one that opens or looks for an issue of
/// the repo, and `<repo_label>` where the brief says which labels that issue
/// carries — `<only_repo_label>` where it carries no other the worker puts
/// on. Of the checkout's own repo they say nothing more than they did before
/// there were trackers (docs/adr/0027).
pub(crate) fn in_tracker(template: &str, tracker: &Tracker) -> String {
    let (flag, path, labelled, said, only) = match (tracker.named(), tracker.repo_label()) {
        (Some(named), Some(label)) => (
            format!(" --repo {named}"),
            named.to_string(),
            format!(" --label {label}"),
            format!(", and the repo label `{label}` (`--label {label}`)"),
            format!(", only the repo label `{label}` (`--label {label}`)"),
        ),
        _ => (
            String::new(),
            "{owner}/{repo}".to_string(),
            String::new(),
            String::new(),
            String::new(),
        ),
    };
    template
        .replace("<cross_repo>", CROSS_REPO_BLOCKER)
        .replace(
            "<blocker_tracker>",
            tracker.named().map_or("<owner>/<tracker-name>", |name| name.as_str()),
        )
        .replace("<tracker>", &flag)
        .replace("<issues>", &path)
        .replace("<labelled>", &labelled)
        .replace("<repo_label>", &said)
        .replace("<only_repo_label>", &only)
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
    use crate::harness::Harness;

    fn fields() -> WorkerFields<'static> {
        WorkerFields {
            number: 7,
            tracker: &Tracker::Own,
            title: "Add {branch} support",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            gate_command: "/opt/bin/afkconvoy gate",
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            ready_label: "agent-ready",
            land_command: "/opt/bin/afkconvoy land --issue 7",
            wake_command: "/opt/bin/afkconvoy wake",
            fast_gate_command: None,
        }
    }

    fn asserts_escalation_boundary(brief: &str) {
        for rule in [
            "A worker's turn has no fixed length",
            "A large, multi-part or long change is not a reason to\nescalate",
            "committed, pushed pieces",
            "spending money, or needing an account, a credential or a\nperson's hands",
            "what cannot be undone",
            "a secret or private data leaving the host",
            "what a product\nis for or who it is for",
            "a conflict between written decisions you cannot settle by the issue's\ngoal",
            "Name which of these requires a human in your escalate comment",
        ] {
            assert!(brief.contains(rule), "missing {rule:?} in {brief}");
        }
    }

    #[test]
    fn unbound_workers_escalate_only_for_a_human() {
        for variant in [Variant::Fresh, Variant::Continue, Variant::TakeUp { pr: 70 }] {
            asserts_escalation_boundary(&worker_prompt(&fields(), variant).unwrap());
        }
    }

    #[test]
    fn a_worker_passes_the_fast_gate_before_its_pr_where_the_repo_declares_one() {
        let without = worker_prompt(&fields(), Variant::Fresh).unwrap();
        assert!(!without.contains("fast gate"), "{without}");
        let fast =
            WorkerFields { fast_gate_command: Some("/opt/bin/afkconvoy gate --fast"), ..fields() };
        let with = worker_prompt(&fast, Variant::Fresh).unwrap();
        let told = with.find("pass the fast gate: `/opt/bin/afkconvoy gate --fast`").expect(&with);
        assert!(told < with.find("3. **Open a PR**").unwrap(), "{with}");
    }

    #[test]
    fn every_issue_command_of_a_brief_names_the_tracker_and_no_pr_command_does() {
        let tracker = Tracker::Other {
            tracker: crate::config::RepoName::read("acme/tracker").unwrap(),
            repo: crate::config::RepoName::read("acme/dev").unwrap(),
        };
        let fields = WorkerFields { tracker: &tracker, ..fields() };
        let briefs: Vec<String> = [
            Variant::Fresh,
            Variant::Continue,
            Variant::Land { pr: 70 },
            Variant::TakeUp { pr: 70 },
        ]
        .into_iter()
        .map(|variant| worker_prompt(&fields, variant).unwrap())
        .collect();
        for brief in &briefs {
            for mark in ["<tracker>", "<issues>", "<labelled>", "<repo_label>", "{owner}"] {
                assert!(!brief.contains(mark), "{mark}: {brief}");
            }
            for (at, _) in brief.match_indices("gh issue ") {
                let command = brief[at..].split(['`', '\n']).next().unwrap();
                assert!(command.contains(" --repo acme/tracker"), "{command}");
            }
            for (at, _) in brief.match_indices("gh api ") {
                assert!(brief[at..].starts_with("gh api repos/acme/tracker/issues/7/"), "{brief}");
            }
            for (at, _) in brief.match_indices("gh pr ") {
                let command = brief[at..].split('\n').next().unwrap();
                assert!(!command.contains("--repo"), "{command}");
            }
        }
        let fresh = &briefs[0];
        for brief in [fresh, &briefs[1], &briefs[3]] {
            assert!(brief.contains("open it on the tracker\n`acme/tracker`"), "{brief}");
            assert!(brief.contains("that repo's `repo:<owner>/<repo>` label"), "{brief}");
            assert!(brief.contains("run `afkconvoy flows <target-checkout>`"), "{brief}");
            assert!(brief.contains("omit the line's flow label while opening"), "{brief}");
            assert_eq!(brief.matches("## Cross-repo blockers").count(), 1, "{brief}");
        }
        assert!(fresh.contains("**Your issue:** acme/tracker#7 — Add {branch} support"));
        assert!(fresh.contains("\n   Closes acme/tracker#7\""), "{fresh}");
    }

    /// Each issue command of `brief` names the tracker `acme/tracker`, every
    /// one that lists issues the repo label of `acme/dev` too — and one that
    /// opens an issue, or the brief beside it — and no PR command names a
    /// repo; no mark is left in it.
    fn names_the_tracker(brief: &str) {
        for mark in ["<tracker>", "<issues>", "<labelled>", "<repo_label>", "<only_repo_label>"] {
            assert!(!brief.contains(mark), "{mark}: {brief}");
        }
        assert!(!brief.contains("{owner}"), "{brief}");
        let named = |(at, _): (usize, &str)| {
            brief[..at].ends_with("acme/tracker")
                || brief[at + 2..].starts_with(|c: char| c.is_ascii_digit())
        };
        assert!(brief.match_indices("#7").all(named), "the issue is the tracker's: {brief}");
        for (at, _) in brief.match_indices("gh issue ") {
            let command = brief[at..].split(['`', '\n']).next().unwrap();
            assert!(command.contains(" --repo acme/tracker"), "{command}");
            let labelled = match command.starts_with("gh issue create") {
                true => brief.contains("`--label repo:acme/dev`") || command.contains("repo:"),
                false => !command.starts_with("gh issue list") || command.contains("repo:"),
            };
            assert!(labelled, "{command}: {brief}");
        }
        for (at, _) in brief.match_indices("gh api ") {
            let command = brief[at..].split(['`', '\n']).next().unwrap();
            assert!(command.contains(" repos/acme/tracker/issues/7/"), "{command}");
        }
        for (at, _) in brief.match_indices("gh pr ") {
            let command = brief[at..].split('\n').next().unwrap();
            assert!(!command.contains("--repo"), "{command}");
        }
    }

    #[test]
    fn every_issue_command_of_a_bound_roles_brief_names_the_tracker() {
        let tracker = Tracker::Other {
            tracker: crate::config::RepoName::read("acme/tracker").unwrap(),
            repo: crate::config::RepoName::read("acme/dev").unwrap(),
        };
        let workflows = ["feature".to_string()];
        let record = |stated, blocked_by| StageRecord {
            stage: "build".to_string(),
            stated: Stated::Declared(stated),
            by: Author::Worker {
                consult: false,
                claim: "c".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            blocked_by,
            text: "v1 or v2?".to_string(),
        };
        let (asked, parked) =
            (record(Declared::Escalate, vec![]), record(Declared::Blocked, vec![11, 12]));
        let stage = StageFields {
            number: 7,
            tracker: &tracker,
            title: "Add support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/acme-tracker-7-add",
            worktree: "/wt/acme-tracker-7",
            base_branch: "develop",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "/opt/bin/afkconvoy intake",
            split: true,
            workflows: &workflows,
            gate: Some(Gate { standalone: "afkconvoy gate", alone: "afkconvoy gate" }),
        };
        let consulting = Consulting {
            rule: "/opt/bin/afkconvoy rule --issue 7 --stage build",
            handed: None,
            show: "afkconvoy show --issue 7",
            gate: "afkconvoy gate",
            doctor: "afkconvoy doctor",
            plan: "afkconvoy plan",
        };
        let escalated = [Said { record: &asked, url: "https://gh/c/5" }];
        let consult = StageFields { records: &escalated, role: "boss", skill: "rule", ..stage };
        let waited = [Said { record: &parked, url: "https://gh/c/6" }];
        let patrol = PatrolFields {
            patrol: "sweep",
            tracker: &tracker,
            role: "dev",
            skill: "implement",
            worktree: "/wt/convoy-patrol-sweep",
            base_branch: "develop",
            team_rules: None,
            workflows: &["feature"],
            room: 2,
            intake_command: "/opt/bin/afkconvoy intake",
            lock_command: "/opt/bin/afkconvoy lock",
            over_command: "/opt/bin/afkconvoy over --run sweep",
        };
        let landing = Landing { role: "dev", skill: "land" };
        let worker = WorkerFields { tracker: &tracker, ..fields() };
        let briefs = [
            ("stage", stage_prompt(&stage).unwrap()),
            ("parked", stage_prompt(&StageFields { records: &waited, ..stage.clone() }).unwrap()),
            ("consult", consult_prompt(&consult, &consulting).unwrap()),
            ("patrol", patrol_prompt(&patrol).unwrap()),
            ("landing", landing_prompt(&worker, 70, landing).unwrap()),
        ];
        for (_, brief) in &briefs {
            names_the_tracker(brief);
        }
        let [stage, parked, consult, patrol, ..] = &briefs.map(|(_, brief)| brief);
        for brief in [stage, parked, consult] {
            assert!(brief.contains("- **Issue:** acme/tracker#7 — Add support\n"), "{brief}");
        }
        assert!(stage.contains("(`gh issue create --repo acme/tracker --label repo:acme/dev`)"));
        assert!(!stage.contains("A red gate that is not yours"));
        assert!(stage.contains("open it on the tracker\n`acme/tracker`"), "{stage}");
        assert!(
            stage.contains(
                "afkconvoy intake -C <target-checkout> --issue <n> --workflow <workflow>"
            ),
            "{stage}"
        );
        assert!(parked.contains("(acme/tracker#11, acme/tracker#12)"), "{parked}");
        assert!(consult.contains("    gh issue close 7 --repo acme/tracker --comment <why>\n"));
        assert!(patrol.contains(
            "open an\nissue for it (`gh issue create --repo acme/tracker --label repo:acme/dev`)"
        ));
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
        assert!(prompt.contains("four") || prompt.contains("holds one turn"));
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
    fn a_worker_taking_up_a_pr_is_told_its_pr_exists_and_to_merge_the_base_gate_and_land() {
        let prompt = worker_prompt(&fields(), Variant::TakeUp { pr: 70 }).unwrap();
        assert!(prompt.contains("PR #70 is open for it"));
        assert!(prompt.contains("The PR already exists"));
        assert!(prompt.contains("**Your PR:** #70, from the branch `tester/issue-7-add`"));
        assert!(prompt.contains("\n1. **Read why it stopped.** `gh issue view 7 --comments`"));
        assert!(prompt.contains(
            "\n2. **Merge the base branch.** `git fetch origin && git merge origin/develop`"
        ));
        assert!(prompt.contains("\n3. **Prepare to land.**") && prompt.contains("\n4. **Land.**"));
        assert!(prompt.contains("   /opt/bin/afkconvoy land --issue 7\n"));
        assert!(prompt.contains("commit the merge and run this again"));
        assert!(prompt.contains("never open another"));
        assert!(prompt.contains(&verdict::marker_format()));
        for absent in ["**Implement**", "gh pr create", "<g>", "<j>", "_step>"] {
            assert!(!prompt.contains(absent), "{absent}");
        }
    }

    #[test]
    fn every_worker_with_an_outcome_is_told_to_wake_the_fleet() {
        let line = "run this once, exactly as written, and then stop. If it\nfails, ignore it:\n\
                    ```bash\n/opt/bin/afkconvoy wake\n```";
        for variant in [Variant::Fresh, Variant::Continue] {
            let prompt = worker_prompt(&fields(), variant).unwrap();
            assert!(prompt.contains(line), "{prompt}");
            assert!(prompt.contains("your PR has landed, or your comment is posted"));
            assert!(prompt.contains("wake the fleet and stop"));
            assert!(prompt.contains("wake the fleet (below)\nand stop. Its **first line**"));
            assert_eq!(prompt.matches("afkconvoy wake").count(), 1, "said in one place");
            assert!(!prompt.contains("<wake>") && !prompt.contains("<outcomes>"));
        }
        let joiner = worker_prompt(&fields(), Variant::Land { pr: 70 }).unwrap();
        assert!(joiner.contains(line), "{joiner}");
        assert!(joiner.contains("Once your outcome is on GitHub — your PR has landed,"));
        assert!(!joiner.contains("comment is posted") && !joiner.contains("<wake>"));
    }

    #[test]
    fn the_brief_of_a_worker_in_its_role_is_a_fixed_form() {
        let fields = StageFields {
            number: 7,
            tracker: &Tracker::Own,
            title: "Add {branch} support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "/opt/bin/afkconvoy intake",
            split: true,
            workflows: &["feature".to_string(), "small fix".to_string()],
            gate: None,
        };
        let prompt = stage_prompt(&fields).unwrap();
        asserts_escalation_boundary(&prompt);
        for said in [
            "- **Issue:** #7 — Add {branch} support\n",
            "- **Stage:** `build`, of the workflow `feature`\n",
            "- **Skill to perform:** `implement`, of the role you were started in (`dev`)\n",
            "- **Worktree:** `/wt/issue-7`, on the branch `tester/issue-7-add` (the base is `develop`)",
            "- **History:** the issue and every comment on it (`gh issue view 7 --comments`)",
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
                harness: Harness::ClaudeCode,
                model: None,
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
                harness: Harness::ClaudeCode,
                model: None,
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
        let consulting = Consulting {
            rule,
            handed: None,
            show: "afkconvoy show --issue 7",
            gate: "afkconvoy gate",
            doctor: "afkconvoy doctor",
            plan: "afkconvoy plan",
        };
        let brief =
            consult_prompt(&StageFields { records: &escalated, ..consulted.clone() }, &consulting)
                .unwrap();
        for said in [
            "started in your role as a **consult**",
            "- **Stage that asked:** `build`, of the workflow `feature`\n",
            "- **Skill to perform:** `rule`, of the role you were started in (`boss`)\n",
            "a consult writes no code, so never commit and never push\n",
            "- `build` declared `escalate` — https://gh/c/5\n",
            "- Where it is and its rework: `afkconvoy show --issue 7`\n",
            "    afkconvoy rule --issue 7 --text-file <file>\n",
            "    gh issue close 7 --comment <why>\n",
            "commit, push, or change the host",
            "## What was asked\n\nThe stage `build` could not decide, and said why:\n\n> v1 or v2?\n",
            "```bash\n  /opt/bin/afkconvoy rule --issue 7 --stage build --text-file <file>\n",
            "/opt/bin/afkconvoy verdict --issue 7 --stage build escalate --text-file <file>\n",
        ] {
            assert!(brief.contains(said), "{said}\n{brief}");
        }
        assert!(consult_prompt(&consulted, &consulting).is_err(), "nothing was asked");
        // Of a hand-off, it is told what a human would be, and no check of its own.
        let handed =
            Consulting { handed: Some("The worker went silent.\nIts screen: …"), ..consulting };
        let brief = consult_prompt(&consulted, &handed).unwrap();
        assert!(
            brief.contains(
                "## What was asked\n\nThe fleet would have handed this issue to a human, and told \
                 them:\n\n> The worker went silent.\n> Its screen: …\n"
            ),
            "{brief}"
        );
        let answered =
            [&escalated[..], &[Said { record: &answer, url: "https://gh/c/6" }]].concat();
        let under = stage_prompt(&StageFields { records: &answered, ..fields.clone() }).unwrap();
        assert!(
            under.contains(
                "- the consult of the stage `build` ruled: the stage runs again under the ruling \
                 — https://gh/c/6\n"
            ),
            "{under}"
        );
        assert!(
            under.contains(
                "## Ruling\n\nThis stage's consult ruled on what it handed on. It runs again \
                 under the ruling. Follow it:\n\n> Keep {v1}.\n"
            ),
            "{under}"
        );
    }

    #[test]
    fn a_stage_is_told_how_to_split_its_issue_and_that_it_judges_the_whole_after_the_park() {
        let fields = StageFields {
            number: 7,
            tracker: &Tracker::Own,
            title: "Add support",
            flow: "feature",
            stage: "plan",
            role: "dev",
            skill: "plan",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage plan",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "cd '/wt/issue-7' && /opt/bin/afkconvoy intake",
            split: true,
            workflows: &["feature".to_string(), "fix".to_string()],
            gate: None,
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

        let off = StageFields { split: false, ..fields.clone() };
        let without = stage_prompt(&off).unwrap();
        assert!(!without.to_lowercase().contains("split"), "{without}");
        assert!(!without.contains("gh issue create"), "{without}");
        assert!(without.contains("## Declare a verdict"));

        // The stage that parked runs again once what it waited on closed.
        let parked = StageRecord {
            stage: "plan".to_string(),
            stated: Stated::Declared(Declared::Blocked),
            by: Author::Worker {
                consult: false,
                claim: "c".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
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
        let without = stage_prompt(&StageFields { records: &records, ..off }).unwrap();
        assert!(!without.contains("split its issue"), "{without}");
        assert!(!without.contains("## Splitting"), "{without}");
        assert!(without.contains("Judge what is left now:"), "{without}");
        // A human's `blocked` names no issue, and parks nothing.
        let unnamed = StageRecord { blocked_by: vec![], by: Author::Human, ..parked.clone() };
        let records = [Said { record: &unnamed, url: "https://gh/c/2" }];
        let human = stage_prompt(&StageFields { records: &records, ..fields.clone() }).unwrap();
        assert!(human.contains("it declared `blocked`, and it named no issue to wait on."));
        // The fleet's record of a closed lock is why a stage runs again after one.
        let closed = StageRecord::lock_closed("build", 13);
        let records = [Said { record: &closed, url: "https://gh/c/3" }];
        let told = stage_prompt(&StageFields { records: &records, ..fields }).unwrap();
        assert!(
            told.contains(
                "- a fleet lock has closed, and the stage `build` runs again — https://gh/c/3\n\n\
                 ## After a fleet lock\n\nThis stage runs again: a fleet lock stood while it was at \
                 work, and has closed."
            ),
            "{told}"
        );
        assert!(told.contains("> A fleet lock stood (#13) and has closed."), "{told}");
        assert!(!told.contains("## After a park") && !told.contains("<rework>"), "{told}");
    }

    #[test]
    fn a_stage_that_may_commit_is_told_to_turn_the_gate_green_before_it_passes() {
        let fields = StageFields {
            number: 7,
            tracker: &Tracker::Own,
            title: "Add support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "/opt/bin/afkconvoy intake",
            split: true,
            workflows: &["feature".to_string()],
            gate: Some(Gate {
                standalone: "cd '/wt/issue-7' && /opt/bin/afkconvoy gate",
                alone: "/opt/bin/afkconvoy gate",
            }),
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
        assert!(prompt.contains("   cd '/wt/issue-7' && /opt/bin/afkconvoy gate\n"));
        assert!(prompt.contains("landing\n   worker takes the merge, gate and land turn"));
        let gate = prompt.find("## The gate").unwrap();
        assert!(gate < prompt.find("## Declare a verdict").unwrap(), "before the verdict");
        assert!(!prompt.contains("<gate") && !prompt.contains("<g>"), "{prompt}");
        // A stage that writes nothing has no PR of its own to turn green.
        let reads = stage_prompt(&StageFields { gate: None, ..fields }).unwrap();
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
        let joiner = worker_prompt(&fields(), Variant::Land { pr: 70 }).unwrap();
        let role = prompt.find("**Your role:**").unwrap();
        let steps = prompt.find("\n## Steps").unwrap();
        assert_eq!(format!("{}{}", &prompt[..role], &prompt[steps..]), joiner);
        assert!(!joiner.contains("<landing>") && !joiner.contains("Your role"), "{joiner}");
    }

    #[test]
    fn workers_open_the_pr_before_the_turn_and_do_not_gate_the_tip_alone() {
        let fresh = worker_prompt(&fields(), Variant::Fresh).unwrap();
        assert!(fresh.find("gh pr create").unwrap() < fresh.find("It asks for a turn").unwrap());
        for variant in [
            Variant::Fresh,
            Variant::Continue,
            Variant::TakeUp { pr: 70 },
            Variant::Land { pr: 70 },
        ] {
            let prompt = worker_prompt(&fields(), variant).unwrap();
            assert!(prompt.contains("pushes that same commit as a fast-forward"), "{prompt}");
            assert!(!prompt.contains("A red gate that is not yours"), "{prompt}");
            assert!(!prompt.contains("git worktree add --detach"), "{prompt}");
        }
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

    #[test]
    fn a_brief_says_where_each_kind_of_rule_is_and_names_the_team_s_only_of_a_team_that_has_them() {
        const TEAM: &str = "/copies/acme/team/def/CLAUDE.md";
        let workflows = ["feature".to_string()];
        let asked = StageRecord {
            stage: "build".to_string(),
            stated: Stated::Declared(Declared::Escalate),
            by: Author::Worker {
                consult: false,
                claim: "c".to_string(),
                role_commit: "r".to_string(),
                team_commit: "t".to_string(),
                harness: Harness::ClaudeCode,
                model: None,
            },
            blocked_by: vec![],
            text: "v1 or v2?".to_string(),
        };
        let escalated = [Said { record: &asked, url: "https://gh/c/5" }];
        let stage = |team_rules| StageFields {
            number: 7,
            tracker: &Tracker::Own,
            title: "Add support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            team_rules,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "/opt/bin/afkconvoy intake",
            split: true,
            workflows: &workflows,
            gate: None,
        };
        let patrol = |team_rules| PatrolFields {
            patrol: "sweep",
            tracker: &Tracker::Own,
            role: "dev",
            skill: "implement",
            worktree: "/wt/convoy-patrol-sweep",
            base_branch: "develop",
            team_rules,
            workflows: &["feature"],
            room: 2,
            intake_command: "/opt/bin/afkconvoy intake",
            lock_command: "/opt/bin/afkconvoy lock",
            over_command: "/opt/bin/afkconvoy over --run sweep",
        };
        let briefs = |team_rules| {
            [
                ("worktree", stage_prompt(&stage(team_rules)).unwrap()),
                (
                    "worktree",
                    consult_prompt(
                        &StageFields { records: &escalated, ..stage(team_rules) },
                        &Consulting {
                            rule: "afkconvoy rule",
                            handed: None,
                            show: "afkconvoy show",
                            gate: "afkconvoy gate",
                            doctor: "afkconvoy doctor",
                            plan: "afkconvoy plan",
                        },
                    )
                    .unwrap(),
                ),
                ("checkout", patrol_prompt(&patrol(team_rules)).unwrap()),
            ]
        };
        for (place, brief) in briefs(Some(TEAM)) {
            let lines = format!(
                "- **The repo's rules:** the rules file at the top of the {place}, where it has \
                 one\n- **Team rules:** `{TEAM}`\n- **Your role's rules:** the rules file of the \
                 directory you were started in\n"
            );
            assert!(brief.contains(&lines), "{brief}");
            // The host user's give way to all of it, wherever they are loaded.
            let host_user = "- **The host user's own rules and skills:** loaded too, unless the \
                             team turns them off. Where they disagree with this brief or the \
                             rules above, this brief and the rules above win";
            assert!(brief.contains(host_user), "{brief}");
        }
        // A team with none: no line for them, and the others as they are.
        for (place, brief) in briefs(None) {
            let lines = format!(
                "the top of the {place}, where it has one\n- **Your role's rules:** the rules file"
            );
            assert!(brief.contains(&lines), "{brief}");
            assert!(!brief.contains("Team rules") && !brief.contains("rules>"), "{brief}");
        }
    }
}
