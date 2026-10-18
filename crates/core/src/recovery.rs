//! Continuation: what survived a worker that died, and the tier it is
//! continued from. Nothing here tears anything down.

use crate::plan::PullRequest;
use crate::stopped::Progress;

/// Where a continued worker starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The worktree is still on this host: the new worker goes in it, on the
    /// same branch. Lossless — it carries even uncommitted work.
    Reuse,
    /// No worktree here, but the dead worker pushed: one is cut at the tip of
    /// its branch. What is lost is bounded to "since the last push".
    Recreate,
    /// Nothing survived: a fresh start from the base branch.
    Fresh,
}

impl Tier {
    pub fn number(self) -> u8 {
        match self {
            Tier::Reuse => 1,
            Tier::Recreate => 2,
            Tier::Fresh => 3,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Tier::Reuse => "reuse_worktree",
            Tier::Recreate => "recreate_at_tip",
            Tier::Fresh => "dispatch_fresh",
        }
    }
}

/// Which worker prompt a worker is started under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    /// A clean checkout of the base branch.
    Fresh,
    /// The worktree or branch carries an earlier worker's progress: inspect
    /// it first, as partial work toward the same criteria.
    Continue,
    /// The issue's PR is open: nothing is left but joining it to the train.
    Join { pr: u64 },
    /// The issue's PR is open and no claim held it: a released claim kept
    /// it. What it was released for is to be read, the base merged, and the
    /// PR joined to the train.
    TakeUp { pr: u64 },
}

impl Variant {
    pub fn word(self) -> &'static str {
        match self {
            Variant::Fresh => "fresh",
            Variant::Continue => "continue",
            Variant::Join { .. } => "join",
            Variant::TakeUp { .. } => "take_up",
        }
    }
}

/// One of an issue's own branches on the remote, against the base branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pushed {
    pub name: String,
    /// Commits past the base; `None` when the count could not be read.
    pub commits_ahead: Option<u64>,
}

/// How a worker is started on a claim: the tier, the prompt, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    pub tier: Tier,
    pub variant: Variant,
    pub reason: String,
}

impl Recovery {
    /// This start, on an issue no claim held before it: an open PR it would
    /// only join was kept by a released claim, and is taken up instead.
    pub fn taken_up(self) -> Recovery {
        match self.variant {
            Variant::Join { pr } => Recovery {
                variant: Variant::TakeUp { pr },
                reason: format!("{}; no claim held it — take it up", self.reason),
                ..self
            },
            _ => self,
        }
    }

    /// The start that follows a discard: nothing is left to continue.
    pub fn discarded() -> Recovery {
        Recovery {
            tier: Tier::Fresh,
            variant: Variant::Fresh,
            reason: "fresh start: the previous attempt was discarded".to_string(),
        }
    }
}

/// How a worker is started on a claim: the tier by what survived, and the
/// join's own brief when the issue's PR (`joining`) is open and its work
/// survived — a start from the base branch has no PR of its own to join.
pub fn select_recovery(
    worktree: Option<&Progress>,
    pushed: Option<&Pushed>,
    joining: Option<u64>,
) -> Recovery {
    let survived = survived(worktree, pushed);
    match joining {
        Some(pr) if survived.tier != Tier::Fresh => Recovery {
            tier: survived.tier,
            variant: Variant::Join { pr },
            reason: format!("{}; PR #{pr} is open — brief only to join the train", survived.reason),
        },
        _ => survived,
    }
}

/// Picks the tier a claim is continued from, by what survived: `worktree` is
/// the progress of its worktree on this host (`None` when there is none),
/// `pushed` the branch of its own furthest ahead on the remote.
///
/// A worktree that is provably pristine — no commit past the base, a clean
/// tree, nothing pushed either — gets the fresh prompt: there is nothing to
/// continue. One whose progress could not be read is never pristine.
fn survived(worktree: Option<&Progress>, pushed: Option<&Pushed>) -> Recovery {
    let pushed_ahead = match pushed {
        Some(pushed) => pushed.commits_ahead.unwrap_or(0),
        None => 0,
    };
    let continued = |tier, variant, reason: String| Recovery { tier, variant, reason };
    if let Some(progress) = worktree {
        let Some(ahead) = progress.commits_ahead else {
            let reason = "local worktree present, progress unreadable — reuse it and inspect";
            return continued(Tier::Reuse, Variant::Continue, reason.to_string());
        };
        if ahead == 0 && !progress.dirty {
            if pushed_ahead == 0 {
                let reason = "local worktree present but pristine — reuse it, nothing to continue";
                return continued(Tier::Reuse, Variant::Fresh, reason.to_string());
            }
            let reason = format!(
                "local worktree present and clean, but its branch carries {pushed_ahead} pushed \
                 commit(s) ahead of base — reuse it and continue from them"
            );
            return continued(Tier::Reuse, Variant::Continue, reason);
        }
        let dirty = if progress.dirty { " and uncommitted changes" } else { "" };
        let reason = format!("local worktree present with {ahead} commit(s) ahead{dirty}");
        return continued(Tier::Reuse, Variant::Continue, reason);
    }
    if let Some(pushed) = pushed
        && pushed_ahead > 0
    {
        let reason = format!(
            "no local worktree; branch {} is {pushed_ahead} commit(s) ahead of base — recreate \
             at its tip",
            pushed.name
        );
        return continued(Tier::Recreate, Variant::Continue, reason);
    }
    let reason = "no local worktree and nothing pushed ahead of base — start from base";
    continued(Tier::Fresh, Variant::Fresh, reason.to_string())
}

/// The remote branches that are an issue's own, sorted: of the remote's
/// `heads`, those the fleet `recorded` on the issue, and the branch of the
/// worktree orca `linked` to it on this host. Nothing is read off a branch's
/// name: one that merely looks like the issue's is somebody else's.
pub fn own_branches(heads: &[String], recorded: &[String], linked: Option<&str>) -> Vec<String> {
    let mut own: Vec<String> = Vec::new();
    for head in heads {
        let is_own = recorded.contains(head) || linked == Some(head.as_str());
        if is_own && !own.contains(head) {
            own.push(head.clone());
        }
    }
    own.sort();
    own
}

/// Of an issue's own branches, the one furthest ahead of the base: an earlier
/// continuation left one behind. Ties, and counts that could not be read, go
/// to the first by name.
pub fn furthest_ahead(candidates: &[Pushed]) -> Option<&Pushed> {
    let mut furthest: Option<&Pushed> = None;
    for candidate in candidates {
        let further = match furthest {
            Some(found) => {
                let ahead = candidate.commits_ahead.unwrap_or(0);
                let found_ahead = found.commits_ahead.unwrap_or(0);
                ahead > found_ahead || (ahead == found_ahead && candidate.name < found.name)
            }
            None => true,
        };
        if further {
            furthest = Some(candidate);
        }
    }
    furthest
}

/// The open PRs a discarded attempt leaves behind: those that close `issue`
/// from one of its `own` branches. A PR somebody else opened for the issue is
/// not the fleet's to close.
pub fn superseded(prs: &[PullRequest], issue: u64, own: &[String]) -> Vec<u64> {
    let mut numbers: Vec<u64> = Vec::new();
    for pr in prs {
        if pr.closes.contains(&issue) && own.contains(&pr.head_branch) {
            numbers.push(pr.number);
        }
    }
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worktree(ahead: Option<u64>, dirty: bool) -> Progress {
        Progress { commits_ahead: ahead, dirty, last_commit: None, touched: None }
    }

    fn pushed(name: &str, ahead: Option<u64>) -> Pushed {
        Pushed { name: name.to_string(), commits_ahead: ahead }
    }

    fn picked(worktree: Option<&Progress>, pushed: Option<&Pushed>) -> (u8, Variant) {
        let recovery = select_recovery(worktree, pushed, None);
        (recovery.tier.number(), recovery.variant)
    }

    #[test]
    fn a_worktree_still_here_is_reused() {
        let branch = pushed("u/a", Some(2));
        assert_eq!(picked(Some(&worktree(Some(3), false)), None), (1, Variant::Continue));
        assert_eq!(picked(Some(&worktree(Some(0), true)), None), (1, Variant::Continue));
        assert_eq!(picked(Some(&worktree(Some(0), false)), Some(&branch)), (1, Variant::Continue));
        // pristine: nothing to continue, so nothing is said to be there
        assert_eq!(picked(Some(&worktree(Some(0), false)), None), (1, Variant::Fresh));
        let nothing_pushed = pushed("u/a", Some(0));
        assert_eq!(
            picked(Some(&worktree(Some(0), false)), Some(&nothing_pushed)),
            (1, Variant::Fresh)
        );
        // unreadable is not pristine
        assert_eq!(picked(Some(&worktree(None, false)), None), (1, Variant::Continue));
        let reason = select_recovery(Some(&worktree(Some(3), true)), None, None).reason;
        assert_eq!(reason, "local worktree present with 3 commit(s) ahead and uncommitted changes");
    }

    #[test]
    fn with_no_worktree_a_pushed_branch_is_recreated_else_the_start_is_fresh() {
        let branch = pushed("u/a", Some(2));
        assert_eq!(picked(None, Some(&branch)), (2, Variant::Continue));
        assert!(
            select_recovery(None, Some(&branch), None).reason.contains("branch u/a is 2 commit(s)")
        );
        assert_eq!(picked(None, Some(&pushed("u/a", Some(0)))), (3, Variant::Fresh));
        assert_eq!(picked(None, Some(&pushed("u/a", None))), (3, Variant::Fresh));
        assert_eq!(picked(None, None), (3, Variant::Fresh));
        assert_eq!(Tier::Recreate.word(), "recreate_at_tip");
    }

    #[test]
    fn with_its_pr_open_what_survived_is_briefed_only_to_join() {
        let branch = pushed("u/a", Some(2));
        let here = select_recovery(Some(&worktree(Some(2), false)), Some(&branch), Some(70));
        assert_eq!((here.tier, here.variant), (Tier::Reuse, Variant::Join { pr: 70 }));
        assert!(here.reason.ends_with("PR #70 is open — brief only to join the train"));
        let gone = select_recovery(None, Some(&branch), Some(70));
        assert_eq!((gone.tier, gone.variant), (Tier::Recreate, Variant::Join { pr: 70 }));
        // nothing of the PR's survived: the start is the whole issue again
        let nothing = select_recovery(None, None, Some(70));
        assert_eq!((nothing.tier, nothing.variant), (Tier::Fresh, Variant::Fresh));
        assert_eq!(Variant::Join { pr: 70 }.word(), "join");
    }

    #[test]
    fn a_pr_no_claim_held_is_taken_up_and_anything_else_is_left_as_it_was() {
        let branch = pushed("u/a", Some(2));
        let kept = select_recovery(None, Some(&branch), Some(70)).taken_up();
        assert_eq!((kept.tier, kept.variant), (Tier::Recreate, Variant::TakeUp { pr: 70 }));
        assert!(kept.reason.ends_with("no claim held it — take it up"));
        assert_eq!(Variant::TakeUp { pr: 70 }.word(), "take_up");
        let continued = select_recovery(None, Some(&branch), None);
        assert_eq!(continued.clone().taken_up(), continued);
    }

    #[test]
    fn after_a_discard_the_start_is_fresh() {
        let by_hand = Recovery::discarded();
        assert_eq!((by_hand.tier, by_hand.variant), (Tier::Fresh, Variant::Fresh));
    }

    #[test]
    fn an_issues_own_branches_are_recorded_or_linked_never_named() {
        let heads: Vec<String> =
            ["main", "u/issue-7-b", "u/issue-7-a", "u/issue-7-lookalike", "u/other"]
                .iter()
                .map(|head| head.to_string())
                .collect();
        let recorded = vec!["u/issue-7-b".to_string(), "u/gone".to_string()];
        assert_eq!(
            own_branches(&heads, &recorded, Some("u/issue-7-a")),
            ["u/issue-7-a", "u/issue-7-b"]
        );
        assert_eq!(own_branches(&heads, &recorded, None), ["u/issue-7-b"]);
        assert_eq!(own_branches(&heads, &[], None), [] as [String; 0]);
    }

    #[test]
    fn the_branch_furthest_ahead_is_continued() {
        let candidates = [pushed("u/b", Some(2)), pushed("u/a", Some(2)), pushed("u/c", None)];
        assert_eq!(furthest_ahead(&candidates).unwrap().name, "u/a");
        let candidates = [pushed("u/a", Some(1)), pushed("u/b", Some(5))];
        assert_eq!(furthest_ahead(&candidates).unwrap().name, "u/b");
        assert_eq!(furthest_ahead(&[]), None);
    }

    #[test]
    fn a_discard_closes_only_the_prs_on_the_issues_own_branches() {
        let pr = |number: u64, head: &str, closes: u64| PullRequest {
            number,
            title: String::new(),
            head_branch: head.to_string(),
            head_sha: String::new(),
            closes: vec![closes],
        };
        let prs = [pr(70, "u/a", 7), pr(71, "someone/else", 7), pr(80, "u/a", 8)];
        assert_eq!(superseded(&prs, 7, &["u/a".to_string()]), [70]);
    }
}
