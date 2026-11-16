//! A PR whose landing turns went red twice in a row is handed to a new worker
//! in a new worktree (docs/adr/0088): the worker that retried it in its own
//! worktree is ended, confirmed stopped, what it left is saved as a patch,
//! its worktree is removed, and a worker of the issue starts in a new one on
//! the PR's branch merged up to the base tip, briefed with both red runs.
//! The claim, the PR and its branch stay; no stage record is written, so
//! every rework count of the issue stays as it was.

use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::closing_pr;
use afkconvoy_core::prompt::{FreshLook, RedTurn as Told, fresh_look};
use afkconvoy_core::verdict::VERDICT;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::claims::Me;
use crate::dispatch::{Agent, StartFrom, dispatch_continuing};
use crate::fleet::Fleet;
use crate::found::Found;
use crate::git::Repo;
use crate::orca::Worktree;
use crate::remote::Fresh;
use crate::turn_answer::RedTurn;

/// What the worker handed a PR after its red turns is told of the worker
/// before it (`prompt::fresh_look`).
pub(crate) struct Handed {
    pub pr: u64,
    pub reds: Vec<RedTurn>,
    pub verdict: Option<String>,
    pub said: Vec<String>,
    pub patch: Option<PathBuf>,
}

impl Handed {
    /// The section its brief ends with: the base branch its branch was
    /// merged up to, and the files that merge left conflicted.
    pub fn brief(&self, base_branch: &str, conflicts: &[String]) -> String {
        let reds: Vec<Told> = self
            .reds
            .iter()
            .map(|red| Told { failing: &red.failing, gate_output: red.gate_output.as_deref() })
            .collect();
        let patch = self.patch.as_ref().map(|patch| patch.display().to_string());
        fresh_look(&FreshLook {
            pr: self.pr,
            base_branch,
            reds: &reds,
            verdict: self.verdict.as_deref(),
            said: &self.said,
            patch: patch.as_deref(),
            conflicts,
        })
    }
}

/// What the status board of a claim says of its PR's hand-on, kept in the
/// checkout under the claim it was made under: the board is rendered every
/// tick and never read back, and this spares it a read of the PR.
#[derive(Serialize, Deserialize)]
struct Said {
    claim: String,
    line: String,
}

fn said_file(fleet: &Fleet, issue: u64) -> Result<PathBuf> {
    Ok(fleet.repo.common_dir()?.join("convoy-fresh-looks").join(format!("{issue}.json")))
}

/// What the board of the claim `claim` on `issue` says of its PR's last
/// hand-on to a new worker, where it had one under that claim.
pub(crate) fn board_line(fleet: &Fleet, issue: u64, claim: &str) -> Option<String> {
    let bytes = std::fs::read(said_file(fleet, issue).ok()?).ok()?;
    let said: Said = serde_json::from_slice(&bytes).ok()?;
    (said.claim == claim).then_some(said.line)
}

/// The line that names a hand-on: the PR, the two red turns, and the new
/// worktree.
pub(crate) fn said(issue: u64, pr: u64, reds: &[RedTurn], worktree: &str) -> String {
    let runs: Vec<String> = reds
        .iter()
        .map(|red| match red.failing.as_slice() {
            [] => "no failing test named".to_string(),
            failing => format!("failing {}", failing.join(", ")),
        })
        .collect();
    format!(
        "handed #{issue}'s PR #{pr} to a new worker in a new worktree {worktree} after {} red \
         turns in a row ({})",
        reds.len(),
        runs.join("; ")
    )
}

/// The text of the last verdict declared on `issue`, where one was.
fn last_verdict(fleet: &Fleet, issue: u64) -> Result<Option<String>> {
    let comments = fleet.gh.comments(issue)?;
    let declared = comments
        .iter()
        .rev()
        .find(|c| afkconvoy_core::record::read_marker(VERDICT, &c.body).is_some());
    Ok(declared.map(|c| c.body.clone()))
}

/// Saves what `worktree` holds that the remote's `branch` does not — its
/// uncommitted and untracked files that git does not ignore, and its commits
/// not pushed — as one binary patch against the branch's remote tip, into
/// `dir` → the patch, `None` where there was nothing to save. Its own index
/// is left as it was: a temporary one stages the files.
fn salvage(
    worktree: &Repo,
    against: &str,
    dir: &Path,
    (issue, now): (u64, u64),
) -> Result<Option<PathBuf>> {
    let index = worktree.git_dir()?.join("convoy-salvage.index");
    let _ = std::fs::remove_file(&index);
    let index_text = index.to_string_lossy().into_owned();
    let env = [("GIT_INDEX_FILE", index_text.as_str())];
    let staged = (|| -> Result<String> {
        for args in [&["read-tree", "HEAD"][..], &["add", "-A", "."][..]] {
            let ran = worktree.try_git_with(args, &env)?;
            if !ran.ok {
                bail!("git {} failed: {}", args.join(" "), ran.stderr.trim());
            }
        }
        let ran = worktree.try_git_with(&["diff", "--cached", "--binary", against], &env)?;
        if !ran.ok {
            bail!("git diff against {against} failed: {}", ran.stderr.trim());
        }
        Ok(ran.stdout)
    })();
    let _ = std::fs::remove_file(&index);
    let diff = staged.with_context(|| {
        format!("cannot save what the worktree {} holds", worktree.dir.display())
    })?;
    if diff.trim().is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let patch = dir.join(format!("{issue}-{now}.patch"));
    std::fs::write(&patch, format!("{}\n", diff.trim_end_matches('\n')))
        .with_context(|| format!("cannot write {}", patch.display()))?;
    Ok(Some(patch))
}

/// Hands the PR of my landing claim on `issue`, whose turns in `worktree`
/// went red `reds` in a row, to a new worker in a new worktree: its worker
/// ended — no second writer is started while it cannot be confirmed
/// stopped, which is an error the tick reports, the worktree kept — what it
/// left saved as a patch, the worktree removed, and a worker of the issue
/// started in a new one, on the PR's branch merged up to the base tip,
/// briefed with both red runs. Its line (`said`) is added to `found` and
/// kept for the issue's status board.
pub(crate) fn hand_on(
    fleet: &mut Fleet,
    found: &mut Found,
    (me, agent): (&Me, &Agent),
    issue: u64,
    worktree: Worktree,
    reds: Vec<RedTurn>,
) -> Result<Value> {
    let claim = crate::judge::mine(fleet, me, issue)?
        .ok_or_else(|| anyhow!("{} holds no claim on issue #{issue}", me.instance))?;
    let prs = fleet.gh.open_prs()?;
    let pr =
        closing_pr(&prs, issue).ok_or_else(|| anyhow!("issue #{issue}: no open PR to hand on"))?;
    let (pr, branch) = (pr.number, pr.head_branch.clone());
    let before = fleet.first_harness().unwrap_or(Harness::DEFAULT);
    let continued = crate::switch::continuing(Some(&worktree), before);
    let verdict = last_verdict(fleet, issue)?;
    worktree.stop_for_continuation().with_context(|| {
        format!(
            "issue #{issue}: PR #{pr} went red {} turns in a row, and its worker could not be \
             confirmed stopped: no new worker was started, and its worktree {} is kept",
            reds.len(),
            worktree.path.display()
        )
    })?;
    let checkout = Repo::new(worktree.path.clone());
    let against = fleet
        .repo
        .remote()
        .fetched(&format!("refs/heads/{branch}"), Fresh::Process)?
        .unwrap_or_else(|| "HEAD".to_string());
    let salvaged = fleet.repo.common_dir()?.join("convoy-salvage");
    let patch = salvage(&checkout, &against, &salvaged, (issue, fleet.now))?;
    worktree.remove()?;
    // What only the local branch held is in the patch: the new worktree takes
    // the PR's branch as the remote has it, under its own name.
    if fleet
        .repo
        .try_git(&["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])?
        .ok
    {
        fleet.repo.git(&["branch", "-D", &branch])?;
    }
    let handed = Handed { pr, reds, verdict, said: continued.said, patch };
    let dispatched =
        dispatch_continuing(fleet, found, me, agent, issue, StartFrom::Auto, None, Some(&handed))?;
    if !dispatched.started() {
        bail!("issue #{issue}: no new worker was started for PR #{pr}: {}", dispatched.answer);
    }
    let started = dispatched.answer;
    let path = started["worktree"].as_str().unwrap_or_default().to_string();
    let line = said(issue, pr, &handed.reds, &path);
    if let Ok(file) = said_file(fleet, issue) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let kept = Said { claim: claim.sha.clone(), line: line.clone() };
        let _ = std::fs::write(&file, serde_json::to_vec(&kept)?);
    }
    found.handed_on.push((issue, line));
    Ok(json!({"issue": issue, "pr": pr, "worktree": path, "branch": started["branch"],
        "terminal": started["terminal"], "patch": handed.patch, "reds": handed.reds.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(failing: &[&str], output: Option<&str>) -> RedTurn {
        RedTurn {
            at: 1,
            failing: failing.iter().map(|s| s.to_string()).collect(),
            gate_output: output.map(str::to_string),
        }
    }

    #[test]
    fn the_brief_names_each_red_run_the_last_verdict_and_the_patch() {
        let handed = Handed {
            pr: 70,
            reds: vec![
                red(&["cli turns::a"], Some("/r/.git/convoy-gate-c1-fast.log")),
                red(&["cli turns::b", "cli turns::c"], None),
            ],
            verdict: Some("<!--convoy:verdict phase=escalate-->\nstuck".to_string()),
            said: vec!["the last line".to_string()],
            patch: Some(PathBuf::from("/r/.git/convoy-salvage/7-9.patch")),
        };
        let text = handed.brief("main", &["src/a.rs".to_string()]);
        for expected in [
            "PR #70 gated red in 2 landing turns in a row",
            "`origin/main`",
            "You may change the approach",
            "**Red run 1:** failing `cli turns::a`; its whole gate output is in \
             `/r/.git/convoy-gate-c1-fast.log`",
            "**Red run 2:** failing `cli turns::b`, `cli turns::c`; its whole gate output was not kept",
            "phase=escalate-->\nstuck",
            "/r/.git/convoy-salvage/7-9.patch",
            "conflicted** on `src/a.rs`",
            "the last line",
        ] {
            assert!(text.contains(expected), "missing {expected}: {text}");
        }
        let none = Handed { verdict: None, patch: None, said: vec![], ..handed }.brief("main", &[]);
        assert!(none.contains("it declared none"), "{none}");
        assert!(none.contains("no patch was saved"), "{none}");
        assert!(!none.contains("conflicted"), "{none}");
    }

    #[test]
    fn the_line_names_the_pr_both_red_turns_and_the_new_worktree() {
        let line = said(7, 70, &[red(&["a"], None), red(&[], None)], "/w/7");
        assert_eq!(
            line,
            "handed #7's PR #70 to a new worker in a new worktree /w/7 after 2 red turns in a \
             row (failing a; no failing test named)"
        );
    }
}
