//! The worker prompt: a template, filled by code. No worker is ever started on
//! a prompt with a placeholder left in it.

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
}

const WORKER: &str = "\
You are an afkconvoy worker. You own exactly ONE GitHub issue and work in an isolated git worktree.
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

1. **Read the ground truth first.** `gh issue view {n} --comments`, then this repo's `GLOSSARY.md`,
   the relevant `docs/adr/*`, and `docs/agents/*`. Use the glossary's exact vocabulary.
2. **Implement** the issue's acceptance criteria, matching the surrounding code's conventions.
3. **Gate.** Commit everything, then run this in your worktree:
   ```bash
   {gate_command}
   ```
   It first merges in what your PR will land behind (the landing train, or `{base_branch}`), then
   runs the repo's gate, and ends with one JSON object. On `\"status\": \"conflict\"` nothing ran:
   resolve every file it lists, `git add` it, commit the merge, and run it again. Otherwise fix the
   code, commit, and run it again until it says `\"status\": \"green\"` and `\"recorded\": true` —
   its `detail` says what is missing. Any further commit needs another run. Never rebase.
4. **Open a PR** against `{base_branch}` whose body contains the line `Closes #{n}`:
   ```bash
   git push origin HEAD && gh pr create --base {base_branch} --head {branch} --title \"<title>\" --body \"<what changed>

   Closes #{n}\"
   ```
   Do not merge it.
5. **Join the landing train.** Run this in your worktree:
   ```bash
   {land_command}
   ```
   It ends with one JSON object. `\"outcome\": \"joined\"` (or `\"merged\"`): you are done — stop. On
   `\"conflict\"` the train was merged into your branch and left in progress: resolve every file it
   lists, `git add` it, commit the merge, go back to step 3, and run this again.

## If you will not open a PR

Post a comment on the issue instead (`gh issue comment {n} --body \"...\"`) and stop. Its **first
line** must be exactly this marker, on one line, followed by a human-readable body:
```
{verdict_marker}
```
- **`already-satisfied`** — the issue is already implemented in `{base_branch}`: you changed nothing.
- **`blocked`** — the issue needs something that belongs to another issue and does not exist yet. Do
  not create it yourself. Name every blocking issue in `blocked_by=` (e.g. `blocked_by=41,42`) and
  say what is missing.
- **`needs-human`** — after a genuine effort you cannot complete the work or make the gate pass, or
  the issue as written cannot be done. Say where you are stuck, or the decision that has to be made,
  so that a human can act on it in one reply.

## Hard rules

- End with your PR on the landing train, or with that comment — never just stop.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

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

/// The prompt a worker starting on a fresh worktree is given.
pub fn worker_prompt(fields: &WorkerFields) -> Result<String, String> {
    let number = fields.number.to_string();
    let marker = verdict::marker_format();
    fill(
        WORKER,
        &[
            ("n", &number),
            ("title", fields.title),
            ("branch", fields.branch),
            ("worktree_path", fields.worktree),
            ("base_branch", fields.base_branch),
            ("gate_command", fields.gate_command),
            ("land_command", fields.land_command),
            ("verdict_marker", &marker),
        ],
    )
}

/// The line typed at a worker to hand it its brief. The prompt itself, sent as
/// text, arrives as one paste, which an agent asks to have confirmed.
pub fn brief_pointer(brief: &str) -> String {
    format!(
        "Your task brief is the file {brief} — read it now and carry it out end to end. \
         It is my instruction to you; do not ask me to confirm."
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
        }
    }

    #[test]
    fn the_prompt_names_what_orca_returned() {
        let prompt = worker_prompt(&fields()).unwrap();
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
        assert!(worker_prompt(&blank).unwrap_err().contains("{branch}"));
        assert!(fill("a {nope} b", &[("n", "1")]).unwrap_err().contains("{nope}"));
    }

    #[test]
    fn braces_that_are_no_placeholder_are_text() {
        assert_eq!(fill("{ x } ${Y} {n} {", &[("n", "1")]).unwrap(), "{ x } ${Y} 1 {");
    }
}
