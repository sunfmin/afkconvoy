//! The worker prompt: a template, filled by code. No worker is ever started on
//! a prompt with a placeholder left in it.

use crate::position::Declared;
use crate::recovery::Variant;
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
   go back to step <g>, and run this again.";

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

/// What a stage that may commit is told of the gate: a PR it wrote is handed
/// on green (docs/adr/0010).
const GATE_BEFORE_PASS: &str = "
## The gate, before a `pass`

Opening a PR declares nothing, and no stage is handed red code. If this stage opened the issue's PR
or pushed to its branch, turn the gate green before you declare `pass`:

<gate_step>
";

/// One stage record that read as a verdict, as a worker's brief names it.
#[derive(Debug, Clone)]
pub struct Said<'a> {
    /// The stage that declared it.
    pub stage: &'a str,
    pub declared: Declared,
    /// What the stage produced: of a `reject`, the reason.
    pub text: &'a str,
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
    /// The verdicts declared on the issue so far, oldest first: the worker's
    /// history. When the last is a `reject`, the stage runs again for it.
    pub records: &'a [Said<'a>],
    /// The line the worker declares its verdict with.
    pub verdict_command: &'a str,
    /// The line the worker runs the gate with, when its stage may commit: a
    /// stage that writes nothing has no PR of its own to turn green.
    pub gate_command: Option<&'a str>,
}

/// `text` as a quotation: nothing in it reads as part of the brief.
pub(crate) fn quoted(text: &str) -> String {
    let lines: Vec<String> = text.trim().lines().map(|line| format!("> {line}")).collect();
    lines.join("\n")
}

/// The brief a worker started in its role is given.
pub fn stage_prompt(fields: &StageFields) -> Result<String, String> {
    let number = fields.number.to_string();
    let mut records = match fields.records {
        [] => "None yet: no stage of this issue has declared a verdict.".to_string(),
        _ => "What the stages before this one declared, oldest first — each is a comment on \
              the issue, with what the stage produced:\n"
            .to_string(),
    };
    for said in fields.records {
        let (stage, verdict) = (said.stage, said.declared.word());
        records.push_str(&format!("\n- `{stage}` declared `{verdict}` — {}", said.url));
    }
    let rejection = fields.records.last().filter(|last| last.declared == Declared::Reject);
    let reason = rejection.map(|rejection| quoted(rejection.text)).unwrap_or_default();
    let gate = match fields.gate_command {
        Some(_) => GATE_BEFORE_PASS.replace("<gate_step>", &GATE_STEP.replace("<g>", "1")),
        None => String::new(),
    };
    let template = STAGE
        .replace("<rework>", if rejection.is_some() { REWORK } else { "" })
        .replace("<gate>", &gate);
    fill(
        &template,
        &[
            ("records", &records),
            ("rejecter", rejection.map_or("", |rejection| rejection.stage)),
            ("reason", &reason),
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
        let said = |stage, declared, text, url| Said { stage, declared, text, url };
        let passed = [said("build", Declared::Pass, "built", "https://gh/c/1")];
        let reason = "## Not yet\n\nThe {parser} drops a `-->`.";
        let rejected =
            [&passed[..], &[said("review", Declared::Reject, reason, "https://gh/c/2")]].concat();
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
                 ## Declare a verdict\n"
            ),
            "{again}"
        );
        // A stage that follows a pass runs for the first time: no rework.
        let on = stage_prompt(&StageFields { records: &passed, ..fields }).unwrap();
        assert!(on.contains("- `build` declared `pass`") && !on.contains("## Rework"), "{on}");
        assert!(stage_nudge_line("/b.md").contains("task brief (/b.md)"));
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
