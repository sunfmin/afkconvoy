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
    pub gate_command: &'a str,
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
3. **Gate.** Commit everything, then run this in your worktree and fix the code until it passes:
   ```bash
   {gate_command}
   ```
4. **Open a PR** against `{base_branch}` whose body contains the line `Closes #{n}`:
   ```bash
   git push origin HEAD && gh pr create --base {base_branch} --head {branch} --title \"<title>\" --body \"<what changed>

   Closes #{n}\"
   ```
   Do not merge it.

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

- End with a PR or that comment — never just stop.
- Never edit files outside your worktree, and never push to `{base_branch}`.
";

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
            gate_command: "make test VAR=${x}",
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
        assert!(prompt.contains("make test VAR=${x}"));
        assert!(prompt.contains("gh pr create --base develop --head tester/issue-7-add"));
        assert!(prompt.contains("Closes #7"));
        assert!(prompt.contains(&verdict::marker_format()));
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
