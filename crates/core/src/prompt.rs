//! The worker prompt: a template, filled by code. No worker is ever started on
//! a prompt with a placeholder left in it.

use crate::position::Declared;
use crate::precedent::entry_form;
use crate::record;
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
    /// The line the worker runs the gate with: `afkconvoy gate --train`.
    pub gate_command: &'a str,
    /// The line the worker gates another checkout with, in that checkout:
    /// `afkconvoy gate`.
    pub gate_alone_command: &'a str,
    /// The line that raises a fleet lock on the issue of a proven red base branch.
    pub lock_command: &'a str,
    /// The label that makes an issue dispatchable: what an issue a worker
    /// opens for a defect it found carries.
    pub ready_label: &'a str,
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

/// What every worker that runs the gate and can park its issue on another is
/// told of a red gate its own work did not cause: the defect it found is
/// opened as an issue the fleet works, and never left to a human. `<proof>`
/// is what is gated alone, `<open>` how the issue enters the fleet's work and
/// `<park>` what the worker does once it is recorded and, for a red base
/// branch, locked.
const DEFECT: &str = "\
## A red gate that is not yours

A red gate is yours to deal with, as above, unless you prove that it is not. The proof is that
<proof> is red the same way on its own. Gate it alone, in a clean checkout outside your worktree,
and remove that checkout after:
```bash
git worktree add --detach <a new directory> <that commit>
cd <that directory> && {gate_alone_command}
```
A red gate you have not shown this way stays yours. A fleet lock standing is the issue of a fault
every issue shares already: do as you are told of the lock, and nothing of this.

Once you have shown it, the failure is a defect outside your work, which you discovered. No human
writes its issue: you do.

1. **Look before opening.** Other workers hit the same red `{base_branch}`: search the open issues
   for one that already records this defect (`gh issue list<tracker><labelled> --state open --search \"<words of the
   failure>\"`). If one does, comment your evidence there — another occurrence, and when — and
   open none.
2. **Otherwise open one** that a fresh worker can pick up (`gh issue create<tracker>`): the symptom; how to
   reproduce it on `{base_branch}`; what the logs show; what is guessed and what is verified; and
   acceptance criteria that require the cause to be found with evidence, and a test that reproduces
   it. <open> Name no blocker unless it has a real one.
3. **Raise the fleet lock for a red base branch.** If you proved only the landing train's tip
   red, gate `origin/{base_branch}` alone in a clean checkout too, as above. Only when the base
   branch is red the same way, make sure the fix's issue — existing or new — is dispatchable as
   above, then run `{lock_command} --issue <n>` on it from your worktree. The fleet works that
   issue first while the lock stands. A defect confined to the train raises no lock.
   A defect belonging to another repo of this host follows the cross-repo blocker procedure below.
4. <park>
";

/// What an issue's worker gates alone to prove a red gate is not its own:
/// what its gate merged in.
const PROOF_MERGED: &str =
    "the commit your gate merged in — its `merged`: the landing train's tip, or
`origin/{base_branch}` —";

/// How an issue a worker opens for a defect is put before the fleet: with the
/// ready label, in an unbound repo, by every worker that opens one.
const DEFECT_READY: &str =
    "Put the label `{ready_label}` on it (`--label {ready_label}`)<repo_label>, so
   the fleet works it like any other issue.";

/// How a worker of an unbound repo parks its issue on the defect's.
const PARK_WORKER: &str =
    "**Then park your issue on it.** Push your branch (`git push origin HEAD`), open no PR,
   and post the `blocked` verdict (below) with that issue — the one you opened, or the one you
   commented on — in `blocked_by=`. When it closes, your issue is the fleet's again: a worker gates
   your branch again and joins it, and nothing waits for a human.";

/// How a worker that took up a kept PR parks its issue on the defect's.
const PARK_TAKER: &str =
    "**Then park your issue on it.** Push your branch (`git push origin HEAD`) and post the
   verdict comment (below) with `phase=blocked` and that issue — the one you opened, or the one you
   commented on — in `blocked_by=`. Your PR stays open: when that issue closes, a worker takes it up
   again, gates it and joins it, and nothing waits for a human.";

/// `DEFECT`, of a worker that gates alone `proof`, puts an issue it opens
/// before the fleet as `open` says and then does `park`.
fn defect(proof: &str, open: &str, park: &str) -> String {
    DEFECT.replace("<proof>", proof).replace("<open>", open).replace("<park>", park)
}

const JOINER: &str = "\
You are an afkconvoy worker taking over an issue whose work is finished: PR #{pr} is open for it, and
is not on the landing train yet. Your ONLY job is to get that PR on the train. Write no feature and
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

- End with your PR on the landing train — never just stop.
- Never close the PR, never open another, never rebase.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

/// The brief of a worker that takes up a PR a released claim kept: the issue
/// went to a human, or was parked, and is the fleet's again.
const TAKER: &str = "\
You are an afkconvoy worker taking up an issue that is the fleet's again: PR #{pr} is open for it,
kept when an earlier worker's claim was released — the issue was escalated or parked — and is not
on the landing train. The PR already exists: your job is to finish it and get it on the train. Open
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
on one line, followed by where you are stuck, so that a human can act on it in one reply — or, for
a defect outside your issue (above), with `phase=blocked` and its issue in `blocked_by=`:
```
{verdict_marker}
```

<wake>

## Hard rules

- End with your PR on the landing train, or with that comment — never just stop.
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
2. **Implement** the issue's acceptance criteria, matching the surrounding code's conventions.
<gate_step>
4. **Open a PR** against `{base_branch}` whose body contains the line `Closes {issue}`:
   ```bash
   git push origin HEAD && gh pr create --base {base_branch} --head {branch} --title \"<title>\" --body \"<what changed>

   Closes {issue}\"
   ```
   Do not merge it.
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
  Reuse its issue. Open one only for an uncovered defect you discovered (above), or for another
  repo of this host, following the cross-repo blocker procedure below. Name every blocking issue
  in `blocked_by=` (e.g. `blocked_by=41,42`) and say what is missing.
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

- **Issue:** {issue} — {title}
- **Stage:** `{stage}`, of the workflow `{flow}`
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Worktree:** `{worktree_path}`, on the branch `{branch}` (the base is `{base_branch}`)
- **History:** the issue and every comment on it (`gh issue view {n}<tracker> --comments`), and what
  the branch holds past `origin/{base_branch}`
<rules>- **Precedents:** `{precedents}`

## Stage records

{records}
<rework><gate><split><cross_repo>
## Before an `escalate`

<precedents_first>
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
`<target-checkout>` and run `afkconvoy flows -C <target-checkout>`
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

/// How a stage puts an issue it opens for a defect before the fleet: it takes
/// it in, as a split does, so the issue enters a workflow.
const DEFECT_INTAKE: &str =
    "Put no label of a workflow on it yourself<only_repo_label>: take it in to the workflow
   that fits — the team has {workflows} — with
   `{intake_command} --issue <n> --workflow <workflow>`.";

/// How a stage parks its issue on the defect's.
const PARK_STAGE: &str = "**Then park your issue on it.** Push your branch (`git push origin HEAD`), then declare
   `blocked` with the command below, with that issue — the one you opened, or the one you commented
   on — in `--blocked-by`. When it closes this stage runs again, and gates your branch again: nothing
   waits for a human.";

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
<rules>- **Precedents:** `{precedents}`

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

<precedents_first>
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

/// The brief of the stock consult in an unbound repo, which has no stages:
/// a fixed form that says what was asked, where to look and what a human
/// answers with there, and carries the stock consult's instruction
/// (`stock::INSTRUCTION`) — the worker starts in the worktree, with no role
/// to give it.
const STOCK_UNBOUND: &str = "\
You are an afkconvoy worker, started as the **stock consult**: the fleet handed one GitHub issue to a
human, and you stand where that human stands. You are asked first, and may answer as they would.
Nobody reads this terminal: never wait for an answer.

- **Issue:** {issue} — {title}
- **Worktree:** `{worktree_path}`, on the branch `{branch}` (the base is `{base_branch}`). It is
  yours to read only: a consult writes no code, so never commit and never push

{instruction}
## What was asked

The fleet handed this issue to a human, and told them:

{asked}

## Where to look further

What a human answering this would look at, and nothing you must: you decide what to look into.

- The issue, its comments and its blocked-by edges both ways: `gh issue view {n}<tracker> --comments`,
  `gh api repos/<issues>/issues/{n}/dependencies/blocked_by` and `…/blocking`
- The worktree, its branch and what only it holds: `git -C {worktree_path} status`, `git log`
- The gate, run again in the worktree: `{gate_command}`
- The fleet and its host: `{doctor_command}`, `{plan_command}`, and any other read-only command

## What you may run

Every command a human runs to answer an escalated issue (`afkconvoy help humans`), and any
read-only command. What you may not do is commit, push, or change the host — delete files, caches or
worktrees, or stop processes. What needs that, say on the issue for the next worker, or open as an
issue.

## End

End with one answer, and not before. Then wake the fleet and stop.

- **Say what is to be done, and hand it back.** The next worker reads the issue's comments:
  ```bash
  gh issue comment {n}<tracker> --body-file <file>
  gh issue edit {n}<tracker> --add-label {ready_label} --remove-label {escalate_label}
  ```
- **Close** it, nothing being left to do: `gh issue close {n}<tracker> --comment <why>`
- **Park** it for new evidence, with a reason. Run in the worktree:
  ```bash
  afkconvoy verdict park --consult --issue {n} --stage consult --text-file <file>
  ```
  It carries `convoy:parked` and leaves the frontier. To bring it back, record the new evidence
  on the issue and restore `{ready_label}`, removing `convoy:parked`.
- **Open the issue you found outside this one**, for the fleet to work, with
  `gh issue create<tracker> --title <title> --body-file <file> --label {ready_label}<labelled>` — record this issue as
  blocked by it (`gh api repos/<issues>/issues/{n}/dependencies/blocked_by -F issue_id=<id>`)
  and hand it back as above, or let it through.
- **Let it through.** It is not yours to answer either: post a comment whose first line is exactly
  this marker, followed by what a human has to decide, and the issue stays with a human:
  ```
  {verdict_marker}
  ```

<wake>
";

/// What the unbound stock consult's brief is filled with beside the issue's
/// fields (`WorkerFields`).
#[derive(Debug, Clone, Copy)]
pub struct StockFields<'a> {
    /// What the human handed the issue was told: the comment of its
    /// escalation.
    pub asked: &'a str,
    pub escalate_label: &'a str,
    /// The lines that look at the fleet.
    pub doctor: &'a str,
    pub plan: &'a str,
}

/// The brief of the stock consult of an unbound repo's escalated issue.
pub fn stock_prompt(fields: &WorkerFields, stock: &StockFields) -> Result<String, String> {
    let number = fields.number.to_string();
    let marker = record::marker(verdict::VERDICT, &[("phase", verdict::Phase::Escalate.word())]);
    let wake = WAKE.replace("<outcomes>", " — your answer is");
    fill(
        &in_tracker(&STOCK_UNBOUND.replace("<wake>", &wake), fields.tracker),
        &[
            ("n", &number),
            ("issue", &fields.tracker.issue(fields.number)),
            ("title", fields.title),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("instruction", crate::stock::INSTRUCTION),
            ("asked", &quoted(stock.asked)),
            ("gate_command", fields.gate_alone_command),
            ("doctor_command", stock.doctor),
            ("plan_command", stock.plan),
            ("ready_label", fields.ready_label),
            ("escalate_label", stock.escalate_label),
            ("verdict_marker", &marker),
            ("wake_command", fields.wake_command),
            // `gh api` fills these in itself.
            ("owner", "{owner}"),
            ("repo", "{repo}"),
        ],
    )
}

/// What a consult is told of the `escalate` it answers.
const ASKED_ESCALATE: &str = "The stage `{stage}` could not decide, and said why:\n\n";

/// What a consult is told of a hand-off it answers.
const ASKED_HAND_OFF: &str =
    "The fleet would have handed this issue to a human, and told them:\n\n";

/// Where the rules a worker works under are, beside the precedents: a line
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
    /// The precedents file, in the fleet's copy of the team.
    pub precedents: &'a str,
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
    /// In its worktree: `afkconvoy gate --train`.
    pub train: &'a str,
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
        Some(_) => GATE_BEFORE_PASS
            .replace("<gate_step>", &GATE_STEP.replace("<g>", "1"))
            .replace("<defect>", &defect(PROOF_MERGED, DEFECT_INTAKE, PARK_STAGE)),
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
        .replace("<precedents_first>", PRECEDENTS_FIRST);
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
            ("precedents", fields.precedents),
            ("team_rules", fields.team_rules.unwrap_or_default()),
            ("verdict_command", fields.verdict_command),
            ("lock_command", fields.lock_command),
            ("gate_command", fields.gate.map_or("", |gate| gate.train)),
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
<rules>- **Precedents:** `{precedents}`
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
    /// The precedents file, in the fleet's copy of the team.
    pub precedents: &'a str,
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
            ("precedents", fields.precedents),
            ("team_rules", fields.team_rules.unwrap_or_default()),
            ("workflows", &workflows),
            ("room", &fields.room.to_string()),
            ("intake_command", fields.intake_command),
            ("lock_command", fields.lock_command),
            ("over_command", fields.over_command),
        ],
    )
}

/// What a worker is told of the precedents before it may ask a human: a
/// settled question is not asked again.
const PRECEDENTS_FIRST: &str = "\
The precedents are rulings a human gave before, kept so that a stage follows them instead of asking
again; none are kept while the file is not there. Read them before you declare `escalate`. Where
one covers what you would ask, follow it and go on. Declare `escalate` only for what none covers,
and say in the text of that verdict that you read the precedents.
";

/// The brief of a worker started to propose a precedent: a fixed form that
/// says what a proposal is and nothing of how to word one — that is the
/// role's to say.
const PROPOSAL: &str = "\
You are an afkconvoy worker, started in your role to propose one **precedent**. You own no issue.
Nobody reads this terminal: never wait for an answer.

- **Ruling:** what a human ruled on issue {issue}, which was escalated to them — {link}
- **Skill to perform:** `{skill}`, of the role you were started in (`{role}`)
- **Team:** `{team}`. Its precedents are the file `{file}` of that repo; as it stands now it
  is `{precedents}`, yours to read only
- **History:** the issue and every comment on it (`gh issue view {n}<tracker> --comments`), read
  from the checkout
- **Checkout:** `{worktree_path}`, the base branch `{base_branch}` of the issue's repo as it stood
  when you started. It is read-only to you: commit nothing in it and push nothing from it
<rules>
## The ruling

{ruling}

## What a proposal is

A precedent is a ruling kept so that later stages follow it instead of escalating. You propose this
one by opening one PR on `{team}`, from the branch `{branch}` of a clone of your own, that appends
one entry to the end of `{file}` and changes nothing else. An entry is one line — the ruling in a
sentence, when it applies, and the link to the ruling:
```
{entry}
```
A human merging the PR is what accepts the precedent; the issue does not wait for it.

If a precedent already kept covers the case, there is nothing to propose: open no PR.

## Hard rules

- At most one PR, on `{team}`, from the branch `{branch}`. The fleet closes one that changes
  anything but `{file}`, or does anything there but append that one line.
- Never merge it, and push nothing but that branch.

## Declare the run over

When the PR is open, or there is nothing to propose, run this once, exactly as written, and then
stop. It tells the fleet that the run is over; until it does, the run holds a slot no issue can be
worked in:
```bash
{over_command}
```
";

/// What the brief of a proposal's worker is filled with.
#[derive(Debug, Clone)]
pub struct ProposalFields<'a> {
    /// The issue the ruling was given, and the comment that is the ruling.
    pub number: u64,
    pub link: &'a str,
    /// What the human ruled.
    pub ruling: &'a str,
    pub role: &'a str,
    pub skill: &'a str,
    /// The team's repo, as `owner/repo`.
    pub team: &'a str,
    /// The team's precedents file, from the root of that repo.
    pub file: &'a str,
    /// The branch of the team's repo the PR is opened from.
    pub branch: &'a str,
    /// The precedents file, in the fleet's copy of the team.
    pub precedents: &'a str,
    /// The team rules, in the fleet's copy of the team: `None` of a team that
    /// has none.
    pub team_rules: Option<&'a str>,
    /// The worker's checkout of the base branch.
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The line the worker declares the run over with.
    pub over_command: &'a str,
    /// The fleet's tracker, which holds the issue the ruling was given on
    /// (`WorkerFields::tracker`); the proposal is the team's repo's.
    pub tracker: &'a Tracker,
}

/// The brief a proposal's worker is given.
pub fn proposal_prompt(fields: &ProposalFields) -> Result<String, String> {
    fill(
        &in_tracker(&with_rules(PROPOSAL, "checkout", fields.team_rules), fields.tracker),
        &[
            ("n", &fields.number.to_string()),
            ("issue", &fields.tracker.issue(fields.number)),
            ("link", fields.link),
            ("ruling", &quoted(fields.ruling)),
            ("role", fields.role),
            ("skill", fields.skill),
            ("team", fields.team),
            ("file", fields.file),
            ("branch", fields.branch),
            ("entry", &entry_form(fields.link)),
            ("precedents", fields.precedents),
            ("team_rules", fields.team_rules.unwrap_or_default()),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
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
    let template = with_rules(CONSULT, "worktree", fields.team_rules)
        .replace("<precedents_first>", PRECEDENTS_FIRST);
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
            ("precedents", fields.precedents),
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
  commit. Then run the command again. When `{base_branch}` may be what is red, and not the train,
  see below first.
- **`conflict`** — what joined, or `{base_branch}`, conflicts with what this worktree holds. The
  merge is **left in progress** here, with `files` unmerged. Resolve every file so both sides' intent
  survives, `git add` it, **commit the merge**, and run the command again. Never rebase, never abort
  the merge.
- **`target_moved`** — `{base_branch}` moved while the gate ran, so the push was refused. Nothing
  landed. Run the command again: it merges the new tip in and gates that.

Keep going until the command says `idle`, or a proven red base branch has you raise a lock below;
nothing counts your attempts.

<defect><cross_repo>
## Hard rules

- Never land anything any other way — no `gh pr merge`, no push to `{base_branch}` of your own.
- Never push to a PR's branch, never comment on, close or reopen a PR or an issue: the command does
  all of that. The one exception is the issue of a defect of `{base_branch}`, above.
- Commit only here, and only to turn a red train green or to finish a merge the command left.
";

/// What the train worker's prompt is filled with.
#[derive(Debug, Clone)]
pub struct TrainFields<'a> {
    pub worktree: &'a str,
    pub base_branch: &'a str,
    /// The line the train worker lands the train with: `afkconvoy land --train`.
    pub land_command: &'a str,
    /// The line the train worker gates another checkout with, in that
    /// checkout: `afkconvoy gate`.
    pub gate_alone_command: &'a str,
    /// The line that raises a fleet lock on the base branch's defect.
    pub lock_command: &'a str,
    /// How an issue the train worker opens for a defect of the base branch
    /// enters the fleet's work.
    pub enters: Enters<'a>,
    /// The fleet's tracker, where that issue is opened.
    pub tracker: &'a Tracker,
}

/// How an issue the train worker opens for a defect enters the fleet's work.
#[derive(Debug, Clone, Copy)]
pub enum Enters<'a> {
    /// In an unbound repo: with the label that makes an issue dispatchable.
    Ready(&'a str),
    /// In a bound repo, where only a flow label dispatches: taken in, with
    /// the line `afkconvoy intake --train`, to one of the team's workflows —
    /// the first, unless another fits it better.
    Intake { command: &'a str, workflows: &'a [String] },
}

/// How the train worker of a bound repo puts an issue it opens for a defect
/// before the fleet: it takes it in, by default to the team's first workflow.
const DEFECT_TRAIN_INTAKE: &str =
    "Put no label of a workflow on it yourself<only_repo_label>: take it in with
   `{intake_command} --issue <n> --workflow <workflow>`, to the team's first workflow,
   `{first_workflow}`, unless another of its workflows fits the defect clearly better — the team
   has {workflows}. The fleet works it like any other issue.";

/// What the train worker does once a defect of the base branch is recorded:
/// it has no issue to park.
const PARK_TRAIN: &str =
    "**You have no issue to park.** Once you raised the lock, stop: the fix's issue is worked
   first. Run the command again once the fix's PR has joined the train or the lock has closed.
   Until then stop without another gate, even if nudged. If the base branch was green, go on as
   for any `gate_red`: fix the train, commit, and run the command again.";

/// The prompt the train worker is given: started on it, and pointed at it
/// again whenever more joined.
pub fn train_prompt(fields: &TrainFields) -> Result<String, String> {
    let proof = "`origin/{base_branch}`, without the train,";
    let (open, ready_label, intake_command, named, first) = match fields.enters {
        Enters::Ready(label) => (DEFECT_READY, label, "", vec![], ""),
        Enters::Intake { command, workflows } => {
            let Some(first) = workflows.first() else {
                return Err("the team has no workflow to take a defect's issue in to".to_string());
            };
            let named = workflows.iter().map(|name| format!("`{name}`")).collect();
            (DEFECT_TRAIN_INTAKE, "", command, named, first.as_str())
        }
    };
    fill(
        &in_tracker(&TRAIN.replace("<defect>", &defect(proof, open, PARK_TRAIN)), fields.tracker),
        &[
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("land_command", fields.land_command),
            ("gate_alone_command", fields.gate_alone_command),
            ("lock_command", fields.lock_command),
            ("ready_label", ready_label),
            ("intake_command", intake_command),
            ("workflows", &named.join(", ")),
            ("first_workflow", first),
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
        Variant::TakeUp { pr } => {
            let outcomes = " — your PR is on the landing train, or your comment is posted";
            let taker = TAKER.replace("<defect>", &defect(PROOF_MERGED, DEFECT_READY, PARK_TAKER));
            (steps(&taker, "3", "4", outcomes), pr.to_string())
        }
    };
    fill(
        &in_tracker(&template, fields.tracker),
        &[
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

/// The worker prompt with a variant's opening and first step in it.
fn briefed(opening: &str, step1: &str) -> String {
    let outcomes = " — your PR is on the landing train, or your comment is posted";
    let worker = WORKER
        .replace("<opening>", opening)
        .replace("<step1>", step1)
        .replace("<defect>", &defect(PROOF_MERGED, DEFECT_READY, PARK_WORKER));
    steps(&worker, "3", "5", outcomes)
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
            gate_command: "/opt/bin/afkconvoy gate --train",
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            ready_label: "agent-ready",
            land_command: "/opt/bin/afkconvoy land --issue 7",
            wake_command: "/opt/bin/afkconvoy wake",
        }
    }

    #[test]
    fn every_issue_command_of_a_brief_names_the_tracker_and_no_pr_command_does() {
        let tracker = Tracker::Other {
            tracker: crate::config::RepoName::read("acme/tracker").unwrap(),
            repo: crate::config::RepoName::read("acme/dev").unwrap(),
        };
        let fields = WorkerFields { tracker: &tracker, ..fields() };
        let stock = StockFields {
            asked: "Its worker went silent.",
            escalate_label: "ready-for-human",
            doctor: "/opt/bin/afkconvoy doctor",
            plan: "/opt/bin/afkconvoy plan",
        };
        let train = train_prompt(&TrainFields {
            worktree: "/wt/convoy-train",
            base_branch: "develop",
            land_command: "/opt/bin/afkconvoy land --train",
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            enters: Enters::Ready("agent-ready"),
            tracker: &tracker,
        })
        .unwrap();
        let mut briefs: Vec<String> = [
            Variant::Fresh,
            Variant::Continue,
            Variant::Join { pr: 70 },
            Variant::TakeUp { pr: 70 },
        ]
        .into_iter()
        .map(|variant| worker_prompt(&fields, variant).unwrap())
        .collect();
        briefs.extend([stock_prompt(&fields, &stock).unwrap(), train]);
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
        for brief in [fresh, &briefs[1], &briefs[3], &briefs[5]] {
            assert!(brief.contains("open it on the tracker\n`acme/tracker`"), "{brief}");
            assert!(brief.contains("that repo's `repo:<owner>/<repo>` label"), "{brief}");
            assert!(brief.contains("run `afkconvoy flows -C <target-checkout>`"), "{brief}");
            assert!(brief.contains("omit the line's flow label while opening"), "{brief}");
            assert_eq!(brief.matches("## Cross-repo blockers").count(), 1, "{brief}");
        }
        assert!(fresh.contains("**Your issue:** acme/tracker#7 — Add {branch} support"));
        assert!(fresh.contains("\n   Closes acme/tracker#7\""), "{fresh}");
        // An issue a worker opens is the repo's in the tracker.
        for brief in [fresh, &briefs[5]] {
            assert!(
                brief.contains("`gh issue create --repo acme/tracker`): the symptom")
                    && brief.contains(
                        "(`--label agent-ready`), and the repo label `repo:acme/dev` (`--label \
                     repo:acme/dev`), so"
                    ),
                "{brief}"
            );
        }
        assert!(briefs[4].contains(
            "`gh issue create --repo acme/tracker --title <title> --body-file <file> --label \
             agent-ready --label repo:acme/dev`"
        ));
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
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "/opt/bin/afkconvoy intake",
            split: true,
            workflows: &workflows,
            gate: Some(Gate { train: "afkconvoy gate --train", alone: "afkconvoy gate" }),
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
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules: None,
            workflows: &["feature"],
            room: 2,
            intake_command: "/opt/bin/afkconvoy intake",
            lock_command: "/opt/bin/afkconvoy lock",
            over_command: "/opt/bin/afkconvoy over --run sweep",
        };
        let proposal = ProposalFields {
            number: 7,
            tracker: &tracker,
            link: "https://github.com/acme/tracker/issues/7#issuecomment-2001",
            ruling: "Keep the v1 schema.",
            role: "clerk",
            skill: "propose",
            team: "acme/team",
            file: "precedents.md",
            branch: "convoy-precedent-7-c2001",
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules: None,
            worktree: "/wt/convoy-precedent-7-c2001",
            base_branch: "develop",
            over_command: "/opt/bin/afkconvoy over --run precedent-7-c2001",
        };
        let landing = Landing { role: "dev", skill: "land" };
        let train = TrainFields {
            worktree: "/wt/convoy-train",
            base_branch: "develop",
            land_command: "/opt/bin/afkconvoy land --train",
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            enters: Enters::Intake {
                command: "/opt/bin/afkconvoy intake --train",
                workflows: &workflows,
            },
            tracker: &tracker,
        };
        let worker = WorkerFields { tracker: &tracker, ..fields() };
        let briefs = [
            ("stage", stage_prompt(&stage).unwrap()),
            ("parked", stage_prompt(&StageFields { records: &waited, ..stage.clone() }).unwrap()),
            ("consult", consult_prompt(&consult, &consulting).unwrap()),
            ("patrol", patrol_prompt(&patrol).unwrap()),
            ("proposal", proposal_prompt(&proposal).unwrap()),
            ("landing", landing_prompt(&worker, 70, landing).unwrap()),
            ("train", train_prompt(&train).unwrap()),
        ];
        for (_, brief) in &briefs {
            names_the_tracker(brief);
        }
        let [stage, parked, consult, patrol, proposal, ..] = &briefs.map(|(_, brief)| brief);
        for brief in [stage, parked, consult] {
            assert!(brief.contains("- **Issue:** acme/tracker#7 — Add support\n"), "{brief}");
        }
        assert!(stage.contains("(`gh issue create --repo acme/tracker --label repo:acme/dev`)"));
        assert!(stage.contains("open it on the tracker\n`acme/tracker`"), "{stage}");
        assert!(
            stage.contains(
                "afkconvoy intake -C <target-checkout> --issue <n> --workflow <workflow>"
            ),
            "{stage}"
        );
        assert!(stage.contains(
            "Put no label of a workflow on it yourself, only the repo label `repo:acme/dev` \
             (`--label repo:acme/dev`): take it in"
        ));
        assert!(parked.contains("(acme/tracker#11, acme/tracker#12)"), "{parked}");
        assert!(consult.contains("    gh issue close 7 --repo acme/tracker --comment <why>\n"));
        assert!(patrol.contains(
            "open an\nissue for it (`gh issue create --repo acme/tracker --label repo:acme/dev`)"
        ));
        assert!(proposal.contains("ruled on issue acme/tracker#7, which"), "{proposal}");
        // The proposal is the team's: its PR names its repo, as it always did.
        assert!(proposal.contains("one PR on `acme/team`"), "{proposal}");
    }

    #[test]
    fn the_unbound_stock_consult_is_told_what_a_human_would_be_and_how_a_human_answers() {
        let stock = StockFields {
            asked: "**afkconvoy: escalated to a human.**\n\nIts worker went silent.",
            escalate_label: "ready-for-human",
            doctor: "/opt/bin/afkconvoy doctor",
            plan: "/opt/bin/afkconvoy plan",
        };
        let brief = stock_prompt(&fields(), &stock).unwrap();
        for said in [
            "started as the **stock consult**",
            crate::stock::INSTRUCTION,
            "told them:\n\n> **afkconvoy: escalated to a human.**\n> \n> Its worker went silent.\n",
            "gh issue edit 7 --add-label agent-ready --remove-label ready-for-human\n",
            "gh issue close 7 --comment <why>",
            "--label agent-ready",
            "<!--convoy:verdict phase=escalate-->",
            "/opt/bin/afkconvoy wake\n",
            "never commit and never push",
        ] {
            assert!(brief.contains(said), "{said}\n{brief}");
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
    fn a_worker_taking_up_a_pr_is_told_its_pr_exists_and_to_merge_the_base_gate_and_land() {
        let prompt = worker_prompt(&fields(), Variant::TakeUp { pr: 70 }).unwrap();
        assert!(prompt.contains("PR #70 is open for it"));
        assert!(prompt.contains("The PR already exists"));
        assert!(prompt.contains("**Your PR:** #70, from the branch `tester/issue-7-add`"));
        assert!(prompt.contains("\n1. **Read why it stopped.** `gh issue view 7 --comments`"));
        assert!(prompt.contains(
            "\n2. **Merge the base branch.** `git fetch origin && git merge origin/develop`"
        ));
        assert!(prompt.contains("\n3. **Gate.**") && prompt.contains("\n4. **Join the landing"));
        assert!(prompt.contains("   /opt/bin/afkconvoy land --issue 7\n"));
        assert!(prompt.contains("go back to step 3, and run this again"));
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
            tracker: &Tracker::Own,
            title: "Add {branch} support",
            flow: "feature",
            stage: "build",
            role: "dev",
            skill: "implement",
            branch: "tester/issue-7-add",
            worktree: "/wt/issue-7",
            base_branch: "develop",
            precedents: "/copies/acme/team/def/precedents.md",
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
        for said in [
            "- **Issue:** #7 — Add {branch} support\n",
            "- **Stage:** `build`, of the workflow `feature`\n",
            "- **Skill to perform:** `implement`, of the role you were started in (`dev`)\n",
            "- **Worktree:** `/wt/issue-7`, on the branch `tester/issue-7-add` (the base is `develop`)",
            "- **History:** the issue and every comment on it (`gh issue view 7 --comments`)",
            "- **Precedents:** `/copies/acme/team/def/precedents.md`\n",
            "Read them before you declare `escalate`.",
            "say in the text of that verdict that you read the precedents.",
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
            precedents: "/copies/acme/team/def/precedents.md",
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
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "/opt/bin/afkconvoy intake",
            split: true,
            workflows: &["feature".to_string()],
            gate: Some(Gate {
                train: "cd '/wt/issue-7' && /opt/bin/afkconvoy gate --train",
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
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            enters: Enters::Ready("agent-ready"),
            tracker: &Tracker::Own,
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
    fn every_worker_that_runs_the_gate_opens_an_issue_for_a_red_gate_that_is_not_its_own() {
        let stage = StageFields {
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
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules: None,
            records: &[],
            verdict_command: "/opt/bin/afkconvoy verdict --issue 7 --stage build",
            lock_command: "/opt/bin/afkconvoy lock",
            intake_command: "cd '/wt/issue-7' && /opt/bin/afkconvoy intake",
            split: true,
            workflows: &["feature".to_string(), "fix".to_string()],
            gate: Some(Gate {
                train: "cd '/wt/issue-7' && /opt/bin/afkconvoy gate --train",
                alone: "/opt/bin/afkconvoy gate",
            }),
        };
        let train = train_prompt(&TrainFields {
            worktree: "/wt/convoy-train",
            base_branch: "develop",
            land_command: "/opt/bin/afkconvoy land --train",
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            enters: Enters::Ready("agent-ready"),
            tracker: &Tracker::Own,
        })
        .unwrap();
        let fresh = worker_prompt(&fields(), Variant::Fresh).unwrap();
        let continuing = worker_prompt(&fields(), Variant::Continue).unwrap();
        let taker = worker_prompt(&fields(), Variant::TakeUp { pr: 70 }).unwrap();
        let staged = stage_prompt(&stage).unwrap();
        let merged = "The proof is that\nthe commit your gate merged in — its `merged`: the \
                      landing train's tip, or\n`origin/develop` — is red the same way on its own.";
        let base = "The proof is that\n`origin/develop`, without the train, is red the same way \
                    on its own.";
        let briefs = [
            (&fresh, merged),
            (&continuing, merged),
            (&taker, merged),
            (&staged, merged),
            (&train, base),
        ];
        for (brief, proof) in briefs {
            for said in [
                "## A red gate that is not yours\n",
                proof,
                "```bash\ngit worktree add --detach <a new directory> <that commit>\n\
                 cd <that directory> && /opt/bin/afkconvoy gate\n```\n",
                "A red gate you have not shown this way stays yours.",
                // Look before opening: a duplicate gets the evidence instead.
                "search the open issues\n   for one that already records this defect \
                 (`gh issue list --state open --search",
                "comment your evidence there — another occurrence, and when — and\n   open none.",
                // What the issue holds.
                "the symptom; how to\n   reproduce it on `develop`; what the logs show; what is \
                 guessed and what is verified; and\n   acceptance criteria that require the cause \
                 to be found with evidence, and a test that reproduces\n   it.",
                "Name no blocker unless it has a real one.",
                "**Raise the fleet lock for a red base branch.**",
                "gate `origin/develop` alone in a clean checkout too",
                "fix's issue — existing or new — is dispatchable",
                "`/opt/bin/afkconvoy lock --issue <n>`",
                "A defect confined to the train raises no lock.",
            ] {
                assert!(brief.contains(said), "{said}\n{brief}");
            }
            assert!(!brief.contains("<proof>") && !brief.contains("<defect>"), "{brief}");
        }
        // An unbound repo's worker and the train worker put the ready label on
        // it; a stage takes it in to a workflow.
        for brief in [&fresh, &continuing, &taker, &train] {
            assert!(brief.contains("Put the label `agent-ready` on it (`--label agent-ready`)"));
        }
        assert!(
            staged.contains(
                "Put no label of a workflow on it yourself: take it in to the workflow\n   that \
                 fits — the team has `feature`, `fix` — with\n   `cd '/wt/issue-7' && \
                 /opt/bin/afkconvoy intake --issue <n> --workflow <workflow>`."
            ),
            "{staged}"
        );
        assert!(!staged.contains("--label"), "{staged}");
        // The lock is raised before parking, for existing and new defects alike.
        for brief in [&fresh, &continuing, &taker, &staged] {
            assert!(
                brief.find("**Raise the fleet lock").unwrap()
                    < brief.find("**Then park your issue").unwrap()
            );
        }
        // Each parks its issue on it; the train worker has none and stops for the fix.
        for brief in [&fresh, &continuing] {
            assert!(brief.contains(
                "**Then park your issue on it.** Push your branch (`git push origin HEAD`), open \
                 no PR,\n   and post the `blocked` verdict (below) with that issue"
            ));
            assert!(brief.contains("commented on — in `blocked_by=`. When it closes, your issue"));
            assert!(brief.contains(
                "Open one only for an uncovered defect you discovered (above), or for another\n  \
                 repo of this host"
            ));
        }
        assert!(taker.contains("verdict comment (below) with `phase=blocked` and that issue"));
        assert!(
            taker.contains("or, for\na defect outside your issue (above), with `phase=blocked`")
        );
        assert!(staged.contains(
            "then declare\n   `blocked` with the command below, with that issue — the one you \
             opened, or the one you commented\n   on — in `--blocked-by`. When it closes this stage \
             runs again"
        ));
        assert!(
            train.contains("When `develop` may be what is red, and not the train,\n  see below")
        );
        assert!(train.contains("**You have no issue to park.** Once you raised the lock, stop"));
        assert!(train.contains("once the fix's PR has joined the train or the lock has closed"));
        assert!(train.contains("Until then stop without another gate, even if nudged."));
        assert!(train.contains("The one exception is the issue of a defect of `develop`, above."));
        // Said in one place: the section is the same text wherever it is given.
        let section = |brief: &str| {
            let from = brief.find("A red gate you have not shown").unwrap();
            brief[from..brief.find("a test that reproduces").unwrap()].to_string()
        };
        for brief in [&continuing, &taker, &staged, &train] {
            assert_eq!(section(brief), section(&fresh));
        }
        // A worker that only joins has no verdict to park with, and a stage
        // that writes nothing runs no gate.
        let joiner = worker_prompt(&fields(), Variant::Join { pr: 70 }).unwrap();
        let reads = stage_prompt(&StageFields { gate: None, ..stage }).unwrap();
        for brief in [joiner, reads] {
            assert!(!brief.contains("A red gate that is not yours"), "{brief}");
        }
    }

    #[test]
    fn in_a_bound_repo_the_train_worker_takes_a_defect_s_issue_in_the_first_workflow_by_default() {
        let workflows = ["feature".to_string(), "fix".to_string()];
        let fields = TrainFields {
            worktree: "/wt/convoy-train",
            base_branch: "develop",
            land_command: "/opt/bin/afkconvoy land --train",
            gate_alone_command: "/opt/bin/afkconvoy gate",
            lock_command: "/opt/bin/afkconvoy lock",
            enters: Enters::Intake {
                command: "/opt/bin/afkconvoy intake --train",
                workflows: &workflows,
            },
            tracker: &Tracker::Own,
        };
        let bound = train_prompt(&fields).unwrap();
        assert!(bound.contains("`/opt/bin/afkconvoy lock --issue <n>`"), "{bound}");
        assert!(
            bound.contains(
                "Put no label of a workflow on it yourself: take it in with\n   \
                 `/opt/bin/afkconvoy intake --train --issue <n> --workflow <workflow>`, to the \
                 team's first workflow,\n   `feature`, unless another of its workflows fits the \
                 defect clearly better — the team\n   has `feature`, `fix`. The fleet works it like \
                 any other issue. Name no blocker"
            ),
            "{bound}"
        );
        assert!(!bound.contains("--label") && !bound.contains("agent-ready"), "{bound}");
        // Only how the issue enters differs: the rest is the unbound repo's.
        let unbound =
            train_prompt(&TrainFields { enters: Enters::Ready("agent-ready"), ..fields }).unwrap();
        let open = |brief: &str| {
            let from = brief.find("   it. ").unwrap() + "   it. ".len();
            let to = brief.find(" Name no blocker").unwrap();
            format!("{}{}", &brief[..from], &brief[to..])
        };
        assert_eq!(open(&bound), open(&unbound));
        assert!(unbound.contains("Put the label `agent-ready` on it (`--label agent-ready`)"));
        // A team with no workflow has nothing to take it in to.
        let none = TrainFields {
            enters: Enters::Intake { command: "/opt/bin/afkconvoy intake --train", workflows: &[] },
            ..fields
        };
        assert!(train_prompt(&none).unwrap_err().contains("no workflow"));
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
    fn a_proposal_s_brief_says_what_a_proposal_is_and_nothing_of_how_to_word_one() {
        let link = "https://github.com/acme/app/issues/7#issuecomment-2001";
        let fields = ProposalFields {
            number: 7,
            tracker: &Tracker::Own,
            link,
            ruling: "Keep the v1 schema.\nMigrate later.",
            role: "clerk",
            skill: "propose",
            file: "precedents.md",
            team: "acme/team",
            branch: "convoy-precedent-7-c2001",
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules: None,
            worktree: "/wt/convoy-precedent-7-c2001",
            base_branch: "develop",
            over_command: "/opt/bin/afkconvoy over --run precedent-7-c2001",
        };
        let prompt = proposal_prompt(&fields).unwrap();
        for said in [
            "- **Ruling:** what a human ruled on issue #7, which was escalated to them — https://github.com/acme/app/issues/7#issuecomment-2001\n",
            "- **Skill to perform:** `propose`, of the role you were started in (`clerk`)\n",
            "`/copies/acme/team/def/precedents.md`, yours to read only\n",
            "> Keep the v1 schema.\n> Migrate later.\n",
            "opening one PR on `acme/team`, from the branch `convoy-precedent-7-c2001`",
            "- <the ruling, in a sentence> **When:** <when it applies> (https://github.com/acme/app/issues/7#issuecomment-2001)\n",
            "- Never merge it, and push nothing but that branch.\n",
        ] {
            assert!(prompt.contains(said), "{said}\n{prompt}");
        }
        assert!(!prompt.contains('{'), "every field is filled: {prompt}");
        // It declares no verdict — a proposal is at no stage — and ends with
        // the line that declares the run over.
        assert!(!prompt.contains("afkconvoy verdict"), "{prompt}");
        let last = "```bash\n/opt/bin/afkconvoy over --run precedent-7-c2001\n```\n";
        assert!(prompt.ends_with(last), "{prompt}");
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
            precedents: "/copies/acme/team/def/precedents.md",
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
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules,
            workflows: &["feature"],
            room: 2,
            intake_command: "/opt/bin/afkconvoy intake",
            lock_command: "/opt/bin/afkconvoy lock",
            over_command: "/opt/bin/afkconvoy over --run sweep",
        };
        let proposal = |team_rules| ProposalFields {
            number: 7,
            tracker: &Tracker::Own,
            link: "https://github.com/acme/app/issues/7#issuecomment-2001",
            ruling: "Keep the v1 schema.",
            role: "clerk",
            skill: "propose",
            team: "acme/team",
            file: "precedents.md",
            branch: "convoy-precedent-7-c2001",
            precedents: "/copies/acme/team/def/precedents.md",
            team_rules,
            worktree: "/wt/convoy-precedent-7-c2001",
            base_branch: "develop",
            over_command: "/opt/bin/afkconvoy over --run precedent-7-c2001",
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
                ("checkout", proposal_prompt(&proposal(team_rules)).unwrap()),
            ]
        };
        for (place, brief) in briefs(Some(TEAM)) {
            let lines = format!(
                "- **The repo's rules:** the rules file at the top of the {place}, where it has \
                 one\n- **Team rules:** `{TEAM}`\n- **Your role's rules:** the rules file of the \
                 directory you were started in\n"
            );
            assert!(brief.contains(&lines), "{brief}");
            assert!(brief.contains("`/copies/acme/team/def/precedents.md`"), "{brief}");
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
